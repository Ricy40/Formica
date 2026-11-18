//! The simulation driver: fixed-timestep stepping, command application and the command log.

use glam::{IVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::agents::{self, Agents};
use crate::rng::{SimRng, SystemId, system_rng};
use crate::scene::{self, SceneConfig};
use crate::snapshot;
use crate::voxel::{Material, Voxel};
use crate::world::World;

/// Everything needed to construct the tick-0 state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimConfig {
    pub seed: u64,
    /// Simulated seconds per tick.
    pub dt: f32,
    pub scene: SceneConfig,
}

impl SimConfig {
    pub const DEFAULT_DT: f32 = 1.0 / 30.0;

    pub fn test_chunk(seed: u64, agents: u32) -> Self {
        Self {
            seed,
            dt: Self::DEFAULT_DT,
            scene: SceneConfig::TestChunk { agents },
        }
    }
}

/// External inputs that change simulation state (god tools). The only way in from outside.
/// Variant order is part of the wire and save format: only append.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SimCommand {
    /// Spawn dummy agents at random surface positions.
    SpawnAgents { count: u32 },
    /// Remove all agents.
    ClearAgents,
    /// Set one voxel (dig or fill).
    SetVoxel { pos: IVec3, material: Material },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedCommand {
    /// Tick at whose start the command was applied.
    pub tick: u64,
    pub command: SimCommand,
}

/// Seed + config + every command with its tick: enough to reproduce a run exactly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandLog {
    pub config: SimConfig,
    pub entries: Vec<LoggedCommand>,
    /// Tick the run had reached when the log was taken.
    pub end_tick: u64,
    /// State hash at `end_tick`, for verifying a replay.
    pub end_hash: u64,
}

/// The complete, serializable simulation state. Its canonical postcard encoding defines the state
/// hash, so it must contain no unordered collections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimState {
    pub config: SimConfig,
    pub tick: u64,
    pub world: World,
    pub agents: Agents,
    pub spawn_rng: SimRng,
}

pub struct Sim {
    state: SimState,
    pending: Vec<SimCommand>,
    log: Vec<LoggedCommand>,
    /// False when started from a snapshot: the log then has no tick-0 base to replay from.
    log_from_start: bool,
}

impl Sim {
    pub fn new(config: SimConfig) -> Self {
        let (world, initial_agents) = match config.scene {
            SceneConfig::TestChunk { agents } => (scene::build_test_chunk(), agents),
        };
        let mut state = SimState {
            config,
            tick: 0,
            world,
            agents: Agents::default(),
            spawn_rng: system_rng(config.seed, SystemId::AgentSpawn),
        };
        spawn_agents(&mut state, initial_agents);
        Sim {
            state,
            pending: vec![],
            log: vec![],
            log_from_start: true,
        }
    }

    /// Resumes from a loaded snapshot.
    pub fn from_state(state: SimState) -> Self {
        Sim {
            state,
            pending: vec![],
            log: vec![],
            log_from_start: false,
        }
    }

    pub fn state(&self) -> &SimState {
        &self.state
    }

    pub fn tick(&self) -> u64 {
        self.state.tick
    }

    pub fn world(&self) -> &World {
        &self.state.world
    }

    /// Voxel edits since the last call, for streaming as deltas. Transient; not part of the state.
    pub fn drain_world_edits(&mut self) -> Vec<crate::world::VoxelEdit> {
        self.state.world.drain_edits()
    }

    pub fn agents(&self) -> &Agents {
        &self.state.agents
    }

    /// Queues a command to be applied at the start of the next tick.
    pub fn queue(&mut self, command: SimCommand) {
        self.pending.push(command);
    }

    /// Advances one fixed timestep.
    pub fn step(&mut self) {
        let tick = self.state.tick;
        for command in std::mem::take(&mut self.pending) {
            apply(&mut self.state, &command);
            self.log.push(LoggedCommand { tick, command });
        }
        let SimState {
            config,
            world,
            agents,
            ..
        } = &mut self.state;
        agents::step_dummies(agents, world, config.seed, tick, config.dt);
        self.state.tick += 1;
    }

    pub fn step_n(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    pub fn state_hash(&self) -> u64 {
        snapshot::state_hash(&self.state)
    }

    /// The replayable log, or `None` if this run started from a snapshot.
    pub fn command_log(&self) -> Option<CommandLog> {
        self.log_from_start.then(|| CommandLog {
            config: self.state.config,
            entries: self.log.clone(),
            end_tick: self.state.tick,
            end_hash: self.state_hash(),
        })
    }

    /// Re-runs a log from tick 0. Check `sim.state_hash() == log.end_hash` to verify it.
    pub fn replay(log: &CommandLog) -> Self {
        let mut sim = Sim::new(log.config);
        let mut next = 0;
        while sim.tick() < log.end_tick {
            while next < log.entries.len() && log.entries[next].tick == sim.tick() {
                sim.queue(log.entries[next].command.clone());
                next += 1;
            }
            sim.step();
        }
        sim
    }
}

fn apply(state: &mut SimState, command: &SimCommand) {
    match command {
        SimCommand::SpawnAgents { count } => spawn_agents(state, *count),
        SimCommand::ClearAgents => state.agents.clear(),
        SimCommand::SetVoxel { pos, material } => {
            let flags = if *material == Material::Air {
                Voxel::FLAG_DUG
            } else {
                0
            };
            state.world.set(
                *pos,
                Voxel {
                    flags,
                    ..Voxel::of(*material)
                },
            );
        }
    }
}

/// Places agents on random dry surface columns using the persistent spawn stream.
fn spawn_agents(state: &mut SimState, count: u32) {
    let ext = state.world.extent();
    for _ in 0..count {
        for _attempt in 0..64 {
            let x = ext.min.x + state.spawn_rng.below(ext.size.x as u32) as i32;
            let z = ext.min.z + state.spawn_rng.below(ext.size.z as u32) as i32;
            let heading = state.spawn_rng.range_f32(0.0, core::f32::consts::TAU);
            let speed = state
                .spawn_rng
                .range_f32(agents::SPEED_RANGE.0, agents::SPEED_RANGE.1);
            if let Some((h, top)) = state.world.surface_at(x, z)
                && top.is_solid()
            {
                let pos = Vec3::new(x as f32 + 0.5, h as f32, z as f32 + 0.5);
                state.agents.push(pos, heading, speed);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_hash() {
        let mut a = Sim::new(SimConfig::test_chunk(42, 300));
        let mut b = Sim::new(SimConfig::test_chunk(42, 300));
        a.step_n(500);
        b.step_n(500);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.state(), b.state());
    }

    #[test]
    fn different_seed_different_hash() {
        let mut a = Sim::new(SimConfig::test_chunk(42, 300));
        let mut b = Sim::new(SimConfig::test_chunk(43, 300));
        a.step_n(10);
        b.step_n(10);
        assert_ne!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn hash_changes_every_tick() {
        let mut s = Sim::new(SimConfig::test_chunk(1, 10));
        let h0 = s.state_hash();
        s.step();
        assert_ne!(h0, s.state_hash());
    }

    #[test]
    fn spawn_count_is_exact() {
        let mut s = Sim::new(SimConfig::test_chunk(5, 300));
        assert_eq!(s.agents().len(), 300);
        s.queue(SimCommand::SpawnAgents { count: 25 });
        s.step();
        assert_eq!(s.agents().len(), 325);
        s.queue(SimCommand::ClearAgents);
        s.step();
        assert!(s.agents().is_empty());
    }

    #[test]
    fn set_voxel_command_edits_world() {
        let mut s = Sim::new(SimConfig::test_chunk(5, 0));
        let p = IVec3::new(3, 30, 3);
        s.queue(SimCommand::SetVoxel {
            pos: p,
            material: Material::Rock,
        });
        s.step();
        assert_eq!(s.world().get(p).material, Material::Rock);
        assert_eq!(s.world().surface_at(3, 3).unwrap(), (31, Material::Rock));
    }

    #[test]
    fn replay_reproduces_hash() {
        let mut s = Sim::new(SimConfig::test_chunk(77, 100));
        s.step_n(20);
        s.queue(SimCommand::SpawnAgents { count: 50 });
        s.step_n(30);
        s.queue(SimCommand::SetVoxel {
            pos: IVec3::new(10, 20, 10),
            material: Material::Air,
        });
        s.queue(SimCommand::SpawnAgents { count: 5 });
        s.step_n(100);
        let log = s.command_log().unwrap();
        assert_eq!(log.entries.len(), 3);
        let replayed = Sim::replay(&log);
        assert_eq!(replayed.state_hash(), log.end_hash);
        assert_eq!(replayed.state(), s.state());
    }

    #[test]
    fn snapshot_resume_matches_continuous_run() {
        let mut continuous = Sim::new(SimConfig::test_chunk(9, 200));
        continuous.step_n(100);
        let bytes = snapshot::save_state(continuous.state()).unwrap();
        continuous.step_n(100);

        let mut resumed = Sim::from_state(snapshot::load_state(&bytes).unwrap());
        assert!(resumed.command_log().is_none());
        resumed.step_n(100);
        assert_eq!(resumed.state_hash(), continuous.state_hash());
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn thread_count_does_not_change_result() {
        let run = |threads: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                let mut s = Sim::new(SimConfig::test_chunk(1234, 20_000));
                s.step_n(50);
                s.state_hash()
            })
        };
        assert_eq!(run(1), run(4));
    }
}

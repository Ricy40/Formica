//! Formica wire protocol (server ⇄ viewer), postcard-encoded over WebSocket binary frames.
//!
//! The TypeScript viewer mirrors these types by hand in `viewer/src/proto`. Any change here must:
//! bump [`PROTOCOL_VERSION`], update the TS mirror, and re-bless the golden fixtures
//! (`FORMICA_BLESS=1 cargo test -p sim-proto`), which the TS tests also decode.
//!
//! Bulk per-agent data travels as packed little-endian byte arrays (`Vec<u8>`) so the viewer can
//! read them with typed arrays instead of decoding field by field.
//!
//! Enum variant order is the wire discriminant: only append.

use serde::{Deserialize, Serialize};
use sim_core::agents::Agents;
use sim_core::world::VoxelEdit;
use sim_core::{CHUNK_VOLUME, Chunk, ChunkCoord, Material, SimCommand};
use thiserror::Error;

pub const PROTOCOL_VERSION: u32 = 1;

/// Bytes per agent in [`AgentFrame::data`]: x, y, z, heading as `f32` LE.
pub const AGENT_STRIDE: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Hello(Hello),
    Chunk(ChunkMsg),
    AgentFrame(AgentFrame),
    VoxelEdits(VoxelEdits),
    Stats(Stats),
    Notice(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { protocol_version: u32 },
    Command(SimCommand),
    Control(Control),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Control {
    Pause,
    Resume,
    /// Multiplier on real-time tick pacing (1.0 = 30 ticks/s at the default dt).
    SetSpeed(f32),
    SaveSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub seed: u64,
    pub tick: u64,
    pub dt: f32,
    /// World extent in voxels.
    pub world_min: [i32; 3],
    pub world_size: [i32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkMsg {
    pub coord: [i32; 3],
    pub data: ChunkData,
}

/// Chunk materials. RLE runs follow the chunk's y-major voxel order (`x + z*32 + y*1024`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ChunkData {
    Uniform(u8),
    Rle(Vec<Run>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// 1..=32768
    pub len: u16,
    pub material: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentFrame {
    pub tick: u64,
    /// `u32` LE per agent: stable ids for interpolation and picking.
    pub ids: Vec<u8>,
    /// [`AGENT_STRIDE`] bytes per agent: x, y, z, heading (`f32` LE).
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoxelEdits {
    pub tick: u64,
    pub edits: Vec<Edit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub pos: [i32; 3],
    pub material: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub tick: u64,
    /// Measured simulation ticks per wall-clock second.
    pub tps: f32,
    /// Wall-clock milliseconds spent in the last `step()`.
    pub step_ms: f32,
    pub agents: u32,
    pub state_hash: u64,
    pub paused: bool,
    pub speed: f32,
}

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("postcard: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("chunk RLE covers {0} voxels, expected {CHUNK_VOLUME}")]
    BadRle(usize),
    #[error("unknown material {0}")]
    BadMaterial(u8),
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    postcard::to_allocvec(msg).expect("protocol messages always serialize")
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, ProtoError> {
    Ok(postcard::from_bytes(bytes)?)
}

impl ChunkMsg {
    pub fn from_chunk(coord: ChunkCoord, chunk: &Chunk) -> Self {
        let data = match chunk {
            Chunk::Uniform(v) => ChunkData::Uniform(v.material as u8),
            Chunk::Dense(_) => {
                let mut runs: Vec<Run> = Vec::new();
                for m in chunk.materials() {
                    match runs.last_mut() {
                        Some(r) if r.material == m as u8 && r.len < u16::MAX => r.len += 1,
                        _ => runs.push(Run {
                            len: 1,
                            material: m as u8,
                        }),
                    }
                }
                ChunkData::Rle(runs)
            }
        };
        ChunkMsg {
            coord: [coord.x, coord.y, coord.z],
            data,
        }
    }

    /// Expands to one material per voxel in chunk order (used by tests; the viewer does the same).
    pub fn expand(&self) -> Result<Vec<Material>, ProtoError> {
        let to_mat = |b: u8| Material::from_u8(b).ok_or(ProtoError::BadMaterial(b));
        match &self.data {
            ChunkData::Uniform(m) => Ok(vec![to_mat(*m)?; CHUNK_VOLUME]),
            ChunkData::Rle(runs) => {
                let mut out = Vec::with_capacity(CHUNK_VOLUME);
                for r in runs {
                    let m = to_mat(r.material)?;
                    out.extend(std::iter::repeat_n(m, r.len as usize));
                }
                if out.len() != CHUNK_VOLUME {
                    return Err(ProtoError::BadRle(out.len()));
                }
                Ok(out)
            }
        }
    }
}

impl AgentFrame {
    pub fn from_agents(tick: u64, agents: &Agents) -> Self {
        let mut ids = Vec::with_capacity(agents.len() * 4);
        let mut data = Vec::with_capacity(agents.len() * AGENT_STRIDE);
        for ((id, p), h) in agents.id.iter().zip(&agents.pos).zip(&agents.heading) {
            ids.extend_from_slice(&id.to_le_bytes());
            for f in [p.x, p.y, p.z, *h] {
                data.extend_from_slice(&f.to_le_bytes());
            }
        }
        AgentFrame { tick, ids, data }
    }

    pub fn len(&self) -> usize {
        self.ids.len() / 4
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

impl VoxelEdits {
    pub fn new(tick: u64, edits: &[VoxelEdit]) -> Self {
        VoxelEdits {
            tick,
            edits: edits
                .iter()
                .map(|e| Edit {
                    pos: e.pos.to_array(),
                    material: e.voxel.material as u8,
                })
                .collect(),
        }
    }
}

/// Fixed messages whose exact bytes are pinned in `fixtures/` and decoded by the TS tests.
/// Keep in sync with `viewer/src/proto/proto.test.ts`.
pub mod fixtures {
    use super::*;
    use sim_core::glam::{IVec3, Vec3};

    pub fn all() -> Vec<(&'static str, Vec<u8>)> {
        let mut agents = Agents::default();
        agents.push(Vec3::new(1.5, 20.0, -3.25), 0.5, 8.0);
        agents.push(Vec3::new(31.0, 0.0, 16.0), 6.0, 8.0);

        let mut dense = Chunk::uniform(Material::Topsoil);
        dense.set(
            sim_core::glam::UVec3::new(0, 0, 0),
            sim_core::Voxel::of(Material::Rock),
        );
        dense.set(sim_core::glam::UVec3::new(31, 31, 31), sim_core::Voxel::AIR);

        vec![
            (
                "server_hello",
                encode(&ServerMsg::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    seed: 42,
                    tick: 300,
                    dt: 1.0 / 30.0,
                    world_min: [0, 0, 0],
                    world_size: [32, 32, 32],
                })),
            ),
            (
                "server_chunk_uniform",
                encode(&ServerMsg::Chunk(ChunkMsg::from_chunk(
                    ChunkCoord::new(-1, 0, 2),
                    &Chunk::uniform(Material::Clay),
                ))),
            ),
            (
                "server_chunk_rle",
                encode(&ServerMsg::Chunk(ChunkMsg::from_chunk(
                    ChunkCoord::new(0, 0, 0),
                    &dense,
                ))),
            ),
            (
                "server_agent_frame",
                encode(&ServerMsg::AgentFrame(AgentFrame::from_agents(7, &agents))),
            ),
            (
                "server_voxel_edits",
                encode(&ServerMsg::VoxelEdits(VoxelEdits {
                    tick: 9,
                    edits: vec![Edit {
                        pos: [3, -4, 200],
                        material: Material::Air as u8,
                    }],
                })),
            ),
            (
                "server_stats",
                encode(&ServerMsg::Stats(Stats {
                    tick: 123_456,
                    tps: 30.0,
                    step_ms: 0.25,
                    agents: 300,
                    state_hash: 0xDEAD_BEEF_CAFE_F00D,
                    paused: false,
                    speed: 1.0,
                })),
            ),
            ("server_notice", encode(&ServerMsg::Notice("saved".into()))),
            (
                "client_hello",
                encode(&ClientMsg::Hello {
                    protocol_version: PROTOCOL_VERSION,
                }),
            ),
            (
                "client_spawn",
                encode(&ClientMsg::Command(SimCommand::SpawnAgents { count: 100 })),
            ),
            (
                "client_set_voxel",
                encode(&ClientMsg::Command(SimCommand::SetVoxel {
                    pos: IVec3::new(10, -1, 300),
                    material: Material::Rock,
                })),
            ),
            (
                "client_speed",
                encode(&ClientMsg::Control(Control::SetSpeed(2.5))),
            ),
            (
                "client_save",
                encode(&ClientMsg::Control(Control::SaveSnapshot)),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::{Sim, SimConfig};
    use std::path::PathBuf;

    #[test]
    fn chunk_rle_roundtrip_on_test_scene() {
        let sim = Sim::new(SimConfig::test_chunk(1, 0));
        for (coord, chunk) in sim.world().chunks() {
            let msg = ChunkMsg::from_chunk(*coord, chunk);
            let back: ServerMsg = decode(&encode(&ServerMsg::Chunk(msg.clone()))).unwrap();
            assert_eq!(back, ServerMsg::Chunk(msg.clone()));
            let mats = msg.expand().unwrap();
            assert!(mats.iter().copied().eq(chunk.materials()));
            if let ChunkData::Rle(runs) = &msg.data {
                // The hand-made chunk compresses well: a few thousand runs, not 32k voxels.
                assert!(runs.len() < 8000, "{} runs", runs.len());
            }
        }
    }

    #[test]
    fn agent_frame_packs_little_endian() {
        let mut sim = Sim::new(SimConfig::test_chunk(1, 3));
        sim.step();
        let f = AgentFrame::from_agents(sim.tick(), sim.agents());
        assert_eq!(f.len(), 3);
        assert_eq!(f.data.len(), 3 * AGENT_STRIDE);
        let x = f32::from_le_bytes(f.data[0..4].try_into().unwrap());
        assert_eq!(x, sim.agents().pos[0].x);
        let id2 = u32::from_le_bytes(f.ids[8..12].try_into().unwrap());
        assert_eq!(id2, sim.agents().id[2]);
    }

    #[test]
    fn rle_rejects_wrong_length() {
        let msg = ChunkMsg {
            coord: [0, 0, 0],
            data: ChunkData::Rle(vec![Run {
                len: 5,
                material: 2,
            }]),
        };
        assert!(matches!(msg.expand(), Err(ProtoError::BadRle(5))));
    }

    #[test]
    fn client_messages_roundtrip() {
        for m in [
            ClientMsg::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
            ClientMsg::Command(SimCommand::ClearAgents),
            ClientMsg::Control(Control::Pause),
        ] {
            assert_eq!(decode::<ClientMsg>(&encode(&m)).unwrap(), m);
        }
    }

    /// Golden fixtures pin the exact wire bytes. Re-bless with `FORMICA_BLESS=1` after an
    /// intentional protocol change (and bump PROTOCOL_VERSION + update the TS mirror).
    #[test]
    fn golden_fixtures() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let bless = std::env::var_os("FORMICA_BLESS").is_some();
        for (name, bytes) in fixtures::all() {
            let path = dir.join(format!("{name}.bin"));
            if bless {
                std::fs::write(&path, &bytes).unwrap();
            } else {
                let want = std::fs::read(&path).unwrap_or_else(|_| {
                    panic!("missing fixture {path:?}; run with FORMICA_BLESS=1")
                });
                assert_eq!(bytes, want, "wire bytes changed for {name}");
            }
        }
    }
}

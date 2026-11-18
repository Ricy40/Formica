//! `sim-headless`: fast runs, benchmarks and reproducibility checks without any networking.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sim_core::{Sim, SimCommand, SimConfig, snapshot};

#[derive(Parser)]
#[command(name = "sim-headless", about = "Formica headless simulation runner")]
struct Cli {
    /// Worker threads for parallel systems (default: all cores).
    #[arg(long, global = true)]
    threads: Option<usize>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the test scene for N ticks and print the final state hash.
    Run {
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value_t = 1000)]
        ticks: u64,
        #[arg(long, default_value_t = 300)]
        agents: u32,
        /// Spawn agents at a tick, as TICK:COUNT (repeatable). Exercises the command log.
        #[arg(long = "spawn", value_parser = parse_spawn)]
        spawns: Vec<(u64, u32)>,
        /// Write a full-state snapshot at the end.
        #[arg(long)]
        save_snapshot: Option<PathBuf>,
        /// Write the seed + command log at the end.
        #[arg(long)]
        save_log: Option<PathBuf>,
    },
    /// Load a snapshot, optionally advance it, and print the state hash.
    Resume {
        snapshot: PathBuf,
        #[arg(long, default_value_t = 0)]
        ticks: u64,
    },
    /// Replay a command log from tick 0 and verify it reproduces the recorded hash.
    Replay { log: PathBuf },
    /// Measure ticks per second.
    Bench {
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 50_000)]
        agents: u32,
        #[arg(long, default_value_t = 300)]
        ticks: u64,
        #[arg(long, default_value_t = 30)]
        warmup: u64,
    },
}

fn parse_spawn(s: &str) -> Result<(u64, u32), String> {
    let (t, c) = s.split_once(':').ok_or("expected TICK:COUNT")?;
    Ok((
        t.parse().map_err(|e| format!("{e}"))?,
        c.parse().map_err(|e| format!("{e}"))?,
    ))
}

fn print_result(sim: &Sim) {
    println!(
        "tick={} agents={} hash={:016x}",
        sim.tick(),
        sim.agents().len(),
        sim.state_hash()
    );
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(n) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    match cli.cmd {
        Cmd::Run {
            seed,
            ticks,
            agents,
            mut spawns,
            save_snapshot,
            save_log,
        } => {
            spawns.sort_by_key(|s| s.0);
            let mut sim = Sim::new(SimConfig::test_chunk(seed, agents));
            let mut next = 0;
            while sim.tick() < ticks {
                while next < spawns.len() && spawns[next].0 == sim.tick() {
                    sim.queue(SimCommand::SpawnAgents {
                        count: spawns[next].1,
                    });
                    next += 1;
                }
                sim.step();
            }
            if let Some(p) = save_snapshot {
                std::fs::write(&p, snapshot::save_state(sim.state())?)
                    .with_context(|| format!("writing {p:?}"))?;
            }
            if let Some(p) = save_log {
                let log = sim.command_log().expect("fresh runs always have a log");
                std::fs::write(&p, snapshot::save_log(&log)?)
                    .with_context(|| format!("writing {p:?}"))?;
            }
            print_result(&sim);
        }
        Cmd::Resume {
            snapshot: path,
            ticks,
        } => {
            let bytes = std::fs::read(&path).with_context(|| format!("reading {path:?}"))?;
            let mut sim = Sim::from_state(snapshot::load_state(&bytes)?);
            sim.step_n(ticks);
            print_result(&sim);
        }
        Cmd::Replay { log } => {
            let bytes = std::fs::read(&log).with_context(|| format!("reading {log:?}"))?;
            let log = snapshot::load_log(&bytes)?;
            let sim = Sim::replay(&log);
            print_result(&sim);
            if sim.state_hash() != log.end_hash {
                bail!("replay diverged: expected {:016x}", log.end_hash);
            }
            println!("replay ok");
        }
        Cmd::Bench {
            seed,
            agents,
            ticks,
            warmup,
        } => {
            let mut sim = Sim::new(SimConfig::test_chunk(seed, agents));
            sim.step_n(warmup);
            let start = Instant::now();
            sim.step_n(ticks);
            let secs = start.elapsed().as_secs_f64();
            let tps = ticks as f64 / secs;
            println!(
                "agents={} ticks={} threads={} elapsed={:.3}s ticks_per_s={:.1} ms_per_tick={:.3} agent_steps_per_s={:.3e}",
                sim.agents().len(),
                ticks,
                rayon::current_num_threads(),
                secs,
                tps,
                1000.0 / tps,
                tps * sim.agents().len() as f64,
            );
        }
    }
    Ok(())
}

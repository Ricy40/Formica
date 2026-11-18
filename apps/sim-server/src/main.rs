use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use sim_core::{Sim, SimConfig, snapshot};
use sim_server::{ServerConfig, serve};

#[derive(Parser)]
#[command(name = "sim-server", about = "Formica simulation server")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    bind: String,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Dummy agents in the Phase 0 test scene.
    #[arg(long, default_value_t = 300)]
    agents: u32,
    /// Resume from a snapshot instead of starting a new scene.
    #[arg(long)]
    load: Option<PathBuf>,
    #[arg(long, default_value = "saves")]
    saves_dir: PathBuf,
    /// Serve a built viewer from this directory (e.g. viewer/dist).
    #[arg(long)]
    static_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let sim = match &args.load {
        Some(path) => {
            let bytes = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
            Sim::from_state(snapshot::load_state(&bytes)?)
        }
        None => Sim::new(SimConfig::test_chunk(args.seed, args.agents)),
    };
    tracing::info!(
        "tick {} | {} agents | hash {:016x}",
        sim.tick(),
        sim.agents().len(),
        sim.state_hash()
    );

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    tracing::info!("listening on ws://{}/ws", listener.local_addr()?);
    let cfg = ServerConfig {
        saves_dir: args.saves_dir,
        static_dir: args.static_dir,
        ..Default::default()
    };
    serve(listener, sim, cfg).await
}

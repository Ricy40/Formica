//! Formica server: runs the world loop on a dedicated thread at a fixed tick rate and streams it to
//! viewers over WebSockets (`/ws`, postcard binary frames, see `sim-proto`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use sim_core::{Sim, SimCommand, snapshot};
use sim_proto::{
    AgentFrame, ChunkMsg, ClientMsg, Control, Hello, PROTOCOL_VERSION, ServerMsg, Stats, VoxelEdits,
};
use tokio::sync::{broadcast, oneshot};
use tower_http::services::ServeDir;

pub type Frame = Arc<Vec<u8>>;

pub struct ServerConfig {
    /// Directory for snapshot/log files written by the `SaveSnapshot` control.
    pub saves_dir: PathBuf,
    /// Optional built viewer (`viewer/dist`) to serve at `/`.
    pub static_dir: Option<PathBuf>,
    /// Ticks between `Stats` messages (which include a state hash).
    pub stats_every: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            saves_dir: "saves".into(),
            static_dir: None,
            stats_every: 15,
        }
    }
}

enum Inbound {
    Command(SimCommand),
    Control(Control),
    /// A viewer joined: reply with the messages that bring it up to date.
    Join(oneshot::Sender<Vec<Frame>>),
}

#[derive(Clone)]
struct AppState {
    to_sim: std_mpsc::Sender<Inbound>,
    frames: broadcast::Sender<Frame>,
}

fn frame(msg: &ServerMsg) -> Frame {
    Arc::new(sim_proto::encode(msg))
}

/// The world loop. Owns the `Sim`; everything else talks to it through channels.
fn run_sim_loop(
    mut sim: Sim,
    inbox: std_mpsc::Receiver<Inbound>,
    frames: broadcast::Sender<Frame>,
    cfg: ServerConfig,
) {
    let dt = sim.state().config.dt as f64;
    let (mut paused, mut speed) = (false, 1.0f64);
    let mut next_deadline = Instant::now();
    let (mut tps, mut ticks_this_second, mut second_start) = (0.0f32, 0u32, Instant::now());
    let mut last_step_ms = 0.0f32;
    let mut last_stats = Instant::now();

    let stats = |sim: &Sim, tps, step_ms, paused, speed: f64| {
        frame(&ServerMsg::Stats(Stats {
            tick: sim.tick(),
            tps,
            step_ms,
            agents: sim.agents().len() as u32,
            state_hash: sim.state_hash(),
            paused,
            speed: speed as f32,
        }))
    };

    loop {
        // Drain inputs. Commands are queued and applied at the start of the next tick.
        loop {
            match inbox.try_recv() {
                Ok(Inbound::Command(c)) => sim.queue(c),
                Ok(Inbound::Control(c)) => match c {
                    Control::Pause => paused = true,
                    Control::Resume => {
                        paused = false;
                        next_deadline = Instant::now();
                    }
                    Control::SetSpeed(s) => speed = (s as f64).clamp(0.05, 64.0),
                    Control::SaveSnapshot => {
                        let note = save(&sim, &cfg.saves_dir)
                            .unwrap_or_else(|e| format!("snapshot failed: {e:#}"));
                        tracing::info!("{note}");
                        let _ = frames.send(frame(&ServerMsg::Notice(note)));
                    }
                },
                Ok(Inbound::Join(reply)) => {
                    let _ = reply.send(join_messages(&sim, tps, last_step_ms, paused, speed));
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => return,
            }
        }

        let now = Instant::now();
        if paused || now < next_deadline {
            if paused && last_stats.elapsed() > Duration::from_millis(500) {
                let _ = frames.send(stats(&sim, 0.0, last_step_ms, paused, speed));
                last_stats = Instant::now();
            }
            let wait = if paused {
                Duration::from_millis(5)
            } else {
                next_deadline - now
            };
            std::thread::sleep(wait.min(Duration::from_millis(5)));
            continue;
        }

        let t0 = Instant::now();
        sim.step();
        last_step_ms = t0.elapsed().as_secs_f32() * 1000.0;

        let edits = sim.drain_world_edits();
        if !edits.is_empty() {
            let _ = frames.send(frame(&ServerMsg::VoxelEdits(VoxelEdits::new(
                sim.tick(),
                &edits,
            ))));
        }
        let _ = frames.send(frame(&ServerMsg::AgentFrame(AgentFrame::from_agents(
            sim.tick(),
            sim.agents(),
        ))));

        ticks_this_second += 1;
        if second_start.elapsed() >= Duration::from_secs(1) {
            tps = ticks_this_second as f32 / second_start.elapsed().as_secs_f32();
            ticks_this_second = 0;
            second_start = Instant::now();
        }
        if sim.tick().is_multiple_of(cfg.stats_every) {
            let _ = frames.send(stats(&sim, tps, last_step_ms, paused, speed));
            last_stats = Instant::now();
        }

        next_deadline += Duration::from_secs_f64(dt / speed);
        // If we fall far behind (slow step, debugger), don't try to catch up in a burst.
        if Instant::now() > next_deadline + Duration::from_millis(250) {
            next_deadline = Instant::now();
        }
    }
}

fn join_messages(sim: &Sim, tps: f32, step_ms: f32, paused: bool, speed: f64) -> Vec<Frame> {
    let ext = sim.world().extent();
    let mut out = vec![frame(&ServerMsg::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        seed: sim.state().config.seed,
        tick: sim.tick(),
        dt: sim.state().config.dt,
        world_min: ext.min.to_array(),
        world_size: ext.size.to_array(),
    }))];
    for (coord, chunk) in sim.world().chunks() {
        out.push(frame(&ServerMsg::Chunk(ChunkMsg::from_chunk(
            *coord, chunk,
        ))));
    }
    out.push(frame(&ServerMsg::AgentFrame(AgentFrame::from_agents(
        sim.tick(),
        sim.agents(),
    ))));
    out.push(frame(&ServerMsg::Stats(Stats {
        tick: sim.tick(),
        tps,
        step_ms,
        agents: sim.agents().len() as u32,
        state_hash: sim.state_hash(),
        paused,
        speed: speed as f32,
    })));
    out
}

fn save(sim: &Sim, dir: &std::path::Path) -> Result<String> {
    std::fs::create_dir_all(dir)?;
    let snap = dir.join(format!("snapshot-{:010}.frmc", sim.tick()));
    std::fs::write(&snap, snapshot::save_state(sim.state())?)?;
    let mut note = format!("saved {} (hash {:016x})", snap.display(), sim.state_hash());
    if let Some(log) = sim.command_log() {
        let path = dir.join(format!("log-{:010}.frmclog", sim.tick()));
        std::fs::write(&path, snapshot::save_log(&log)?)?;
        note.push_str(&format!(" + {}", path.display()));
    }
    Ok(note)
}

async fn ws_handler(ws: WebSocketUpgrade, State(app): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, app))
}

async fn handle_socket(socket: WebSocket, app: AppState) {
    let (mut tx, mut rx) = socket.split();

    // Handshake: the first client message must be a Hello with our protocol version.
    let hello = match rx.next().await {
        Some(Ok(Message::Binary(b))) => sim_proto::decode::<ClientMsg>(&b).ok(),
        _ => None,
    };
    match hello {
        Some(ClientMsg::Hello { protocol_version }) if protocol_version == PROTOCOL_VERSION => {}
        other => {
            let why = format!("expected Hello v{PROTOCOL_VERSION}, got {other:?}");
            tracing::warn!("{why}");
            let _ = tx
                .send(Message::Binary(
                    sim_proto::encode(&ServerMsg::Notice(why)).into(),
                ))
                .await;
            return;
        }
    }

    // Subscribe before asking for the join state so no message falls in the gap. Anything that
    // overlaps is idempotent (frames are full state; edits are absolute).
    let mut frames = app.frames.subscribe();
    let (reply_tx, reply_rx) = oneshot::channel();
    if app.to_sim.send(Inbound::Join(reply_tx)).is_err() {
        return;
    }
    let Ok(initial) = reply_rx.await else { return };
    for f in initial {
        if tx.send(Message::Binary(f.to_vec().into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            msg = frames.recv() => match msg {
                Ok(f) => {
                    if tx.send(Message::Binary(f.to_vec().into())).await.is_err() {
                        break;
                    }
                }
                // Slow viewer: drop what it missed; the next frame is full state anyway.
                Err(broadcast::error::RecvError::Lagged(n)) => tracing::debug!("viewer lagged {n}"),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = rx.next() => match incoming {
                Some(Ok(Message::Binary(b))) => match sim_proto::decode::<ClientMsg>(&b) {
                    Ok(ClientMsg::Command(c)) => { let _ = app.to_sim.send(Inbound::Command(c)); }
                    Ok(ClientMsg::Control(c)) => { let _ = app.to_sim.send(Inbound::Control(c)); }
                    Ok(ClientMsg::Hello { .. }) => {}
                    Err(e) => tracing::warn!("bad client message: {e}"),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Starts the sim thread and serves until the listener fails or the process exits.
pub async fn serve(listener: tokio::net::TcpListener, sim: Sim, cfg: ServerConfig) -> Result<()> {
    let (to_sim, inbox) = std_mpsc::channel();
    let (frames, _) = broadcast::channel(256);
    let static_dir = cfg.static_dir.clone();
    {
        let frames = frames.clone();
        std::thread::Builder::new()
            .name("sim".into())
            .spawn(move || run_sim_loop(sim, inbox, frames, cfg))?;
    }

    let mut app = Router::new().route("/ws", get(ws_handler));
    if let Some(dir) = static_dir {
        app = app.fallback_service(ServeDir::new(dir));
    }
    let app = app.with_state(AppState { to_sim, frames });
    axum::serve(listener, app).await?;
    Ok(())
}

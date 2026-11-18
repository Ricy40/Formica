//! End-to-end: start the server on an ephemeral port, connect over WebSocket, check the join
//! sequence, send a god-tool command and a control, and see their effects streamed back.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sim_core::{Sim, SimCommand, SimConfig};
use sim_proto::{ClientMsg, Control, PROTOCOL_VERSION, ServerMsg};
use sim_server::{ServerConfig, serve};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_msg(ws: &mut Ws) -> ServerMsg {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("server went quiet")
            .expect("stream ended")
            .unwrap();
        if let Message::Binary(b) = m {
            return sim_proto::decode(&b).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, msg: &ClientMsg) {
    ws.send(Message::Binary(sim_proto::encode(msg).into()))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn join_stream_and_commands() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let saves = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ws-saves");
    let cfg = ServerConfig {
        saves_dir: saves.clone(),
        ..Default::default()
    };
    tokio::spawn(serve(
        listener,
        Sim::new(SimConfig::test_chunk(42, 300)),
        cfg,
    ));

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    send(
        &mut ws,
        &ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await;

    // Join sequence: Hello, chunk(s), an agent frame, stats.
    let ServerMsg::Hello(hello) = next_msg(&mut ws).await else {
        panic!("expected Hello")
    };
    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
    assert_eq!(hello.world_size, [32, 32, 32]);
    let ServerMsg::Chunk(chunk) = next_msg(&mut ws).await else {
        panic!("expected Chunk")
    };
    assert_eq!(chunk.expand().unwrap().len(), sim_core::CHUNK_VOLUME);
    let ServerMsg::AgentFrame(frame) = next_msg(&mut ws).await else {
        panic!("expected frame")
    };
    assert_eq!(frame.len(), 300);

    // Frames keep coming with increasing ticks.
    let mut last_tick = frame.tick;
    let mut frames = 0;
    while frames < 5 {
        if let ServerMsg::AgentFrame(f) = next_msg(&mut ws).await {
            assert!(f.tick > last_tick);
            last_tick = f.tick;
            frames += 1;
        }
    }

    // A god-tool command takes effect on the stream.
    send(
        &mut ws,
        &ClientMsg::Command(SimCommand::SpawnAgents { count: 20 }),
    )
    .await;
    loop {
        if let ServerMsg::AgentFrame(f) = next_msg(&mut ws).await
            && f.len() == 320
        {
            break;
        }
    }

    // Voxel edits are streamed as deltas.
    let pos = sim_core::glam::IVec3::new(2, 30, 2);
    send(
        &mut ws,
        &ClientMsg::Command(SimCommand::SetVoxel {
            pos,
            material: sim_core::Material::Rock,
        }),
    )
    .await;
    loop {
        if let ServerMsg::VoxelEdits(e) = next_msg(&mut ws).await {
            assert_eq!(e.edits[0].pos, [2, 30, 2]);
            assert_eq!(e.edits[0].material, sim_core::Material::Rock as u8);
            break;
        }
    }

    // Snapshot control writes a file and announces it.
    send(&mut ws, &ClientMsg::Control(Control::SaveSnapshot)).await;
    loop {
        if let ServerMsg::Notice(n) = next_msg(&mut ws).await {
            assert!(n.starts_with("saved"), "{n}");
            break;
        }
    }
    assert!(
        std::fs::read_dir(&saves)
            .unwrap()
            .any(|e| { e.unwrap().file_name().to_string_lossy().ends_with(".frmc") })
    );

    // Pause stops frames; stats report paused.
    send(&mut ws, &ClientMsg::Control(Control::Pause)).await;
    loop {
        if let ServerMsg::Stats(s) = next_msg(&mut ws).await
            && s.paused
        {
            break;
        }
    }
}

#[tokio::test]
async fn rejects_wrong_protocol_version() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(
        listener,
        Sim::new(SimConfig::test_chunk(1, 1)),
        ServerConfig::default(),
    ));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    send(
        &mut ws,
        &ClientMsg::Hello {
            protocol_version: 999,
        },
    )
    .await;
    let ServerMsg::Notice(n) = next_msg(&mut ws).await else {
        panic!("expected Notice")
    };
    assert!(n.contains("expected Hello"), "{n}");
}

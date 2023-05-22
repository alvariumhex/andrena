#![allow(dead_code)]
#![deny(unsafe_code)]

use actors::{channel_sup::ChannelSupervisor, communication::discord::DiscordActor};
use log::{debug, error, info, warn};
use ractor::Actor;
use serenity::futures::StreamExt;
use tokio::net::TcpListener;

mod actors;
mod ai_context;

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();

    let (_, _) = Actor::spawn(None, DiscordActor, "Lovelace".to_owned())
        .await
        .expect("Failed to spawn actor");

    let (_, _) = Actor::spawn(
        Some("typing".to_owned()),
        actors::communication::typing::TypingActor,
        (),
    )
    .await
    .expect("Failed to spawn actor");

    let server = ractor_cluster::NodeServer::new(
        8022,
        "cookie".to_owned(),
        "andrena".to_owned(),
        "localhost".to_owned(),
        None,
        None,
    );

    let (actor, _) = Actor::spawn(None, server, ())
        .await
        .expect("Failed to spawn actor");

    // cluster interlinking does not work yet so we manually also connect to epeolus
    if let Err(error) = ractor_cluster::node::client::connect(&actor, "127.0.0.1:8023").await {
        warn!("Failed to connect to cluster(epeolus): {}", error);
    } else {
        info!("Connected to cluster");
    }

    let (_, _) = Actor::spawn(Some(String::from("channel_sup")), ChannelSupervisor, ())
        .await
        .expect("Failed to spawn channel supervisor actor");

    // web socket listening thread
    tokio::spawn(async move {
        info!("Initializing websocket server");
        let addr = "0.0.0.0:3001";
        let server = TcpListener::bind(addr).await.unwrap();
        info!("Websocket server listening on {}", addr);
        loop {
            match server.accept().await {
                Ok((stream, _)) => {
                    debug!("Accepted connection from {:?}", stream.peer_addr());

                    let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let (write, read) = socket.split();
                    Actor::spawn(
                        None,
                        actors::communication::websocket::WebSocketActor,
                        (write, read),
                    )
                    .await
                    .expect("Failed to spawn websocket actor");
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    });

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c");
}

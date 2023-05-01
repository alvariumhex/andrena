use actors::gpt::GptActor;
use log::{error, info};

use ractor::Actor;

mod actors;
mod ai_context;

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();

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

    if let Err(error) = ractor_cluster::node::client::connect(&actor, "127.0.0.1:8021").await {
        error!("Failed to connect to cluster: {}", error);
    } else {
        info!("Connected to cluster");
    }

    let (_, _) = Actor::spawn(None, GptActor, ())
        .await
        .expect("Failed to spawn gpt actor");

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c");
}

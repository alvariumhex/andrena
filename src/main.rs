use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, info, trace};

use actix::prelude::*;

use serde::{Deserialize, Serialize};
use paho_mqtt::{self as mqtt};
use tokio::sync::Mutex;
use actors::mqtt::MqttActor;
use async_openai::Client as GptClient;
use crate::actors::mqtt::MqttMessage;
use crate::actors::openai::OpenaiActor;
use crate::actors::typing::TypingActor;

mod actors;
mod ai_context;

#[derive(Debug, Clone, Serialize, Deserialize, Message)]
#[rtype(result = "Vec<(Embedding, f32)>")]
pub struct EmbeddingsRequest {
    pub message: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Message)]
#[rtype(result = "()")]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, String>,
    pub id: String,
    pub timestamp: u64,
}


#[derive(Serialize, Deserialize, Message)]
#[rtype(result = "()")]
struct DiscordMessage {
    author: String,
    content: String,
    attachments: Vec<Attachment>,
    channel: u64,
}

#[derive(Serialize, Deserialize, Message)]
#[rtype(result = "()")]
struct DiscordSend {
    content: String,
    channel: u64,
}

#[derive(Message)]
#[rtype(result = "()")]
struct RegisterActor(pub Addr<MqttActor>);

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Attachment {
    url: String,
    content_type: String,
    title: String,
}

const TOPICS: &[&str] = &["carpenter/discord/receive", "epeolus/response/all"];
const QOS: &[i32] = &[1, 1];

#[actix_rt::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri("mqtt://localhost:1883")
        .finalize();

    let mut async_client = mqtt::AsyncClient::new(create_opts).unwrap();
    let stream = async_client.get_stream(25);
    let mqtt_client = Arc::new(Mutex::new(async_client));

    {
        let client = mqtt_client.lock().await;

        client.connect(None).await.unwrap();
        info!("MQTT connected");
        client.subscribe_many(TOPICS, QOS).await.unwrap();
        info!("Subscribed to: {:?}", TOPICS);
    }

    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let client = GptClient::new().with_api_key(key);

    let typing_actor = TypingActor {
        mqtt_actor: None,
        channels: vec![],
    }.start();

    let openai_actor = OpenaiActor {
        client: client.clone(),
        name: "lovelace".to_owned(),
        context: HashMap::new(),
        model: "gpt-3.5-turbo".to_owned(),
        mqtt_actor: None,
        typing_actor: typing_actor.clone(),
    }.start();

    let mqtt_actor = MqttActor {
        client: mqtt_client.clone(),
        openai_actor: openai_actor.clone(),
    }.start();

    openai_actor.send(RegisterActor(mqtt_actor.clone())).await.unwrap();
    typing_actor.send(RegisterActor(mqtt_actor.clone())).await.unwrap();

   tokio::spawn(async move {
        loop {
            while let Ok(message_options) = stream.recv().await {
                if let Some(message) = message_options {
                    trace!("Received message: {:?}", message);
                    mqtt_actor.send(MqttMessage(message.clone())).await.unwrap();
                } else {
                    error!("Lost connection. Attempting reconnect.");
                    let client = mqtt_client.lock().await;
                    while let Err(err) = client.reconnect().await {
                        println!("Error reconnecting: {}", err);
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                    }
                    info!("Reconnected");
                    client.subscribe_many(TOPICS, QOS).await.unwrap();
                }
            }
        }
    }).await.unwrap();
}

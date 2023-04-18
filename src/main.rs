use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use ai_context::AiContext;
use async_openai::error::OpenAIError;
use async_openai::Client as GptClient;

use async_openai::types::CreateChatCompletionRequestArgs;
use async_recursion::async_recursion;
use log::{debug, error, info, trace};

use mqtt::AsyncClient;
use serde::{Deserialize, Serialize};
use actix::prelude::*;

use paho_mqtt::{self as mqtt};
use tiktoken_rs::get_chat_completion_max_tokens;
use tokio::join;
use tokio::sync::mpsc::{channel as tokio_channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::{self};
use actors::mqtt_actor::MqttActor;

use crate::actors::mqtt_actor::MqttMessage;
use crate::actors::openai_actor::OpenaiActor;
use crate::actors::typing_actor::TypingActor;

mod actors;
mod ai_context;

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



#[derive(Debug, Serialize, Deserialize)]
struct EmbeddingsQuery {
    embedding: Vec<f32>,
    limit: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct EmbeddingsResponse {
    embeddings: Vec<(String, f32)>,
}

pub struct GptContext {
    name: String,
    client: GptClient,
    model: String,
    context: HashMap<String, AiContext>,
}

impl GptContext {
    #[async_recursion]
    async fn get_response(
        &mut self,
        channel_id: &str,
        mqtt_client: &AsyncClient,
        embeddings_rx: &mut Receiver<String>,
    ) -> Result<String, &str> {
        let context = self.context.get_mut(channel_id).unwrap();

        // context.retain(|m| {
        //     match m.role {
        //         Role::User => true,
        //         _ => false,
        //     }
        // });
        // let content = context.iter().fold(String::new(), |a, b| {
        //     format!("{}\n{}", a, b.content)
        // });
        // let request = CreateEmbeddingRequestArgs::default()
        //     .model("text-embedding-ada-002")
        //     .input([content.clone()])
        //     .build()
        //     .unwrap();

        // let response = self.client.embeddings().create(request).await.unwrap();
        // let vector = response.data[0].embedding.clone();
        // let embedding = EmbeddingsQuery {
        //     embedding: vector,
        //     limit: 3,
        // };
        // let json = serde_json::to_string(&embedding).unwrap();
        // let mqtt_message = paho_mqtt::Message::new("epeolus/query/all", json, 1);
        // mqtt_client
        //     .publish(mqtt_message)
        //     .await
        //     .expect("Failed to publish message");

        // let embeddings_message = embeddings_rx
        //     .recv()
        //     .await
        //     .expect("Failed to receive message");
        // let response: EmbeddingsResponse = serde_json::from_str(&embeddings_message).unwrap();
        // for embedding in response.embeddings {
        //     if embedding.1 >= 0.2 {
        //         continue;
        //     }
        //     debug!("Adding embedding: {:?}", embedding);
        // context.push(
        //     ChatCompletionRequestMessageArgs::default()
        //         .role(Role::User)
        //         .content(&embedding.0)
        //         .name("SYSTEM")
        //         .build()
        //         .unwrap(),
        // );
        // }

        context.manage_tokens(&self.model);

        let max_tokens = get_chat_completion_max_tokens(&self.model, &context.to_chat_history()).unwrap();

        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens((max_tokens as u16) - 110)
            .model(&self.model)
            .messages(context.to_chat_history())
            .build()
            .expect("Failed to build request");

        let response = self.client.chat().create(request).await;
        match response {
            Ok(response) => {
                if let Some(usage) = response.usage {
                    debug!("tokens: {}", usage.total_tokens);
                }

                if let Some(resp) = response.choices.first() {
                    Ok(resp.message.content.clone())
                } else {
                    Err("No choices")
                }
            }
            Err(OpenAIError::ApiError(err)) => {
                error!("Request error: {:?}", err);
                if let Some(code) = err.code {
                    if code == "context_length_exceeded" {
                        Err("Context length exceeded")
                    } else {
                        Err("Request error")
                    }
                } else {
                    Err("Request error")
                }
            }
            _ => Err("Request error"),
        }
    }

    async fn new(name: &str, model: &str, key: &str) -> Result<Self, Box<dyn Error>> {
        let client = GptClient::new().with_api_key(key);

        let context = HashMap::new();
        Ok(Self {
            name: name.to_owned(),
            client,
            model: model.to_owned(),
            context,
        })
    }

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

    let gpt = GptContext::new(
        "Lovelace",
        "gpt-3.5-turbo",
        "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",
    ).await.unwrap();

    let typing_actor = TypingActor {
        mqtt_actor: None,
        channels: vec![],
    }.start();

    let openai_actor = OpenaiActor {
        client: gpt.client.clone(),
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

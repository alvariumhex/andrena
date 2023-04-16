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

    async fn insert_message(&mut self, message: &DiscordMessage) {
        let mut content: String = message.content.to_owned();

        content = content.trim().to_owned();
        let model = self.model.clone();

        // insert into context for channel id
        let channel_id = message.channel.to_string();
        let context = self.get_context_for_id(&channel_id);
        context.push_history((message.author.clone(), content.clone()));
        context.manage_tokens(&model);
    }

    fn get_context_for_id(&mut self, channel_id: &str) -> &mut AiContext {
        match self.context.contains_key(channel_id) {
            true => self.context.get_mut(channel_id).unwrap(),
            false => {
                let mut context = AiContext::new();
                context.set_static_context("You can use following emotes in the conversation if you see fit, each emote has a meaning next to it in one or multiple words, the next emote is denoted with a ; : <:kekdog:1090251469988573184> big laughter; <a:mwaa:1090251284617101362> frustration; <:oof:1090251382801571850> dissapointment or frustration; <:finnyikes:1090251493950627942> uncomfortable disgusted dissapointment; <a:catpls:1090251360693407834> silly mischievious; <:gunma:1090251357316988948> demanding angry; <:snicker:1091728748145033358> flabergasted suprised; <:crystalheart:1090323901583736832> love appreciation; <:dogwat:1090253587273236580> disbelief suprise");
                self.context.insert(channel_id.to_owned(), context);
                self.context.get_mut(channel_id).unwrap()
            }
        }
    }
}

const TOPICS: &[&str] = &["carpenter/discord/receive", "epeolus/response/all"];
const QOS: &[i32] = &[1, 1];

async fn process_discord_message(
    gpt: &mut GptContext,
    mqtt_message: &str,
    channel: &Mutex<String>,
    mqtt_client: &AsyncClient,
    embeddings_rx: &mut Receiver<String>,
) {
    let discord_message: DiscordMessage = serde_json::from_str(&mqtt_message).unwrap();

    gpt.insert_message(&discord_message).await;
    if discord_message.author.to_lowercase() != gpt.name.to_lowercase()
        && discord_message
            .content
            .to_lowercase()
            .contains(gpt.name.to_lowercase().as_str())
    {
        {
            debug!("Setting channel to: {}", discord_message.channel);
            let mut channel = channel.lock().await;
            *channel = discord_message.channel.to_string();
        } // drop lock

        let response = gpt
            .get_response(
                &discord_message.channel.to_string(),
                mqtt_client,
                embeddings_rx,
            )
            .await
            .unwrap(); // split message only if needed as discord has a 2k character limit
        debug!("response: {}", response);

        let json = serde_json::to_string(&DiscordSend {
            content: response,
            channel: discord_message.channel,
        })
        .unwrap();
        let send_message = mqtt::Message::new("carpenter/discord/send", json.as_str(), mqtt::QOS_1);
        mqtt_client
            .publish(send_message)
            .await
            .expect("Failed to send message");

        debug!("Cleaning up message receival");
        {
            let mut channel = channel.lock().await;
            *channel = String::new();
        } // drop lock
    }
}

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

    let channel = Arc::new(Mutex::new(String::new()));

    let typing_channel = channel.clone();

    let local_client = mqtt_client.clone();
    let typing_task = tokio::task::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let client = local_client.lock().await;
            let typing_channel = typing_channel.lock().await;
            // debug!("typing channel: {}", typing_channel);
            if typing_channel.is_empty() {
                continue;
            }
            trace!("sending typing message: {}", typing_channel);
            let typing_message = mqtt::Message::new(
                "carpenter/discord/typing",
                typing_channel.clone(),
                mqtt::QOS_0,
            );
            client.publish(typing_message);
        }
    });

    let gpt = GptContext::new(
        "Lovelace",
        "gpt-3.5-turbo",
        "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",
    ).await.unwrap();

    let openai_actor = OpenaiActor {
        client: gpt.client.clone(),
        name: "lovelace".to_owned(),
        context: HashMap::new(),
        model: "gpt-3.5-turbo".to_owned(),
        mqtt_actor: None,
    }.start();

    let mqtt_actor = MqttActor {
        client: mqtt_client.clone(),
        openai_actor: openai_actor.clone(),
    }.start();

    openai_actor.send(RegisterActor(mqtt_actor.clone())).await.unwrap();

    let mqtt_task = tokio::spawn(async move {
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
    });

    join!(mqtt_task, typing_task);
}

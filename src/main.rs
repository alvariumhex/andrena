use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_openai::error::OpenAIError;
use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};
use async_openai::types::{CreateChatCompletionRequestArgs, CreateEmbeddingRequestArgs};
use async_openai::Client as GptClient;

use async_recursion::async_recursion;
use log::{debug, error, info, trace};

use mqtt::AsyncClient;
use serde::{Deserialize, Serialize};

use tiktoken_rs::model::get_context_size;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

use regex::Regex;

use paho_mqtt::{self as mqtt};
use tokio::sync::mpsc::{channel as tokio_channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::{self};

use crate::context::text_attachment::TextAttachment;
use crate::context::traits::ContextItem;
use crate::context::youtube_video::YoutubeVideo;
mod context;

#[derive(Serialize, Deserialize)]
struct DiscordMessage {
    author: String,
    content: String,
    attachments: Vec<Attachment>,
    channel: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Attachment {
    url: String,
    content_type: String,
    title: String,
}

#[derive(Serialize, Deserialize)]
struct DiscordSend {
    content: String,
    channel: u64,
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
    context: HashMap<String, Vec<ChatCompletionRequestMessage>>,
    token_count: HashMap<String, u16>,
}

impl GptContext {
    #[async_recursion]
    async fn get_response(&mut self, channel_id: &str, mqtt_client: &AsyncClient, embeddings_rx: &mut Receiver<String>) -> Result<String, &str> {
        let context = self.context.get_mut(channel_id).unwrap();

        context.retain(|m| {
            match m.role {
                Role::User => true,
                _ => false,
            }
        });
        let content = context.iter().fold(String::new(), |a, b| {
            format!("{}\n{}", a, b.content)
        });
        let request = CreateEmbeddingRequestArgs::default()
            .model("text-embedding-ada-002")
            .input([content.clone()])
            .build()
            .unwrap();

        let response = self.client.embeddings().create(request).await.unwrap();
        let vector = response.data[0].embedding.clone();
        let embedding = EmbeddingsQuery {
            embedding: vector,
            limit: 3,
        };
        let json = serde_json::to_string(&embedding).unwrap();
        let mqtt_message = paho_mqtt::Message::new("epeolus/query/all", json, 1);
        mqtt_client
            .publish(mqtt_message)
            .await
            .expect("Failed to publish message");

        let embeddings_message = embeddings_rx
            .recv()
            .await
            .expect("Failed to receive message");
        let response: EmbeddingsResponse = serde_json::from_str(&embeddings_message).unwrap();
        for embedding in response.embeddings {
            if embedding.1 >= 0.2 {
                continue;
            }
            debug!("Adding embedding: {:?}", embedding);
            context.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .content(&embedding.0)
                    .name("SYSTEM")
                    .build()
                    .unwrap(),
            );
        }

        let token_count = manage_tokens(&self.model, context).await;

        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens((token_count as u16) - 110) // Token count is not perfect. So we
            // remove some extras to prevent requesting to many tokens
            .model(&self.model)
            .messages(context.clone())
            .build()
            .expect("Failed to build request");
        match self.client.chat().create(request).await {
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
                        // self.context.remove(0);
                        // self.get_response().await
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
        let token_count = HashMap::new();
        Ok(Self {
            name: name.to_owned(),
            client,
            model: model.to_owned(),
            context,
            token_count,
        })
    }

    async fn insert_message(
        &mut self,
        message: &DiscordMessage,
    ) {
        let mut content: String = message.content.to_owned();
        let attachment_text = get_attachment_text(&message.attachments)
            .await
            .expect("Failed to read attachments");
        content = format!("{} \n {}", content, attachment_text);

        let bpe = get_bpe_from_model(&self.model).unwrap();
        let token_count_message = bpe.encode_with_special_tokens(&content).len();

        // web content
        let web_contents = get_web_content(&message.content).await;
        let token_count_web_content = bpe.encode_with_special_tokens(&web_contents).len();

        debug!("Token count of web content: {}", token_count_web_content);
        debug!("Token count of message: {}", token_count_message);

        if (token_count_web_content + token_count_message) < get_context_size(&self.model) - 200 {
            content = format!("{} \n {}", content, web_contents);
        } else {
            debug!("Web url content longer than context");
            // Explain to GPT that the article couldn't be included, this prevents GPT of
            // hallucinating an answer based on the url
            content = format!("{} \n {}", content, "SYSTEM: The content was too long to include in the chat. Mention that you can only assume content based on the url")
        }

        // video content
        let video_contents = get_video_transcription(&message.content, &self.client).await;
        let token_count_video_content = bpe.encode_with_special_tokens(&video_contents).len();
        if (token_count_video_content + token_count_message) < get_context_size(&self.model) - 200 {
            content = format!("{} \n {}", content, video_contents);
        } else {
            debug!("Video transcription longer than contenxt");
            content = format!(
                "{} \n {}",
                content, "SYSTEM: The video content was too long to include in the chat."
            );
        }

        content = content.trim().to_owned();
        debug!("end message content: {}", content);

        // insert into context for channel id
        let channel_id = message.channel.to_string();
        let mut context = self.get_context_for_id(&channel_id);

        context.push(
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content(&content)
                .name(message.author.clone())
                .build()
                .unwrap(),
        );

        manage_tokens(&self.model, &mut context).await;

        self.set_context_for_id(&channel_id, context);
    }

    fn set_context_for_id(
        &mut self,
        channel_id: &String,
        context: Vec<ChatCompletionRequestMessage>,
    ) {
        self.context.insert(channel_id.clone(), context);
    }

    fn get_context_for_id(&self, channel_id: &str) -> Vec<ChatCompletionRequestMessage> {
        match self.context.contains_key(channel_id) {
            true => self.context.get(channel_id).unwrap().to_owned(),
            false => vec![
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .content("You can use following emotes in the conversation if you see fit, each emote has a meaning next to it in one or multiple words, the next emote is denoted with a ; : <:kekdog:1090251469988573184> big laughter; <a:mwaa:1090251284617101362> frustration; <:oof:1090251382801571850> dissapointment or frustration; <:finnyikes:1090251493950627942> uncomfortable disgusted dissapointment; <a:catpls:1090251360693407834> silly mischievious; <:gunma:1090251357316988948> demanding angry; <:snicker:1091728748145033358> flabergasted suprised; <:crystalheart:1090323901583736832> love appreciation; <:dogwat:1090253587273236580> disbelief suprise")
                    .name("SYSTEM")
                    .build()
                    .unwrap(),
            ],
        }
    }
}

async fn manage_tokens(model: &str, context: &mut Vec<ChatCompletionRequestMessage>) -> usize {
    let mut token_count = get_chat_completion_max_tokens(model, &context)
        .expect("Failed to get max tokens");
    while token_count < 750 {
        info!("Reached max token count, removing oldest message from context");
        context.remove(0);
        token_count = get_chat_completion_max_tokens(model, &context)
            .expect("Failed to get max tokens");
    }

    token_count
}

pub async fn get_video_transcription(content: &str, client: &GptClient) -> String {
    let regex = Regex::new(r"(?m)((?:https?:)?//)?((?:www|m)\.)?((?:youtube(-nocookie)?\.com|youtu.be))(/(?:[\w\-]+\?v=|embed/|v/)?)([\w\-]+)(\S+)?").unwrap();
    let captures = regex.captures_iter(content);
    let mut content = String::new();
    for capture in captures {
        debug!("captures: {:?}", capture);
        if let Some(url) = capture.get(0) {
            trace!("Found youtube link in message: {}", url.as_str());
            let mut video = YoutubeVideo::new(url.as_str().to_owned());
            video.fetch_metadata().await;
            video.fetch_transcription(client).await;

            content = format!("{} \n {}", content, video.raw_text());
        }
    }
    debug!("Video content: {}", content);
    content.trim().to_owned()
}

pub async fn get_web_content(content: &str) -> String {
    let regex = Regex::new(r"(?m)https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{2,256}\.[a-z]{2,4}\b([-a-zA-Z0-9@:%_\+.~#?&//=]*)").unwrap();

    let mut web_contents: Vec<String> = vec![];
    let captures = regex.captures_iter(content);
    for capture in captures {
        if let Some(url) = capture.get(0) {
            trace!("Found link in message: {}", url.as_str());
            let query = [
                ("url", url.as_str()),
                ("apikey", "26c6635eb2f70e76292f938d8cc64f2ffec5074a"),
            ];
            let client = reqwest::Client::new();
            let response = client
                .get("http://localhost:8080/article")
                .query(&query)
                .send()
                .await
                .unwrap();

            let url_content = response.text().await.unwrap();
            trace!("Web contents: {}", url_content);
            web_contents.push(format!(
                "\n link: {} \n link content: {}",
                url.as_str(),
                url_content
            ));
        };
    }

    web_contents.join("\n")
}

async fn get_attachment_text(attachments: &[Attachment]) -> Result<String, Box<dyn Error>> {
    let mut content: String = String::new();
    for attachment in attachments {
        debug!("{:?}", attachment);

        if attachment.content_type == "text/plain; charset=utf-8" {
            trace!("Found text file in message, extracting content");
            let mut text_attachment = TextAttachment::new(attachment.url.clone());
            text_attachment
                .fetch_content()
                .await
                .expect("Failed to fetch attachment text");
            content = format!("{} \n {}", content, text_attachment.raw_text());
        }
    }

    Ok(content.trim().to_owned())
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

    gpt.insert_message(&discord_message)
        .await;
    trace!(
        "remaining token count: {}",
        gpt.token_count
            .get(&discord_message.channel.to_string())
            .unwrap_or(&0)
    );

    if discord_message.author.to_lowercase() != gpt.name.to_lowercase()
        && discord_message
            .content
            .to_lowercase()
            .contains(gpt.name.to_lowercase().as_str())
    {
        {
            let mut channel = channel.lock().await;
            *channel = discord_message.channel.to_string();
        } // drop lock

        let response = gpt
            .get_response(&discord_message.channel.to_string(), mqtt_client, embeddings_rx)
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

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();

    let mut gpt = GptContext::new(
        "Lovelace",
        "gpt-3.5-turbo",
        "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",
    )
    .await
    .expect("Failed to create GptContext");

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri("mqtt://localhost:1883")
        .finalize();

    let mut async_client = mqtt::AsyncClient::new(create_opts).unwrap();
    let stream = async_client.get_stream(25);
    let mqtt_client = Arc::new(Mutex::new(async_client));

    let (discord_tx, mut discord_rx): (Sender<String>, Receiver<String>) = tokio_channel(1);
    let (embeddings_tx, mut embeddings_rx): (Sender<String>, Receiver<String>) = tokio_channel(1);

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
    tokio::spawn(async move {
        loop {
            while let Some(message) = discord_rx.recv().await {
                let mqtt_client = local_client.lock().await;
                process_discord_message(
                    &mut gpt,
                    &message,
                    &channel,
                    &mqtt_client,
                    &mut embeddings_rx,
                )
                .await;
            }
        }
    });

    let local_client = mqtt_client.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let client = local_client.lock().await;
            let typing_channel = typing_channel.lock().await;
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

    tokio::spawn(async move {
        loop {
            while let Ok(message_options) = stream.recv().await {
                if let Some(message) = message_options {
                    if message.topic().starts_with("carpenter/discord/receive") {
                        let json_string = String::from_utf8(message.payload().to_vec()).unwrap();
                        discord_tx.send(json_string).await.unwrap();
                    } else if message.topic().starts_with("epeolus/response/all") {
                        let json_string = String::from_utf8(message.payload().to_vec()).unwrap();
                        embeddings_tx.send(json_string).await.unwrap();
                    }
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
    })
    .await
    .unwrap();
}

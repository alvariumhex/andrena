use std::error::Error;
use std::time::Duration;

use async_openai::error::OpenAIError;
use async_openai::types::CreateChatCompletionRequestArgs;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, CreateTranscriptionRequestArgs,
    Role,
};
use async_openai::Client as GptClient;

use async_recursion::async_recursion;
use log::{debug, error, info, trace};

use rustube::{Id, VideoFetcher};
use serde::{Deserialize, Serialize};

use tiktoken_rs::model::get_context_size;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

use regex::Regex;

use paho_mqtt::{self as mqtt, MQTT_VERSION_5};

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

pub struct GptContext {
    name: String,
    client: GptClient,
    model: String,
    context: Vec<ChatCompletionRequestMessage>,
    token_count: usize,
}

impl GptContext {
    #[async_recursion]
    async fn get_response(&mut self) -> Result<String, &str> {
        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens((self.token_count as u16) - 110) // Token count is not perfect. So we
            // remove some extras to prevent requesting to many tokens
            .model(&self.model)
            .messages(self.context.clone())
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
                        self.context.remove(0);
                        self.get_response().await
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
        let context = vec![
            ChatCompletionRequestMessageArgs::default()
                .role(Role::System)
                .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. The names of the people you talk with are Xuna & Amo. They are both female.")
                .build()
                .unwrap(),
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. The names of the people you talk with are Xuna & Amo. They are both female.")
                .build()
                .unwrap(),
        ];
        let token_count = get_chat_completion_max_tokens(model, &context)?;
        Ok(Self {
            name: name.to_owned(),
            client,
            model: model.to_owned(),
            context,
            token_count,
        })
    }

    async fn manage_tokens(&mut self) {
        self.token_count = get_chat_completion_max_tokens(&self.model, &self.context)
            .expect("Failed to get max tokens");
        while self.token_count < 750 {
            info!("Reached max token count, removing oldest message from context");
            self.context.remove(0);
            self.token_count = get_chat_completion_max_tokens(&self.model, &self.context)
                .expect("Failed to get max tokens");
        }
    }

    async fn insert_message(&mut self, message: &DiscordMessage) {
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

        self.context.push(
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content(&content)
                .name(message.author.clone())
                .build()
                .unwrap(),
        );
    }
}

pub async fn get_video_transcription(content: &str, client: &GptClient) -> String {
    let regex = Regex::new(r"(?m)((?:https?:)?//)?((?:www|m)\.)?((?:youtube(-nocookie)?\.com|youtu.be))(/(?:[\w\-]+\?v=|embed/|v/)?)([\w\-]+)(\S+)?").unwrap();
    let captures = regex.captures_iter(content);
    let mut content = String::new();
    for capture in captures {
        debug!("captures: {:?}", capture);
        if let Some(url) = capture.get(0) {
            trace!("Found youtube link in message: {}", url.as_str());
            let id = Id::from_raw(url.as_str()).unwrap();
            let descrambler = VideoFetcher::from_id(id.into_owned())
                .unwrap()
                .fetch()
                .await
                .expect("Failed to get video metadata");
            let video = descrambler.descramble().unwrap();
            let path_to_video = video
                .worst_audio()
                .unwrap()
                .download()
                .await
                .expect("Could not download video audio");

            debug!(
                "Downloaded video audio: {}, {:?}",
                url.as_str(),
                path_to_video.to_str()
            );

            let request = CreateTranscriptionRequestArgs::default()
                .file(path_to_video.to_str().unwrap())
                .model("whisper-1")
                .build()
                .unwrap();
            trace!("Starting transcription request");
            let response = client
                .audio()
                .transcribe(request)
                .await
                .expect("Failed to fetch transcription")
                .text;
            content = format!(
                "{} \n video title: {} \n video transcription: {}",
                content,
                video.title(),
                response
            );
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

async fn get_attachment_text(attachments: &Vec<Attachment>) -> Result<String, Box<dyn Error>> {
    let mut content: String = String::new();
    for attachment in attachments.clone() {
        debug!("{:?}", attachment);
        if attachment.content_type == "text/plain; charset=utf-8" {
            trace!("Found text file in message, extracting content");
            let response = reqwest::get(attachment.url.clone()).await?;
            let bytes = response.bytes().await?;

            let attachment_text = String::from_utf8_lossy(&bytes).into_owned();
            content = format!("{} \n {}", content, attachment_text);
        }
    }

    Ok(content.trim().to_owned())
}

const TOPICS: &[&str] = &["carpenter/discord/receive"];
const QOS: &[i32] = &[1];

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();

    let mut gpt = GptContext::new(
        "Lovelace",
        "gpt-4-0314",
        "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",
    )
    .await
    .expect("Failed to create GptContext");

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri("mqtt://localhost:1883")
        .finalize();

    let mut client = mqtt::AsyncClient::new(create_opts).unwrap();
    let stream = client.get_stream(25);
    let conn_opts = mqtt::ConnectOptionsBuilder::with_mqtt_version(MQTT_VERSION_5)
        .clean_start(false)
        .properties(mqtt::properties![mqtt::PropertyCode::SessionExpiryInterval => 3600])
        .finalize();

    client.connect(conn_opts).await.unwrap();
    info!("MQTT connected");
    let sub_opts = vec![mqtt::SubscribeOptions::with_retain_as_published(); TOPICS.len()];
    client
        .subscribe_many_with_options(TOPICS, QOS, &sub_opts, None)
        .await
        .unwrap();
    info!("Subscribed to: {:?}", TOPICS);

    tokio::spawn(async move {
        loop {
            while let Ok(message_options) = stream.recv().await {
                if let Some(message) = message_options {
                    if message.retained() {
                        info!("(R) ");
                    }
                    let json_string: String =
                        String::from_utf8(message.payload().to_vec()).unwrap();
                    let discord_message: DiscordMessage =
                        serde_json::from_str(&json_string).unwrap();
                    debug!("{}: {}", discord_message.author, discord_message.content);


                    gpt.insert_message(&discord_message).await;
                    gpt.manage_tokens().await;
                    trace!("remaining token count: {}", gpt.token_count);

                    if discord_message.author.to_lowercase() != "lovelace" && discord_message.content.to_lowercase().contains("lovelace") {
                        let response = gpt.get_response().await.unwrap(); // split message only if needed as discord has a 2k character limit
                        debug!("response: {}", response);

                        let json = serde_json::to_string(&DiscordSend { content: response, channel: discord_message.channel }).unwrap();
                        let send_message = mqtt::Message::new("carpenter/discord/send", json.as_str(), mqtt::QOS_2);
                        client.publish(send_message).await.expect("Failed to send message");
                    }
                } else {
                    error!("Lost connection. Attempting reconnect.");
                    while let Err(err) = client.reconnect().await {
                        println!("Error reconnecting: {}", err);
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                    }
                }
            }
        }
    }).await.unwrap();
}

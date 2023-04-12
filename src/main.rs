use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_openai::error::OpenAIError;
use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};
use async_openai::types::{CreateChatCompletionRequestArgs};
use async_openai::Client as GptClient;

use async_recursion::async_recursion;
use log::{debug, error, info, trace};

use serde::{Deserialize, Serialize};

use tiktoken_rs::model::get_context_size;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

use regex::Regex;

use paho_mqtt::{self as mqtt};
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

pub struct Embedding {
    text: String,
    embedding: Vec<f32>,
}

pub struct GptContext {
    name: String,
    client: GptClient,
    model: String,
    context: Vec<ChatCompletionRequestMessage>,
    token_count: usize,
    embeddings: Vec<Embedding>,
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
                    // let request = CreateEmbeddingRequestArgs::default()
                    //     .model("text-embedding-ada-002")
                    //     .input([self.context.last().unwrap().content.clone()])
                    //     .build()
                    //     .unwrap();
                    // let response = self.client.embeddings().create(request).await.unwrap();
                    // let query_embedding = response.data.first().unwrap().clone();
                    // // compare embedding to all other embeddings
                    // let mut matrix: Vec<(f32, String)> = vec![];
                    // for embedding in &self.embeddings {
                    //     let distance = cosine_dist_rust_loop_vec(
                    //         &embedding.embedding[..],
                    //         &query_embedding.embedding[..],
                    //         &1536,
                    //     );
                    //     matrix.push((distance, embedding.text.clone()));
                    // }
                    // warn!("Matrix length: {:?}", matrix.len());
                    // let display_matrix: Vec<(f32, String)> = matrix
                    //     .iter()
                    //     .map(|e| (e.0, e.1[0..20].to_owned()))
                    //     .collect();
                    // warn!("Matrix: {:?}", display_matrix);

                    // matrix.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                    // let closest = matrix.first().unwrap();
                    // // insert into context
                    // self.context.push(
                    //     ChatCompletionRequestMessageArgs::default()
                    //         .role(Role::Assistant)
                    //         .content(closest.1.clone())
                    //         .build()
                    //         .unwrap(),
                    // );
                    // // request new response
                    // let tokens: u16 = 4000;
                    // let request = CreateChatCompletionRequestArgs::default()
                    //     .max_tokens(tokens)
                    //     .model(&self.model)
                    //     .messages(self.context.clone())
                    //     .build()
                    //     .expect("Failed to build request");

                    // let response = self.client.chat().create(request).await.unwrap();
                    // let final_response = format!("**Without embeddings:** {}\n\n\n**With embeddings:** {}", resp.message.content, response.choices.first().unwrap().message.content);

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
            // ChatCompletionRequestMessageArgs::default()
            //     .role(Role::System)
            //     .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. Do not address people in the conversation using mr/sir or any other form of respect.")
            //     .build()
            //     .unwrap(),
            // ChatCompletionRequestMessageArgs::default()
            //     .role(Role::User)
            //     .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. Avoid formal addressing of people. If you address people, use the female form.")
            //     .build()
            //     .unwrap(),
        ];
        let token_count = get_chat_completion_max_tokens(model, &context)?;
        Ok(Self {
            name: name.to_owned(),
            client,
            model: model.to_owned(),
            context,
            token_count,
            embeddings: vec![],
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

fn cosine_dist_rust_loop_vec(vec_a: &[f32], vec_b: &[f32], vec_size: &i32) -> f32 {
    let mut a_dot_b: f32 = 0.0;
    let mut a_mag: f32 = 0.0;
    let mut b_mag: f32 = 0.0;

    for i in 0..*vec_size as usize {
        a_dot_b += vec_a[i] * vec_b[i];
        a_mag += vec_a[i] * vec_a[i];
        b_mag += vec_b[i] * vec_b[i];
    }

    1.0 - (a_dot_b / (a_mag.sqrt() * b_mag.sqrt()))
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
        "gpt-3.5-turbo",
        "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",
    )
    .await
    .expect("Failed to create GptContext");

    // let request = CreateEmbeddingRequestArgs::default()
    //     .model("text-embedding-ada-002")
    //     .input([AWS_GG_CLI_COMP, AWS_GG_CREATE_COMPONENT, RUST_INSTALLATION])
    //     .build()
    //     .unwrap();

    // let response = gpt.client.embeddings().create(request).await.unwrap();
    // for data in response.data {
    //     println!("embedding length: {:?}", data.embedding.len());
    //     if data.index == 0 {
    //         gpt.embeddings.push(Embedding {
    //             embedding: data.embedding,
    //             text: AWS_GG_CLI_COMP.to_string(),
    //         });
    //     } else if data.index == 1 {
    //         gpt.embeddings.push(Embedding {
    //             embedding: data.embedding,
    //             text: AWS_GG_CREATE_COMPONENT.to_string(),
    //         });
    //     } else if data.index == 2 {
    //         gpt.embeddings.push(Embedding {
    //             embedding: data.embedding,
    //             text: RUST_INSTALLATION.to_string(),
    //         });
    //     }
    // }

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri("mqtt://localhost:1883")
        .finalize();

    let mut async_client = mqtt::AsyncClient::new(create_opts).unwrap();
    let stream = async_client.get_stream(25);
    let mqtt_client = Arc::new(Mutex::new(async_client)) ;

    {
        let client = mqtt_client.lock().await;
    
        client.connect(None).await.unwrap();
        info!("MQTT connected");
        client.subscribe_many(TOPICS, QOS).await.unwrap();
        info!("Subscribed to: {:?}", TOPICS);
    }

    let channel = Arc::new(Mutex::new(String::new()));

    let typing_channel = channel.clone();
    let typing_client = mqtt_client.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let client = typing_client.lock().await;
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

    let messaging_client = mqtt_client.clone();
    tokio::spawn(async move {
        loop {
            while let Ok(message_options) = stream.recv().await {
                if let Some(message) = message_options {
                    let json_string: String =
                        String::from_utf8(message.payload().to_vec()).unwrap();
                    let discord_message: DiscordMessage =
                        serde_json::from_str(&json_string).unwrap();

                    gpt.insert_message(&discord_message).await;
                    gpt.manage_tokens().await;
                    trace!("remaining token count: {}", gpt.token_count);

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

                        let response = gpt.get_response().await.unwrap(); // split message only if needed as discord has a 2k character limit
                        debug!("response: {}", response);

                        let json = serde_json::to_string(&DiscordSend {
                            content: response,
                            channel: discord_message.channel,
                        })
                        .unwrap();
                        let send_message = mqtt::Message::new(
                            "carpenter/discord/send",
                            json.as_str(),
                            mqtt::QOS_0,
                        );
                        let client = messaging_client.lock().await;
                        client
                            .publish(send_message)
                            .await
                            .expect("Failed to send message");
                        
                        {
                            let mut channel = channel.lock().await;
                            *channel = String::new();
                        } // drop lock
                    }
                } else {
                    error!("Lost connection. Attempting reconnect.");
                    let client = messaging_client.lock().await;
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

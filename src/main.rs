use std::error::Error;
use std::sync::Arc;

use async_openai::error::OpenAIError;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs,
    CreateChatCompletionRequestArgs, Role,
};
use async_openai::Client as GptClient;

use async_recursion::async_recursion;
use log::{debug, error, info, warn};

use serenity::model::channel::Message;
use serenity::{async_trait, prelude::*, Client};

use tiktoken_rs::get_chat_completion_max_tokens;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, context: Context, message: Message) {
        debug!("{}: {}", message.author.name, message.content);
        let data_read = context.data.read().await;
        let mut gpt = data_read
            .get::<GptContext>()
            .expect("No Gpt Context")
            .lock()
            .await;

        gpt.insert_message(&message).await;
        gpt.manage_tokens().await;
        debug!("token count: {}", gpt.token_count);

        if !message.author.bot
            && message
                .content
                .to_lowercase()
                .contains(&gpt.name.to_lowercase())
        {
            let typing = message.channel_id.start_typing(&context.http).unwrap();

            let response = gpt.get_response().await.unwrap();            // split message only if needed as discord has a 2k character limit
            for content in split_string(&response) {
                message
                    .channel_id
                    .say(&context.http, content)
                    .await
                    .expect("failed to send message");
            }
            typing.stop().unwrap();
        }
    }
}

struct GptContext {
    name: String,
    client: GptClient,
    model: String,
    context: Vec<ChatCompletionRequestMessage>,
    token_count: usize,
}

impl GptContext {
    #[async_recursion]
    async fn get_response(&mut self) -> Result<String, &str>{
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
                // .content("I want you to act as a personal assistant in a conversation. Each message will start with the persons name and then their message content. You're behaviour should match that of Jarvis from Iron Man. You will respond exactly as Jarvis. You're name is Lovelace instead of Jarvis. Do not prefix your messages with 'Lovelace:'.")
                .build()
                .unwrap(),
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                // .content("I want you to act as a personal assistant in a conversation. Each message will start with the persons name and then their message content. You're behaviour should match that of Jarvis from Iron Man. You will respond exactly as Jarvis. You're name is Lovelace instead of Jarvis. Do not prefix your messages with 'Lovelace:'.")
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

    async fn insert_message(&mut self, message: &Message) {
        let mut content: String = message.content.to_owned();
        let attachment_text = message
            .get_attachment_text()
            .await
            .expect("Failed to read attachments");
        content = format!("{} \n {}", content, attachment_text);

        self.context.push(
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content(&content)
                .name(message.author.name.clone())
                .build()
                .unwrap(),
        );
    }
}

#[async_trait]
trait MessageExtensions {
    async fn get_attachment_text(&self) -> Result<String, Box<dyn Error>>;
}

#[async_trait]
impl MessageExtensions for Message {
    async fn get_attachment_text(&self) -> Result<String, Box<dyn Error>> {
        let mut content: String = self.content.to_owned();
        for attachment in self.attachments.clone() {
            debug!("{:?}", attachment);
            if let Some(content_type) = attachment.content_type {
                if content_type == "text/plain; charset=utf-8" {
                    debug!("Found text file in message, extracting content");
                    let response = reqwest::get(attachment.url.clone()).await?;
                    let bytes = response.bytes().await?;

                    let attachment_text = String::from_utf8_lossy(&bytes).into_owned();
                    content = format!("{} \n {}", content, attachment_text);
                }
            }
        }

        Ok(content)
    }
}

impl TypeMapKey for GptContext {
    type Value = Arc<Mutex<GptContext>>;
}

fn split_string(input_string: &str) -> Vec<String> {
    let mut output = vec![];
    let mut current_chunk = String::new();
    for line in input_string.lines() {
        let new_chunk = format!("{}\n{}", current_chunk, line);
        if new_chunk.len() > 2000 {
            output.push(current_chunk.clone());
            current_chunk = line.to_string();
        } else {
            current_chunk = new_chunk;
        }
    }
    output.push(current_chunk);
    output
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .init();
    let discord_token = "MTA5MDAwODk2MDk5NzgwNjE0MA.GPZ2yG.fd_YtL88A7zm1uP1K0F-Rw1-jVgsqnP7kkqlA8";
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(discord_token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    let gpt = GptContext::new(
        "Lovelace",
        "gpt-3.5-turbo",
        "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",
    )
    .await
    .expect("Failed to create GptContext");

    {
        let mut data = client.data.write().await;
        data.insert::<GptContext>(Arc::new(Mutex::new(gpt)));
    }

    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
}

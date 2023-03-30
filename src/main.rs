use std::sync::Arc;

use async_openai::error::OpenAIError;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs,
    CreateChatCompletionRequestArgs, Role,
};
use async_openai::Client as GptClient;

use log::{debug, error, info, warn, trace};

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

        gpt.context.push(
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content(&message.content)
                .name(message.author.name)
                .build()
                .unwrap(),
        );

        trace!("messages in context: {}", gpt.context.len());
        
        let mut token_count = get_chat_completion_max_tokens(&gpt.model, &gpt.context).unwrap();
        debug!("token count: {}", token_count);
        while token_count < 500 {
            info!("Reached max token count, removing oldest message from context");
            gpt.context.remove(2);
            token_count = get_chat_completion_max_tokens(&gpt.model, &gpt.context).unwrap();
        }

        if !message.author.bot
            && message
                .content
                .to_lowercase()
                .contains(&gpt.name.to_lowercase())
        {
            let typing = message.channel_id.start_typing(&context.http).unwrap();

            let request = CreateChatCompletionRequestArgs::default()
                .max_tokens((token_count as u16) - 100)
                .model(&gpt.model)
                .messages(gpt.context.clone())
                .build()
                .expect("Failed to build request");
            match gpt.client.chat().create(request).await {
                Ok(response) => {
                    if let Some(usage) = response.usage {
                        debug!("tokens: {}", usage.total_tokens);
                    }

                    if let Some(resp) = response.choices.first() {
                        for content in split_string(&resp.message.content) {
                            message
                                .channel_id
                                .say(&context.http, content)
                                .await
                                .expect("failed to send message");

                        }
                    } else {
                        warn!("no choices");
                    }
                }
                Err(OpenAIError::ApiError(err)) => error!("Request error: {:?}", err),
                _ => error!("Request error"),
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

    {
        let mut data = client.data.write().await;
        data.insert::<GptContext>(Arc::new(Mutex::new(GptContext{
            name: "Lovelace".to_owned(),
            model: "gpt-3.5-turbo".to_owned(),
            client: GptClient::new().with_api_key("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"),
            context: vec![
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
            ]
        })));
    }

    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
}

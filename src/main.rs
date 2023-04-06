use std::error::Error;
use std::sync::Arc;

use async_openai::error::OpenAIError;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs,
    CreateChatCompletionRequestArgs, Role,
};
use async_openai::Client as GptClient;

use async_recursion::async_recursion;
use log::{debug, error, info};

use serenity::model::channel::Message;
use serenity::{async_trait, prelude::*, Client};

use tiktoken_rs::model::get_context_size;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

use regex::Regex;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, context: Context, interaction: Interaction) {
        if let Interaction::ApplicationCommand(command) = interaction {
            debug!("Received command interaction: {:#?}", command);

            let data_read = context.data.read().await;
            let mut gpt = data_read
                .get::<GptContext>()
                .expect("No Gpt Context")
                .lock()
                .await;

            let content = match command.data.name.as_str() {
                "clear_context" => commands::clear_context::run(&command.data.options, &mut gpt),
                _ => {
                    error!("Command not implemented: {}", command.data.name);
                    "Command not implemented".to_owned()
                }
            };

            if let Err(why) = command
                .create_interaction_response(&context.http, |response| {
                    response
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|message| message.content(content))
                })
                .await
            {
                error!("Cannot respond to slash command: {}", why);
            }
        };
    }

    async fn ready(&self, context: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);

        Command::create_global_application_command(&context.http, |command| {
            commands::clear_context::register(command)
        })
        .await
        .expect("Failed to create global command");
    }
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

            let response = gpt.get_response().await.unwrap(); // split message only if needed as discord has a 2k character limit
            debug!("response: {}", response);
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
                .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. You will refer to us as ms. The names of the people you talk with are Xuna & Amo.")
                .build()
                .unwrap(),
            ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .content("I want you to act like Jarvis from Iron Man. I want you to respond and answer like Jarvis using the tone, manner and vocabulary Jarvis would use. Do not write any explanations. Only answer like Jarvis. You must know all of the knowledge of Jarvis. But you will be named Lovelace. You will refer to us as ms. The names of the people you talk with are Xuna & Amo.")
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
        let regex = Regex::new(r"(?m)https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{2,256}\.[a-z]{2,4}\b([-a-zA-Z0-9@:%_\+.~#?&//=]*)").unwrap();

        let mut web_contents: Vec<String> = vec![];
        let captures = regex.captures_iter(&content);
        for capture in captures {
            if let Some(url) = capture.get(0) {
                debug!("Found link in message: {}", url.as_str());
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
                debug!("Web contents: {}", url_content);
                web_contents.push(format!(
                    "\n link: {} \n link content: {}",
                    url.as_str(),
                    url_content
                ));
            };
        }

        let web_contents = web_contents.join("\n");
        let bpe = get_bpe_from_model(&self.model).unwrap();
        let token_count_web_content = bpe.encode_with_special_tokens(&web_contents).len();
        let token_count_message = bpe.encode_with_special_tokens(&content).len();

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

        content = content.trim().to_owned();
        debug!("end message content: {}", content);

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
        let mut content: String = String::new();
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

        Ok(content.trim().to_owned())
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
    output.push(current_chunk.trim().to_owned());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_splits_into_chunks_of_max_2000_chars() {
        let long_string = "Lorem ipsum dolor sit amet, 
            consectetur adipiscing elit. 
            Proin sit amet risus ut enim hendrerit varius. 
            Mauris bibendum sodales mauris, 
            non hendrerit augue congue id. 
            Nulla facilisi. 
            Vestibulum iaculis velit eget mauris efficitur, 
            at sagittis eros faucibus. 
            Sed porttitor libero non ex ullamcorper, 
            nec varius purus consectetur. 
            Praesent quis pharetra urna. 
            Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; 
            Donec nec scelerisque urna. 
            Sed efficitur quam id volutpat consectetur. 
            Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; 
            Sed maximus lacus a maximus posuere. 
            Donec sollicitudin mollis ullamcorper. 
            Sed pellentesque justo neque, at blandit enim semper non."
            .repeat(3);
        let chunks = split_string(&long_string);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn it_handles_empty_input() {
        let input = "";
        let chunks = split_string(input);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn it_handles_small_texts() {
        let input = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.";
        let chunks = split_string(input);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], input);
    }
}

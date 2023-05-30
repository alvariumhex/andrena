use std::{collections::HashMap, env};

use log::{error, info, trace};
use ractor::{call, Actor, ActorProcessingErr, ActorRef, BytesConvertable, Message};
use serde::{Deserialize, Serialize};
use serenity::{
    async_trait,
    http::Http,
    model::prelude::{ChannelId, Message as DiscordMessage, Ready},
    prelude::{Context, EventHandler, GatewayIntents, TypeMapKey},
    Client,
};

use crate::actors::{channel_sup::ChannelSupervisorMessage, gpt::ChatMessage};

extern crate ractor;

pub struct DiscordActor;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ChannelMessage {
    Register(ChatMessage),
    ClearContext,
    SetWakeword(String),
    SetModel(String),
}

impl Message for ChannelMessage {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ChatActorMessage {
    Send(ChatMessage),
    Typing(u64),
    Receive(ChatMessage),
}

impl Message for ChatActorMessage {}

pub struct DiscordState {
    http: Http,
    channels: Vec<u64>,
}

#[async_trait]
impl EventHandler for DiscordActor {
    async fn message(&self, context: Context, message: DiscordMessage) {
        let data_read = context.data.read().await;
        let data = data_read.get::<ClientContext>().unwrap();
        let mut metadata: HashMap<String, String> = HashMap::new();
        metadata.insert("provider".to_owned(), "discord".to_owned());
        metadata.insert("wakeword".to_owned(), data.name.clone());
        data.myself
            .send_message(ChatActorMessage::Receive(ChatMessage {
                channel: message.channel_id.0,
                content: message.content,
                author: message.author.name,
                metadata,
            }))
            .unwrap();
    }

    async fn ready(&self, _context: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
    }
}

struct ClientContext {
    myself: ActorRef<ChatActorMessage>,
    name: String,
}

impl TypeMapKey for ClientContext {
    type Value = ClientContext;
}

fn split_string(s: &str, max_len: usize) -> Vec<String> {
    let mut result: Vec<String> = vec![];
    let mut start = 0;
    let mut end;

    while start < s.len() {
        if start + max_len < s.len() {
            end = s
                .char_indices()
                .enumerate()
                .skip_while(|(i, _)| *i < start + max_len)
                .map(|(_, (j, _))| j)
                .find(|&j| s[j..].starts_with('\n') || s[j..].starts_with(' '))
                .unwrap_or(s.len());
        } else {
            end = s.len();
        }

        result.push(s[start..end].to_string());
        start = end + 1;
    }

    result
}

#[async_trait]
impl Actor for DiscordActor {
    type State = DiscordState;
    type Msg = ChatActorMessage;
    type Arguments = String;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let discord_token = env::var("DISCORD_TOKEN").expect("No DISCORD_TOKEN provided");
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
        let http = Http::new(&discord_token);
        let mut client = Client::builder(discord_token, intents)
            .event_handler(Self)
            .await?;

        ractor::pg::join("messages_send".to_owned(), vec![myself.get_cell()]);

        tokio::spawn(async move {
            let context = ClientContext { myself, name: args };

            client.data.write().await.insert::<ClientContext>(context);
            client
                .start()
                .await
                .expect("Failed to start discord client");
        });

        info!("Started and registered Discord actor");
        Ok(DiscordState {
            http,
            channels: vec![],
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            ChatActorMessage::Send(msg) => {
                if state.channels.contains(&msg.channel) {
                    let messages = split_string(&msg.content, 2000);
                    for message in messages {
                        trace!("Sending message: {}", message);
                        let channel = ChannelId(msg.channel);
                        channel.say(&state.http, message).await.unwrap();
                    }
                }
                Ok(())
            }
            ChatActorMessage::Typing(channel_id) => {
                if state.channels.contains(&channel_id) {
                    trace!("Typing in channel: {}", channel_id);
                    ChannelId(channel_id)
                        .broadcast_typing(&state.http)
                        .await
                        .unwrap();
                }

                Ok(())
            }
            ChatActorMessage::Receive(msg) => {
                trace!("Received message: {}: {}", msg.author, msg.content);
                if !state.channels.contains(&msg.channel) {
                    state.channels.push(msg.channel);
                }

                let channel_registry = ractor::registry::where_is("channel_sup".to_owned());
                if channel_registry.is_none() {
                    error!("Channel supervisor not found");
                    return Ok(());
                }

                let channel_supervisor: ActorRef<ChannelSupervisorMessage> =
                    channel_registry.unwrap().into();
                let channel = call!(
                    channel_supervisor,
                    ChannelSupervisorMessage::FetchChannel,
                    msg.channel
                )
                .unwrap();

                channel
                    .send_message(crate::actors::channel::ChannelMessage::Register(msg))
                    .unwrap();
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_string() {
        let input = "This is a test string.\nIt has multiple lines and some very long lines without a newline character to test the splitting on whitespace functionality. The purpose of the test is to ensure that the split_string function works as expected and provides the correct output.";
        let max_len = 40;

        let result = split_string(input, max_len);
        println!("Result: {:?}", result);

        assert_eq!(result.len(), 7);

        // assert_eq!(result[0], "This is a test string.\nIt has multiple lines");
        // assert_eq!(result[1], " and some very long lines without a newline");
        // assert_eq!(result[2], " character");
        // assert_eq!(result[3], "to test the splitting on whitespace");
    }
}

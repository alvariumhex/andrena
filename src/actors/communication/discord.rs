use std::{
    collections::HashMap,
    env,
    sync::{Arc, Mutex},
};

use log::{error, info, trace};
use ractor::{call, Actor, ActorProcessingErr, ActorRef, BytesConvertable};
use serde::{Deserialize, Serialize};
use serenity::{
    async_trait,
    http::Http,
    model::prelude::{ChannelId, Message, Ready},
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

impl BytesConvertable for ChannelMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ChatActorMessage {
    Send(ChatMessage),
    Typing(u64),
    Receive(ChatMessage),
}

impl BytesConvertable for ChatActorMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

pub struct DiscordState {
    http: Http,
    channels: Vec<u64>,
}

#[async_trait]
impl EventHandler for DiscordActor {
    async fn message(&self, context: Context, message: Message) {
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
            let context = ClientContext {
                myself,
                name: args,
            };

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
                    trace!("Sending message: {}", msg.content);
                    let channel = ChannelId(msg.channel);
                    channel.say(&state.http, msg.content).await.unwrap();
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

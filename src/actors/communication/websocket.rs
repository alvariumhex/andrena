use std::time::Duration;

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use log::{error, info, trace};
use ractor::{Actor, ActorProcessingErr, ActorRef};
use serde::{Deserialize, Serialize};
use serenity::async_trait;
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use crate::actors::gpt::ChatMessage;

use super::discord::ChatActorMessage;

#[derive(Debug, Serialize, Deserialize)]
pub struct WebSocketMessage {
    op: u8,
    d: String,
}

pub struct WebSocketActor;

pub struct WebSocketState {
    socket: SplitSink<WebSocketStream<TcpStream>, Message>,
    channels: Vec<u64>,
}

#[async_trait]
impl Actor for WebSocketActor {
    type Msg = ChatActorMessage;
    type State = WebSocketState;
    type Arguments = (
        SplitSink<WebSocketStream<TcpStream>, Message>,
        SplitStream<WebSocketStream<TcpStream>>,
    );

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        info!("Websocket actor started");

        ractor::pg::join("messages_send".to_owned(), vec![myself.get_cell()]);
        let (write, mut read) = args;
        tokio::task::spawn(async move {
            // tokio interval
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                {
                    match read.next().await {
                        Some(Ok(msg)) => {
                            info!("Received message: {:?}", msg);
                            if msg.is_text() {
                                let mut message =
                                    serde_json::from_str::<ChatMessage>(&msg.to_string())
                                        .expect("Failed to convert message to GptMessage");

                                message
                                    .metadata
                                    .insert("provider".to_owned(), "websocket".to_owned());

                                myself
                                    .send_message(ChatActorMessage::Receive(message))
                                    .expect("Failed to send message to actor");
                            }
                        }
                        Some(Err(err)) => {
                            error!("Error reading message: {:?}", err);
                            break;
                        }
                        None => {
                            info!("Websocket closed {}", myself.get_id());
                            myself.kill();
                            break;
                        }
                    }
                }
            }
        });

        Ok(WebSocketState {
            socket: write,
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
                    let string = serde_json::to_string_pretty::<ChatMessage>(&msg).unwrap();
                    let message = WebSocketMessage { op: 0, d: string };
                    let message =
                        serde_json::to_string_pretty::<WebSocketMessage>(&message).unwrap();
                    state.socket.send(Message::Text(message)).await.unwrap();
                }
                Ok(())
            }
            ChatActorMessage::Typing(channel_id) => {
                trace!("Sending typing message: {}", channel_id);
                if state.channels.contains(&channel_id) {
                    let string =
                        serde_json::to_string_pretty::<WebSocketMessage>(&WebSocketMessage {
                            op: 1,
                            d: String::new(),
                        })
                        .expect("Failed to convert message to WebSocketMessage");
                    state.socket.send(Message::Text(string)).await.unwrap();
                }
                Ok(())
            }
            ChatActorMessage::Receive(msg) => {
                trace!("Received message: {}: {}", msg.author, msg.content);
                if !state.channels.contains(&msg.channel) {
                    trace!("Registring new channel: {}", msg.channel);
                    state.channels.push(msg.channel);
                }
                Ok(())
            }
        }
    }
}

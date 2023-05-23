use std::{collections::HashMap, time::Duration};

use log::{info, trace};
use ractor::{BytesConvertable, Actor, ActorRef, ActorProcessingErr, rpc::cast};
use serde::{Serialize, Deserialize};
use serenity::async_trait;

use crate::actors::communication::discord::ChatActorMessage;


#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TypingMessage {
    Start(u64),
    Stop(u64),
    Trigger,
}

impl BytesConvertable for TypingMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

pub struct TypingState {
    pub channels: HashMap<u64, bool>,
}

pub(crate) struct TypingActor;

#[async_trait]
impl Actor for TypingActor {
    type Msg = TypingMessage;
    type State = TypingState;
    type Arguments = ();

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        _args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        myself.send_interval(Duration::from_secs(3), || TypingMessage::Trigger);
        info!("Started and registered typing actor");

        Ok(TypingState {
            channels: HashMap::new(),
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            TypingMessage::Start(channel) => {
                state.channels.insert(channel, true);
                myself.send_message(TypingMessage::Trigger).unwrap();
                Ok(())
            }
            TypingMessage::Stop(channel) => {
                state.channels.insert(channel, false);
                myself.send_message(TypingMessage::Trigger).unwrap();
                Ok(())
            },
            TypingMessage::Trigger => {
                let actors = ractor::pg::get_members(&"messages_send".to_owned());

                for (channel, typing) in &state.channels {
                    if *typing {
                        trace!("Typing in channel {}", channel);
                        for actor in actors.clone() {
                            cast(&actor, ChatActorMessage::Typing(*channel)).unwrap();
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

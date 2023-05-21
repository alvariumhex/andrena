use std::collections::HashMap;

use log::warn;
use ractor::{
    Actor, ActorCell, ActorProcessingErr, ActorRef, Message, RpcReplyPort, SupervisionEvent, BytesConvertable,
};
use ractor_cluster::RactorClusterMessage;

use super::{channel::{ChannelActor, ChannelMessage}};

pub struct ChannelSupervisorState {
    pub channels: HashMap<u64, ActorRef<ChannelMessage>>,
}

pub enum ChannelSupervisorMessage {
    FetchChannel(u64, RpcReplyPort<ActorRef<ChannelMessage>>),
    ChannelExists(u64, RpcReplyPort<bool>),
    CreateChannel(Option<u64>, RpcReplyPort<ActorRef<ChannelMessage>>),
}

impl Message for ChannelSupervisorMessage {}

pub enum RefRepsonse {
    Exists(ActorRef<ChannelMessage>),
    DoesNotExist,
}

pub struct ChannelSupervisor;

#[async_trait::async_trait]
impl Actor for ChannelSupervisor {
    type Msg = ChannelSupervisorMessage;
    type State = ChannelSupervisorState;
    type Arguments = ();

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        _: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        ractor::pg::join("channel_sup".to_owned(), vec![myself.get_cell()]);
        Ok(ChannelSupervisorState {
            channels: HashMap::new(),
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            ChannelSupervisorMessage::FetchChannel(id, reply_port) => {
                if let Some(channel) = state.channels.get(&id) {
                    reply_port.send(channel.clone()).unwrap();
                } else {
                    let (channel, _) =
                        Actor::spawn(Some(format!("channel-{}", id)), ChannelActor, Some(id))
                            .await?;
                    state.channels.insert(id, channel.clone());
                    reply_port.send(channel).unwrap();
                }
            }
            ChannelSupervisorMessage::ChannelExists(id, reply_port) => {
                reply_port.send(state.channels.contains_key(&id)).unwrap();
            },
            ChannelSupervisorMessage::CreateChannel(id, reply_port) => {
                let id = id.unwrap_or_else(|| {
                    rand::random::<u64>()
                });

                if let Some(channel) = state.channels.get(&id) {
                    warn!("Channel {} already exists", id);
                    reply_port.send(channel.clone()).unwrap();
                } else {
                    let (channel, _) =
                        Actor::spawn(Some(format!("channel-{}", id)), ChannelActor, Some(id))
                            .await?;
                    state.channels.insert(id, channel.clone());
                    reply_port.send(channel).unwrap();
                }
            }
        }
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(cell, child_state, id) => {}
            _ => {}
        }

        Ok(())
    }
}

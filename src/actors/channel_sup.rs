use std::collections::HashMap;

use log::{info, warn};
use ractor::{Actor, ActorProcessingErr, ActorRef, Message, RpcReplyPort, SupervisionEvent};

use super::channel::{ChannelActor, ChannelMessage, ChannelState};

pub struct ChannelSupervisorState {
    pub channels: HashMap<u64, ActorRef<ChannelMessage>>,
}

#[allow(dead_code)]
pub enum ChannelSupervisorMessage {
    FetchChannel(u64, RpcReplyPort<ActorRef<ChannelMessage>>),
    ChannelExists(u64, RpcReplyPort<bool>),
    CreateChannel(Option<u64>, RpcReplyPort<(u64, ActorRef<ChannelMessage>)>),
}

impl Message for ChannelSupervisorMessage {}

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
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            ChannelSupervisorMessage::FetchChannel(id, reply_port) => {
                if let Some(channel) = state.channels.get(&id) {
                    reply_port.send(channel.clone()).unwrap();
                } else {
                    info!("Fetching channel that does not exist, creating {}", id);
                    let (channel, _) = Actor::spawn_linked(
                        Some(format!("channel-{}", id)),
                        ChannelActor,
                        Some(id),
                        myself.get_cell(),
                    )
                    .await?;
                    state.channels.insert(id, channel.clone());
                    reply_port.send(channel).unwrap();
                }
            }
            ChannelSupervisorMessage::ChannelExists(id, reply_port) => {
                reply_port.send(state.channels.contains_key(&id)).unwrap();
            }
            ChannelSupervisorMessage::CreateChannel(id, reply_port) => {
                let id = id.unwrap_or_else(rand::random::<u64>);

                if let Some(channel) = state.channels.get(&id) {
                    warn!("Channel {} already exists", id);
                    reply_port.send((id, channel.clone())).unwrap();
                } else {
                    let (channel, _) = Actor::spawn_linked(
                        Some(format!("channel-{}", id)),
                        ChannelActor,
                        Some(id),
                        myself.get_cell(),
                    )
                    .await?;
                    state.channels.insert(id, channel.clone());
                    reply_port.send((id, channel)).unwrap();
                }
            }
        }
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(_cell, child_state, _reason) => {
                let child_state: ChannelState = child_state.unwrap().take().unwrap();
                state.channels.remove(&child_state.id);
            }
            SupervisionEvent::ActorPanicked(cell, _error) => {
                // untested behaviour
                // how would I get the state (id) of a dead actor?
                let id = cell
                    .get_name()
                    .unwrap()
                    .strip_prefix("channel-")
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                state.channels.remove(&id);
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::env;

    use super::*;
    use ractor::{call, Actor};

    #[tokio::test]
    async fn create_on_fetch() {
        env::set_var("OPENAI_API_KEY", "dummy_key");
        let (supervisor, _) = Actor::spawn(None, ChannelSupervisor, ()).await.unwrap();
        let channel = call!(supervisor, ChannelSupervisorMessage::FetchChannel, 12345);
        match channel {
            Ok(_) => assert!(true),
            Err(e) => panic!("Failed to fetch channel: {}", e),
        }

        supervisor.kill();
    }

    #[tokio::test]
    async fn stop_removes_channel() {
        env::set_var("OPENAI_API_KEY", "dummy_key");
        let (supervisor, _) = Actor::spawn(None, ChannelSupervisor, ()).await.unwrap();
        let channel = call!(supervisor, ChannelSupervisorMessage::FetchChannel, 1234).unwrap();

        // channel should exist
        let exists = call!(supervisor, ChannelSupervisorMessage::ChannelExists, 1234);
        match exists {
            Ok(exists) => assert!(exists),
            Err(e) => panic!("Failed to fetch channel: {}", e),
        }

        channel.stop(None); // stop channel

        // channel should be removed
        let exists = call!(supervisor, ChannelSupervisorMessage::ChannelExists, 1234);
        match exists {
            Ok(exists) => assert!(!exists),
            Err(e) => {
                panic!("Failed to fetch channel: {}", e);
            }
        }

        supervisor.kill();
    }

    #[tokio::test]
    async fn create_channel() {
        env::set_var("OPENAI_API_KEY", "dummy_key");
        let (supervisor, _) = Actor::spawn(None, ChannelSupervisor, ()).await.unwrap();
        let (id, channel_1) =
            call!(supervisor, ChannelSupervisorMessage::CreateChannel, None).unwrap();

        // recreating a channel with the same id should return the same channel
        let (_, channel_2) = call!(
            supervisor,
            ChannelSupervisorMessage::CreateChannel,
            Some(id)
        )
        .unwrap();

        assert_eq!(channel_1.get_id(), channel_2.get_id());
    }
}

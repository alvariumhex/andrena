use actix::{Addr, Actor, Handler, Context, Message, AsyncContext};
use log::{info, trace};
use serde::{Serialize, Deserialize};

use crate::RegisterActor;

use super::mqtt::{MqttActor, SendTyping};

#[derive(Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub struct TypingMessage {
    pub channel: u64,
    pub typing: bool,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct TriggerTyping;

pub struct TypingActor {
    pub mqtt_actor: Option<Addr<MqttActor>>,
    pub channels: Vec<u64>,
}

impl Actor for TypingActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        info!("Typing actor started");
        let addr = ctx.address();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        tokio::spawn(async move {
            loop {
                interval.tick().await;
                addr.do_send(TriggerTyping);
            }
        });
    }
}

impl Handler<TriggerTyping> for TypingActor {
    type Result = ();

    fn handle(&mut self, _msg: TriggerTyping, _ctx: &mut Context<Self>) -> Self::Result {
        if let Some(mqtt_actor) = &self.mqtt_actor {
            for channel in &self.channels {
                info!("Sending typing to channel {}", channel);
                mqtt_actor.do_send(SendTyping(*channel));
            }
        }
    }
}

impl Handler<RegisterActor> for TypingActor {
    type Result = ();

    fn handle(&mut self, msg: RegisterActor, _ctx: &mut Context<Self>) -> Self::Result {
        self.mqtt_actor = Some(msg.0);
    }
}


impl Handler<TypingMessage> for TypingActor {
    type Result = ();

    fn handle(&mut self, msg: TypingMessage, _ctx: &mut Context<Self>) -> Self::Result {
        if msg.typing {
            if !self.channels.contains(&msg.channel) {
                self.channels.push(msg.channel);
            }
        } else {
            if self.channels.contains(&msg.channel) {
                self.channels.retain(|&x| x != msg.channel);
            }
        }
    }
}



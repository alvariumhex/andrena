use std::sync::Arc;

use actix::prelude::*;
use log::{trace, info};
use paho_mqtt::Message as PahoMqttMessage;
use tokio::sync::Mutex;

use crate::{DiscordMessage, DiscordSend, EmbeddingsRequest, Embedding};

use super::openai::OpenaiActor;

#[derive(Message)]
#[rtype(result = "()")]
pub struct MqttMessage(pub PahoMqttMessage);

#[derive(Message)]
#[rtype(result = "()")]
pub struct SendTyping(pub u64);
pub struct MqttActor {
    pub openai_actor: Addr<OpenaiActor>,
    pub client: Arc<Mutex<paho_mqtt::AsyncClient>>,
}

impl Actor for MqttActor {
    type Context = Context<Self>;
}

impl Handler<MqttMessage> for MqttActor {
    type Result = ();

    fn handle(&mut self, msg: MqttMessage, _ctx: &mut Context<Self>) -> Self::Result {
        let json_string = String::from_utf8(msg.0.payload().to_vec()).unwrap();
        if msg.0.topic() == "carpenter/discord/receive" {
            info!("Received message from discord: {}", json_string);
            self.openai_actor
                .do_send(serde_json::from_str::<DiscordMessage>(&json_string).unwrap());
        } else {
            trace!("Received message on {} at {}", msg.0.topic(), json_string);
        }
    }
}

impl Handler<DiscordSend> for MqttActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, msg: DiscordSend, _ctx: &mut Context<Self>) -> Self::Result {
        let client = self.client.clone();
        info!("Sending message to discord: {}", msg.content);
        Box::pin(async move {
            let json_string = serde_json::to_string(&msg).unwrap();
            let message = PahoMqttMessage::new("carpenter/discord/send", json_string, 1);
            client
                .lock()
                .await
                .publish(message)
                .await
                .expect("Failed to send message");
        })
    }
}

impl Handler<SendTyping> for MqttActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, msg: SendTyping, _ctx: &mut Context<Self>) -> Self::Result {
        let client = self.client.clone();
        info!("Sending typing to discord for channel: {}", msg.0);
        Box::pin(async move {
            let message = PahoMqttMessage::new("carpenter/discord/typing", msg.0.to_string(), 1);
            client
                .lock()
                .await
                .publish(message)
                .await
                .expect("Failed to send message");
            ()
        })
    }
}

impl Handler<EmbeddingsRequest> for MqttActor {
    type Result = ResponseFuture<Vec<(Embedding, f32)>>;

    fn handle(&mut self, msg: EmbeddingsRequest, _ctx: &mut Context<Self>) -> Self::Result {
        let client = self.client.clone();
        info!("Sending embeddings request to discord: {}", msg.message);
        Box::pin(async move {
            let json_string = serde_json::to_string(&msg).unwrap();
            let message = PahoMqttMessage::new("carpenter/embeddings", json_string, 1);
            client
                .lock()
                .await
                .publish(message)
                .await
                .expect("Failed to send message");
            vec![]
        })
    }
}
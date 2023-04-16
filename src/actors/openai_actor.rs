use std::collections::HashMap;

use actix::{
    Actor, Addr, AsyncContext, Context, Handler, WrapFuture
};
use async_openai::{
    types::{CreateChatCompletionRequest, CreateChatCompletionRequestArgs},
    Client,
};
use log::{debug, error, info};
use tiktoken_rs::get_chat_completion_max_tokens;

use crate::{ai_context::AiContext, DiscordMessage, DiscordSend, RegisterActor};

use super::mqtt_actor::MqttActor;

pub struct OpenaiActor {
    pub name: String,
    pub client: Client,
    pub model: String,
    pub context: HashMap<String, AiContext>,
    pub mqtt_actor: Option<Addr<MqttActor>>,
}

impl OpenaiActor {
    fn get_context_for_id(&mut self, channel_id: &str) -> &mut AiContext {
        match self.context.contains_key(channel_id) {
            true => self.context.get_mut(channel_id).unwrap(),
            false => {
                let mut context = AiContext::new();
                context.set_static_context("You can use following emotes in the conversation if you see fit, each emote has a meaning next to it in one or multiple words, the next emote is denoted with a ; : <:kekdog:1090251469988573184> big laughter; <a:mwaa:1090251284617101362> frustration; <:oof:1090251382801571850> dissapointment or frustration; <:finnyikes:1090251493950627942> uncomfortable disgusted dissapointment; <a:catpls:1090251360693407834> silly mischievious; <:gunma:1090251357316988948> demanding angry; <:snicker:1091728748145033358> flabergasted suprised; <:crystalheart:1090323901583736832> love appreciation; <:dogwat:1090253587273236580> disbelief suprise");
                self.context.insert(channel_id.to_owned(), context);
                self.context.get_mut(channel_id).unwrap()
            }
        }
    }

    fn insert_message(&mut self, msg: &DiscordMessage) {
        let context = self.get_context_for_id(&msg.channel.to_string());
        context.push_history((msg.author.clone(), msg.content.clone()));
    }

    fn generate_response<'a>(&'a mut self, channel: u64) -> CreateChatCompletionRequest {
        debug!("Generating response for channel: {}", channel);
        let model = self.model.clone();

        let context = self.get_context_for_id(&channel.to_string());
        context.manage_tokens(&model);
        let max_tokens =
            get_chat_completion_max_tokens(&model, &context.to_chat_history()).unwrap();
        CreateChatCompletionRequestArgs::default()
            .max_tokens((max_tokens as u16) - 110)
            .model(model)
            .messages(context.to_chat_history())
            .build()
            .expect("Failed to build request")
    }
}

impl Actor for OpenaiActor {
    type Context = Context<Self>;
}

impl Handler<RegisterActor> for OpenaiActor {
    type Result = ();

    fn handle(&mut self, msg: RegisterActor, _ctx: &mut Context<Self>) -> Self::Result {
        self.mqtt_actor = Some(msg.0);
    }
}

impl Handler<DiscordMessage> for OpenaiActor {
    type Result = ();

    fn handle(&mut self, msg: DiscordMessage, ctx: &mut Context<Self>) -> Self::Result {
        info!("{}: {}", msg.author, msg.content);
        self.insert_message(&msg);
        let name = self.name.clone();
        if msg.author.to_lowercase() == name.to_lowercase() {
            return;
        }
        debug!("Spawning future for channel: {}", msg.channel);
        let client = self.client.clone();
        let request = self.generate_response(msg.channel);
        let mqtt_actor = self.mqtt_actor.clone().unwrap();
        let channel = msg.channel;
        let ac_fut = Box::pin(async move {
            let response = client.chat().create(request).await;
            match response {
                Ok(response) => {
                    if let Some(usage) = response.usage {
                        debug!("tokens: {}", usage.total_tokens);
                    }
                    debug!("response: {}", response.choices[0].message.content);
                    if let Some(resp) = response.choices.first() {
                        mqtt_actor
                            .send(DiscordSend {
                                channel: channel,
                                content: resp.message.content.clone(),
                            })
                            .await
                            .unwrap();
                    } else {
                        mqtt_actor
                            .send(DiscordSend {
                                channel: channel,
                                content: "Failed to generate response: No choices".to_owned(),
                            })
                            .await
                            .unwrap();
                    }
                }
                Err(e) => {
                    error!("Failed to generate response: {:?}", e);
                    mqtt_actor
                        .send(DiscordSend {
                            channel: channel,
                            content: "Failed to generate response".to_owned(),
                        })
                        .await
                        .unwrap();
                }
            }
        })
        .into_actor(self);
        ctx.spawn(ac_fut);
    }
}

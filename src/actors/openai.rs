// use std::collections::{HashMap, HashSet, BTreeSet};

// use actix::{Actor, Addr, AsyncContext, Context, Handler, WrapFuture};
// use async_openai::{
//     types::{CreateChatCompletionRequest, CreateChatCompletionRequestArgs},
//     Client,
// };
// use log::{debug, error, info};
// use tiktoken_rs::get_chat_completion_max_tokens;

// use crate::{
//     actors::typing::TypingMessage, ai_context::GptContext, DiscordMessage, DiscordSend,
//     EmbeddingsRequest, EmbeddingsResponse, RegisterActor,
// };

// use super::{mqtt::MqttActor, typing::TypingActor};

// pub struct GptActorActix {
//     pub name: String,
//     pub client: Client,
//     pub model: String,
//     pub context: HashMap<String, GptContext>,
//     pub mqtt_actor: Option<Addr<MqttActor>>,
//     pub typing_actor: Addr<TypingActor>,
//     pub channel: Option<u64>,
//     pub enable_embeddings: bool,
// }

// impl GptActorActix {
//     fn get_context_for_id(&mut self, channel_id: &str) -> &mut GptContext {
//         match self.context.contains_key(channel_id) {
//             true => self.context.get_mut(channel_id).unwrap(),
//             false => {
//                 let mut context = GptContext::new();
//                 // context.set_static_context("You can use following emotes in the conversation if you see fit, each emote has a meaning next to it in one or multiple words, the next emote is denoted with a ; : <:kekdog:1090251469988573184> big laughter; <a:mwaa:1090251284617101362> frustration; <:oof:1090251382801571850> dissapointment or frustration; <:finnyikes:1090251493950627942> uncomfortable disgusted dissapointment; <a:catpls:1090251360693407834> silly mischievious; <:gunma:1090251357316988948> demanding angry; <:snicker:1091728748145033358> flabergasted suprised; <:crystalheart:1090323901583736832> love appreciation; <:dogwat:1090253587273236580> disbelief suprise");
//                 self.context.insert(channel_id.to_owned(), context);
//                 self.context.get_mut(channel_id).unwrap()
//             }
//         }
//     }

//     fn insert_message(&mut self, msg: &DiscordMessage) {
//         let context = self.get_context_for_id(&msg.channel.to_string());
//         context.push_history((msg.author.clone(), msg.content.clone()));
//     }

//     fn generate_response<'a>(&'a mut self, channel: u64) -> CreateChatCompletionRequest {
//         debug!("Generating response for channel: {}", channel);
//         let model = self.model.clone();

//         let context = self.get_context_for_id(&channel.to_string());
//         context.manage_tokens(&model);
//         let max_tokens =
//             get_chat_completion_max_tokens(&model, &context.to_openai_chat_history()).unwrap();
//         CreateChatCompletionRequestArgs::default()
//             .max_tokens((max_tokens as u16) - 110)
//             .model(model)
//             .messages(context.to_openai_chat_history())
//             .build()
//             .expect("Failed to build request")
//     }

//     fn clear_embeddings(&mut self, channel: u64) {
//         let context = self.get_context_for_id(&channel.to_string());
//         context.clear_embeddings();
//     }

//     fn insert_embeddings(&mut self, channel: u64, embeddings: Vec<String>) {
//         let context = self.get_context_for_id(&channel.to_string());
//         context.embeddings = embeddings;
//         context.embeddings.push("Given above documentation, answer the question. If you cannot find the answer in the documentation, mention it.".to_string());
//     }

//     fn get_semantic_query(&mut self, channel: u64) -> String {
//         let context = self.get_context_for_id(&channel.to_string());
//         context.fetch_semantic_query()
//     }
// }

// impl Actor for GptActorActix {
//     type Context = Context<Self>;

//     fn started(&mut self, _ctx: &mut Self::Context) {
//         info!("GptActor started, embeddings enabled: {}", self.enable_embeddings);
//     }
// }

// impl Handler<RegisterActor> for GptActorActix {
//     type Result = ();

//     fn handle(&mut self, msg: RegisterActor, _ctx: &mut Context<Self>) -> Self::Result {
//         self.mqtt_actor = Some(msg.0);
//     }
// }

// impl Handler<DiscordMessage> for GptActorActix {
//     type Result = ();
    
//     fn handle(&mut self, msg: DiscordMessage, ctx: &mut Context<Self>) -> Self::Result {
//         info!("{}: {}", msg.author, msg.content);
//         if !msg.content.contains("With embeddings repsonse") {
//             self.insert_message(&msg);
//         }
//         let name = self.name.clone();
//         if msg.author.to_lowercase() == name.to_lowercase() {
//             return;
//         }

//         if !msg.content.to_lowercase().contains(&name.to_lowercase()) && msg.channel != 1098877701231742978 {
//             return;
//         }

//         self.typing_actor.do_send(TypingMessage {
//             typing: true,
//             channel: msg.channel,
//         });

//         debug!("Spawning future for channel: {}", msg.channel.clone());
//         // There has to be a better way to do this but I'm not sure how
//         let client = self.client.clone();
//         self.clear_embeddings(msg.channel);
//         let request = self.generate_response(msg.channel);
//         let mqtt_actor = self.mqtt_actor.clone().unwrap();
//         let typing_actor = self.typing_actor.clone();
//         self.channel = Some(msg.channel);
//         let channel = msg.channel;
//         let enable_embeddings = self.enable_embeddings;
//         let semantic_query = self.get_semantic_query(channel);
//         debug!("semantic query: {}", semantic_query);
//         let ac_fut = Box::pin(async move {
//             let response = client.chat().create(request).await;
//             let response_text = match response {
//                 Ok(response) => {
//                     if let Some(usage) = response.usage {
//                         debug!("tokens: {}", usage.total_tokens);
//                     }
//                     debug!("response: {}", response.choices[0].message.content);
//                     if let Some(resp) = response.choices.first() {
//                         resp.message.content.clone()
//                     } else {
//                         "Failed to generate response: No choices".to_owned()
//                     }
//                 }
//                 Err(e) => {
//                     error!("Failed to generate response: {:?}", e);
//                     "Failed to generate response".to_owned()
//                 }
//             };

//             if enable_embeddings {
//                 mqtt_actor
//                     .send(DiscordSend {
//                         channel: msg.channel,
//                         content: format!("***With embeddings repsonse:***\n{}", response_text),
//                     })
//                     .await
//                     .unwrap();

//                 mqtt_actor
//                     .send(EmbeddingsRequest {
//                         message: semantic_query,
//                         limit: 2,
//                     })
//                     .await
//                     .unwrap()
//             } else {
//                 typing_actor.send(TypingMessage {
//                     typing: false,
//                     channel,
//                 }).await.unwrap();

//                 mqtt_actor
//                     .send(DiscordSend {
//                         channel: msg.channel,
//                         content: response_text,
//                     })
//                     .await
//                     .unwrap();
//             }
//         })
//         .into_actor(self);
//         ctx.spawn(ac_fut);
//     }
// }

// impl Handler<EmbeddingsResponse> for GptActorActix {
//     type Result = ();

//     fn handle(&mut self, msg: EmbeddingsResponse, ctx: &mut Context<Self>) -> Self::Result {
//         debug!("Embeddings response: {:?}", msg);
//         let channel = self.channel.unwrap();
//         let client = self.client.clone();
//         let embeddings: Vec<String> = msg
//             .0
//             .iter()
//             .map(|x| format!(
//                 "source: {}\n repo: {}/{}\n content: {}",
//                 x.0.metadata.get("source").unwrap().clone(),
//                 x.0.metadata.get("author").unwrap().clone(),
//                 x.0.metadata.get("repo").unwrap().clone(),
//                 x.0.metadata.get("content").unwrap().clone()
//             ))
//             .collect();

//         let mut sources = vec![];
//         for embedding in msg.0 {
//             let source = format!("<{}>", embedding.0.metadata.get("source").unwrap().to_owned());
//             if !sources.contains(&source) {
//                 sources.push(source);
//             }
//         }

//         self.insert_embeddings(channel, embeddings);
//         let request = self.generate_response(channel);
//         let mqtt_actor = self.mqtt_actor.clone().unwrap();
//         let typing_actor = self.typing_actor.clone();
//         let ac_fut = Box::pin(async move {
//             let response = client.chat().create(request).await;
//             let response_text = match response {
//                 Ok(response) => {
//                     if let Some(usage) = response.usage {
//                         debug!("tokens: {}", usage.total_tokens);
//                     }
//                     debug!("response: {}", response.choices[0].message.content);
//                     if let Some(resp) = response.choices.first() {
//                         format!("{}\n\nSources:\n{}", resp.message.content.clone(), sources.join("\n"))
//                     } else {
//                         "Failed to generate response: No choices".to_owned()
//                     }
//                 }
//                 Err(e) => {
//                     error!("Failed to generate response: {:?}", e);
//                     "Failed to generate response".to_owned()
//                 }
//             };

//             typing_actor.send(TypingMessage {
//                 typing: false,
//                 channel,
//             }).await.unwrap();

//             mqtt_actor
//                 .send(DiscordSend {
//                     channel: channel,
//                     content: format!("\n\n***Embeddings repsonse:***\n{}", response_text),
//                 })
//                 .await
//                 .unwrap();
//         })
//         .into_actor(self);
//         ctx.spawn(ac_fut);
//     }
// }

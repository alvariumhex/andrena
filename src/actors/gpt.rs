use async_openai::{
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs,
        CreateChatCompletionRequest, CreateChatCompletionRequestArgs, Role,
    },
    Client,
};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use ractor::{
    call, rpc::cast, Actor, ActorProcessingErr, ActorRef, BytesConvertable, RpcReplyPort,
};
use ractor_cluster::RactorClusterMessage;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, vec};
use tiktoken_rs::get_chat_completion_max_tokens;

use crate::ai_context::GptContext;

#[derive(RactorClusterMessage)]
pub enum RemoteStoreRequestMessage {
    #[rpc]
    Retrieve(String, u8, RpcReplyPort<String>), // sends back a JSON Serialized Vec<(String, f32)>
}

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub content: String,
    pub channel: u64,
    pub author: String,
    pub metadata: HashMap<String, String>,
}
impl ractor::Message for ChatMessage {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Embedding {
    pub content: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum GptMessage {
    Register(ChatMessage),
    ClearContext(u64),
    SetEmbeddingsFlag(bool),
    SetModel(String),
}

impl BytesConvertable for GptMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum DiscordMessage {
    Send(ChatMessage),
    Typing(u64),
    Receive(ChatMessage),
}

impl BytesConvertable for DiscordMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

pub struct GptState {
    pub client: Client,
    pub context: HashMap<u64, GptContext>,
    pub embeddings: bool,
    pub model: String,
    pub name: String,
}

impl GptState {
    fn insert_message(&mut self, msg: ChatMessage) {
        let context = self.get_context_for_id(&msg.channel);
        context.push_history((msg.author, msg.content));
    }

    fn get_context_for_id(&mut self, channel_id: &u64) -> &mut GptContext {
        match self.context.contains_key(channel_id) {
            true => self.context.get_mut(channel_id).unwrap(),
            false => {
                let mut context = GptContext::new();
                context.set_static_context("You are a chatbot that helps users answer questions about documentation that is provided within the chat");
                self.context.insert(channel_id.to_owned(), context);
                self.context.get_mut(channel_id).unwrap()
            }
        }
    }

    fn clear_context(&mut self, channel_id: &u64) {
        self.context.remove(channel_id);
    }

    async fn fetch_embeddings(&self, query: String, limit: u8) -> Vec<(Embedding, f32)> {
        let actors = ractor::pg::get_members(&String::from("embed_store"));
        let store = actors.first();
        if store.is_none() {
            warn!("No store found");
            return vec![];
        }

        let store = ActorRef::<RemoteStoreRequestMessage>::from(store.unwrap().clone());
        let res = call!(store, RemoteStoreRequestMessage::Retrieve, query, limit).unwrap();
        let embeddings: Vec<(Embedding, f32)> = serde_json::from_str(&res).unwrap();
        embeddings
    }

    fn generate_response(&mut self, channel: u64) -> CreateChatCompletionRequest {
        debug!("Generating response for channel: {}", channel);
        let model = self.model.clone();
        let context = self.get_context_for_id(&channel);

        let include_static_context = context.embeddings.len() > 0;

        context.manage_tokens(&model);
        let max_tokens = get_chat_completion_max_tokens(
            &model,
            &context.to_openai_chat_history(include_static_context),
        )
        .unwrap();
        CreateChatCompletionRequestArgs::default()
            .max_tokens((max_tokens as u16) - 110)
            .model(model)
            .messages(context.to_openai_chat_history(include_static_context))
            .build()
            .expect("Failed to build request")
    }

    fn clear_embeddings(&mut self, channel: u64) {
        let context = self.get_context_for_id(&channel);
        context.clear_embeddings();
    }

    fn insert_embeddings(&mut self, channel: u64, embeddings: Vec<String>) {
        if embeddings.is_empty() {
            return;
        }
        let context = self.get_context_for_id(&channel);
        context.embeddings = embeddings;
        context.embeddings.push("Given above documentation, answer the question. If you cannot find the answer in the documentation, mention it. Inlucde the url of the document in your response".to_string());
    }

    fn get_semantic_query(&mut self, channel: u64) -> String {
        let context = self.get_context_for_id(&channel);
        context.fetch_semantic_query()
    }

    async fn embeddings_only_response(&self, embeddings: Vec<String>, query: String) -> String {
        let model = self.model.clone();
        let mut chat: Vec<ChatCompletionRequestMessage> = vec![];

        for embedding in embeddings {
            let entry = ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .name("SYSTEM")
                .content(embedding)
                .build()
                .unwrap();
            chat.push(entry);
        }

        let context = ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .name("SYSTEM")
            .content(format!("Above is the relevant documentation to answer the Question. Your respone will later be consolidated with other responses to create a full answer to the question. If you believe the documentation does not answer the question give an empty response back\nQuestion: {}", query))
            .build()
            .unwrap();

        chat.push(context);

        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens(2000 as u16)
            .model(model)
            .messages(chat)
            .build()
            .expect("Failed to build request");

        let response = self.client.chat().create(request).await;
        match response {
            Ok(response) => {
                if let Some(usage) = response.usage {
                    debug!("tokens: {}", usage.total_tokens);
                }
                debug!(
                    "intermediate response: {}",
                    response.choices[0].message.content
                );
                if let Some(resp) = response.choices.first() {
                    resp.message.content.clone()
                } else {
                    "Failed to generate intermediate response: No choices".to_owned()
                }
            }
            Err(e) => {
                error!("Failed to generate intermediate response: {:?}", e);
                "Failed to generate intermediate response".to_owned()
            }
        }
    }

    async fn consolidate_responses(&self, responses: Vec<String>, query: String) -> String {
        let model = self.model.clone();
        let mut chat: Vec<ChatCompletionRequestMessage> = vec![];
        for response in responses {
            let entry = ChatCompletionRequestMessageArgs::default()
                .role(Role::User)
                .name("SYSTEM")
                .content(response)
                .build()
                .unwrap();
            chat.push(entry);
        }

        let context = ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .name("SYSTEM")
            .content(format!("Above is a collection of generated responses, I want you to consolidate these into a single response. Do not leave out any information.\nOriginal question: {}", query))
            .build()
            .unwrap();

        chat.push(context);

        let request = CreateChatCompletionRequestArgs::default()
            .max_tokens(2000 as u16)
            .model(model)
            .messages(chat)
            .build()
            .expect("Failed to build request");

        let response = self.client.chat().create(request).await;
        match response {
            Ok(response) => {
                if let Some(usage) = response.usage {
                    debug!("tokens: {}", usage.total_tokens);
                }
                debug!(
                    "consolidated response: {}",
                    response.choices[0].message.content
                );
                if let Some(resp) = response.choices.first() {
                    resp.message.content.clone()
                } else {
                    "Failed to generate consolidated response: No choices".to_owned()
                }
            }
            Err(e) => {
                error!("Failed to generate consolidated response: {:?}", e);
                "Failed to generate consolidated response".to_owned()
            }
        }
    }
}

pub struct GptActor;

#[async_trait]
impl Actor for GptActor {
    type Msg = GptMessage;
    type State = GptState;
    type Arguments = ();

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        _args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
        let client = Client::new().with_api_key(api_key);

        let context = HashMap::new();
        let embeddings = false;
        let model = "gpt-3.5-turbo".to_owned();

        debug!("Registering with the messages_receive group");
        ractor::pg::join("messages_receive".to_owned(), vec![myself.get_cell()]);

        Ok(GptState {
            client,
            context,
            embeddings,
            model,
            name: "computer".to_string(),
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            GptMessage::Register(chat_message) => {
                info!("Received message: {:?}", chat_message);
                if !chat_message.content.contains("With embeddings repsonse") {
                    state.insert_message(chat_message.clone());
                }
                let name = state.name.clone();
                if chat_message.author.to_lowercase() == name.to_lowercase() {
                    return Ok(());
                }

                if !chat_message
                    .content
                    .to_lowercase()
                    .contains(&name.to_lowercase())
                    && chat_message.metadata.get("provider") == Some(&"discord".to_owned())
                {
                    return Ok(());
                }

                debug!("Changing status to typing");
                let actors = ractor::pg::get_members(&"typing".to_owned());
                for actor in actors.clone() {
                    cast(&actor, TypingMessage::Start(chat_message.channel)).unwrap();
                }

                let embeddings = state
                    .fetch_embeddings(chat_message.content.clone(), 4)
                    .await;
                debug!(
                    "Embeddings distances: {:?}",
                    embeddings.iter().map(|e| e.1).collect::<Vec<f32>>()
                );
                let mut embeddings: Vec<String> = embeddings
                    .iter()
                    .filter(|e| e.1 < 0.25)
                    .map(|(s, _)| s.clone().content)
                    .collect();

                // reverse so that the most similar item is latest in the context, this improves the quality of the response
                embeddings.reverse();

                debug!("Embeddings {}: {:?}", embeddings.len(), embeddings);

                state.clear_embeddings(chat_message.channel);
                state.insert_embeddings(chat_message.channel, embeddings);

                // let mut responses: Vec<String> = vec![];
                // for chunk in embeddings.chunks(3) {
                //     let result = state
                //         .embeddings_only_response(chunk.to_vec(), chat_message.content.clone())
                //         .await;
                //     responses.push(result);
                // }
                // let response_text_embeddings = state
                //     .consolidate_responses(responses, chat_message.content.clone())
                //     .await;

                let request = state.generate_response(chat_message.channel);
                let response = state.client.chat().create(request).await;
                let response_text = match response {
                    Ok(response) => {
                        if let Some(usage) = response.usage {
                            debug!("tokens: {}", usage.total_tokens);
                        }
                        debug!("response: {}", response.choices[0].message.content);
                        if let Some(resp) = response.choices.first() {
                            resp.message.content.clone()
                        } else {
                            "Failed to generate response: No choices".to_owned()
                        }
                    }
                    Err(e) => {
                        error!("Failed to generate response: {:?}", e);
                        "Failed to generate response".to_owned()
                    }
                };

                info!("Sending response: {}", response_text);

                for actor in actors {
                    cast(&actor, TypingMessage::Stop(chat_message.channel)).unwrap();
                }
                let subscribers = ractor::pg::get_members(&"messages_send".to_owned());

                for subscriber in subscribers {
                    cast(
                        &subscriber,
                        DiscordMessage::Send(ChatMessage {
                            channel: chat_message.channel,
                            content: response_text.clone(),
                            author: state.name.clone(),
                            metadata: HashMap::new(),
                        }),
                    )
                    .unwrap();
                }

                Ok(())
            }
            GptMessage::ClearContext(channel) => {
                state.clear_context(&channel);
                Ok(())
            }
            GptMessage::SetEmbeddingsFlag(flag) => {
                state.embeddings = flag;
                Ok(())
            }
            GptMessage::SetModel(model) => {
                state.model = model;
                Ok(())
            }
        }
    }
}

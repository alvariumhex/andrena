use std::{collections::HashMap, env};

use async_openai::{
    types::{CreateChatCompletionRequest, CreateChatCompletionRequestArgs},
    Client,
};
use log::{debug, error, info, warn};
use ractor::{
    actor::messages::BoxedState, call, rpc::cast, Actor, ActorProcessingErr, ActorRef,
    BytesConvertable, Message,
};
use ractor_cluster::RactorClusterMessage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tiktoken_rs::get_chat_completion_max_tokens;

use crate::{
    actors::{
        communication::{discord::ChatActorMessage, typing::TypingMessage},
        tools::transcribe::{TranscribeTool, TranscribeToolMessage},
    },
    ai_context::GptContext,
};

use super::gpt::{ChatMessage, Embedding, RemoteStoreRequestMessage};

#[derive(Deserialize, Serialize, Clone, Debug)]
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

impl From<ChatMessage> for ChannelMessage {
    fn from(msg: ChatMessage) -> Self {
        Self::Register(msg)
    }
}

pub struct ChannelState {
    id: u64,
    wakeword: Option<String>,
    model: String,
    client: Client,
    context: GptContext,
    tools: Vec<String>,
}

pub struct ChannelActor;

impl ChannelState {
    fn insert_message(&mut self, msg: ChatMessage) {
        self.context.push_history((msg.author, msg.content));
    }

    fn clear_context(&mut self) {
        self.context.history.clear();
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

    fn generate_response(&mut self) -> CreateChatCompletionRequest {
        debug!("Generating response for channel: {}", self.id);
        let model = self.model.clone();
        let include_static_context = self.context.embeddings.len() > 0;

        self.context.manage_tokens(&model);
        let max_tokens = get_chat_completion_max_tokens(
            &model,
            &self.context.to_openai_chat_history(include_static_context),
        )
        .unwrap();
        CreateChatCompletionRequestArgs::default()
            .max_tokens((max_tokens as u16) - 110)
            .model(model)
            .messages(self.context.to_openai_chat_history(include_static_context))
            .build()
            .expect("Failed to build request")
    }

    fn clear_embeddings(&mut self) {
        self.context.clear_embeddings();
    }

    fn insert_embeddings(&mut self, embeddings: Vec<String>) {
        if embeddings.is_empty() {
            return;
        }
        self.context.embeddings = embeddings;
        self.context.embeddings.push("Given above documentation, answer the question. If you cannot find the answer in the documentation, mention it. Inlucde the url of the document in your response".to_string());
    }

    async fn pre_process(&mut self, content: String) -> String {
        // transcribe tool
        let mut content = content;
        if self.tools.contains(&"transcribe".to_owned()) {
            let regex = Regex::new(r"(?m)https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)").unwrap();
            let mut urls = regex
                .find_iter(&content)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<String>>();
            for url in urls.iter_mut() {
                let (trans_actor, _) = Actor::spawn(None, TranscribeTool, ()).await.unwrap();
                let response =
                    call!(&trans_actor, TranscribeToolMessage::Transcribe, url.clone()).unwrap();

                match response {
                    Ok(response) => {
                        info!("Transcription response: {:?}", response);
                        // replace the url with the transcription
                        let response = format!("\"{}\"", response);
                        content = content.replace(&url.clone(), &response);
                    }
                    _ => {
                        info!("Transcription failed for url {}", url);
                    }
                }
            }
        }

        content
    }
}

#[async_trait::async_trait]
impl Actor for ChannelActor {
    type Msg = ChannelMessage;
    type State = ChannelState;
    type Arguments = Option<u64>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        id: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let mut id = id;
        if id.is_none() {
            id = Some(rand::random());
        }
        let client = Client::new().with_api_key(env::var("OPENAI_API_KEY").unwrap());
        let context = GptContext::new();

        Ok(ChannelState {
            id: id.unwrap(),
            wakeword: Some("Lovelace".to_owned()),
            model: "gpt-3.5-turbo".to_owned(),
            client,
            context,
            tools: vec!["transcribe".to_owned()],
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            ChannelMessage::Register(chat_message) => {
                info!("Received message: {:?}", chat_message);
                let mut content = chat_message.content.clone();
                let name = state
                    .wakeword
                    .clone()
                    .or(Some("Computer".to_owned()))
                    .unwrap()
                    .clone();

                if state.wakeword.is_some()
                    && chat_message.author.to_lowercase()
                        == state.wakeword.clone().unwrap().to_lowercase()
                {
                    state.insert_message(chat_message.clone());
                    return Ok(());
                }

                if !content.to_lowercase().contains(&name.to_lowercase())
                    && chat_message.metadata.get("provider") == Some(&"discord".to_owned())
                {
                    state.insert_message(chat_message.clone());

                    return Ok(());
                }

                debug!("Changing status to typing");
                let actor = ractor::registry::where_is("typing".to_owned()).unwrap();
                cast(&actor, TypingMessage::Start(chat_message.channel)).unwrap();

                content = state.pre_process(content).await;

                info!("Content: {}", content);
                state.insert_message(ChatMessage {
                    content: content.clone(),
                    channel: chat_message.channel,
                    author: chat_message.author,
                    metadata: chat_message.metadata,
                });
                let embeddings = state.fetch_embeddings(content.clone(), 4).await;
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

                state.clear_embeddings();
                state.insert_embeddings(embeddings);

                let request = state.generate_response();
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

                cast(&actor, TypingMessage::Stop(chat_message.channel)).unwrap();
                let subscribers = ractor::pg::get_members(&"messages_send".to_owned());

                for subscriber in subscribers {
                    cast(
                        &subscriber,
                        ChatActorMessage::Send(ChatMessage {
                            channel: chat_message.channel,
                            content: response_text.clone(),
                            author: name.clone(),
                            metadata: HashMap::new(),
                        }),
                    )
                    .unwrap();
                }
            }
            ChannelMessage::ClearContext => {
                state.clear_context();
            }
            ChannelMessage::SetWakeword(wakeword) => {
                state.wakeword = Some(wakeword);
            }
            ChannelMessage::SetModel(model) => {
                state.model = model;
            }
        }

        Ok(())
    }
}

// unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_start() {
        assert!(Actor::spawn(None, ChannelActor, None).await.is_ok());
    }
}

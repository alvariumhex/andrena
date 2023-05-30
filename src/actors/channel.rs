use std::{collections::HashMap, env};

use async_openai::{
    types::{CreateChatCompletionRequest, CreateChatCompletionRequestArgs},
    Client,
};
use log::{debug, error, info};
use ractor::{call, rpc::cast, Actor, ActorProcessingErr, ActorRef, Message, RpcReplyPort};
use regex::Regex;

use tiktoken_rs::get_chat_completion_max_tokens;

use crate::{
    actors::{
        communication::{discord::ChatActorMessage, typing::TypingMessage},
        tools::{
            embeddings::{Embeddable, EmbeddingGenerator, EmbeddingGeneratorMessage},
            transcribe::{TranscribeTool, TranscribeToolMessage, TranscriptionResult},
        },
    },
    ai_context::GptContext,
};

use super::{
    gpt::ChatMessage,
    tools::{
        embeddings::Embedding,
        github::{GithubScraperActor, GithubScraperMessage},
    },
};

#[derive(Debug)]
pub enum ChannelMessage {
    Register(ChatMessage),
    GetHistory(RpcReplyPort<Vec<(String, String)>>),
    ClearContext,
    SetWakeword(String),
    SetModel(String),
}

impl Message for ChannelMessage {}

impl From<ChatMessage> for ChannelMessage {
    fn from(msg: ChatMessage) -> Self {
        Self::Register(msg)
    }
}

pub struct ChannelState {
    pub id: u64,
    pub wakeword: Option<String>,
    pub model: String,
    client: Client,
    context: GptContext,
    pub tools: Vec<String>,
}

pub struct ChannelActor;

fn cosine_dist(vec_a: &[f32], vec_b: &[f32], vec_size: &usize) -> f32 {
    let mut a_dot_b: f32 = 0.0;
    let mut a_mag: f32 = 0.0;
    let mut b_mag: f32 = 0.0;

    for i in 0..*vec_size {
        a_dot_b += vec_a[i] * vec_b[i];
        a_mag += vec_a[i] * vec_a[i];
        b_mag += vec_b[i] * vec_b[i];
    }

    1.0 - (a_dot_b / (a_mag.sqrt() * b_mag.sqrt()))
}

impl ChannelState {
    fn insert_message(&mut self, msg: ChatMessage) {
        self.context.push_history((msg.author, msg.content));
    }

    fn clear_context(&mut self) {
        self.context.history.clear();
    }

    async fn fetch_embeddings(&self, query: String, limit: u8) -> Vec<(&Embedding, f32)> {
        // TODO spawn one child actor to handle this and store it in state so we don't have to recreate the actor every message
        let (embed_actor, _) = Actor::spawn(None, EmbeddingGenerator, ()).await.unwrap();
        let mut query = query;

        // remove wakeword from query to improve embedding accuracy
        if self.wakeword.is_some() {
            query = query.replace(&self.wakeword.clone().unwrap(), "");
        }

        let query_embedding: Vec<f32> =
            call!(embed_actor, EmbeddingGeneratorMessage::Query, query).unwrap();

        let mut sorted_embeds: Vec<(&Embedding, f32)> = Vec::new();
        self.context.embeddings.iter().for_each(|e| {
            sorted_embeds.push((
                e,
                cosine_dist(
                    &e.vector,
                    query_embedding.as_slice(),
                    &query_embedding.len(),
                ),
            ))
        });
        sorted_embeds.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let limit = std::cmp::min(sorted_embeds.len(), limit.into());
        sorted_embeds.truncate(limit);
        sorted_embeds
    }

    fn create_response_request(&mut self) -> CreateChatCompletionRequest {
        debug!("Generating response for channel: {}", self.id);
        let model = self.model.clone();
        let include_static_context = !self.context.embeddings.is_empty();

        self.context.manage_tokens(&model);
        let max_tokens = get_chat_completion_max_tokens(
            &model,
            &self.context.to_openai_chat_history(include_static_context),
        )
        .unwrap();

        CreateChatCompletionRequestArgs::default()
            .max_tokens(
                u16::try_from(max_tokens).expect("max_tokens value too large for openAI") - 110,
            )
            .model(model)
            .messages(self.context.to_openai_chat_history(include_static_context))
            .build()
            .expect("Failed to build request")
    }

    async fn generate_response(&mut self, chat_message: ChatMessage, content: String) {
        debug!("Changing status to typing");
        let actor = ractor::registry::where_is("typing".to_owned()).unwrap();
        cast(&actor, TypingMessage::Start(chat_message.channel)).unwrap();

        info!("Content: {}", content);
        self.insert_message(chat_message.clone());
        let embeddings = self.fetch_embeddings(content.clone(), 4).await;
        debug!(
            "Embeddings distances: {:?}",
            embeddings.iter().map(|e| e.1).collect::<Vec<f32>>()
        );
        let mut embeddings: Vec<Embedding> = embeddings
            .iter()
            .filter(|e| e.1 < 0.35)
            .map(|(s, _)| (*s).clone())
            .collect();

        // reverse so that the most similar item is latest in the context, this improves the quality of the response
        embeddings.reverse();
        debug!("Embeddings {}", embeddings.len());
        self.context.selected_embeddings = embeddings.iter().map(|e| e.clone()).collect();

        let request = self.create_response_request();
        let response = self.client.chat().create(request).await;
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

        let response_message = self.send_message(chat_message.clone(), response_text);
        self.insert_message(response_message);
    }

    fn clear_embeddings(&mut self) {
        self.context.clear_embeddings();
    }

    fn insert_embeddings(&mut self, embeddings: Vec<Embedding>) {
        self.context.embeddings.extend(embeddings);
    }

    fn send_message(&self, message: ChatMessage, content: String) -> ChatMessage {
        let response_message = ChatMessage {
            channel: message.channel,
            content,
            author: self.wakeword.clone().unwrap_or("Computer".to_owned()),
            metadata: HashMap::new(),
        };

        let subscribers = ractor::pg::get_members(&"messages_send".to_owned());
        for subscriber in subscribers {
            cast(
                &subscriber,
                ChatActorMessage::Send(response_message.clone()),
            )
            .unwrap();
        }

        response_message
    }

    async fn execute_command(
        &mut self,
        command: String,
        params: Option<String>,
        chat_message: ChatMessage,
    ) {
        if command == "transcribe" {
            info!("Executing transcribe command");
            let mut content = params.unwrap();
            let regex = Regex::new(r"(?m)https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)").unwrap();
            let mut urls = regex
                .find_iter(&content)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<String>>();
            for url in &mut urls {
                let (trans_actor, _) = Actor::spawn(None, TranscribeTool, ()).await.unwrap();
                self.send_message(
                    chat_message.clone(),
                    format!("Transcribing url"),
                );

                let response =
                    call!(&trans_actor, TranscribeToolMessage::Transcribe, url.clone()).unwrap();

                let metadata =
                    call!(&trans_actor, TranscribeToolMessage::Metadata, url.clone()).unwrap();

                self.send_message(
                    chat_message.clone(),
                    format!("Finished transcribing url"),
                );

                match response {
                    Ok(response) => {
                        info!("Transcription response: {:?}", response);
                        // replace the url with the transcription
                        content = content.replace(&url.clone(), &response);
                        match metadata {
                            Ok(metadata) => {
                                info!("Transcription metadata: {:?}", metadata);
                                // replace the url with the transcription
                                let tr = TranscriptionResult {
                                    metadata,
                                    text: response.clone(),
                                    url: url.clone(),
                                };

                                let chunks: Vec<Embedding> = tr
                                    .get_chunks(300)
                                    .iter()
                                    .map(|c| Embedding {
                                        content: c.clone(),
                                        vector: vec![],
                                        graph_vertex: url.clone(),
                                    })
                                    .collect();

                                let (embed_actor, _) =
                                    Actor::spawn(None, EmbeddingGenerator, ()).await.unwrap();
                                let embeddings = call!(
                                    &embed_actor,
                                    EmbeddingGeneratorMessage::Generate,
                                    chunks,
                                    300
                                )
                                .unwrap();

                                self.insert_embeddings(embeddings);
                            }
                            Err(e) => {
                                info!("Transcription metadata failed: {:?}", e);
                            }
                        }
                    }
                    _ => {
                        info!("Transcription failed for url {}", url);
                    }
                }
            }
        } else if command == "github" {
            info!("Executing github command");
            let content = params.unwrap();
            let regex = Regex::new(r"(?m)https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&//=]*)").unwrap();
            let mut urls = regex
                .find_iter(&content)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<String>>();
            for _url in &mut urls {
                let github_token = env::var("GH_ACCESS_TOKEN").unwrap();

                let (github_actor, _) = Actor::spawn(None, GithubScraperActor, github_token)
                    .await
                    .unwrap();
                self.send_message(chat_message.clone(), "Fetching github url".to_string());

                // hardcoded for now
                let regex = Regex::new(r"(?m).*github\.com/([\w\-_]+)/([\w\-_]+)(.+)?").unwrap();
                let captures = regex.captures(&_url).unwrap();
                let owner = captures.get(1).unwrap().as_str();
                let repo = captures.get(2).unwrap().as_str();

                let response = call!(
                    &github_actor,
                    GithubScraperMessage::ScrapeRepo,
                    owner.to_owned(),
                    repo.to_owned(),
                    "default".to_owned()
                )
                .unwrap();

                let files: Vec<Embedding> = response
                    .unwrap()
                    .iter()
                    .map(|f| (f, f.get_chunks(300)))
                    .flat_map(|t| {
                        t.1.iter()
                            .map(|c| Embedding {
                                content: c.clone(),
                                vector: vec![],
                                graph_vertex: t.0.metadata.get("url").unwrap().clone(),
                            })
                            .collect::<Vec<Embedding>>()
                    })
                    .collect();

                self.send_message(
                    chat_message.clone(),
                    format!("Fetched {} files, processing them", files.len()),
                );

                let (embed_actor, _) = Actor::spawn(None, EmbeddingGenerator, ()).await.unwrap();
                let embeddings = call!(
                    &embed_actor,
                    EmbeddingGeneratorMessage::Generate,
                    files,
                    300
                )
                .unwrap();

                self.insert_embeddings(embeddings);

                debug!(
                    "Finished processing files, new embedding count: {}",
                    self.context.embeddings.len()
                );

                self.send_message(
                    chat_message.clone(),
                    "Finished fetching github url".to_string(),
                );
            }
        } else if command == "debug" {
            self.send_message(chat_message.clone(), "Utilized embeddings:".to_owned());
            for embed in &self.context.selected_embeddings {
                self.send_message(
                    chat_message.clone(),
                    format!("source: {}\ncontent {}", embed.graph_vertex, embed.content),
                );
            }

            self.send_message(
                chat_message,
                format!(
                    "Current embeddings available for channel: {}",
                    self.context.embeddings.len()
                ),
            );
        } else if command == "model" {
            if let Some(model) = params {
                if model != "gpt-3.5-turbo" && model != "gpt-4" && model != "gpt-4-32k	" {
                    self.send_message(
                        chat_message,
                        format!("Unknown model {}, defaulting to gpt-3.5-turbo\nAvailable models: gpt-3.5-turbo, gpt-4, gpt-4-32k", model),
                    );
                    return;
                }
                self.model = model;
                self.send_message(
                    chat_message,
                    format!("Set model to {}", self.model),
                );
            } else {
                self.send_message(
                    chat_message,
                    format!("Current model is {}", self.model),
                );
            }
        }
    }
}

fn command_extract(content: &str) -> (String, Option<String>) {
    let regex = Regex::new(r"(?m)^!(\w+) ?(.+)?").unwrap();
    let mut result = regex.captures_iter(content);
    let matches = result.next().unwrap();
    let command = matches.get(1).unwrap().as_str().to_owned();
    if let Some(param) = matches.get(2) {
        (command, Some(param.as_str().to_owned()))
    } else {
        (command, None)
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
            tools: vec!["transcribe".to_owned(), "github".to_owned()],
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
                let content = chat_message.content.clone();
                let name = state
                    .wakeword
                    .clone()
                    .unwrap_or("Computer".to_owned())
                    .clone();

                if content.starts_with('!') {
                    let (command, params) = command_extract(&content);
                    debug!("command invoked with {} and {:?}", command, params);
                    state
                        .execute_command(command, params, chat_message.clone())
                        .await;
                    state.insert_message(chat_message);
                    return Ok(());
                }

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

                state.generate_response(chat_message, content).await;
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
            ChannelMessage::GetHistory(port) => {
                port.send(state.context.history.clone()).unwrap();
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
    async fn start() {
        env::set_var("OPENAI_API_KEY", "dummy_key");
        assert!(Actor::spawn(None, ChannelActor, None).await.is_ok());
    }

    #[test]
    fn command_test() {
        let (command, params) = command_extract("!github https://github.com");

        assert_eq!(command, "github");
        assert_eq!(params.unwrap(), "https://github.com");

        let (command, params) = command_extract("!transcribe https://youtube.be");

        assert_eq!(command, "transcribe");
        assert_eq!(params.unwrap(), "https://youtube.be");

        let (command, params) = command_extract("!empty");

        assert_eq!(command, "empty");
        assert!(params.is_none())
    }
}

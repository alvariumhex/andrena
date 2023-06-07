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

impl ChannelState {
    fn insert_message(&mut self, author: String, content: String) {
        self.context.push_history((author, content));
    }

    fn create_response_request(&mut self) -> CreateChatCompletionRequest {
        debug!("Generating response for channel: {}", self.id);
        let model = self.model.clone();

        self.context.manage_tokens(&model);
        let max_tokens =
            get_chat_completion_max_tokens(&model, &self.context.to_openai_chat_history(true))
                .unwrap();

        CreateChatCompletionRequestArgs::default()
            .max_tokens(
                u16::try_from(max_tokens).expect("max_tokens value too large for openAI") - 110,
            )
            .model(model)
            .messages(self.context.to_openai_chat_history(true))
            .build()
            .expect("Failed to build request")
    }

    async fn generate_response(&mut self, chat_message: ChatMessage) -> Result<String, String> {
        debug!("Changing status to typing");
        let actor = ractor::registry::where_is("typing".to_owned()).unwrap();
        cast(&actor, TypingMessage::Start(chat_message.channel)).unwrap();

        let request = self.create_response_request();
        let response = self.client.chat().create(request).await;
        match response {
            Ok(response) => {
                if let Some(usage) = response.usage {
                    debug!("tokens: {}", usage.total_tokens);
                }
                if let Some(resp) = response.choices.first() {
                    Ok(resp.message.content.clone())
                } else {
                    Err("SYSTEM: Failed to generate response: No choices".to_owned())
                }
            }
            Err(e) => {
                error!("Failed to generate response: {:?}", e);
                Err("SYSTEM: Failed to generate response".to_owned())
            }
        }
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
}

pub struct ChannelActor;

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
        let id = id.unwrap_or_else(|| rand::random::<u64>());
        let client = Client::new().with_api_key(env::var("OPENAI_API_KEY").unwrap());
        let context = GptContext::new();
        let tools = vec![];

        Ok(ChannelState {
            id,
            client,
            context,
            tools,
            wakeword: Some("Lovelace".to_owned()),
            model: "gpt-4".to_owned(),
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            ChannelMessage::Register(msg) => {
                let content = format!("QUESTION: {}", msg.content.clone());

                state.insert_message(msg.author.clone(), content.clone());

                let wakeword = state
                    .wakeword
                    .clone();

                if wakeword.is_some() && !content.to_lowercase().contains(&wakeword.unwrap().to_lowercase()) {
                    info!("Channel {} received message: {}", state.id, content);
                    info!("Channel {} is not woken up", state.id);
                    return Ok(());
                }

                let mut debug_history: Vec<(String, String)> = state
                    .context
                    .static_context
                    .iter()
                    .map(|c| (String::from("SYSTEM"), c.clone()))
                    .collect();

                debug_history.clear();
                debug_history.extend(state.context.history.clone());

                info!("History: {:?}", debug_history);
                info!("Channel {} received message: {}", state.id, content);

                let response = state.generate_response(msg.clone()).await.unwrap();
                info!("Channel {} generated response: {}", state.id, response);
                state.insert_message(String::from("Assistant"), response.clone());

                if is_answer_faulty(&response) {
                    warn!("Answer is faulty, retrying");
                    state.insert_message(
                        String::from("SYSTEM"),
                        "Only answer with at most one THOUGHT or ANSWER".to_owned(),
                    );
                    let response = state.generate_response(msg.clone()).await.unwrap();
                    info!("Channel {} generated response: {}", state.id, response);
                }

                state.send_message(msg.clone(), response.clone());

                let actor = ractor::registry::where_is("typing".to_owned()).unwrap();
                cast(&actor, TypingMessage::Stop(msg.channel)).unwrap();
            }
            ChannelMessage::GetHistory(port) => {
                port.send(state.context.history.clone()).unwrap();
            }
            ChannelMessage::ClearContext => {
                state.context = GptContext::new();
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

fn is_answer_faulty(answer: &str) -> bool {
    let regex = Regex::new(r"(?mi)\w+: ").unwrap();
    let mut count = 0;
    regex
        .captures_iter(answer)
        .inspect(|_| count += 1)
        .for_each(drop); // look it's probably not the best way to do this but it works
    count > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_answer_faulty() {
        assert!(!is_answer_faulty("THOUGHT: What is the meaning of life?"));
        assert!(!is_answer_faulty("ANSWER: 42"));

        assert!(is_answer_faulty(
            "THOUGHT: What is the meaning of life?\nANSWER: 42"
        ));
    }
}

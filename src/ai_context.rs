use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};
use log::info;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

pub struct GptContext {
    pub static_context: Vec<String>,
    pub embeddings: Vec<String>,
    pub history: Vec<(String, String)>,
}

impl GptContext {
    pub fn new() -> GptContext {
        GptContext {
            static_context: Vec::new(),
            history: Vec::new(),
            embeddings: Vec::new(),
        }
    }

    pub fn set_static_context(&mut self, context: &str) {
        self.static_context = vec![context.to_owned()];
    }

    pub fn fetch_semantic_query(&self) -> String {
        let mut history = self.history.clone();
        history.retain(|h| h.0 != "Lovelace");
        history
            .iter()
            .map(|h| h.1.clone())
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub fn push_history(&mut self, entry: (String, String)) {
        self.history.push(entry);
    }

    pub fn to_openai_chat_history(
        &self,
        include_static_context: bool,
    ) -> Vec<ChatCompletionRequestMessage> {
        let mut chat: Vec<ChatCompletionRequestMessage> = Vec::new();
        if include_static_context {
            for h in &self.static_context {
                chat.push(
                    ChatCompletionRequestMessageArgs::default()
                        .role(Role::System)
                        .name("system")
                        .content(h)
                        .build()
                        .unwrap(),
                );
            }
        }

        for h in &self.history {
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .name(h.0.clone())
                    .content(h.1.clone())
                    .build()
                    .unwrap(),
            );
        }

        chat.pop();

        for h in &self.embeddings {
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .name("documentation")
                    .content(h)
                    .build()
                    .unwrap(),
            );
        }

        if self.history.len() > 0 {
            let last_history = self.history.last().unwrap();
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .name(last_history.0.clone())
                    .content(last_history.1.clone())
                    .build()
                    .unwrap(),
            )
        }

        chat
    }

    pub fn manage_tokens(&mut self, model: &str) {
        let mut token_count =
            get_chat_completion_max_tokens(model, &self.to_openai_chat_history(true))
                .expect("Failed to get max tokens");
        while token_count < 750 {
            info!("Reached max token count, removing oldest message from context");
            if self.history.is_empty() {
                panic!("History is empty but token count was reached");
            }
            self.history.remove(0);
            token_count = get_chat_completion_max_tokens(model, &self.to_openai_chat_history(true))
                .expect("Failed to get max tokens");
        }
    }

    pub fn clear_embeddings(&mut self) {
        self.embeddings.clear();
    }

    pub fn calculate_tokens(&self, model: &str) -> usize {
        let mut tokens = 0;
        let bpe = get_bpe_from_model(model).unwrap();
        for h in &self.static_context {
            tokens += bpe.encode_ordinary(h).len();
        }

        for h in &self.embeddings {
            tokens += bpe.encode_ordinary(h).len();
        }

        for h in &self.history {
            tokens += bpe.encode_ordinary(&h.1).len();
        }

        tokens
    }
}

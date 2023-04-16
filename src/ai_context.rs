use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};
use log::info;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

pub struct AiContext {
    pub static_context: Vec<String>,
    pub history: Vec<(String, String)>,
}

impl AiContext {
    pub fn new() -> AiContext {
        AiContext {
            static_context: Vec::new(),
            history: Vec::new(),
        }
    }

    pub fn set_static_context(&mut self, context: &str) {
        self.static_context = vec![
            "Systems status is degraded: \n - typing indicator not operational\n - embedding not operational\n - video transcription is not operational\n - attachment extraction is not available\n - article extraction is not available\nReason: systems is undergoing maintenance".to_owned(),
            context.to_owned()
        ];
    }

    pub fn push_history(&mut self, entry: (String, String)) {
        self.history.push(entry);
    }

    pub fn to_chat_history(&self) -> Vec<ChatCompletionRequestMessage> {
        let mut chat: Vec<ChatCompletionRequestMessage> = Vec::new();
        for h in &self.static_context {
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .name("SYSTEM")
                    .content(h)
                    .build()
                    .unwrap(),
            );
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

        chat
    }

    pub fn manage_tokens(&mut self, model: &str) {
        let mut token_count = get_chat_completion_max_tokens(model, &self.to_chat_history())
            .expect("Failed to get max tokens");
        while token_count < 750 {
            info!("Reached max token count, removing oldest message from context");
            self.history.remove(0);
            token_count = get_chat_completion_max_tokens(model, &self.to_chat_history())
                .expect("Failed to get max tokens");
        }
    }

    pub fn calculate_tokens(&self, model: &str) -> usize {
        let mut tokens = 0;
        let bpe = get_bpe_from_model(model).unwrap();
        for h in &self.static_context {
            tokens += bpe.encode_ordinary(h).len();
        }

        for h in &self.history {
            tokens += bpe.encode_ordinary(&h.1).len();
        }

        tokens
    }
}

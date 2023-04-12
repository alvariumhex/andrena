use async_openai::{
    error::OpenAIError,
    types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role},
};

use super::traits::ContextItem;

pub struct GenericMessage {
    message: String,
    author: String,
}

impl ContextItem for GenericMessage {
    fn raw_text(&self) -> String {
        self.message.clone()
    }

    fn convert_to_entry(&self) -> Result<ChatCompletionRequestMessage, OpenAIError> {
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .content(self.raw_text())
            .name(&self.author)
            .build()
    }
}

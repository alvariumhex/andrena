use async_openai::{
    error::OpenAIError,
    types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role},
};

pub trait ContextItem {
    fn convert_to_entry(&self) -> Result<ChatCompletionRequestMessage, OpenAIError> {
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .name("System")
            .content(self.raw_text())
            .build()
    }
    fn raw_text(&self) -> String;
}

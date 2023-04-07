use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};

pub struct GenericMessage {
    message: String,
    author: String,
}

impl ContextCreator for GenericMessage {
    fn convert_to_entry(&self) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .content(&self.message)
            .name(&self.author)
            .build()
            .unwrap()
    }
}

pub struct Attachment {
    filename: String,
    url: String,
    content: Option<String>,
}

impl ContextCreator for Attachment {
    fn convert_to_entry(&self) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .content(format!(
                "filename: {}\n file content: {}",
                self.filename,
                self.content
                    .clone()
                    .unwrap_or("File content is still being downloaded".to_owned())
            ))
            .name("System")
            .build()
            .unwrap()
    }
}

pub struct YoutubeVideo {
    title: String,
    url: String,
    transcription: Option<String>,
}

impl ContextCreator for YoutubeVideo {
    fn convert_to_entry(&self) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessageArgs::default()
            .role(Role::User)
            .name("System")
            .content(format!(
                "Video title: {}\nVideo transcription: {}",
                self.title,
                self.transcription
                    .clone()
                    .unwrap_or("Video still being transcribed".to_owned())
            ))
            .build()
            .unwrap()
    }
}

pub trait ContextCreator {
    fn convert_to_entry(&self) -> ChatCompletionRequestMessage;
}

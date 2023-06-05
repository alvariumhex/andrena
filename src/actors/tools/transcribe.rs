use std::{collections::HashMap, env, process::Command};

use super::embeddings::Embeddable;
use async_openai::{types::CreateTranscriptionRequestArgs, Client};
use log::{info, warn};
use ractor::{Actor, ActorProcessingErr, ActorRef, Message, RpcReplyPort};
use rustube::{Id, VideoFetcher};

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub url: String,
    pub text: String,
    pub metadata: HashMap<String, String>,
}

impl Embeddable for TranscriptionResult {
    fn human_readable_source(&self) -> String {
        self.url.clone()
    }

    fn long_description(&self) -> String {
        self.metadata
            .get("description")
            .unwrap_or(&String::new())
            .clone()
    }

    fn short_description(&self) -> String {
        self.metadata
            .get("description")
            .unwrap_or(&String::new())
            .clone()
    }

    fn get_chunks(&self, size: usize) -> Vec<String> {
        // metadata string
        let metadata = format!(
            "\ntitle: {}\n url: {}, \n description: {}",
            self.metadata.get("title").unwrap_or(&"No Title".to_owned()),
            self.url.clone(),
            self.short_description(),
        );

        self.text
            .split_whitespace()
            .collect::<Vec<&str>>()
            .chunks(size)
            .map(|chunk| {
                let mut result = chunk.join(" ");
                result.push_str(&metadata);
                result
            })
            .collect::<Vec<String>>()
    }
}

pub enum TranscribeToolMessage {
    Transcribe(String, RpcReplyPort<Result<String, ()>>),
    Metadata(String, RpcReplyPort<Result<HashMap<String, String>, ()>>),
}

impl Message for TranscribeToolMessage {}

pub struct TranscribeToolState {
    client: Client,
}
pub struct TranscribeTool;

#[async_trait::async_trait]
impl Actor for TranscribeTool {
    type Arguments = ();
    type Msg = TranscribeToolMessage;
    type State = TranscribeToolState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let client = Client::new().with_api_key(env::var("OPENAI_API_KEY").unwrap());
        Ok(TranscribeToolState { client })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            TranscribeToolMessage::Transcribe(url, rpc) => {
                // rand file name
                info!("Transcribing: {}", url);
                let file_name = rand::random::<u64>().to_string();
                let output = Command::new("yt-dlp")
                    .arg("--no-check-certificate") // TODO dirty fix for self signed cert
                    .arg("-f")
                    .arg("bestaudio")
                    .arg("-o")
                    .arg(format!("{file_name}.webm"))
                    .arg(url.clone())
                    .output()
                    .expect("failed to execute process");

                assert!(
                    output.status.success(),
                    "Failed to download video: {}\n{}",
                    url,
                    String::from_utf8(output.stdout).unwrap()
                );

                let res = state
                    .client
                    .audio()
                    .transcribe(
                        CreateTranscriptionRequestArgs::default()
                            .file(format!("{file_name}.webm"))
                            .model("whisper-1".to_owned())
                            .build()
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                // cleanup file
                std::fs::remove_file(format!("{file_name}.webm")).unwrap();

                rpc.send(Ok(res.text)).unwrap();
            }
            TranscribeToolMessage::Metadata(url, port) => {
                let Ok(id) = Id::from_raw(&url) else {
                    warn!("Invalid youtube url: {}", url);
                    port.send(Err(())).unwrap();
                    return Ok(());
                };

                let descrambler = VideoFetcher::from_id(id.into_owned())
                    .unwrap()
                    .fetch()
                    .await
                    .unwrap();

                let mut metadata: HashMap<String, String> = HashMap::new();

                metadata.insert("title".to_owned(), descrambler.video_title().clone());
                metadata.insert(
                    "description".to_owned(),
                    descrambler.video_details().short_description.clone(),
                );
                metadata.insert(
                    "author".to_owned(),
                    descrambler.video_details().author.clone(),
                );

                port.send(Ok(metadata)).unwrap();
            }
        }
        Ok(())
    }
}

// tests
#[cfg(test)]
mod tests {
    use super::*;
    use ractor::call;
    use tokio::{fs::File, io::AsyncWriteExt};

    #[tokio::test]
    #[ignore = "uses paying service"]
    async fn transcribe_youtube_video() {
        let (actor_ref, _) = Actor::spawn(None, TranscribeTool, ()).await.unwrap();
        let rep = call!(
            &actor_ref,
            TranscribeToolMessage::Transcribe,
            "https://www.youtube.com/shorts/CEV_zDWsxGA".to_owned()
        )
        .unwrap();
        assert!(rep.is_ok());
        assert!(rep.clone().unwrap().starts_with("I can't tell"));

        // save to file
        let mut file = File::create("test_transctibe.txt").await.unwrap();
        file.write_all(rep.unwrap().as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn metadata() {
        let (actor, _) = Actor::spawn(None, TranscribeTool, ()).await.unwrap();
        let rep = call!(
            &actor,
            TranscribeToolMessage::Metadata,
            "https://www.youtube.com/watch?v=CEV_zDWsxGA".to_owned()
        )
        .unwrap();

        assert!(rep.is_ok());
        assert!(
            rep.unwrap().get("title").unwrap()
                == "Another perfect insult from Holt #shorts | Brooklyn Nine-Nine"
        );
    }

    #[tokio::test]
    async fn invalid_metadata() {
        let (actor, _) = Actor::spawn(None, TranscribeTool, ()).await.unwrap();
        let rep = call!(
            &actor,
            TranscribeToolMessage::Metadata,
            "https://www.example.com".to_owned()
        )
        .unwrap();

        assert!(rep.is_err());
    }
}

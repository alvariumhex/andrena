use std::{env, fmt::format, path::Path, process::Command};

use async_openai::{types::CreateTranscriptionRequestArgs, Client};
use hound::SampleFormat;
use log::info;
use ractor::{Actor, ActorProcessingErr, ActorRef, Message, RpcReplyPort};

pub enum TranscribeToolMessage {
    Transcribe(String, RpcReplyPort<Result<String, ()>>),
}

impl Message for TranscribeToolMessage {}

pub struct TranscribeToolState {
    client: Client,
}

fn parse_wav(path: &Path) -> Vec<i16> {
    let mut reader = hound::WavReader::open(path).unwrap();
    if reader.spec().channels != 1 {
        panic!("expected mono audio file");
    }
    if reader.spec().sample_format != SampleFormat::Int {
        panic!("expected integer sample format");
    }
    if reader.spec().sample_rate != 16000 {
        panic!("expected 16KHz sample rate");
    }
    if reader.spec().bits_per_sample != 16 {
        panic!("expected 16 bits per sample");
    }

    reader
        .into_samples::<i16>()
        .map(|x| x.expect("sample"))
        .collect::<Vec<_>>()
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
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            TranscribeToolMessage::Transcribe(url, rpc) => {
                // rand file name
                info!("Transcribing: {}", url);
                let file_name = rand::random::<u64>().to_string();
                let output = Command::new("yt-dlp")
                    .arg("-f")
                    .arg("bestaudio")
                    .arg("-o")
                    .arg(format!("{}.webm", file_name))
                    .arg(url.clone())
                    .output()
                    .expect("failed to execute process");

                if !output.status.success() {
                    panic!(
                        "Failed to download video: {}\n{}",
                        url,
                        String::from_utf8(output.stdout).unwrap()
                    );
                }

                let res = state
                    .client
                    .audio()
                    .transcribe(
                        CreateTranscriptionRequestArgs::default()
                            .file(format!("{}.webm", file_name))
                            .model("whisper-1".to_owned())
                            .build()
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                // cleanup file
                std::fs::remove_file(format!("{}.webm", file_name)).unwrap();

                rpc.send(Ok(res.text)).unwrap();
                myself.stop(Some("done".to_owned()));
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
    async fn test_transcribe_tool() {
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
        // let mut file = File::create("test.txt").await.unwrap();
        // file.write_all(rep.unwrap().as_bytes()).await.unwrap();
    }
}

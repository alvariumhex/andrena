use async_openai::{types::CreateTranscriptionRequestArgs, Client};
use rustube::{Id, VideoFetcher};

use super::traits::ContextItem;

pub struct YoutubeVideoMetadata {
    title: String,
    description: String,
    author: String,
}

pub struct YoutubeVideo {
    url: String,
    metadata: Option<YoutubeVideoMetadata>,
    transcription: Option<String>,
}

impl YoutubeVideo {
    pub fn new(url: String) -> Self {
        Self {
            url,
            metadata: None,
            transcription: None,
        }
    }

    pub async fn fetch_metadata(&mut self) {
        let id = Id::from_raw(self.url.as_str()).unwrap();
        let descrambler = VideoFetcher::from_id(id.into_owned())
            .unwrap()
            .fetch()
            .await
            .unwrap();

        let title = descrambler.video_title().to_owned();
        let description = descrambler.video_details().short_description.to_owned();
        let author = descrambler.video_details().author.to_owned();

        self.metadata = Some(YoutubeVideoMetadata {
            title,
            description,
            author,
        });
    }

    pub async fn fetch_transcription(&mut self, client: &Client) {
        let id = Id::from_raw(self.url.as_str()).unwrap();
        let descrambler = VideoFetcher::from_id(id.into_owned())
            .unwrap()
            .fetch()
            .await
            .unwrap();

        let video = descrambler.descramble().unwrap();
        let path_to_video = video
            .worst_audio()
            .unwrap()
            .download()
            .await
            .expect("Could not download video audio");
        let request = CreateTranscriptionRequestArgs::default()
            .file(path_to_video.to_str().unwrap())
            .model("whisper-1")
            .build()
            .expect("Failed to create transcription request");

        let response = client
            .audio()
            .transcribe(request)
            .await
            .expect("Failed to fetch transcription")
            .text;

        self.transcription = Some(response);
    }
}

impl ContextItem for YoutubeVideo {
    fn raw_text(&self) -> String {
        if let Some(metadata) = &self.metadata {
            format!(
                "Video title: {}\nVideo Description: {}\nVideo Author: {}\nVideo transcription: {}",
                metadata.title,
                metadata.description,
                metadata.author,
                self.transcription
                    .clone()
                    .unwrap_or("Video still being transcribed".to_owned())
            )
        } else {
            format!(
                "Video transcription: {}",
                self.transcription
                    .clone()
                    .unwrap_or("Video still being transcribed".to_owned())
            )
        }
        
    }
}

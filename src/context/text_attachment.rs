use super::traits::ContextItem;

pub struct TextAttachment {
    filename: String,
    url: String,
    content: Option<String>,
}

impl TextAttachment {
    pub fn new(url: String) -> Self {
        let filename = url.split('/').last().unwrap().to_owned();
        Self {
            filename,
            url,
            content: None,
        }
    }

    pub async fn fetch_content(&mut self) -> Result<(), reqwest::Error> {
        let response = reqwest::get(self.url.clone()).await?;
        let bytes = response.bytes().await?;

        self.content = Some(String::from_utf8_lossy(&bytes).into_owned());
        Ok(())
    }
}

impl ContextItem for TextAttachment {
    fn raw_text(&self) -> String {
        format!(
            "filename: {}\n file content: {}",
            self.filename,
            self.content
                .clone()
                .unwrap_or("File content is still being downloaded".to_owned())
        )
    }
}

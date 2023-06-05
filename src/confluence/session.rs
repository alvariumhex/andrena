use base64::{engine::general_purpose, Engine};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Space {
    pub id: u64,
    pub key: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PageChildren {
    pub page: PaginatedRepsonse<Page>,
    #[serde(rename = "_links")]
    pub links: Links,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub status: String,
    pub space: Option<Space>,
    pub body: Option<Body>,
    #[serde(rename = "_links")]
    pub links: Links,
    pub children: Option<PageChildren>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Body {
    pub view: Option<BodyView>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BodyView {
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct SpacesResult {
    results: Vec<Space>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Links {
    pub next: Option<String>,
    pub prev: Option<String>,
    pub base: Option<String>,
    pub webui: Option<String>,
    #[serde(rename = "self")]
    pub _self: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PaginatedRepsonse<T> {
    pub size: u64,
    pub limit: u64,
    pub start: u64,
    #[serde(rename = "_links")]
    pub links: Links,
    pub results: Vec<T>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpaceContentResult {
    pub page: PaginatedRepsonse<Page>,
}

pub struct Session {
    username: String,
    api_key: String,
    base_url: String,
    client: Client,
}

impl Session {
    pub fn new(username: String, api_key: String, base_url: String) -> Session {
        let auth_header_val = general_purpose::STANDARD.encode(format!("{}:{}", username, api_key));

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {auth_header_val}")).unwrap(),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();

        Session {
            username,
            api_key,
            base_url,
            client,
        }
    }

    pub async fn get_page_by_id(&self, id: u64) -> Result<Page, ()> {
        let url = format!("{}/rest/api/content/{}", self.base_url, id);
        trace!("GET {}", url);
        let response = self
            .client
            .get(url)
            .query(&[("expand", "body.view,space,children.page")])
            .send()
            .await
            .unwrap();
        if response.status().is_client_error() || response.status().is_server_error() {
            error!("Error getting page: {}", response.text().await.unwrap());
            return Err(());
        }
        let page: Page = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        Ok(page)
    }

    pub async fn get_spaces(&self) -> Result<Vec<Space>, ()> {
        let url = format!("{}/rest/api/space", self.base_url);
        trace!("GET {}", url);
        let response = self
            .client
            .get(url)
            .query(&[("type", "global")])
            .send()
            .await
            .unwrap();
        if response.status().is_client_error() || response.status().is_server_error() {
            error!("Error getting page: {}", response.text().await.unwrap());
            return Err(());
        }
        let result: SpacesResult = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        Ok(result.results)
    }

    #[async_recursion::async_recursion]
    pub async fn get_pages_for_space(
        &self,
        space_key: &str,
        next: Option<String>,
    ) -> Result<Vec<Page>, ()> {
        let url = if next.is_some() {
            format!("{}{}", self.base_url, next.unwrap().replace("/page", ""))
        } else {
            format!("{}/rest/api/space/{}/content", self.base_url, space_key)
        };
        trace!("GET {}", url);
        let response = self
            .client
            .get(url)
            .query(&[("expand", "body.view,space,children.page")])
            .send()
            .await
            .unwrap();
        if response.status().is_client_error() || response.status().is_server_error() {
            error!("Error getting page: {}", response.text().await.unwrap());
            return Err(());
        }
        let result: SpaceContentResult =
            serde_json::from_str(&response.text().await.unwrap()).unwrap();
        let mut pages = result.page.results;
        if result.page.links.next.is_some() {
            let mut next_pages = self
                .get_pages_for_space(space_key, result.page.links.next)
                .await
                .unwrap();
            pages.append(&mut next_pages);
            return Ok(pages);
        }
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let _session = Session::new("".to_owned(), "".to_owned(), "".to_owned());
    }
}

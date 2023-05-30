use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_recursion::async_recursion;
use async_trait::async_trait;
use futures::{future::join_all, prelude::*};
use hubcaps::{repositories::Repository, Credentials, Github};
use log::{debug, error, info, trace};
use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};

use super::embeddings::Embeddable;

#[derive(Debug, Clone)]
pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
    pub branch: String,
}

#[derive(Debug, Clone)]
pub struct GitHubFile {
    pub path: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
    pub repo: GitHubRepo,
}

impl Embeddable for GitHubFile {
    fn human_readable_source(&self) -> String {
        self.path.clone()
    }

    fn short_description(&self) -> String {
        unimplemented!()
    }

    fn long_description(&self) -> String {
        unimplemented!()
    }

    fn get_chunks(&self, size: usize) -> Vec<String> {
        // split content every 300 words
        self.content
            .split_whitespace()
            .collect::<Vec<&str>>()
            .chunks(size)
            .map(|chunk| chunk.join(" "))
            .collect::<Vec<String>>()
    }
}

pub enum GithubScraperMessage {
    ScrapeRepo(
        String,
        String,
        String,
        RpcReplyPort<Result<Vec<GitHubFile>, ()>>,
    ),
    ScrapeOrg(String, RpcReplyPort<Result<Vec<GitHubFile>, ()>>),
}

impl ractor::Message for GithubScraperMessage {}

pub struct GithubScraperState {
    github: Github,
}

pub struct GithubScraperActor;

impl GithubScraperActor {
    async fn fetch_github_file_contents(
        repo: &Repository,
        path: &str,
        branch: &str,
    ) -> Result<GitHubFile, Box<dyn std::error::Error>> {
        debug!("downloading file {}", path);

        let file = repo
            .content()
            .file(path, branch)
            .await
            .expect("unable to fetch file");

        let content_url = file.download_url;

        let bytes = reqwest::get(&content_url)
            .await?
            .bytes()
            .await
            .unwrap()
            .to_vec();
        let content = String::from_utf8(bytes)?;

        let mut metadata: HashMap<String, String> = HashMap::new();

        let repo_metadata = repo.get().await.unwrap();

        metadata.insert(String::from("provider"), String::from("github"));
        metadata.insert(String::from("url"), file.html_url.clone());
        metadata.insert(String::from("repo"), repo_metadata.name.clone());
        metadata.insert(String::from("author"), repo_metadata.owner.login.clone());

        Ok(GitHubFile {
            content,
            path: file.html_url,
            metadata,
            repo: GitHubRepo {
                owner: repo_metadata.owner.login,
                name: repo_metadata.name,
                branch: String::from(branch),
            },
        })
    }

    async fn fetch_all_github_contents(
        repo: (&str, &str),
        branch: &str,
        state: &GithubScraperState,
    ) -> Result<Vec<GitHubFile>, Box<dyn std::error::Error>> {
        #[async_recursion]
        async fn get_files_recursively(
            repo: &Repository,
            branch: &str,
            path: String,
        ) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
            let mut file_list = HashSet::new();
            trace!("Fetching files from path: {}", path);

            let maybe_contents = match path.as_str() {
                "" => repo.content().root(branch).try_collect::<Vec<_>>().await,
                _ => {
                    repo.content()
                        .iter(&path, branch)
                        .try_collect::<Vec<_>>()
                        .await
                }
            };

            let contents = match maybe_contents {
                Ok(contents) => contents,
                Err(err) => {
                    panic!("Failed to fetch contents: {err}");
                    // TODO: Handle this error
                }
            };

            for content in contents {
                match content._type.as_str() {
                    "dir" => {
                        let sub_path = format!("{}/{}", path, content.name);
                        debug!("Found directory: {}", sub_path);
                        let sub_files = get_files_recursively(repo, branch, sub_path).await?;
                        file_list.extend(sub_files);
                    }
                    "file" => {
                        let file_path = format!("{}/{}", path, content.name);
                        debug!("Found file: {}", file_path);
                        file_list.insert(file_path);
                    }
                    _ => error!("Unknown content type: {}", content._type),
                }
            }

            Ok(file_list)
        }

        let repo = state.github.repo(repo.0, repo.1);
        let mut branch = branch;
        if branch == "default" {
            if repo.branches().get("master").await.is_ok() {
                branch = "master";
            }

            if repo.branches().get("main").await.is_ok() {
                branch = "main";
            }
        }

        assert!(branch != "default", "No default branch found for repo");

        let files = get_files_recursively(&repo, branch, String::new()).await?;

        debug!("Found {} files", files.len());

        let mut contents: Vec<GitHubFile> = Vec::new();

        let mut tasks = vec![];

        for file_path in files {
            let repo_arc = Arc::new(&repo);
            let file_path_clone = file_path.clone();
            let task = async move {
                if let Ok(content) =
                    Self::fetch_github_file_contents(&repo_arc, &file_path_clone, branch).await
                {
                    content
                } else {
                    panic!("Failed to fetch file: {file_path_clone}")
                }
            };
            tasks.push(task);
        }

        let results = join_all(tasks).await;

        for result in results {
            contents.push(result);
        }

        Ok(contents)
    }
}

#[async_trait]
impl Actor for GithubScraperActor {
    type Msg = GithubScraperMessage;
    type State = GithubScraperState;
    type Arguments = String;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let github = Github::new("github.com", Credentials::Token(args)).unwrap();

        Ok(GithubScraperState { github })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            GithubScraperMessage::ScrapeRepo(owner, repo, branch, port) => {
                info!("Scraping {}/{} on branch {}", owner, repo, branch);
                let path = (owner.as_str(), repo.as_str());
                let contents = Self::fetch_all_github_contents(path, &branch, state)
                    .await
                    .unwrap();
                debug!(
                    "Collected {} files from {}/{} on branch {}",
                    contents.len(),
                    owner,
                    repo,
                    branch
                );
                port.send(Ok(contents)).unwrap();
            }
            GithubScraperMessage::ScrapeOrg(_, _) => {
                unimplemented!()
            }
        }
        Ok(())
    }
}

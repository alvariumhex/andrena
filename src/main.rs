#![allow(dead_code)]
#![deny(unsafe_code)]

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use actors::{
    channel::ChannelMessage,
    channel_sup::{ChannelSupervisor, ChannelSupervisorMessage},
    communication::discord::DiscordActor,
};
use confluence::Session;
use graph::{Edge, Graph, Vertex};
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use ractor::{call, Actor, ActorRef};
use regex::Regex;
use rocket::{http::Method, serde::json::Json};
use rocket_cors::{AllowedHeaders, AllowedOrigins, CorsOptions, Method as CorsMethod};
use serenity::futures::StreamExt;
use tokio::net::TcpListener;

mod actors;
mod ai_context;
mod graph;

#[macro_use]
extern crate rocket;

#[get("/channel/<id>")]
async fn channel(id: u64) -> Json<Vec<(String, String)>> {
    let channel_registry: ActorRef<ChannelSupervisorMessage> =
        ractor::registry::where_is("channel_sup".to_owned())
            .unwrap()
            .into();

    let channel = call!(channel_registry, ChannelSupervisorMessage::FetchChannel, id).unwrap();
    let history = call!(channel, ChannelMessage::GetHistory).unwrap();

    Json(history)
}

static GRAPH: Lazy<Arc<Mutex<Graph>>> = Lazy::new(|| Arc::new(Mutex::new(Graph::new())));

#[get("/graph/vertices")]
async fn graph_nodes() -> Json<Vec<Vertex>> {
    let graph = GRAPH.lock().unwrap();
    Json(graph.vertices.clone())
}

#[get("/graph/edges")]
async fn graph_edges() -> Json<Vec<Edge>> {
    let graph = GRAPH.lock().unwrap();
    Json(graph.edges.clone())
}

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter(Some("andrena"), log::LevelFilter::Trace)
        .filter(Some("rocket"), log::LevelFilter::Trace)
        .init();

    let (_, _) = Actor::spawn(None, DiscordActor, "Lovelace".to_owned())
        .await
        .expect("Failed to spawn actor");

    let (_, _) = Actor::spawn(
        Some("typing".to_owned()),
        actors::communication::typing::TypingActor,
        (),
    )
    .await
    .expect("Failed to spawn actor");

    let (_, _) = Actor::spawn(Some(String::from("channel_sup")), ChannelSupervisor, ())
        .await
        .expect("Failed to spawn channel supervisor actor");

    // web socket listening thread
    tokio::spawn(async move {
        info!("Initializing websocket server");
        let addr = "0.0.0.0:3001";
        let server = TcpListener::bind(addr).await.unwrap();
        info!("Websocket server listening on {}", addr);
        loop {
            match server.accept().await {
                Ok((stream, _)) => {
                    debug!("Accepted connection from {:?}", stream.peer_addr());

                    if let Ok(socket) = tokio_tungstenite::accept_async(stream).await {
                        let (write, read) = socket.split();
                        Actor::spawn(
                            None,
                            actors::communication::websocket::WebSocketActor,
                            (write, read),
                        )
                        .await
                        .expect("Failed to spawn websocket actor");
                    } else {
                        error!("Failed to accept websocket connection");
                    }
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    });

    tokio::spawn(async move {
        info!("Launching rocket server");
        let cors = CorsOptions::default()
            .allowed_origins(AllowedOrigins::all())
            .allowed_methods(
                vec![Method::Get, Method::Post, Method::Patch, Method::Options]
                    .into_iter()
                    .map(|m| m.into())
                    .collect::<HashSet<CorsMethod>>(),
            )
            .allowed_headers(AllowedHeaders::all())
            .allow_credentials(true)
            .to_cors()
            .expect("Failed to build CORS");

        rocket::build()
            .mount("/", routes![channel, graph_nodes, graph_edges])
            .mount("/", rocket_cors::catch_all_options_routes())
            .attach(cors)
            .launch()
            .await
            .unwrap();
    });

    extract_confluence().await;

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c");
}

async fn extract_confluence() {
    let session = Session::new(
        "hannah.witvrouwen@external.engie.com".to_string(),
        "".to_string(),
        "https://laborelec.atlassian.net/wiki".to_string(),
    );

    let spaces = session.get_spaces().await.expect("Failed to get spaces");
    for space in spaces {
        let pages = session
            .get_pages_for_space(&space.key, None)
            .await
            .expect("Failed to get pages");

        debug!(
            "Space({:?}): {:?} with {} pages",
            space.key,
            space.name,
            pages.len()
        );

        for page in pages {
            let mut graph = GRAPH.lock().unwrap();
            let md = html2md::parse_html(&page.body.unwrap().view.unwrap().value);

            // replace relative links with absolute links
            let md = md.replace("(/wiki/", "(https://laborelec.atlassian.net/wiki/");

            let regex =
                Regex::new(r"(?m)\(https://laborelec\.atlassian\.net/wiki/.*/pages/(\d+)/?.*\)")
                    .unwrap();
            let result = regex.captures_iter(&md);
            let link = page.links.clone()._self;
            if page.children.is_some() {
                for child in &page.children.unwrap().page.results {
                    let link_to = format!(
                        "https://laborelec.atlassian.net/wiki/rest/api/content/{}",
                        child.id
                    );
                    trace!("Child Link: {:?} -> {:?}", link, link_to);
                    graph.add_edge(link.clone(), "child of".to_owned(), link_to)
                }
            }

            for cap in result {
                let id = cap.get(1).unwrap().as_str();
                let link_to = format!(
                    "https://laborelec.atlassian.net/wiki/rest/api/content/{}",
                    id
                );

                trace!("Link: {:?} -> {:?}", link, link_to);
                graph.add_edge(link.clone(), "links to".to_owned(), link_to)
            }

            let mut metadata = HashMap::new();
            metadata.insert("title".to_owned(), page.title.clone());
            metadata.insert("space".to_owned(), space.name.clone());
            metadata.insert("space_key".to_owned(), space.key.clone());
            metadata.insert("id".to_owned(), page.id.clone());
            metadata.insert("content".to_owned(), md.clone());

            if page.links.clone().webui.is_some() {
                let source = format!(
                    "https://laborelec.atlassian.net/wiki{}",
                    page.links.clone().webui.unwrap()
                );
                metadata.insert("source".to_owned(), source);
            } else {
                debug!("No source for page {:?}", page.id);
                trace!("Links: {:?}", page.links);
            }

            graph.add_or_replace_vertex(link, metadata);
        }
    }
}

#[cfg(test)]
mod test {
    use regex::Regex;

    #[ctor::ctor]
    fn init() {
        pretty_env_logger::formatted_builder()
            // .filter(Some("andrena"), log::LevelFilter::Trace)
            .init();
    }

    #[test]
    fn match_url() {
        let regex =
            Regex::new(r"(?m)\(https://laborelec\.atlassian\.net/wiki/.*/pages/(\d+)/?.*\)")
                .unwrap();
        let test = "[](https://laborelec.atlassian.net/wiki/spaces/EP/pages/110985217/Proposed+common+solution+for+public+interface+of+transverse+components)";
        let mut result = regex.captures_iter(test);
        assert_eq!(result.next().unwrap().get(1).unwrap().as_str(), "110985217")
    }
}

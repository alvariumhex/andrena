use ractor::{Message, RpcReplyPort};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub enum RemoteStoreRequestMessage {
    Retrieve(String, u8, RpcReplyPort<String>), // sends back a JSON Serialized Vec<(String, f32)>
}

impl Message for RemoteStoreRequestMessage {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub content: String,
    pub channel: u64,
    pub author: String,
    pub metadata: HashMap<String, String>,
}

impl Message for ChatMessage {}

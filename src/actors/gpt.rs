use ractor::{BytesConvertable, RpcReplyPort};
use ractor_cluster::RactorClusterMessage;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, vec};

#[derive(RactorClusterMessage)]
pub enum RemoteStoreRequestMessage {
    #[rpc]
    Retrieve(String, u8, RpcReplyPort<String>), // sends back a JSON Serialized Vec<(String, f32)>
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub content: String,
    pub channel: u64,
    pub author: String,
    pub metadata: HashMap<String, String>,
}

impl BytesConvertable for ChatMessage {
    fn into_bytes(self) -> Vec<u8> {
        bincode::serialize(&self).unwrap()
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        bincode::deserialize(&bytes).unwrap()
    }
}

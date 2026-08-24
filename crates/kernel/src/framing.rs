use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::Sha256;
use yi_types::kernel::{JupyterHeader, JupyterMessage};

pub const DELIM: &[u8] = b"<IDS|MSG>";
pub const PROTOCOL_VERSION: &str = "5.3";

pub fn build_message(
    msg_type: &str,
    content: Map<String, Value>,
    session: &str,
    username: &str,
    msg_id: String,
    date: String,
) -> JupyterMessage {
    JupyterMessage {
        header: JupyterHeader {
            msg_id,
            session: session.to_owned(),
            username: username.to_owned(),
            date,
            msg_type: msg_type.to_owned(),
            version: PROTOCOL_VERSION.to_owned(),
        },
        parent_header: Map::new(),
        metadata: Map::new(),
        content,
    }
}

pub fn sign(parts: &[Vec<u8>], key: &str) -> Vec<u8> {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key.as_bytes()) else {
        return Vec::new();
    };
    for part in parts {
        mac.update(part);
    }
    let digest = mac.finalize().into_bytes();
    hex(&digest).into_bytes()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn encode(message: &JupyterMessage, key: &str) -> Vec<Vec<u8>> {
    let parts = vec![
        serde_json::to_vec(&message.header).unwrap_or_default(),
        serde_json::to_vec(&message.parent_header).unwrap_or_default(),
        serde_json::to_vec(&message.metadata).unwrap_or_default(),
        serde_json::to_vec(&message.content).unwrap_or_default(),
    ];
    let mut frames = Vec::with_capacity(parts.len().saturating_add(2));
    frames.push(DELIM.to_vec());
    frames.push(sign(&parts, key));
    frames.extend(parts);
    frames
}

pub fn decode(frames: &[Vec<u8>]) -> Option<JupyterMessage> {
    let delim = frames.iter().position(|frame| frame == DELIM)?;
    let header = frames.get(delim.checked_add(2)?)?;
    let parent_header = frames.get(delim.checked_add(3)?)?;
    let metadata = frames.get(delim.checked_add(4)?)?;
    let content = frames.get(delim.checked_add(5)?)?;
    Some(JupyterMessage {
        header: serde_json::from_slice(header).ok()?,
        parent_header: serde_json::from_slice(parent_header).ok()?,
        metadata: serde_json::from_slice(metadata).ok()?,
        content: serde_json::from_slice(content).ok()?,
    })
}

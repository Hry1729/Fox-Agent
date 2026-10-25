//! Compare managed runtime adaptations with build-time pins. A package's
//! own manifest cannot declare an unknown patch to be trusted.
use crate::port::AgentBackend;
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn verified(root: &Path, backend: AgentBackend) -> bool {
    let prefix = match backend {
        AgentBackend::Pi => "backends/pi/",
        AgentBackend::Deepseek => "backends/dsh/",
        _ => return false,
    };
    let pins: serde_json::Value =
        serde_json::from_str(include_str!("../runtime-assets/model-adapters/pins.json"))
            .expect("build-time adapter pins");
    pins["files"]
        .as_object()
        .expect("file pins")
        .iter()
        .filter(|(key, _)| key.starts_with(prefix))
        .all(|(key, expected)| {
            let Ok(bytes) = std::fs::read(root.join(key)) else {
                return false;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                return false;
            };
            expected.as_str()
                == Some(format!("{:x}", Sha256::digest(text.replace("\r\n", "\n"))).as_str())
        })
}

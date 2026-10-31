//! Compare managed runtime adaptations with build-time pins. A package's
//! own manifest cannot declare an unknown patch to be trusted.
use crate::port::AgentBackend;
use sha2::{Digest, Sha256};
use std::path::Path;

/// The pi-acp adaptation Fox still relies on (plan C1): `fox-turn-outcome`
/// turns a model / transport failure into an ACP error instead of a silent
/// `end_turn`. The two pi-coding-agent "model proxy" patches only change
/// anything when `FOX_MODEL_PROXY_REQUIRED=1`, which only the removed SRT
/// network proxy set (plan B2); they stay pinned in the package but no
/// longer decide whether pi may start.
pub const PI_TURN_OUTCOME_ADAPTER: [&str; 2] = [
    "backends/pi/node_modules/pi-acp/dist/index.js",
    "backends/pi/node_modules/pi-acp/dist/fox-turn-outcome.mjs",
];

fn build_time_pins() -> serde_json::Map<String, serde_json::Value> {
    let pins: serde_json::Value =
        serde_json::from_str(include_str!("../runtime-assets/model-adapters/pins.json")).expect("build-time adapter pins");
    pins["files"].as_object().expect("file pins").clone()
}

fn file_matches(root: &Path, key: &str, expected: &serde_json::Value) -> bool {
    let Ok(bytes) = std::fs::read(root.join(key)) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return false;
    };
    expected.as_str() == Some(format!("{:x}", Sha256::digest(text.replace("\r\n", "\n"))).as_str())
}

/// Every pinned adaptation of `backend` under `root` matches its build-time
/// pin (the package check).
pub fn verified(root: &Path, backend: AgentBackend) -> bool {
    let prefix = match backend {
        AgentBackend::Pi => "backends/pi/",
        AgentBackend::Deepseek => "backends/dsh/",
        _ => return false,
    };
    build_time_pins().iter().filter(|(key, _)| key.starts_with(prefix)).all(|(key, expected)| file_matches(root, key, expected))
}

/// Every file of `keys` is pinned at build time and matches its pin.
pub fn verified_files(root: &Path, keys: &[&str]) -> bool {
    verified_files_with(root, keys, &build_time_pins())
}

/// [`verified_files`] against explicit pins (an unpinned key fails).
pub fn verified_files_with(root: &Path, keys: &[&str], pins: &serde_json::Map<String, serde_json::Value>) -> bool {
    !keys.is_empty() && keys.iter().all(|key| pins.get(*key).is_some_and(|expected| file_matches(root, key, expected)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_turn_outcome_adapter_is_pinned_and_checked_file_by_file() {
        let pins = build_time_pins();
        for key in PI_TURN_OUTCOME_ADAPTER {
            assert!(pins.contains_key(key), "{key} is pinned at build time");
        }
        let root = std::env::temp_dir().join(format!("fox-adapter-pins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for key in PI_TURN_OUTCOME_ADAPTER {
            std::fs::create_dir_all(root.join(key).parent().unwrap()).unwrap();
            std::fs::write(root.join(key), format!("// {key}\r\n")).unwrap();
        }
        let mut fake = serde_json::Map::new();
        for key in PI_TURN_OUTCOME_ADAPTER {
            fake.insert(key.into(), serde_json::json!(format!("{:x}", Sha256::digest(format!("// {key}\n")))));
        }
        assert!(verified_files_with(&root, &PI_TURN_OUTCOME_ADAPTER, &fake), "CRLF-normalized match");
        assert!(!verified_files(&root, &PI_TURN_OUTCOME_ADAPTER), "not the build-time content");
        assert!(!verified_files_with(&root, &[], &fake), "nothing to verify is not verified");
        let mut unpinned = fake.clone();
        unpinned.remove(PI_TURN_OUTCOME_ADAPTER[1]);
        assert!(!verified_files_with(&root, &PI_TURN_OUTCOME_ADAPTER, &unpinned));
        std::fs::write(root.join(PI_TURN_OUTCOME_ADAPTER[0]), "tampered").unwrap();
        assert!(!verified_files_with(&root, &PI_TURN_OUTCOME_ADAPTER, &fake));
        let _ = std::fs::remove_dir_all(&root);
    }
}

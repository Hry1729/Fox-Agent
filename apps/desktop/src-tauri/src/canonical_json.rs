//! Canonical JSON for the desktop Host: re-export of
//! [`fox_agent_kernel::canonical_json`] (see its docs for the why and the
//! exact byte contract).
//!
//! Since `afb0764` the desktop binary enables `fox-agent-backend/acp`, whose
//! `agent-client-protocol` dependency turns on serde_json's `preserve_order`
//! for the whole binary. Every place where JSON bytes matter (hashes, frozen
//! snapshots, tool input facts written to the database and checked by
//! triggers, idempotency / audit keys, `package_snapshot_hash`, Kernel config
//! hashes) goes through this module, never through `serde_json::to_string`,
//! `serde_json::to_vec` or `Value::to_string`
//! (`value.canonical_string()` is the drop-in for the latter).
#[allow(unused_imports)]
pub(crate) use fox_agent_kernel::canonical_json::{
    sorted, to_string, to_string_pretty, to_vec, to_vec_pretty, to_writer, to_writer_pretty,
    value_bytes, value_string, CanonicalJson,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_build_has_preserve_order_and_canonical_json_still_sorts() {
        // The condition that caused the regression is real in this binary...
        assert_eq!(
            serde_json::to_string(&json!({"b": 1, "a": 2})).unwrap(),
            r#"{"b":1,"a":2}"#,
            "expected serde_json preserve_order in the desktop build (agent-client-protocol via fox-agent-backend/acp)"
        );
        // ...and the canonical functions are immune to it.
        let value = json!({"schemaVersion": 1, "id": "durable_v2", "nested": {"z": [ {"y": 1, "x": 2} ], "a": null}});
        let expected = r#"{"id":"durable_v2","nested":{"a":null,"z":[{"x":2,"y":1}]},"schemaVersion":1}"#;
        assert_eq!(to_string(&value).unwrap(), expected);
        assert_eq!(value.canonical_string(), expected);
        assert_eq!(String::from_utf8(to_vec(&value).unwrap()).unwrap(), expected);
    }

    /// No raw serde_json serializer may come back into the Host source: every
    /// serialization whose bytes can matter goes through this module.
    #[test]
    fn host_source_has_no_raw_serde_json_serializers() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = [
            "serde_json::to_string(",
            "serde_json::to_string_pretty(",
            "serde_json::to_vec(",
            "serde_json::to_vec_pretty(",
            "serde_json::to_writer(",
            "serde_json::to_writer_pretty(",
            ".map(serde_json::to_string)",
        ];
        let mut offenders = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "bin") {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("canonical_json.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (index, line) in text.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    if forbidden.iter().any(|needle| code.contains(needle)) {
                        offenders.push(format!("{}:{}", path.display(), index + 1));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "use crate::canonical_json instead of serde_json serializers: {offenders:?}");
    }
}

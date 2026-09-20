use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const READ_ONLY_TOOLS: [&str; 4] = ["read", "grep", "find", "ls"];

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovedToolCall {
    pub tool: String,
    pub input: Value,
    pub resolved_path: PathBuf,
}

pub fn approve_read_only_tool(
    tool: &str,
    input: &Value,
    project_root: Option<&str>,
) -> Result<ApprovedToolCall, String> {
    if !READ_ONLY_TOOLS.contains(&tool) {
        return Err(format!("tool is not allowed in read-only mode: {tool}"));
    }
    let root = project_root
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "this conversation has no authorized project folder".to_owned())?;
    let canonical_root = canonical_directory(Path::new(root), "project folder")?;
    // The canonical ls/find/grep schemas allow path to be omitted. Legacy
    // Runtime supplied this default before preflight; Kernel dispatch calls
    // the guard directly, so normalize here for both execution routes.
    let fields = input.as_object().ok_or("tool input must be an object")?;
    let path = match fields.get("path") {
        None if tool != "read" => ".",
        Some(Value::String(path)) if path.trim().is_empty() && tool != "read" => ".",
        Some(Value::String(path)) if !path.trim().is_empty() => path.trim(),
        _ => return Err("tool input must contain a non-empty path".to_owned()),
    };
    if path.contains('\0') {
        return Err("tool path contains an invalid null byte".to_owned());
    }

    let requested = Path::new(path);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        canonical_root.join(requested)
    };
    let canonical_target = joined
        .canonicalize()
        .map_err(|error| format!("tool path cannot be resolved: {error}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("tool path is outside the authorized project folder".to_owned());
    }

    let mut normalized = input
        .as_object()
        .cloned()
        .ok_or_else(|| "tool input must be an object".to_owned())?;
    if tool == "read" {
        let has_line = fields.contains_key("startLine") || fields.contains_key("lineCount");
        let has_units = fields.contains_key("offset") || fields.contains_key("limit");
        if has_line && has_units {
            return Err("startLine/lineCount and offset/limit are mutually exclusive; use one mode only".to_owned());
        }
        if has_line {
            let positive_integer = |name: &str| {
                fields
                    .get(name)
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0)
                    .is_some()
            };
            if !positive_integer("startLine") || !positive_integer("lineCount") {
                return Err("startLine and lineCount must both be positive integers (startLine is 1-based)".to_owned());
            }
        }
    }
    if matches!(tool, "find" | "grep") {
        if fields.get("pattern").and_then(Value::as_str).is_none() {
            return Err("search input must contain a string pattern".to_owned());
        }
        if fields
            .get("cursor")
            .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
        {
            return Err("search cursor must be a non-empty string".to_owned());
        }
    }
    normalized.insert(
        "path".to_owned(),
        Value::String(canonical_target.to_string_lossy().into_owned()),
    );
    Ok(ApprovedToolCall {
        tool: tool.to_owned(),
        input: Value::Object(normalized),
        resolved_path: canonical_target,
    })
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("{label} cannot be resolved: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(canonical)
}

pub fn preflight_payload(
    tool: &str,
    input: &Value,
    project_root: Option<&str>,
    permission_mode: &str,
) -> (bool, Value) {
    match approve_read_only_tool(tool, input, project_root) {
        Ok(approved) => (
            true,
            Value::Object(Map::from_iter([
                ("decision".to_owned(), Value::String("allow".to_owned())),
                (
                    "permissionMode".to_owned(),
                    Value::String(permission_mode.to_owned()),
                ),
                ("tool".to_owned(), Value::String(approved.tool)),
                ("input".to_owned(), approved.input),
            ])),
        ),
        Err(message) => (
            false,
            Value::Object(Map::from_iter([
                ("decision".to_owned(), Value::String("block".to_owned())),
                ("message".to_owned(), Value::String(message)),
            ])),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn test_project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-tool-guard-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).expect("create test project");
        fs::write(root.join("src").join("note.txt"), "hello Fox").expect("write test file");
        root
    }

    #[test]
    fn allows_supported_tools_inside_the_project() {
        let root = test_project();
        let approved = approve_read_only_tool(
            "read",
            &serde_json::json!({"path": "src/note.txt"}),
            root.to_str(),
        )
        .expect("approve project file");
        assert!(approved
            .resolved_path
            .starts_with(root.canonicalize().unwrap()));
        assert!(approved.input["path"]
            .as_str()
            .unwrap()
            .ends_with("note.txt"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_unknown_tools_and_missing_projects() {
        let root = test_project();
        assert!(approve_read_only_tool(
            "bash",
            &serde_json::json!({"path": "src/note.txt"}),
            root.to_str(),
        )
        .is_err());
        assert!(approve_read_only_tool("read", &serde_json::json!({"path": "."}), None).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_paths_outside_the_project() {
        let root = test_project();
        let outside = root
            .parent()
            .unwrap()
            .join(format!("outside-{}.txt", Uuid::new_v4()));
        fs::write(&outside, "outside").expect("write outside file");
        let result =
            approve_read_only_tool("read", &serde_json::json!({"path": outside}), root.to_str());
        assert!(result.is_err());
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validates_frozen_read_ranges_and_search_cursor_shape() {
        let root = test_project();
        let root_text = root.to_str();
        assert!(approve_read_only_tool(
            "read",
            &serde_json::json!({"path":"src/note.txt","startLine":1}),
            root_text,
        ).unwrap_err().contains("positive integers"));
        assert!(approve_read_only_tool(
            "read",
            &serde_json::json!({"path":"src/note.txt","startLine":1,"lineCount":1,"offset":0}),
            root_text,
        ).unwrap_err().contains("mutually exclusive"));
        assert!(approve_read_only_tool(
            "grep",
            &serde_json::json!({"path":".","pattern":"hello","cursor":7}),
            root_text,
        ).unwrap_err().contains("cursor"));
        assert!(approve_read_only_tool(
            "find",
            &serde_json::json!({"path":".","pattern":"note","cursor":""}),
            root_text,
        ).unwrap_err().contains("cursor"));
        let _ = fs::remove_dir_all(root);
    }
}

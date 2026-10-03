//! First Office reuse trial: one in-place edit target, operation, and bounded
//! element/property set. This only names a permission; Office preparation,
//! version checks, execution admission, and commit checks remain independent.

use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfficeReuseScope {
    pub key: String,
    pub target: String,
    pub selectors: Vec<String>,
}

/// Returns `None` for every Office operation whose effects cannot be described
/// narrowly. Old `office-request` grants continue to match only the exact input
/// in their original run; only this new key can enter process-local reuse.
pub(crate) fn reusable_edit_scope(input: &Value, project_root: Option<&str>) -> Option<OfficeReuseScope> {
    if input.get("serverId")?.as_str()? != crate::office::SERVER_ID
        || input.get("tool")?.as_str()? != "office_edit"
    {
        return None;
    }
    let root = project_root?;
    let args = input.get("arguments")?.as_object()?;
    if args.len() != 4
        || !args.contains_key("file") || !args.contains_key("output")
        || !args.contains_key("overwrite") || !args.contains_key("operations")
        || args.get("overwrite")?.as_bool() != Some(true)
    {
        return None;
    }
    let file = args.get("file")?.as_str()?;
    let output = args.get("output")?.as_str()?;
    let target = crate::tool_host::canonical_file_identity(std::path::Path::new(root), file).ok()?;
    let output_identity = crate::tool_host::canonical_file_identity(std::path::Path::new(root), output).ok()?;
    if target != output_identity {
        return None;
    }
    let prepared = crate::office::prepare("office_edit", &Value::Object(args.clone()), Some(root), "ask").ok()?;
    if !prepared.mutates || prepared.target_path()?.to_string_lossy() != target {
        return None;
    }
    let operations = args.get("operations")?.as_array()?;
    if operations.is_empty() || operations.len() > 16 {
        return None;
    }
    let mut selectors = BTreeSet::new();
    for operation in operations {
        let op = operation.as_object()?;
        if op.get("command")?.as_str()? != "set" || op.keys().any(|key| !matches!(key.as_str(), "command" | "path" | "props")) {
            return None;
        }
        let path = op.get("path")?.as_str()?;
        if path.len() > 256 || !path.starts_with('/') || path.split('/').filter(|part| !part.is_empty()).count() < 2 {
            return None;
        }
        let props = op.get("props")?.as_object()?;
        if props.is_empty() || props.len() > 8 || props.keys().any(|key| !matches!(key.as_str(),
            "text" | "value" | "bold" | "italic" | "fontSize" | "fontFamily" | "color" | "backgroundColor" | "numberFormat"))
        {
            return None;
        }
        selectors.insert(format!("{path} [{}]", props.keys().cloned().collect::<Vec<_>>().join(", ")));
    }
    // Property VALUES may change inside this exact document/element/property
    // set. New paths, operation kinds, or property kinds produce a different key.
    let selectors = selectors.into_iter().collect::<Vec<_>>();
    let identity = json!({"server": crate::office::SERVER_ID, "tool": "office_edit", "target": &target, "operation": "set", "selectors": &selectors});
    Some(OfficeReuseScope {
        key: super::permission_scope_hash("office-edit-v1", identity.to_string().as_bytes()),
        target,
        selectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_same_document_element_and_property_can_reuse_a_scope() {
        let root = std::env::temp_dir().join(format!("fox-office-grant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("book.xlsx"), b"fixture").unwrap();
        std::fs::write(root.join("other.xlsx"), b"fixture").unwrap();
        let input = json!({"serverId":crate::office::SERVER_ID,"tool":"office_edit","arguments":{
            "file":"book.xlsx","output":"book.xlsx","overwrite":true,
            "operations":[{"command":"set","path":"/Sheet1/A1","props":{"value":"first"}}]}});
        let root_text = root.to_str().unwrap();
        let first = reusable_edit_scope(&input, Some(root_text)).expect("narrow in-place edit");
        let mut changed = input.clone();
        changed["arguments"]["operations"][0]["props"]["value"] = json!("second");
        assert_eq!(reusable_edit_scope(&changed, Some(root_text)).unwrap().key, first.key);
        for changed_input in [
            { let mut x = input.clone(); x["arguments"]["file"] = json!("other.xlsx"); x["arguments"]["output"] = json!("other.xlsx"); x },
            { let mut x = input.clone(); x["arguments"]["operations"][0]["path"] = json!("/Sheet1/B1"); x },
            { let mut x = input.clone(); x["arguments"]["operations"][0]["command"] = json!("remove"); x },
            { let mut x = input.clone(); x["arguments"]["operations"][0]["props"] = json!({"formula":"=1"}); x },
            { let mut x = input.clone(); x["arguments"]["output"] = json!("other.xlsx"); x },
        ] {
            assert_ne!(reusable_edit_scope(&changed_input, Some(root_text)).map(|scope| scope.key), Some(first.key.clone()));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

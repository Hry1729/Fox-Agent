use crate::database::SkillRecord;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SKILL_BYTES: u64 = 128 * 1024;
const SUPPORTED_TOOLS: &[&str] = &[
    "read",
    "ls",
    "find",
    "grep",
    "read_attachment",
    "write_file",
    "edit_file",
    "run_command",
    "web_search",
    "web_read",
    "http_request",
    "system_info",
    "sqlite_read",
    "structured_data",
    "git_read",
    "test_run",
    "code_check",
    "format_code",
    "tabular_data",
    "list_knowledge_bases",
    "search_knowledge",
    "read_knowledge_document",
    "query_knowledge_graph",
];

#[derive(Debug, Default, Deserialize)]
struct SkillMetadata {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(default)]
    required_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub record: SkillRecord,
    pub instructions: String,
}

pub fn scan_skills(root: &Path, enabled: &[String]) -> Result<Vec<LoadedSkill>, String> {
    let enabled = enabled.iter().cloned().collect::<HashSet<_>>();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut skills = Vec::new();
    let mut seen = HashMap::<String, PathBuf>::new();
    for entry in entries.flatten() {
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let path = directory.join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        let mut skill = load_skill(
            &path,
            directory.file_name().and_then(|value| value.to_str()),
        )?;
        if let Some(previous) = seen.insert(skill.record.id.clone(), path.clone()) {
            skill.record.valid = false;
            skill.record.validation_error =
                Some(format!("Skill ID 与 {} 重复", previous.to_string_lossy()));
        }
        skill.record.enabled = enabled.contains(&skill.record.id) && skill.record.valid;
        skills.push(skill);
    }
    skills.sort_by(|left, right| left.record.name.cmp(&right.record.name));
    Ok(skills)
}

pub fn enabled_skill_prompt(root: &Path, enabled: &[String]) -> Result<String, String> {
    let skills = scan_skills(root, enabled)?;
    let blocks = skills
        .into_iter()
        .filter(|skill| skill.record.enabled && skill.record.valid)
        .map(|skill| {
            format!(
                "## Skill: {} ({})\n{}",
                skill.record.name, skill.record.id, skill.instructions
            )
        })
        .collect::<Vec<_>>();
    Ok(blocks.join("\n\n"))
}

fn load_skill(path: &Path, directory_id: Option<&str>) -> Result<LoadedSkill, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let source_path = path.to_string_lossy().into_owned();
    if metadata.len() > MAX_SKILL_BYTES {
        return Ok(invalid_skill(
            directory_id.unwrap_or("invalid"),
            &source_path,
            "SKILL.md 不能超过 128 KB",
        ));
    }
    let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (frontmatter, instructions) = split_frontmatter(&source);
    let metadata = frontmatter
        .map(serde_yaml::from_str::<SkillMetadata>)
        .transpose()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .unwrap_or_default();
    let id = metadata
        .id
        .as_deref()
        .or(directory_id)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let name = metadata.name.unwrap_or_else(|| id.clone());
    let description = metadata.description.unwrap_or_default();
    let version = metadata.version.unwrap_or_else(|| "0.1.0".to_owned());
    let mut validation_errors = Vec::new();
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        validation_errors.push("Skill ID 只能包含字母、数字、- 和 _".to_owned());
    }
    if name.trim().is_empty() {
        validation_errors.push("Skill 名称不能为空".to_owned());
    }
    if instructions.trim().is_empty() {
        validation_errors.push("Skill 指令正文不能为空".to_owned());
    }
    for tool in &metadata.required_tools {
        if !SUPPORTED_TOOLS.contains(&tool.as_str()) {
            validation_errors.push(format!("未知工具: {tool}"));
        }
    }
    let valid = validation_errors.is_empty();
    Ok(LoadedSkill {
        record: SkillRecord {
            id,
            name,
            description,
            version,
            required_tools: metadata.required_tools,
            source_path,
            enabled: false,
            valid,
            validation_error: (!valid).then(|| validation_errors.join("；")),
            instructions: instructions.trim().to_owned(),
        },
        instructions: instructions.trim().to_owned(),
    })
}

fn invalid_skill(id: &str, source_path: &str, error: &str) -> LoadedSkill {
    LoadedSkill {
        record: SkillRecord {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            version: "unknown".to_owned(),
            required_tools: Vec::new(),
            source_path: source_path.to_owned(),
            enabled: false,
            valid: false,
            validation_error: Some(error.to_owned()),
            instructions: String::new(),
        },
        instructions: String::new(),
    }
}

fn split_frontmatter(source: &str) -> (Option<&str>, &str) {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    if !source.starts_with("---\n") && !source.starts_with("---\r\n") {
        return (None, source);
    }
    let body_start = source
        .find('\n')
        .map(|index| index + 1)
        .unwrap_or(source.len());
    let rest = &source[body_start..];
    for marker in ["\n---\n", "\r\n---\r\n"] {
        if let Some(index) = rest.find(marker) {
            let end = index;
            let content_start = index + marker.len();
            return (Some(&rest[..end]), &rest[content_start..]);
        }
    }
    (None, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates_a_skill_package() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        let directory = root.join("research");
        std::fs::create_dir_all(&directory).expect("create skill directory");
        std::fs::write(
            directory.join("SKILL.md"),
            "---\nid: research\nname: Research\nversion: 1.0.0\nrequired_tools: [read, grep]\n---\nUse evidence before answering.",
        )
        .expect("write skill");
        let skills = scan_skills(&root, &["research".to_owned()]).expect("scan skills");
        assert_eq!(skills.len(), 1);
        assert!(skills[0].record.valid);
        assert!(skills[0].record.enabled);
        assert!(enabled_skill_prompt(&root, &["research".to_owned()])
            .unwrap()
            .contains("Use evidence"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_skills_that_request_unknown_tools() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        let directory = root.join("unsafe");
        std::fs::create_dir_all(&directory).expect("create skill directory");
        std::fs::write(
            directory.join("SKILL.md"),
            "---\nname: Unsafe\nrequired_tools: [install_everything]\n---\nDo work.",
        )
        .expect("write skill");
        let skills = scan_skills(&root, &[]).expect("scan skills");
        assert!(!skills[0].record.valid);
        assert!(skills[0]
            .record
            .validation_error
            .as_deref()
            .unwrap_or_default()
            .contains("未知工具"));
        let _ = std::fs::remove_dir_all(root);
    }
}

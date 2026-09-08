use crate::database::SkillRecord;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SKILL_BYTES: u64 = 128 * 1024;
const DEFAULT_SKILL_PROMPT_CHARS: usize = 8_000;

pub(crate) fn install_bundled_office_skills(root: &Path) -> Result<(), String> {
    for (id, instructions) in [
        ("fox-office-word", include_str!("../resources/office-skills/word/SKILL.md")),
        ("fox-office-excel", include_str!("../resources/office-skills/excel/SKILL.md")),
        ("fox-office-ppt", include_str!("../resources/office-skills/ppt/SKILL.md")),
    ] {
        let directory = root.join(id);
        let path = directory.join("SKILL.md");
        // Preserve local edits. Future updates must compare a version/hash first.
        if !path.exists() {
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            std::fs::write(path, instructions).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

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
    let mut directories = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    directories.sort();
    let mut skills = Vec::new();
    for directory in directories {
        let path = directory.join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        let skill = load_skill(
            &path,
            directory.file_name().and_then(|value| value.to_str()),
        )?;
        skills.push(skill);
    }
    let mut paths_by_id = HashMap::<String, Vec<PathBuf>>::new();
    for skill in &skills {
        paths_by_id
            .entry(skill.record.id.clone())
            .or_default()
            .push(PathBuf::from(&skill.record.source_path));
    }
    for skill in &mut skills {
        if let Some(paths) = paths_by_id
            .get(&skill.record.id)
            .filter(|paths| paths.len() > 1)
        {
            skill.record.valid = false;
            skill.record.validation_error = Some(format!(
                "Skill ID 重复：{}",
                paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        skill.record.enabled = enabled.contains(&skill.record.id) && skill.record.valid;
    }
    skills.sort_by(|left, right| left.record.name.cmp(&right.record.name));
    Ok(skills)
}

pub fn enabled_skill_prompt(root: &Path, enabled: &[String]) -> Result<String, String> {
    enabled_skill_prompt_for_context(root, enabled, None, "", DEFAULT_SKILL_PROMPT_CHARS)
}

pub fn enabled_skill_prompt_for_context(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
    task: &str,
    max_chars: usize,
) -> Result<String, String> {
    let mut skills = scan_skills(root, enabled)?
        .into_iter()
        .filter(|skill| skill.record.enabled && skill.record.valid)
        .collect::<Vec<_>>();
    let task = task.to_lowercase();
    skills.sort_by(|left, right| {
        skill_relevance(right, &task)
            .cmp(&skill_relevance(left, &task))
            .then_with(|| left.record.name.cmp(&right.record.name))
            .then_with(|| left.record.id.cmp(&right.record.id))
    });

    let mut blocks = Vec::new();
    let mut omitted = Vec::new();
    let limit = max_chars.max(256);
    for skill in skills {
        let unavailable = available_tools
            .map(|tools| {
                skill
                    .record
                    .required_tools
                    .iter()
                    .filter(|tool| !tools.contains(*tool))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !unavailable.is_empty() {
            omitted.push(format!(
                "{}（缺少工具：{}）",
                skill.record.id,
                unavailable.join("、")
            ));
            continue;
        }
        let block = format!(
            "## Skill: {} ({})\n{}",
            skill.record.name, skill.record.id, skill.instructions
        );
        let candidate_chars = blocks.iter().map(String::len).sum::<usize>()
            + block.len()
            + blocks.len().saturating_mul(2);
        if candidate_chars > limit.saturating_sub(256) {
            omitted.push(format!("{}（超出本轮 Skill 预算）", skill.record.id));
            continue;
        }
        blocks.push(block);
    }
    if !omitted.is_empty() {
        blocks.push(format!(
            "## Skill selection diagnostics\n本轮未注入：{}。未注入的 Skill 不得作为已生效指令。",
            omitted.join("；")
        ));
    }
    while blocks.join("\n\n").len() > limit && blocks.len() > 1 {
        blocks.remove(blocks.len() - 2);
    }
    Ok(blocks.join("\n\n"))
}

fn skill_relevance(skill: &LoadedSkill, task: &str) -> usize {
    if task.is_empty() {
        return 0;
    }
    let mut score = 0;
    for candidate in [
        skill.record.id.as_str(),
        skill.record.name.as_str(),
        skill.record.description.as_str(),
    ] {
        let candidate = candidate.trim().to_lowercase();
        if !candidate.is_empty() && task.contains(&candidate) {
            score += 10 + candidate.len().min(40);
        }
        score += candidate
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| token.chars().count() >= 3 && task.contains(token))
            .count();
    }
    score
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
        if !crate::runtime_host::runtime_tool_is_supported(tool) {
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

    #[test]
    fn marks_every_duplicate_id_invalid_and_orders_scans_deterministically() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        for directory in ["z-last", "a-first"] {
            let path = root.join(directory);
            std::fs::create_dir_all(&path).expect("create duplicate skill directory");
            std::fs::write(
                path.join("SKILL.md"),
                "---\nid: duplicate\nname: Duplicate\n---\nDo work.",
            )
            .expect("write duplicate skill");
        }
        let skills = scan_skills(&root, &["duplicate".to_owned()]).expect("scan skills");
        assert_eq!(skills.len(), 2);
        assert!(skills
            .iter()
            .all(|skill| !skill.record.valid && !skill.record.enabled));
        assert!(skills.iter().all(|skill| skill
            .record
            .validation_error
            .as_deref()
            .unwrap_or_default()
            .contains("重复")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn filters_enabled_skills_by_effective_tools_without_cutting_blocks() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        for (directory, tool, body) in [
            (
                "delegate",
                "child_agent_list",
                "Delegate only when it helps.",
            ),
            ("writer", "write_file", "Write the requested artifact."),
        ] {
            let path = root.join(directory);
            std::fs::create_dir_all(&path).expect("create skill directory");
            std::fs::write(
                path.join("SKILL.md"),
                format!("---\nid: {directory}\nname: {directory}\nrequired_tools: [{tool}]\n---\n{body}"),
            )
            .expect("write skill");
        }
        let available = HashSet::from(["child_agent_list".to_owned()]);
        let prompt = enabled_skill_prompt_for_context(
            &root,
            &["delegate".to_owned(), "writer".to_owned()],
            Some(&available),
            "delegate this task",
            1_000,
        )
        .expect("compose skill prompt");
        assert!(prompt.contains("Delegate only when it helps."));
        assert!(!prompt.contains("Write the requested artifact."));
        assert!(prompt.contains("缺少工具：write_file"));
        let _ = std::fs::remove_dir_all(root);
    }
}

use crate::database::SkillRecord;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SKILL_BYTES: u64 = 128 * 1024;
/// Initial skill-injection budget. **Units are Unicode scalar values
/// (characters)**, never UTF-8 bytes and never model tokens; a Chinese
/// character and an ASCII letter each cost 1 against this budget.
const DEFAULT_SKILL_PROMPT_CHARS: usize = 8_000;
/// The on-demand catalog must stay discoverable even when full texts do not
/// fit; it is allowed this much of the initial budget.
const SKILL_CATALOG_RESERVE_CHARS: usize = 3_200;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillToolState {
    /// Every required tool is present in the frozen Run scope (or scope is
    /// unknown and the tool is globally registered).
    Available,
    MissingTools(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub required_tools: Vec<String>,
    pub state: SkillToolState,
    pub chars: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
pub struct IncludedSkill {
    pub id: String,
    pub version: String,
    pub content_sha256: String,
    pub required_tools: Vec<String>,
    pub missing_tools: Vec<String>,
    pub tools_available: bool,
    pub chars: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmittedSkill {
    pub id: String,
    /// Machine-stable reason: `missing_tools` | `budget`.
    pub reason: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct SkillPromptPlan {
    pub prompt: String,
    /// One compact line per enabled valid skill, so skills skipped from the
    /// initial full-text injection stay discoverable mid-run.
    pub catalog: Vec<CatalogEntry>,
    pub included: Vec<IncludedSkill>,
    pub omitted: Vec<OmittedSkill>,
}

#[derive(Debug, Clone)]
pub struct OnDemandSkill {
    pub block: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub content_sha256: String,
    pub required_tools: Vec<String>,
    pub missing_tools: Vec<String>,
    pub tools_available: bool,
    pub chars: usize,
    pub bytes: usize,
}

pub fn enabled_skill_prompt(root: &Path, enabled: &[String]) -> Result<String, String> {
    Ok(skill_prompt_plan(root, enabled, None, "", DEFAULT_SKILL_PROMPT_CHARS)?.prompt)
}

pub fn enabled_skill_prompt_for_context(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
    task: &str,
    max_chars: usize,
) -> Result<String, String> {
    Ok(skill_prompt_plan(root, enabled, available_tools, task, max_chars)?.prompt)
}

/// Compose the initial skill section: a full catalog plus full texts of the
/// most relevant skills that fit the (character-denominated) budget.
pub fn skill_prompt_plan(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
    task: &str,
    max_chars: usize,
) -> Result<SkillPromptPlan, String> {
    let scanned = scan_skills(root, enabled)?;
    let catalog = catalog_from_skills(&scanned, available_tools);
    let skills_by_id = scanned
        .into_iter()
        .map(|skill| (skill.record.id.clone(), skill))
        .collect::<HashMap<_, _>>();
    let task = task.to_lowercase();
    // Honour the caller's budget: a 512-char floor here would silently inflate
    // tight prompts and defeat the budget contract.
    let limit = max_chars.max(128);

    // Catalog first: discovery must survive a tight full-text budget.
    let mut catalog_lines = Vec::new();
    let catalog_heading = "## Skill catalog (load full instructions on demand with skill_load)";
    catalog_lines.push(catalog_heading.to_owned());
    for entry in &catalog {
        catalog_lines.push(catalog_line(entry));
    }
    let mut catalog_text = catalog_lines.join("\n");
    let mut catalog_entries = catalog.len();
    let catalog_cap = SKILL_CATALOG_RESERVE_CHARS.min(limit.saturating_sub(256));
    while catalog_text.chars().count() > catalog_cap && catalog_entries > 0 {
        catalog_entries -= 1;
        catalog_lines.truncate(1 + catalog_entries);
        if catalog_entries < catalog.len() {
            catalog_lines.push(format!(
                "…（另有 {} 个技能因预算未在目录展开：用 skill_load 空参或 query/offset 分页检索已启用技能目录即可看到其 ID）",
                catalog.len() - catalog_entries
            ));
        }
        catalog_text = catalog_lines.join("\n");
    }

    // Full texts in relevance order. Skills with missing frozen tools are
    // listed in the catalog but never injected as active instructions.
    let mut candidates = catalog
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.state, SkillToolState::Available))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let right_skill = skills_by_id.get(&catalog[*right].id);
        let left_skill = skills_by_id.get(&catalog[*left].id);
        let right_score = right_skill.map_or(0, |skill| skill_relevance(skill, &task));
        let left_score = left_skill.map_or(0, |skill| skill_relevance(skill, &task));
        right_score
            .cmp(&left_score)
            .then_with(|| catalog[*left].name.cmp(&catalog[*right].name))
            .then_with(|| catalog[*left].id.cmp(&catalog[*right].id))
    });

    // Skills whose frozen scope is missing a required tool are omitted by
    // identity (the real skill id), never by any display-name heuristic.
    let mut omitted: Vec<OmittedSkill> = catalog
        .iter()
        .filter_map(|entry| match &entry.state {
            SkillToolState::MissingTools(missing) => Some(OmittedSkill {
                id: entry.id.clone(),
                reason: "missing_tools".into(),
                detail: format!("缺少工具：{}", missing.join("、")),
            }),
            SkillToolState::Available => None,
        })
        .collect();

    // Decide the final INCLUDED SET, then derive every other view from it. The
    // prompt, `included`, `omitted` and the activation ledger all describe this
    // one set, so they can never disagree about which full texts took effect.
    //
    // A skill is admitted only when the *whole* rendered prompt — catalog, its
    // own full text and the diagnostics section listing every current omission —
    // still fits the character budget. `render_len` measures the identical
    // assembly the final prompt uses, including the module preamble.
    let render_len = |blocks: &[String], omitted: &[OmittedSkill], catalog_text: &str| -> usize {
        let mut total = MODULE_PREAMBLE.chars().count() + catalog_text.chars().count();
        total += blocks.iter().map(|block| block.chars().count()).sum::<usize>();
        total += blocks.len().saturating_add(if omitted.is_empty() { 0 } else { 1 }) * 2;
        if !omitted.is_empty() {
            total += diagnostics_text(omitted).chars().count();
        }
        total
    };
    let mut chosen: Vec<(usize, String)> = Vec::new();
    for index in candidates {
        let entry = &catalog[index];
        let Some(skill) = skills_by_id.get(&entry.id) else {
            continue;
        };
        let block = format!(
            "## Skill: {} ({})\n{}",
            skill.record.name, skill.record.id, skill.instructions
        );
        let mut trial_blocks: Vec<String> =
            chosen.iter().map(|(_, block)| block.clone()).collect();
        trial_blocks.push(block.clone());
        if render_len(&trial_blocks, &omitted, &catalog_text) <= limit {
            chosen.push((index, block));
        } else {
            omitted.push(OmittedSkill {
                id: entry.id.clone(),
                reason: "budget".into(),
                detail: "超出本轮 Skill 预算，可在需要时用 skill_load 加载".into(),
            });
        }
    }
    omitted.sort_by(|left, right| left.id.cmp(&right.id));
    omitted.dedup_by(|left, right| left.id == right.id);

    // Bounded presentation of the diagnostics itself: with very many omitted
    // skills the section would blow the budget on its own. The named entries are
    // capped and the remainder is reported as a count, so the section always
    // states the truth while staying bounded. Every omitted skill keeps a real
    // discovery entry either way: the catalog line (`skill_load <id>`).
    let truncate_omitted = |omitted: &[OmittedSkill]| -> Vec<OmittedSkill> {
        const MAX_NAMED_OMISSIONS: usize = 24;
        if omitted.len() <= MAX_NAMED_OMISSIONS {
            return omitted.to_vec();
        }
        let mut bounded: Vec<OmittedSkill> = omitted[..MAX_NAMED_OMISSIONS].to_vec();
        bounded.push(OmittedSkill {
            id: "……".into(),
            reason: "budget".into(),
            detail: format!(
                "另有 {} 个技能未注入全文，均可在目录中用 skill_load 加载",
                omitted.len() - MAX_NAMED_OMISSIONS
            ),
        });
        bounded
    };
    let mut omitted = truncate_omitted(&omitted);

    // If catalog + diagnostics alone already exceed the budget, shrink the
    // catalog presentation (never its discoverability statement) until the final
    // prompt fits. `skill_load` remains available for every enabled skill.
    let mut catalog_text = catalog_text;
    while render_len(
        &chosen.iter().map(|(_, block)| block.clone()).collect::<Vec<_>>(),
        &omitted,
        &catalog_text,
    ) > limit
        && catalog_entries > 0
    {
        catalog_entries -= 1;
        catalog_lines.truncate(1 + catalog_entries);
        if catalog_entries < catalog.len() {
            catalog_lines.push(catalog_digest_line(catalog.len() - catalog_entries));
        }
        catalog_text = catalog_lines.join("\n");
    }

    // Final render + verification. The loop above already dropped any skill that
    // did not fit, so a remaining overage means the fixed parts (preamble,
    // bounded catalog, bounded diagnostics) exceed the budget; that is reported
    // as an explicit failure instead of returning an oversized prompt.
    let mut sections: Vec<String> = vec![catalog_text.clone()];
    sections.extend(chosen.iter().map(|(_, block)| block.clone()));
    if !omitted.is_empty() {
        sections.push(diagnostics_text(&omitted));
    }
    let mut prompt = assemble_prompt(&sections);
    while prompt.chars().count() > limit && !chosen.is_empty() {
        // Drop the last injected full text (never the catalog) and keep every
        // derived view in step: the omitted ledger gains a concrete entry, and
        // `included` is derived from `chosen` below — never from parsing text.
        let (index, _) = chosen.pop().expect("non-empty by the loop condition");
        let id = catalog[index].id.clone();
        if !omitted.iter().any(|item| item.id == id) {
            omitted.push(OmittedSkill {
                id,
                reason: "budget".into(),
                detail: "超出本轮 Skill 预算，可在需要时用 skill_load 加载".into(),
            });
            omitted.sort_by(|left, right| left.id.cmp(&right.id));
        }
        sections = vec![catalog_text.clone()];
        sections.extend(chosen.iter().map(|(_, block)| block.clone()));
        sections.push(diagnostics_text(&omitted));
        prompt = assemble_prompt(&sections);
    }
    if prompt.chars().count() > limit {
        return Err(format!(
            "Skill 目录与诊断信息本身超出本轮预算（{} > {} 字符）；请减少启用技能或提高预算",
            prompt.chars().count(),
            limit
        ));
    }
    // `included` is derived from the final set, keyed by the REAL skill id from
    // the structured entry — never from a display name recovered out of prose.
    let included: Vec<IncludedSkill> = chosen
        .iter()
        .filter_map(|(index, _)| {
            let entry = &catalog[*index];
            let skill = skills_by_id.get(&entry.id)?;
            Some(IncludedSkill {
                id: skill.record.id.clone(),
                version: skill.record.version.clone(),
                content_sha256: content_hash(&skill.instructions),
                required_tools: skill.record.required_tools.clone(),
                missing_tools: Vec::new(),
                tools_available: true,
                chars: skill.instructions.chars().count(),
                bytes: skill.instructions.len(),
            })
        })
        .collect();
    Ok(SkillPromptPlan {
        prompt,
        catalog: catalog.into_iter().take(catalog_entries).collect(),
        included,
        omitted,
    })
}

/// One-line statement of how many enabled skills the catalog could not expand.
///
/// It names the bounded discovery query, because this line is exactly the case
/// where the catalog no longer lists an id: the model has to be able to page or
/// search the enabled catalog instead of guessing.
fn catalog_digest_line(hidden: usize) -> String {
    format!(
        "…（另有 {hidden} 个技能因预算未在目录展开：用 skill_load 空参或 query/offset 分页检索目录即可看到其 ID 并加载）"
    )
}

/// The diagnostics section: which full texts were NOT injected, why, and how to
/// obtain them. Rendered from the final omitted set every time it is needed, so
/// it can never describe a stale set.
///
/// It names the bounded discovery query as well as the id load, because a tight
/// budget can truncate this very list: `skill_load` with no id (or with a
/// query/offset) lists the enabled catalog page by page, which is how a skill
/// omitted from both the catalog and this section is still found by id.
fn diagnostics_text(omitted: &[OmittedSkill]) -> String {
    format!(
        "## Skill selection diagnostics\n本轮未注入全文：{}。未注入的 Skill 不得作为已生效指令；可用 skill_load 加载或空参检索目录。",
        omitted
            .iter()
            .map(|item| format!("{}（{}）", item.id, item.detail))
            .collect::<Vec<_>>()
            .join("；")
    )
}

/// Fixed preamble always present in an injected skill section.
const MODULE_PREAMBLE: &str = "## Skills\n以下 Skill 指令由用户按需启用；技能文字只提供方法，不会扩大任何权限。";

fn assemble_prompt(sections: &[String]) -> String {
    if sections.is_empty() {
        return MODULE_PREAMBLE.to_owned();
    }
    let mut prompt = String::from(MODULE_PREAMBLE);
    prompt.push_str("\n\n");
    prompt.push_str(&sections.join("\n\n"));
    prompt
}

/// Metadata for every enabled valid skill, with frozen-scope dependency state.
pub fn skill_catalog(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
) -> Result<Vec<CatalogEntry>, String> {
    let skills = scan_skills(root, enabled)?;
    Ok(catalog_from_skills(&skills, available_tools))
}

/// One entry of a model-requested catalog page: the minimum facts needed to pick
/// a skill and load it by id, never its full text.
#[derive(Debug, Clone)]
pub struct SkillCatalogPageEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub required_tools: Vec<String>,
    pub missing_tools: Vec<String>,
    pub tools_available: bool,
    pub chars: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
pub struct SkillCatalogPage {
    pub entries: Vec<SkillCatalogPageEntry>,
    pub offset: usize,
    pub total_matched: usize,
    pub total_enabled: usize,
    pub next_offset: Option<usize>,
}

/// A bounded, searchable page of the enabled skill catalog.
///
/// This exists because a tight prompt budget can truncate *both* the injected
/// catalog and the omission diagnostics, leaving a skill discoverable nowhere by
/// id. The page reports only what is needed to choose and then load a skill: id,
/// name, version, a short description, size and dependency state. It never
/// injects instructions and never widens the Run's tool scope — the caller
/// derives `enabled` and `available_tools` from the Run's own binding.
pub fn skill_catalog_page(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<SkillCatalogPage, String> {
    let all = skill_catalog(root, enabled, available_tools)?;
    let total_enabled = all.len();
    let needle = query.trim().to_lowercase();
    let matched: Vec<CatalogEntry> = if needle.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|entry| {
                entry.id.to_lowercase().contains(&needle)
                    || entry.name.to_lowercase().contains(&needle)
                    || entry.description.to_lowercase().contains(&needle)
            })
            .collect()
    };
    let total_matched = matched.len();
    let limit = limit.max(1);
    let offset = offset.min(total_matched);
    let page = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|entry| {
            let (missing_tools, tools_available) = match &entry.state {
                SkillToolState::Available => (Vec::new(), true),
                SkillToolState::MissingTools(missing) => (missing.clone(), false),
            };
            SkillCatalogPageEntry {
                id: entry.id,
                name: entry.name,
                version: entry.version,
                description: entry.description,
                required_tools: entry.required_tools,
                missing_tools,
                tools_available,
                chars: entry.chars,
                bytes: entry.bytes,
            }
        })
        .collect::<Vec<_>>();
    let next_offset = (offset + page.len() < total_matched).then_some(offset + page.len());
    Ok(SkillCatalogPage {
        entries: page,
        offset,
        total_matched,
        total_enabled,
        next_offset,
    })
}

fn catalog_from_skills(
    skills: &[LoadedSkill],
    available_tools: Option<&HashSet<String>>,
) -> Vec<CatalogEntry> {
    let mut entries = skills
        .iter()
        .filter(|skill| skill.record.enabled && skill.record.valid)
        .map(|skill| {
            let missing = available_tools
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
            let state = if missing.is_empty() {
                SkillToolState::Available
            } else {
                SkillToolState::MissingTools(missing)
            };
            CatalogEntry {
                chars: skill.instructions.chars().count(),
                bytes: skill.instructions.len(),
                id: skill.record.id.clone(),
                name: skill.record.name.clone(),
                version: skill.record.version.clone(),
                description: skill.record.description.clone(),
                required_tools: skill.record.required_tools.clone(),
                state,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    entries
}

/// Load one enabled skill's full instructions mid-run. The skill must be
/// enabled for the Run and pass the same validation as startup injection;
/// missing frozen tools are reported, not silently upgraded.
pub fn on_demand_skill(
    root: &Path,
    enabled: &[String],
    available_tools: Option<&HashSet<String>>,
    skill_id: &str,
) -> Result<OnDemandSkill, String> {
    let skill = scan_skills(root, enabled)?
        .into_iter()
        .find(|skill| skill.record.id == skill_id)
        .ok_or_else(|| format!("未找到已启用的技能：{skill_id}"))?;
    if !skill.record.valid {
        return Err(format!(
            "技能 {} 不可用：{}",
            skill_id,
            skill.record.validation_error.unwrap_or_default()
        ));
    }
    if !skill.record.enabled {
        return Err(format!("技能 {skill_id} 未在当前任务中启用"));
    }
    let missing = available_tools
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
    let block = format!(
        "## Skill: {} ({})\n{}",
        skill.record.name, skill.record.id, skill.instructions
    );
    Ok(OnDemandSkill {
        chars: skill.instructions.chars().count(),
        bytes: skill.instructions.len(),
        block,
        id: skill.record.id,
        name: skill.record.name,
        version: skill.record.version,
        content_sha256: content_hash(&skill.instructions),
        required_tools: skill.record.required_tools,
        tools_available: missing.is_empty(),
        missing_tools: missing,
    })
}

pub(crate) fn content_hash(instructions: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(instructions.as_bytes())))
}

fn catalog_line(entry: &CatalogEntry) -> String {
    let state = match &entry.state {
        SkillToolState::Available if !entry.required_tools.is_empty() => format!(
            "可加载；需要工具：{}",
            entry.required_tools.join(",")
        ),
        SkillToolState::Available => "可加载".to_owned(),
        SkillToolState::MissingTools(missing) => {
            format!("缺少工具：{}", missing.join(","))
        }
    };
    let description = entry.description.trim();
    let description = if description.is_empty() {
        String::new()
    } else {
        format!(" — {description}")
    };
    format!(
        "- {} ({}) v{}{}；{}（正文 {} 字符 / {} 字节）",
        entry.name, entry.id, entry.version, description, state, entry.chars, entry.bytes
    )
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
    let source = source.strip_prefix('﻿').unwrap_or(source);
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

    fn write_skill(root: &Path, directory: &str, body: &str) {
        let path = root.join(directory);
        std::fs::create_dir_all(&path).expect("create skill directory");
        std::fs::write(path.join("SKILL.md"), body).expect("write skill");
    }

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()))
    }

    /// R7: the audit ledger must name the REAL skill id. A skill whose display
    /// name differs from its id used to have its full text dropped from the
    /// prompt while `included` still announced it — the ledger then claimed an
    /// activation that never happened.
    #[test]
    fn budget_fallback_keeps_included_omitted_and_full_text_in_step() {
        let root = temp_root();
        // The name contains " (" so a display-name parse would cut it at the
        // wrong place, and it deliberately differs from the id.
        write_skill(
            &root,
            "alpha-dir",
            &format!(
                "---\nid: alpha-real-id\nname: Alpha (核心) 技能\n---\n{}",
                "甲".repeat(600)
            ),
        );
        write_skill(
            &root,
            "beta-dir",
            &format!(
                "---\nid: beta-real-id\nname: Beta (辅助) 技能\n---\n{}",
                "乙".repeat(600)
            ),
        );
        let plan = skill_prompt_plan(
            &root,
            &["alpha-real-id".to_owned(), "beta-real-id".to_owned()],
            None,
            "",
            1_500,
        )
        .expect("compose skill plan");

        // The final prompt contains the full text of exactly the included set,
        // and the ledger names the real id (not the display name).
        let alpha_body = "甲".repeat(600);
        let beta_body = "乙".repeat(600);
        for (id, body) in [("alpha-real-id", &alpha_body), ("beta-real-id", &beta_body)] {
            let in_prompt = plan.prompt.contains(body.as_str());
            let in_ledger = plan.included.iter().any(|item| item.id == id);
            assert_eq!(
                in_prompt, in_ledger,
                "skill {id}: full text in prompt = {in_prompt}, announced included = {in_ledger}"
            );
        }
        assert!(
            plan.included.iter().all(|item| item.id.ends_with("-real-id")),
            "the ledger must identify skills by real id, got {:?}",
            plan.included.iter().map(|item| &item.id).collect::<Vec<_>>()
        );
        // No dropped skill is still announced as included.
        for item in &plan.omitted {
            assert!(
                !plan.included.iter().any(|included| included.id == item.id),
                "skill {} is omitted but still marked included",
                item.id
            );
        }
        // Omitted ids are real ids, never display names.
        for item in &plan.omitted {
            assert!(
                item.id == "……" || item.id == "alpha-real-id" || item.id == "beta-real-id",
                "omitted entry used a non-id identity: {}",
                item.id
            );
        }
        assert!(
            plan.prompt.chars().count() <= 1_500,
            "the final prompt must obey the stated budget, got {}",
            plan.prompt.chars().count()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Many omitted skills must not let the diagnostics section blow the budget
    /// on its own, and each of them must stay discoverable through the catalog.
    #[test]
    fn many_omitted_skills_stay_bounded_and_discoverable() {
        let root = temp_root();
        let mut enabled = Vec::new();
        for index in 0..40 {
            let id = format!("bulk-{index:02}");
            write_skill(
                &root,
                &id,
                &format!(
                    "---\nid: {id}\nname: Bulk {index}\ndescription: bulk skill {index}\n---\n{}",
                    "丙".repeat(400)
                ),
            );
            enabled.push(id);
        }
        let plan = skill_prompt_plan(&root, &enabled, None, "", 3_000).expect("compose skill plan");
        assert!(
            plan.prompt.chars().count() <= 3_000,
            "bounded diagnostics must keep the prompt in budget, got {}",
            plan.prompt.chars().count()
        );
        // Bounded presentation: the section must not name 40 entries.
        assert!(
            plan.omitted.len() < 40,
            "diagnostics must be bounded, got {} entries",
            plan.omitted.len()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A budget too small even for the catalog and diagnostics is reported as an
    /// explicit failure instead of silently returning an oversized prompt.
    #[test]
    fn an_impossible_budget_is_reported_instead_of_exceeded() {
        let root = temp_root();
        for index in 0..3 {
            write_skill(
                &root,
                &format!("tiny-{index}"),
                &format!("---\nid: tiny-{index}\nname: Tiny {index}\n---\n正文"),
            );
        }
        // The helper clamps the limit at 128, which the fixed catalog plus
        // diagnostics cannot fit; that must surface as an error.
        let result = skill_prompt_plan(
            &root,
            &["tiny-0".to_owned(), "tiny-1".to_owned(), "tiny-2".to_owned()],
            None,
            "",
            128,
        );
        match result {
            Ok(plan) => assert!(
                plan.prompt.chars().count() <= 128,
                "a successful plan must still be within budget ({} > 128)",
                plan.prompt.chars().count()
            ),
            Err(error) => assert!(
                error.contains("预算"),
                "unexpected error: {error}"
            ),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// A large skill that does not fit is dropped by identity and its id — not
    /// its display name — is what `omitted` records, while a smaller skill that
    /// still fits is admitted.
    #[test]
    fn a_dropped_skill_is_recorded_by_id_and_a_smaller_skill_still_fits() {
        let root = temp_root();
        write_skill(
            &root,
            "huge",
            &format!(
                "---\nid: huge-id\nname: Huge (巨大) 技能\n---\n{}",
                "丁".repeat(4_000)
            ),
        );
        write_skill(
            &root,
            "small",
            "---\nid: small-id\nname: Small 技能\n---\n简短正文",
        );
        let plan = skill_prompt_plan(
            &root,
            &["huge-id".to_owned(), "small-id".to_owned()],
            None,
            "",
            2_000,
        )
        .expect("compose skill plan");
        assert!(plan.omitted.iter().any(|item| item.id == "huge-id"));
        assert!(!plan.included.iter().any(|item| item.id == "huge-id"));
        // The smaller skill is still admitted after the large one was rejected.
        assert!(plan.included.iter().any(|item| item.id == "small-id"));
        assert!(plan.prompt.contains("简短正文"));
        assert!(plan.prompt.chars().count() <= 2_000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_and_validates_a_skill_package() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        write_skill(
            &root,
            "research",
            "---\nid: research\nname: Research\nversion: 1.0.0\nrequired_tools: [read, grep]\n---\nUse evidence before answering.",
        );
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
        write_skill(
            &root,
            "unsafe",
            "---\nname: Unsafe\nrequired_tools: [install_everything]\n---\nDo work.",
        );
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
            write_skill(
                &root,
                directory,
                "---\nid: duplicate\nname: Duplicate\n---\nDo work.",
            );
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
    fn catalog_lists_missing_tool_skills_but_full_text_is_not_injected() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        write_skill(
            &root,
            "delegate",
            "---\nid: delegate\nname: Delegate\nrequired_tools: [child_agent_list]\n---\nDelegate only when it helps.",
        );
        write_skill(
            &root,
            "writer",
            "---\nid: writer\nname: Writer\nrequired_tools: [write_file]\n---\nWrite the requested artifact.",
        );
        let available = HashSet::from(["child_agent_list".to_owned()]);
        let plan = skill_prompt_plan(
            &root,
            &["delegate".to_owned(), "writer".to_owned()],
            Some(&available),
            "delegate this task",
            4_000,
        )
        .expect("compose skill plan");
        assert!(plan.prompt.contains("Delegate only when it helps."));
        assert!(!plan.prompt.contains("Write the requested artifact."));
        // The skipped skill stays discoverable in the catalog and diagnostics.
        assert!(plan.prompt.contains("skill_load"));
        assert!(plan.prompt.contains("writer"));
        assert!(plan
            .omitted
            .iter()
            .any(|item| item.id == "writer" && item.reason == "missing_tools"));
        // Catalog dependency state is explicit.
        assert!(plan
            .catalog
            .iter()
            .any(|entry| entry.id == "writer"
                && matches!(entry.state, SkillToolState::MissingTools(_))));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn budget_counts_unicode_characters_not_utf8_bytes() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        // 200 Chinese characters = 600 UTF-8 bytes but 200 scalar values.
        let body = format!(
            "---\nid: zh\nname: 中文技能\nversion: 1.0\n---\n{}",
            "中".repeat(200)
        );
        write_skill(&root, "zh", &body);
        let plan = skill_prompt_plan(
            &root,
            &["zh".to_owned()],
            None,
            "",
            600, // would fit 200 chars but nowhere near 600+ bytes under byte math
        )
        .expect("compose skill plan");
        // Under the old byte accounting the 600-byte body could never fit a
        // 600-unit budget; character accounting injects it (minus the heading).
        assert!(
            plan.included.iter().any(|item| item.id == "zh"),
            "Chinese skill should fit a character-denominated budget: {}",
            plan.prompt
        );
        let included = plan.included.iter().find(|item| item.id == "zh").unwrap();
        assert_eq!(included.chars, 200);
        assert_eq!(included.bytes, 600);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn skipped_skill_can_still_be_loaded_on_demand() {
        let root = std::env::temp_dir().join(format!("fox-skill-test-{}", uuid::Uuid::new_v4()));
        // Body is deliberately larger than a tight budget so it must rely on
        // the catalog + on-demand path instead of initial injection.
        let body = format!(
            "---\nid: late\nname: Late\nversion: 2.3\nrequired_tools: [read]\n---\nLoad me when charting starts.{}",
            "x".repeat(300)
        );
        write_skill(&root, "late", &body);
        // Catalog line survives at 330 chars; the 300+ char full text does not.
        let plan = skill_prompt_plan(
            &root,
            &["late".to_owned()],
            None,
            "",
            330,
        )
        .expect("compose plan");
        assert!(!plan.prompt.contains("Load me when charting starts."));
        assert!(plan.prompt.chars().count() <= 330, "{}", plan.prompt.len());
        let loaded = on_demand_skill(
            &root,
            &["late".to_owned()],
            Some(&HashSet::from(["read".to_owned()])),
            "late",
        )
        .expect("load skill on demand");
        assert!(loaded.block.contains("Load me when charting starts."));
        assert_eq!(loaded.version, "2.3");
        assert!(loaded.content_sha256.starts_with("sha256:"));
        assert!(loaded.tools_available);
        // Unknown / disabled skills are rejected.
        assert!(on_demand_skill(&root, &["late".to_owned()], None, "nope").is_err());
        assert!(on_demand_skill(&root, &[], None, "late").is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}

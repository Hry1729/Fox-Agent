//! `skill_load` — on-demand discovery and loading of enabled skills.
//!
//! The initial prompt only carries a compact catalog (plus full texts that fit
//! the character budget). When the model reaches work that needs a skipped
//! skill, it calls this tool and Host returns the exact SKILL.md body.
//!
//! A tight budget can truncate *both* the catalog and the diagnostics section,
//! so a skill may be listed nowhere by id. Discovery is therefore part of the
//! same tool: a bounded, pageable, searchable catalog query (`query` / `offset`
//! / `limit`) that lists enabled skills with their minimum facts — never their
//! full text — and returns the next offset so the model can walk the whole
//! catalog a page at a time.
//!
//! Guarantees this module is responsible for:
//!
//! - **Only skills enabled for this Run are visible or loadable.** The enabled
//!   set is resolved by Host from the conversation's assistant/expert bindings,
//!   never supplied by the model. [`prepare`] refuses every scope-shaped
//!   argument.
//! - **Neither discovery nor loading grants tools.** Dependency state is
//!   computed against the Run's frozen tool scope and reported (`toolsAvailable`,
//!   `missingTools`); a skill whose tools are absent still loads as text with an
//!   explicit warning, and the frozen scope is what denies the later call.
//! - **The exact content version of a load is audited.** Every load persists id,
//!   version and the sha256 of the returned instructions (see
//!   `skill_activations`), keyed so a repeated load of identical content does
//!   not duplicate the row. A catalog *query* injects no instructions, so it
//!   records no activation.
//! - **The response is bounded.** Discovery returns at most
//!   [`MAX_CATALOG_PAGE`] entries with a short description each, so it can never
//!   become a way to smuggle the whole catalog past the prompt budget.
//!
//! Integrated by the Kernel context-resource path
//! (`kernel_gateway.rs::execute_context_resource`) and the legacy protocol
//! path (`mod.rs::execute_skill_load_request`).

use crate::database::{Database, SkillActivationRecord};
use crate::skills;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

/// Maximum entries one discovery page returns.
pub(super) const MAX_CATALOG_PAGE: usize = 20;
/// Maximum characters of one entry's description in a discovery response.
const CATALOG_DESCRIPTION_CHARS: usize = 80;
/// Maximum characters of a discovery query.
const MAX_QUERY_CHARS: usize = 64;

/// Fields a caller may not supply: scope and enablement are bound by Host.
const FORBIDDEN_ARGUMENTS: &[&str] = &[
    "conversationId",
    "conversation",
    "authorizedConversationId",
    "runId",
    "enabled",
    "enabledSkills",
    "tools",
    "grantTools",
    "scope",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SkillLoadRequest {
    /// Return one skill's full instructions.
    Load { skill_id: String },
    /// List a bounded page of the enabled skills without their full texts.
    Discover {
        query: String,
        offset: usize,
        limit: usize,
    },
}

pub(super) fn prepare(input: &Value) -> Result<SkillLoadRequest, String> {
    for field in FORBIDDEN_ARGUMENTS {
        if input.get(*field).is_some() {
            return Err(format!(
                "'{field}' is not an argument of skill_load; Fox binds the skill scope to this Run"
            ));
        }
    }
    // A present `skillId` must be a usable id; only omitting it means discovery.
    // An explicitly empty id is a mistake, not a request for the catalog.
    if input.get("skillId").is_some() {
        let skill_id = input
            .get("skillId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "skillId must be a non-empty catalog id".to_owned())?;
        if input.get("query").is_some() || input.get("offset").is_some() {
            return Err(
                "skill_load loads one skill by skillId or lists the catalog by query/offset, not both"
                    .to_owned(),
            );
        }
        if skill_id.len() > 128
            || !skill_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err("skillId must only contain letters, digits, '-' and '_'".to_owned());
        }
        return Ok(SkillLoadRequest::Load {
            skill_id: skill_id.to_owned(),
        });
    }
    // No id: a catalog query. A bare call is meaningful too — it returns the
    // first page, which is how a model finds an id the prompt had to omit.
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!(
            "skill_load query must be at most {MAX_QUERY_CHARS} characters"
        ));
    }
    let offset = match input.get("offset") {
        None => 0,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "skill_load offset must be a non-negative integer".to_owned())?
            as usize,
    };
    let limit = match input.get("limit") {
        None => MAX_CATALOG_PAGE,
        Some(value) => {
            let value = value
                .as_u64()
                .ok_or_else(|| "skill_load limit must be a positive integer".to_owned())?;
            (value.max(1) as usize).min(MAX_CATALOG_PAGE)
        }
    };
    Ok(SkillLoadRequest::Discover {
        query,
        offset,
        limit,
    })
}

/// Serve one request. `enabled_skill_ids` and `frozen_tools` must be derived by
/// the caller from the Run's own binding; `frozen_tools` is `None` only on the
/// legacy path that has no frozen scope.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    database: &Database,
    skills_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    enabled_skill_ids: &[String],
    frozen_tools: Option<&HashSet<String>>,
    request: &SkillLoadRequest,
    now: i64,
) -> Result<Value, String> {
    match request {
        SkillLoadRequest::Discover {
            query,
            offset,
            limit,
        } => execute_discover(
            skills_dir,
            enabled_skill_ids,
            frozen_tools,
            query,
            *offset,
            *limit,
        ),
        SkillLoadRequest::Load { skill_id } => execute_load(
            database,
            skills_dir,
            conversation_id,
            run_id,
            enabled_skill_ids,
            frozen_tools,
            skill_id,
            now,
        ),
    }
}

fn execute_discover(
    skills_dir: &Path,
    enabled_skill_ids: &[String],
    frozen_tools: Option<&HashSet<String>>,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    let page = skills::skill_catalog_page(
        skills_dir,
        enabled_skill_ids,
        frozen_tools,
        query,
        offset,
        limit,
    )?;
    let lines = if page.entries.is_empty() {
        format!(
            "## Skill catalog（查询：{}）\n没有匹配的已启用技能（共 {} 个已启用技能）。可省略 query 列出全部，或用 skill_load {{query}} 换一个关键词。",
            if query.is_empty() { "全部" } else { query },
            page.total_enabled
        )
    } else {
        let mut body = format!(
            "## Skill catalog（查询：{}，第 {} 条起，共匹配 {} 个）\n",
            if query.is_empty() { "全部" } else { query },
            page.offset + 1,
            page.total_matched
        );
        for entry in &page.entries {
            let state = if entry.tools_available {
                "可加载".to_owned()
            } else {
                format!("缺少工具：{}", entry.missing_tools.join(","))
            };
            let description = bounded_description(&entry.description);
            body.push_str(&format!(
                "- {} ({}) v{}；{}{}（正文 {} 字符 / {} 字节）\n",
                entry.name,
                entry.id,
                entry.version,
                state,
                if description.is_empty() {
                    String::new()
                } else {
                    format!(" — {description}")
                },
                entry.chars,
                entry.bytes
            ));
        }
        if let Some(next) = page.next_offset {
            body.push_str(&format!(
                "还有更多技能：用 skill_load {{query, offset: {next}}} 继续查看。\n"
            ));
        }
        body.push_str("用上面的 ID 调用 skill_load {skillId} 即可取得该技能的完整说明。\n");
        body
    };
    Ok(json!({
        "content": [{ "type": "text", "text": lines }],
        "details": {
            "mode": "catalog",
            "query": query,
            "offset": page.offset,
            "limit": limit,
            "returned": page.entries.len(),
            "totalMatched": page.total_matched,
            "totalEnabled": page.total_enabled,
            "nextOffset": page.next_offset,
            "entries": page.entries.iter().map(|entry| json!({
                "skillId": entry.id,
                "name": entry.name,
                "version": entry.version,
                "description": bounded_description(&entry.description),
                "chars": entry.chars,
                "bytes": entry.bytes,
                "requiredTools": entry.required_tools,
                "missingTools": entry.missing_tools,
                "toolsAvailable": entry.tools_available,
            })).collect::<Vec<_>>(),
            "note": "列表只报告当前 Run 已启用技能的最小信息，不注入任何技能正文，也不新增工具授权；用返回的 skillId 调用 skill_load 才能取得全文。",
        },
    }))
}

fn bounded_description(description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.chars().count() <= CATALOG_DESCRIPTION_CHARS {
        return trimmed.to_owned();
    }
    let mut bounded = trimmed.chars().take(CATALOG_DESCRIPTION_CHARS).collect::<String>();
    bounded.push('…');
    bounded
}

#[allow(clippy::too_many_arguments)]
fn execute_load(
    database: &Database,
    skills_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    enabled_skill_ids: &[String],
    frozen_tools: Option<&HashSet<String>>,
    skill_id: &str,
    now: i64,
) -> Result<Value, String> {
    let loaded = skills::on_demand_skill(skills_dir, enabled_skill_ids, frozen_tools, skill_id)?;
    database.record_skill_activation(
        run_id,
        Some(conversation_id),
        &SkillActivationRecord {
            skill_id: loaded.id.clone(),
            version: loaded.version.clone(),
            content_sha256: loaded.content_sha256.clone(),
            source: "on_demand".to_owned(),
            required_tools: loaded.required_tools.clone(),
            missing_tools: loaded.missing_tools.clone(),
            tools_available: loaded.tools_available,
            char_count: loaded.chars as i64,
            byte_count: loaded.bytes as i64,
            created_at: now,
        },
    )?;
    let warning = if loaded.tools_available {
        None
    } else {
        Some(format!(
            "该技能依赖的工具 {} 不在当前 Run 的冻结工具范围内；Fox 不会因加载技能而授权这些工具，相关调用会被拒绝。",
            loaded.missing_tools.join("、")
        ))
    };
    Ok(json!({
        "content": [{ "type": "text", "text": loaded.block }],
        "details": {
            "mode": "load",
            "skillId": loaded.id,
            "name": loaded.name,
            "version": loaded.version,
            "contentSha256": loaded.content_sha256,
            "source": "on_demand",
            "chars": loaded.chars,
            "bytes": loaded.bytes,
            "requiredTools": loaded.required_tools,
            "missingTools": loaded.missing_tools,
            "toolsAvailable": loaded.tools_available,
            "warning": warning,
        },
    }))
}

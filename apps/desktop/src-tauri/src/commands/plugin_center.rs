use crate::{
    app_state::AppState,
    database::{
        ApiResponse, Database, McpRuntimeStatus, PluginActivation, PluginActivationUpdate,
        PluginCardView, PluginCatalogDTO, PluginCatalogEntryRecord, PluginCatalogQuery,
        PluginInstallStatus, PluginInstallationRecord, PluginInstallationsDTO, PluginKind,
        PluginOrigin, PluginSetActivationRequest,
    },
};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};
use tauri::State;

const DEFAULT_PAGE_SIZE: usize = 24;
const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug)]
struct PluginAdapterError {
    code: &'static str,
    message: String,
    retryable: bool,
}

impl PluginAdapterError {
    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    fn storage(message: impl Into<String>) -> Self {
        Self::new("plugin.storage_failed", message, true)
    }
}

#[tauri::command]
pub fn plugin_catalog_list(
    state: State<'_, AppState>,
    request: PluginCatalogQuery,
) -> ApiResponse<PluginCatalogDTO> {
    match aggregate_plugin_cards(&state.database, &state.skills_dir) {
        Ok(cards) => ApiResponse::success(page_catalog(cards, request)),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

#[tauri::command]
pub fn plugin_installations_list(
    state: State<'_, AppState>,
) -> ApiResponse<PluginInstallationsDTO> {
    match aggregate_plugin_cards(&state.database, &state.skills_dir) {
        Ok(cards) => {
            let items = cards
                .into_iter()
                .filter(|card| !matches!(card.install_status, PluginInstallStatus::NotInstalled))
                .collect::<Vec<_>>();
            ApiResponse::success(PluginInstallationsDTO {
                total: items.len(),
                items,
            })
        }
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

#[tauri::command]
pub fn plugin_set_activation(
    state: State<'_, AppState>,
    request: PluginSetActivationRequest,
) -> ApiResponse<PluginCardView> {
    match set_plugin_activation(&state.database, &state.skills_dir, &request) {
        Ok(card) => ApiResponse::success(card),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

fn aggregate_plugin_cards(
    database: &Database,
    skills_dir: &Path,
) -> Result<Vec<PluginCardView>, PluginAdapterError> {
    let catalog_entries = database
        .list_plugin_catalog_entries()
        .map_err(PluginAdapterError::storage)?;
    let installations = database
        .list_plugin_installations()
        .map_err(PluginAdapterError::storage)?;
    let mut cards = catalog_cards(&catalog_entries);

    let mcp_servers = database
        .list_mcp_servers()
        .map_err(PluginAdapterError::storage)?;
    let mcp_sources = database
        .list_mcp_plugin_sources()
        .map_err(PluginAdapterError::storage)?;
    let source_by_server = mcp_sources
        .into_iter()
        .map(|source| (source.server_id.clone(), source))
        .collect::<HashMap<_, _>>();
    for server in mcp_servers {
        let source = source_by_server.get(&server.id);
        let canonical_id = source
            .and_then(|value| value.catalog_plugin_id.clone())
            .unwrap_or_else(|| format!("mcp:{}", server.id));
        let aliases = [
            canonical_id.clone(),
            server.id.clone(),
            format!("mcp:{}", server.id),
            format!("mcp-{}", server.id),
        ];
        let index = find_card_index(&cards, &aliases);
        let card_id = index
            .and_then(|index| cards.get(index).map(|card| card.id.clone()))
            .unwrap_or(canonical_id);
        let origin = source
            .map(|value| value.origin.clone())
            .unwrap_or(PluginOrigin::Local);
        let install_status = installation_for(&installations, &aliases)
            .map(|value| value.install_status.clone())
            .unwrap_or(PluginInstallStatus::Installed);
        let version =
            installation_for(&installations, &aliases).map(|value| value.installed_version.clone());
        let runtime_status = Some(mcp_runtime_status(&server.status));
        let live = PluginCardView {
            id: card_id,
            kind: PluginKind::Mcp,
            origin,
            category: "MCP 服务".to_owned(),
            name: server.name.clone(),
            description: format!("连接 {} 提供的外部工具和能力。", server.name),
            icon: Some("cable".to_owned()),
            version,
            install_status,
            activation: PluginActivation::Global {
                enabled: server.enabled,
            },
            runtime_status,
            permissions: Vec::new(),
            compatible: true,
            incompatibility_reason: None,
            installed_at: installation_for(&installations, &aliases)
                .map(|value| value.installed_at),
            updated_at: Some(server.updated_at),
        };
        merge_live_card(&mut cards, index, live);
    }

    let skills = crate::skills::scan_skills(skills_dir, &[])
        .map_err(|error| PluginAdapterError::new("skills.scan_failed", error, true))?;
    for skill in skills {
        let record = skill.record;
        let canonical_id = format!("skill:{}", record.id);
        let aliases = [
            canonical_id.clone(),
            record.id.clone(),
            format!("skill:{}", record.id),
            format!("skill-{}", record.id),
        ];
        let index = find_card_index(&cards, &aliases);
        let card_id = index
            .and_then(|index| cards.get(index).map(|card| card.id.clone()))
            .unwrap_or(canonical_id);
        let enabled_agent_count = database
            .enabled_agent_skill_count(&record.id)
            .map_err(PluginAdapterError::storage)?;
        let installation = installation_for(&installations, &aliases);
        let install_status = installation
            .map(|value| value.install_status.clone())
            .unwrap_or(PluginInstallStatus::Installed);
        let live = PluginCardView {
            id: card_id,
            kind: PluginKind::Skill,
            origin: index
                .and_then(|index| cards.get(index).map(|card| card.origin.clone()))
                .unwrap_or(PluginOrigin::Local),
            category: "Skills".to_owned(),
            name: record.name,
            description: record.description,
            icon: Some("sparkles".to_owned()),
            version: Some(record.version),
            install_status,
            activation: PluginActivation::PerAgent {
                enabled_agent_count,
            },
            runtime_status: None,
            permissions: record.required_tools,
            compatible: record.valid,
            incompatibility_reason: record.validation_error,
            installed_at: installation.map(|value| value.installed_at),
            updated_at: installation.map(|value| value.updated_at),
        };
        merge_live_card(&mut cards, index, live);
    }

    let tools = builtin_tool_names(database).map_err(PluginAdapterError::storage)?;
    for tool in tools {
        let canonical_id = format!("tool:{}", tool);
        let aliases = [canonical_id.clone(), tool.clone(), format!("tool-{}", tool)];
        let index = find_card_index(&cards, &aliases);
        let card_id = index
            .and_then(|index| cards.get(index).map(|card| card.id.clone()))
            .unwrap_or(canonical_id);
        let live = PluginCardView {
            id: card_id,
            kind: PluginKind::Tool,
            origin: PluginOrigin::Builtin,
            category: builtin_tool_category(&tool).to_owned(),
            name: tool.clone(),
            description: builtin_tool_description(&tool).to_owned(),
            icon: Some("wrench".to_owned()),
            version: None,
            install_status: PluginInstallStatus::Installed,
            activation: PluginActivation::NotApplicable,
            runtime_status: None,
            permissions: Vec::new(),
            compatible: true,
            incompatibility_reason: None,
            installed_at: None,
            updated_at: None,
        };
        merge_live_card(&mut cards, index, live);
    }

    for card in &mut cards {
        if let Some(installation) = installation_for(&installations, std::slice::from_ref(&card.id))
        {
            card.install_status = installation.install_status.clone();
            card.installed_at = Some(installation.installed_at);
            card.updated_at = Some(installation.updated_at);
            if card.version.is_none() {
                card.version = Some(installation.installed_version.clone());
            }
        }
    }
    cards.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(cards)
}

fn set_plugin_activation(
    database: &Database,
    skills_dir: &Path,
    request: &PluginSetActivationRequest,
) -> Result<PluginCardView, PluginAdapterError> {
    let cards = aggregate_plugin_cards(database, skills_dir)?;
    let card = cards
        .iter()
        .find(|card| card.id == request.plugin_id)
        .ok_or_else(|| {
            PluginAdapterError::new(
                "plugin.not_found",
                format!("未找到插件：{}", request.plugin_id),
                false,
            )
        })?;

    match (&card.kind, &request.update) {
        (_, PluginActivationUpdate::NotApplicable) => {
            return Err(PluginAdapterError::new(
                "plugin.activation_not_applicable",
                "该插件不支持用户修改启用状态",
                false,
            ));
        }
        (PluginKind::Mcp, PluginActivationUpdate::Global { enabled }) => {
            let server_id =
                mcp_server_id_for_plugin(database, &request.plugin_id)?.ok_or_else(|| {
                    PluginAdapterError::new(
                        "plugin.not_installed",
                        "该 MCP 尚未安装或没有可用配置",
                        false,
                    )
                })?;
            database
                .set_mcp_server_enabled(&server_id, *enabled)
                .map_err(PluginAdapterError::storage)?
                .ok_or_else(|| {
                    PluginAdapterError::new("plugin.not_found", "MCP Server 不存在", false)
                })?;
        }
        (PluginKind::Skill, PluginActivationUpdate::PerAgent { agent_id, enabled }) => {
            let agent_id = agent_id.trim();
            if agent_id.is_empty() {
                return Err(PluginAdapterError::new(
                    "plugin.agent_required",
                    "按 Agent 启用 Skill 时必须提供 agentId",
                    false,
                ));
            }
            if database
                .get_agent(agent_id)
                .map_err(PluginAdapterError::storage)?
                .is_none()
            {
                return Err(PluginAdapterError::new(
                    "plugin.agent_not_found",
                    format!("未找到 Agent：{agent_id}"),
                    false,
                ));
            }
            let skill_id = skill_id_for_plugin(database, skills_dir, &request.plugin_id)?
                .ok_or_else(|| {
                    PluginAdapterError::new(
                        "plugin.not_installed",
                        "该 Skill 尚未安装或目录无效",
                        false,
                    )
                })?;
            database
                .set_agent_skill_enabled(agent_id, &skill_id, *enabled)
                .map_err(PluginAdapterError::storage)?;
        }
        (PluginKind::Mcp, PluginActivationUpdate::PerAgent { .. }) => {
            return Err(PluginAdapterError::new(
                "plugin.activation_scope_mismatch",
                "MCP 只支持全局启用状态",
                false,
            ));
        }
        (PluginKind::Skill, PluginActivationUpdate::Global { .. }) => {
            return Err(PluginAdapterError::new(
                "plugin.activation_scope_mismatch",
                "Skill 必须按 Agent 配置启用范围",
                false,
            ));
        }
        (PluginKind::Tool, _) => {
            return Err(PluginAdapterError::new(
                "plugin.activation_not_applicable",
                "内建 Tool 的启用状态由 Host 管理",
                false,
            ));
        }
    }

    aggregate_plugin_cards(database, skills_dir)?
        .into_iter()
        .find(|value| value.id == request.plugin_id)
        .ok_or_else(|| {
            PluginAdapterError::new("plugin.not_found", "激活状态更新后无法重新加载插件", true)
        })
}

fn catalog_cards(entries: &[PluginCatalogEntryRecord]) -> Vec<PluginCardView> {
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .filter(|entry| seen.insert(entry.plugin_id.clone()))
        .map(|entry| {
            let manifest = &entry.manifest;
            let permissions = manifest_strings(manifest, "permissions");
            let category = manifest_string(manifest, "category")
                .unwrap_or_else(|| default_category(&entry.kind).to_owned());
            PluginCardView {
                id: entry.plugin_id.clone(),
                kind: entry.kind.clone(),
                origin: entry.origin.clone(),
                category,
                name: manifest_string(manifest, "name").unwrap_or_else(|| entry.plugin_id.clone()),
                description: manifest_string(manifest, "description").unwrap_or_default(),
                icon: manifest_string(manifest, "icon"),
                version: Some(entry.version.clone()),
                install_status: PluginInstallStatus::NotInstalled,
                activation: default_activation(&entry.kind),
                runtime_status: None,
                permissions,
                compatible: manifest
                    .get("compatible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                incompatibility_reason: manifest_string(manifest, "incompatibilityReason"),
                installed_at: None,
                updated_at: Some(entry.fetched_at),
            }
        })
        .collect()
}

fn page_catalog(mut cards: Vec<PluginCardView>, request: PluginCatalogQuery) -> PluginCatalogDTO {
    cards.retain(|card| card.kind == request.kind);
    let categories = cards
        .iter()
        .map(|card| card.category.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let search = request
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let category = request
        .category
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    cards.retain(|card| {
        let matches_category = category.is_none_or(|value| card.category == value);
        let matches_search = search.as_deref().is_none_or(|value| {
            [
                card.id.as_str(),
                card.name.as_str(),
                card.description.as_str(),
            ]
            .iter()
            .any(|candidate| candidate.to_lowercase().contains(value))
        });
        matches_category && matches_search
    });
    let total = cards.len();
    let start = request
        .cursor
        .as_deref()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
        .min(total);
    let page_size = request
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let next_cursor = (start + page_size < total).then(|| (start + page_size).to_string());
    let items = cards.into_iter().skip(start).take(page_size).collect();
    PluginCatalogDTO {
        items,
        total,
        categories,
        next_cursor,
    }
}

fn merge_live_card(cards: &mut Vec<PluginCardView>, index: Option<usize>, live: PluginCardView) {
    if let Some(index) = index {
        let existing = &mut cards[index];
        let id = existing.id.clone();
        let mut merged = live;
        merged.id = id;
        if merged.description.starts_with("连接 ") && !existing.description.is_empty() {
            merged.description = existing.description.clone();
        }
        if merged.icon == Some("sparkles".to_owned()) && existing.icon.is_some() {
            merged.icon = existing.icon.clone();
        }
        if merged.icon == Some("wrench".to_owned()) && existing.icon.is_some() {
            merged.icon = existing.icon.clone();
        }
        if merged.version.is_none() {
            merged.version = existing.version.clone();
        }
        if merged.permissions.is_empty() {
            merged.permissions = existing.permissions.clone();
        }
        *existing = merged;
    } else {
        cards.push(live);
    }
}

fn find_card_index(cards: &[PluginCardView], aliases: &[String]) -> Option<usize> {
    cards
        .iter()
        .position(|card| aliases.iter().any(|alias| alias == &card.id))
}

fn installation_for<'a>(
    installations: &'a [PluginInstallationRecord],
    aliases: &[String],
) -> Option<&'a PluginInstallationRecord> {
    installations.iter().find(|installation| {
        aliases
            .iter()
            .any(|alias| plugin_ids_match(alias, &installation.plugin_id))
    })
}

fn plugin_ids_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    ["mcp:", "skill:", "tool:"].iter().any(|prefix| {
        left.strip_prefix(prefix) == Some(right) || right.strip_prefix(prefix) == Some(left)
    })
}

fn mcp_server_id_for_plugin(
    database: &Database,
    plugin_id: &str,
) -> Result<Option<String>, PluginAdapterError> {
    let servers = database
        .list_mcp_servers()
        .map_err(PluginAdapterError::storage)?;
    let sources = database
        .list_mcp_plugin_sources()
        .map_err(PluginAdapterError::storage)?;
    Ok(servers.into_iter().find_map(|server| {
        let source = sources.iter().find(|value| value.server_id == server.id);
        let aliases = [
            server.id.clone(),
            format!("mcp:{}", server.id),
            format!("mcp-{}", server.id),
            source
                .and_then(|value| value.catalog_plugin_id.clone())
                .unwrap_or_default(),
        ];
        aliases
            .iter()
            .any(|value| value == plugin_id)
            .then_some(server.id)
    }))
}

fn skill_id_for_plugin(
    database: &Database,
    skills_dir: &Path,
    plugin_id: &str,
) -> Result<Option<String>, PluginAdapterError> {
    let enabled = Vec::new();
    let skills = crate::skills::scan_skills(skills_dir, &enabled)
        .map_err(|error| PluginAdapterError::new("skills.scan_failed", error, true))?;
    let _ = database;
    Ok(skills.into_iter().find_map(|skill| {
        let id = skill.record.id;
        let aliases = [id.clone(), format!("skill:{}", id), format!("skill-{}", id)];
        aliases.iter().any(|value| value == plugin_id).then_some(id)
    }))
}

fn builtin_tool_names(database: &Database) -> Result<Vec<String>, String> {
    let mut names = BTreeSet::new();
    for agent in database.list_agents()? {
        if !agent.is_builtin {
            continue;
        }
        if let Some(tools) = agent
            .package_manifest
            .get("allowedTools")
            .and_then(Value::as_array)
        {
            names.extend(tools.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    Ok(names.into_iter().collect())
}

fn manifest_string(manifest: &Value, key: &str) -> Option<String> {
    manifest
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn manifest_strings(manifest: &Value, key: &str) -> Vec<String> {
    manifest
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn default_activation(kind: &PluginKind) -> PluginActivation {
    match kind {
        PluginKind::Mcp => PluginActivation::Global { enabled: false },
        PluginKind::Skill => PluginActivation::PerAgent {
            enabled_agent_count: 0,
        },
        PluginKind::Tool => PluginActivation::NotApplicable,
    }
}

fn default_category(kind: &PluginKind) -> &'static str {
    match kind {
        PluginKind::Mcp => "MCP 服务",
        PluginKind::Skill => "Skills",
        PluginKind::Tool => "内建工具",
    }
}

fn builtin_tool_category(tool: &str) -> &'static str {
    match tool {
        "read" | "ls" | "find" | "grep" | "read_attachment" => "文件与办公",
        "write_file" | "edit_file" | "run_command" | "git_read" | "test_run" | "code_check"
        | "format_code" => "开发工具",
        "web_search" | "web_read" | "http_request" => "网络与检索",
        "system_info" => "系统诊断",
        "structured_data" | "tabular_data" | "sqlite_read" => "数据处理",
        "child_agent_list" | "child_run_start" | "child_run_collect" | "child_run_cancel" => {
            "Agent 编排"
        }
        "memory_search" | "memory_propose" => "记忆与上下文",
        "list_knowledge_bases"
        | "search_knowledge"
        | "read_knowledge_document"
        | "query_knowledge_graph" => "知识库",
        "list_mcp_tools" | "call_mcp_tool" => "连接器",
        _ => "工作流",
    }
}

fn builtin_tool_description(tool: &str) -> &'static str {
    match tool {
        "read" => "读取授权项目内的文本文件，用于理解源码、配置和文档内容。",
        "ls" => "列出授权目录中的文件和文件夹，帮助快速浏览项目结构。",
        "find" => "按名称或路径模式查找项目文件，快速定位目标资源。",
        "grep" => "在项目文件中搜索文本或正则表达式，定位代码与配置引用。",
        "read_attachment" => "读取当前对话中的文本或 DOCX 附件内容。",
        "write_file" => "在授权项目内创建或覆盖文本文件，并遵循当前写入权限。",
        "edit_file" => "精确替换文件中的指定文本片段，生成可审查的修改。",
        "run_command" => "在授权项目目录运行非交互命令，每次执行都需要明确批准。",
        "web_search" => "检索实时网页并返回统一格式的标题、链接与摘要；默认无需额外配置搜索密钥。",
        "web_read" => "读取公开网页正文并转换为 Markdown 或纯文本，同时阻止私有网络地址。",
        "http_request" => "向精确白名单中的公网主机发送受审批 HTTP 请求；认证 API 使用 Keyring 支持的 OpenAPI Connector。",
        "system_info" => "读取有界 CPU、内存、磁盘和可选进程摘要；环境变量仅返回固定安全白名单。",
        "sqlite_read" => "对授权项目内 SQLite 文件执行单条只读查询，并限制行数、列数、结果大小和时长。",
        "structured_data" => "解析、格式化、转换、查询或校验 JSON、YAML、TOML、XML、CSV 与 TSV。",
        "git_read" => "以只读方式查看 Git 状态、差异、提交历史和逐行归属信息。",
        "test_run" => "自动识别 Rust、Node 或 Python 项目，并运行已有测试命令。",
        "code_check" => {
            "调用项目已有的检查器执行 lint、类型检查、Cargo Check、Clippy、Ruff 或 Mypy。"
        }
        "format_code" => "使用项目已有格式化器检查或应用代码格式，不会自动下载工具。",
        "tabular_data" => "预览、筛选和聚合 CSV、TSV 或 JSON 表格数据。",
        "child_agent_list" => "列出 Host 允许承担隔离 Child Run 的本地 Agent。",
        "child_run_start" => "按显式目标、上下文和预算异步启动一个隔离 Child Run。",
        "child_run_collect" => "收集直属 Child Run 的状态、有限结果、用量与错误。",
        "child_run_cancel" => "取消当前父 Run 拥有的一个活动 Child Run。",
        "memory_search" => "按当前 Agent 与项目作用域检索已确认且启用的受治理记忆。",
        "memory_propose" => "基于当前 Run 的明确证据提交候选记忆，等待用户治理。",
        "list_knowledge_bases" => "列出当前可访问的本地与远程知识库。",
        "search_knowledge" => "在已授权知识库中检索与问题相关的内容。",
        "read_knowledge_document" => "读取知识库文档正文或指定内容片段。",
        "query_knowledge_graph" => "查询知识图谱中的实体、关系与关联路径。",
        "list_mcp_tools" => "列出当前已连接 MCP 服务提供的可用工具。",
        "call_mcp_tool" => "调用已授权的 MCP 工具完成外部服务操作。",
        "work_snapshot_get" => "获取当前目标、任务、证据与执行进度快照。",
        "goal_propose" => "根据用户的明确要求创建或完善一个可追踪目标。",
        "goal_complete" => "在任务与证据满足条件后提交目标完成。",
        "task_create_many" => "为当前目标批量创建有明确顺序的执行任务。",
        "task_update" => "更新任务状态、阻塞原因与并发版本信息。",
        "task_evidence_add" => "为任务关联工具调用、测试或文件修改等执行证据。",
        "task_evidence_validate" => "重新检查任务证据是否仍然有效。",
        "plan_revision_create" => "保存完整的新计划版本及其任务顺序。",
        "review_finding_add" => "记录独立审查发现、严重级别与处理状态。",
        "review_finding_resolve" => "将审查发现标记为已解决或已接受。",
        "acceptance_submit" => "汇总计划、审查与证据并提交最终验收。",
        "workflow_snapshot_get" => "读取当前专家工作流、阶段、Gate 与恢复检查点。",
        "workflow_start" => "按冻结能力包中的声明启动一个持久专家工作流。",
        "workflow_stage_start" => "启动或有限重试当前工作流阶段。",
        "workflow_stage_complete" => "校验阶段输出 Schema、证据和验收条件后完成阶段。",
        "workflow_stage_fail" => "记录阶段失败并应用能力包声明的有限重试策略。",
        "workflow_cancel" => "取消活动专家工作流并收口关联任务状态。",
        _ => "此工具尚未提供功能说明。",
    }
}

fn mcp_runtime_status(status: &str) -> McpRuntimeStatus {
    match status {
        "connected" | "healthy" | "ready" => McpRuntimeStatus::Healthy,
        "unavailable" | "error" => McpRuntimeStatus::Error,
        "disabled" => McpRuntimeStatus::Degraded,
        "connecting" => McpRuntimeStatus::Connecting,
        _ => McpRuntimeStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::PluginActivationUpdate;
    use std::fs;
    use uuid::Uuid;

    fn database() -> (Database, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-plugin-test-{}.db", Uuid::new_v4()));
        (
            Database::open(path.clone()).expect("open plugin test database"),
            path,
        )
    }

    fn skill_dir() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("fox-plugin-skills-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("research")).expect("create skill directory");
        fs::write(
            root.join("research").join("SKILL.md"),
            "---\nid: research\nname: Research\nversion: 1.0.0\n---\nUse evidence.",
        )
        .expect("write skill");
        root
    }

    #[test]
    fn catalog_aggregates_mcp_skill_and_builtin_tools() {
        let (database, database_path) = database();
        database
            .save_mcp_server("mcp-test", "Test MCP", "echo", &[], "stdio", None, None)
            .expect("save MCP");
        let skills = skill_dir();
        let cards = aggregate_plugin_cards(&database, &skills).expect("aggregate cards");
        assert!(cards.iter().any(|card| card.kind == PluginKind::Mcp));
        assert!(cards.iter().any(|card| card.kind == PluginKind::Skill));
        assert!(cards.iter().any(|card| card.kind == PluginKind::Tool));
        assert!(cards.iter().any(|card| card.id == "mcp:mcp-test"
            && card.install_status == PluginInstallStatus::Installed));
        let tool_cards = cards
            .iter()
            .filter(|card| card.kind == PluginKind::Tool)
            .collect::<Vec<_>>();
        let tool_descriptions = tool_cards
            .iter()
            .map(|card| card.description.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(tool_descriptions.len(), tool_cards.len());
        assert!(tool_cards.iter().all(|card| {
            card.description != "Fox 内建工具，由 Host 统一执行和授权。"
                && card.description != "此工具尚未提供功能说明。"
        }));
        let _ = fs::remove_dir_all(skills);
        drop(database);
        let _ = fs::remove_file(database_path);
    }

    #[test]
    fn global_activation_updates_mcp_fact_source() {
        let (database, database_path) = database();
        database
            .save_mcp_server("mcp-test", "Test MCP", "echo", &[], "stdio", None, None)
            .expect("save MCP");
        let skills = skill_dir();
        let result = set_plugin_activation(
            &database,
            &skills,
            &PluginSetActivationRequest {
                plugin_id: "mcp:mcp-test".to_owned(),
                update: PluginActivationUpdate::Global { enabled: false },
            },
        )
        .expect("disable MCP");
        assert_eq!(
            result.activation,
            PluginActivation::Global { enabled: false }
        );
        assert!(
            !database
                .get_mcp_server("mcp-test")
                .expect("load MCP")
                .expect("MCP exists")
                .enabled
        );
        let _ = fs::remove_dir_all(skills);
        drop(database);
        let _ = fs::remove_file(database_path);
    }

    #[test]
    fn per_agent_activation_updates_skill_fact_source() {
        let (database, database_path) = database();
        let skills = skill_dir();
        let result = set_plugin_activation(
            &database,
            &skills,
            &PluginSetActivationRequest {
                plugin_id: "skill:research".to_owned(),
                update: PluginActivationUpdate::PerAgent {
                    agent_id: "fox-general".to_owned(),
                    enabled: true,
                },
            },
        )
        .expect("enable Skill for agent");
        assert_eq!(
            result.activation,
            PluginActivation::PerAgent {
                enabled_agent_count: 1
            }
        );
        assert_eq!(database.enabled_agent_skill_count("research").unwrap(), 1);
        let _ = fs::remove_dir_all(skills);
        drop(database);
        let _ = fs::remove_file(database_path);
    }

    #[test]
    fn not_applicable_activation_is_rejected() {
        let (database, database_path) = database();
        let skills = skill_dir();
        let error = set_plugin_activation(
            &database,
            &skills,
            &PluginSetActivationRequest {
                plugin_id: "tool:read".to_owned(),
                update: PluginActivationUpdate::NotApplicable,
            },
        )
        .expect_err("tool activation must be rejected");
        assert_eq!(error.code, "plugin.activation_not_applicable");
        let _ = fs::remove_dir_all(skills);
        drop(database);
        let _ = fs::remove_file(database_path);
    }
}

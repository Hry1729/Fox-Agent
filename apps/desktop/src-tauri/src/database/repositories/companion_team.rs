// Ported from OpenMausBot @ 1a4e4da9f380804ad98c7fd1b141bb0dbfafced4: server/team-setup-requests.ts, server/profile-requests.ts, server/direct-apply.ts, server/default-bot.ts, server/drivers/agents-catalog.ts and server/peer-roster.ts (Apache-2.0).
use super::super::companion_ownership::resolve_run_on;
use super::{
    create_companion_inner, now_ms, profile, require_idle, save_model, save_profile,
    CompanionError, CompanionPreferenceEdit, CompanionProfileSaveInput, CompanionView, Database,
    Result,
};
use crate::{app_state::AppState, commands::companion_deletions};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Create,
    Profile,
    Model,
    Delete,
    Archive,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Profile => "profile",
            Self::Model => "model",
            Self::Delete => "delete",
            Self::Archive => "archive",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Applied,
    Pending,
    Undone,
    Confirmed,
    Dismissed,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Undo,
    Confirm,
    Dismiss,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTeamAction {
    id: String,
    pub(crate) companion_id: String,
    conversation_id: String,
    run_id: String,
    target_id: String,
    target_name: String,
    kind: Kind,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<Value>,
    created_at: i64,
    updated_at: i64,
}

struct Actor<'a> {
    companion_id: &'a str,
    report_conversation_id: String,
    run_id: &'a str,
}

impl<'a> Actor<'a> {
    fn authorize(
        conn: &Connection,
        companion_id: &'a str,
        conversation_id: &str,
        run_id: &'a str,
    ) -> Result<Self> {
        let scope = resolve_run_on(conn, run_id)
            .map_err(CompanionError::Storage)?
            .filter(|scope| {
                !scope.is_child
                    && scope.companion_id == companion_id
                    && scope.conversation_id == conversation_id
            })
            .ok_or_else(authority_denied)?;
        let allowed = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM runs r CROSS JOIN companion_profiles p
                WHERE r.id=?1 AND p.id=?2 AND r.status='running' AND p.is_default=1
                  AND p.archived_at IS NULL AND p.deleting_at IS NULL
            )",
            params![run_id, companion_id],
            |r| r.get::<_, bool>(0),
        )?;
        if !allowed {
            return Err(authority_denied());
        }
        Ok(Self {
            companion_id,
            report_conversation_id: scope
                .report_conversation_id
                .unwrap_or(scope.conversation_id),
            run_id,
        })
    }

    fn record(&self, target: &CompanionView, kind: Kind, status: Status) -> CompanionTeamAction {
        let now = now_ms();
        CompanionTeamAction {
            id: Uuid::new_v4().to_string(),
            companion_id: self.companion_id.to_owned(),
            conversation_id: self.report_conversation_id.clone(),
            run_id: self.run_id.to_owned(),
            target_id: target.profile.id.clone(),
            target_name: target.profile.display_name.clone(),
            kind,
            status,
            before: None,
            after: None,
            preview: None,
            created_at: now,
            updated_at: now,
        }
    }
}

fn authority_denied() -> CompanionError {
    CompanionError::TeamAction("companion.team_tool_denied", "只有小狐的主回合可以管理伙伴")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    name: String,
    duty: String,
    instructions: Option<String>,
    provider_id: Option<String>,
    model_ref: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    target: String,
    name: Option<String>,
    duty: Option<String>,
    instructions: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelInput {
    target: String,
    #[serde(default, deserialize_with = "present_nullable")]
    provider_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "present_nullable")]
    model_ref: Option<Option<String>>,
    model: Option<String>,
}

fn present_nullable<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<Option<String>>, D::Error> {
    Option::deserialize(decoder).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {}

fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T> {
    serde_json::from_value(input)
        .map_err(|_| CompanionError::InvalidInput("参数缺失、类型不正确或包含不允许修改的字段"))
}

fn check_instructions(value: Option<&str>) -> Result<()> {
    if value.is_some_and(|text| text.chars().count() > 4_000) {
        return Err(CompanionError::InvalidInput("工作要求最多 4000 字"));
    }
    Ok(())
}

pub(crate) fn execute(
    db: &Database,
    root: &Path,
    companion_id: &str,
    conversation_id: &str,
    run_id: &str,
    tool: &str,
    input: Value,
) -> Result<Value> {
    if tool == "companion_list" {
        db.with_connection(|conn| {
            Ok(Actor::authorize(
                conn,
                companion_id,
                conversation_id,
                run_id,
            ))
        })
        .map_err(CompanionError::Storage)??;
        parse::<ListInput>(input)?;
        return Ok(json!({"companions": roster(db, companion_id)?}));
    }

    let action = db.companion_transaction(|tx| {
        let actor = Actor::authorize(tx, companion_id, conversation_id, run_id)?;
        match tool {
            "companion_create" => {
                let input: CreateInput = parse(input)?;
                check_instructions(input.instructions.as_deref())?;
                create(tx, root, &actor, input)
            }
            "companion_propose_profile" => {
                let input: ProfileInput = parse(input)?;
                check_instructions(input.instructions.as_deref())?;
                edit_profile(tx, &actor, input)
            }
            "companion_propose_model" => edit_model(tx, &actor, parse(input)?),
            "companion_propose_deletion" => {
                let input: TargetInput = parse(input)?;
                propose(tx, root, &actor, &input.target, Kind::Delete)
            }
            "companion_propose_archive" => {
                let input: TargetInput = parse(input)?;
                propose(tx, root, &actor, &input.target, Kind::Archive)
            }
            _ => Err(CompanionError::InvalidInput("未知的伙伴管理操作")),
        }
    })?;
    let mut result = json!({"action": receipt(&action)});
    if action.kind == Kind::Create {
        let ready = crate::companion_workspace::repair(db, root, &action.target_id).is_ok();
        result["workspaceReady"] = json!(ready);
        if !ready {
            result["workspaceError"] =
                json!("伙伴已新建，工作目录准备失败。请在该伙伴的资料页重新创建工作目录。");
        }
    }
    Ok(result)
}

fn receipt(action: &CompanionTeamAction) -> Value {
    json!({
        "id": action.id,
        "actionId": action.id,
        "targetId": action.target_id,
        "targetName": safe_label(&action.target_name, 40),
        "kind": action.kind,
        "status": action.status,
        "after": action.after,
    })
}

fn safe_label(value: &str, limit: usize) -> String {
    let flat = value
        .replace(
            |c: char| c.is_control() || matches!(c, '[' | ']' | '［' | '］'),
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() > limit {
        flat.chars().take(limit - 1).chain(['…']).collect()
    } else {
        flat
    }
}

pub(crate) fn roster(db: &Database, actor_id: &str) -> Result<Value> {
    let peers = db
        .list_companions()?
        .companions
        .into_iter()
        .filter(|peer| {
            peer.profile.id != actor_id
                && peer.profile.archived_at.is_none()
                && peer.profile.deleting_at.is_none()
        })
        .map(|peer| {
            let service = db.resolve_effective_model(&peer.conversation_id);
            let (status, detail) = peer_status(&peer, &service);
            let model = service.ok().flatten().map(|service| service.model_id);
            json!({
                "id": peer.profile.id,
                "name": safe_label(&peer.profile.display_name, 40),
                "duty": safe_label(&peer.profile.duty, 120),
                "model": model,
                "status": status,
                "statusDetail": detail,
            })
        })
        .collect::<Vec<_>>();
    Ok(Value::Array(peers))
}

fn peer_status(
    peer: &CompanionView,
    model: &std::result::Result<Option<crate::database::ModelServiceRecord>, String>,
) -> (&'static str, String) {
    if peer.pending_action_count > 0
        || peer.active_run_status.as_deref() == Some("awaiting_confirmation")
    {
        (
            "waiting-on-user",
            format!("有 {} 项等你确认", peer.pending_action_count.max(1)),
        )
    } else if peer.active_run_status.is_some() {
        ("working", "工作中".into())
    } else if peer.profile.workspace_state != "ready" {
        ("unavailable", "工作区不可用".into())
    } else if !matches!(model, Ok(Some(_))) {
        ("unavailable", "模型未配置或不可用".into())
    } else {
        ("available", "空闲".into())
    }
}

fn target(conn: &Connection, raw: &str, kind: Kind) -> Result<CompanionView> {
    let raw = raw.trim();
    let exact = conn
        .query_row(
            "SELECT id FROM companion_profiles WHERE id=?1",
            [raw],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let id = if let Some(id) = exact {
        id
    } else {
        let name = folded_name(raw);
        let loose = loose_name(raw);
        let mut query = conn.prepare(
            "SELECT id, display_name FROM companion_profiles WHERE archived_at IS NULL AND deleting_at IS NULL"
        )?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut exact_names = Vec::new();
        let mut loose_names = Vec::new();
        for row in rows {
            let (id, display_name) = row?;
            if folded_name(&display_name) == name {
                exact_names.push(id);
            } else if loose_name(&display_name) == loose {
                loose_names.push(id);
            }
        }
        let matches = if exact_names.is_empty() {
            loose_names
        } else {
            exact_names
        };
        match matches.as_slice() {
            [id] => id.clone(),
            [] => return Err(CompanionError::TeamAction(
                "companion.not_found",
                "没有找到这个伙伴，可能已改名、被撤销或删除。请先用 companion_list 核对名单。",
            )),
            _ => {
                return Err(CompanionError::TeamAction(
                    "companion.target_ambiguous",
                    "有多个同名伙伴，请用伙伴 ID 指定目标",
                ))
            }
        }
    };
    let target = profile(conn, &id)?;
    if target.profile.is_default {
        return Err(match kind {
            Kind::Delete => CompanionError::DefaultProtected(
                "companion.default_not_deletable",
                "小狐是默认伙伴，不能删除",
            ),
            Kind::Archive => CompanionError::DefaultProtected(
                "companion.default_not_archivable",
                "小狐是默认伙伴，不能归档",
            ),
            _ => CompanionError::DefaultProtected(
                "companion.default_profile_self",
                "小狐请用自己的资料设置更新资料",
            ),
        });
    }
    if target.profile.archived_at.is_some() {
        return Err(CompanionError::Archived);
    }
    Ok(target)
}

fn folded_name(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('@')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn loose_name(value: &str) -> String {
    folded_name(&value.replace(['-', '_'], " "))
}

fn snapshot(view: &CompanionView, kind: Kind) -> Map<String, Value> {
    let p = &view.profile;
    let mut fields = Map::new();
    if matches!(kind, Kind::Create | Kind::Profile) {
        fields.insert("name".into(), json!(p.display_name));
        fields.insert("duty".into(), json!(p.duty));
        fields.insert("instructions".into(), json!(p.persona_override));
    }
    if matches!(kind, Kind::Create | Kind::Model) {
        fields.insert("providerId".into(), json!(p.model_provider_id));
        fields.insert("modelRef".into(), json!(p.model_ref));
    }
    fields
}

fn create(
    tx: &Transaction<'_>,
    root: &Path,
    actor: &Actor<'_>,
    input: CreateInput,
) -> Result<CompanionTeamAction> {
    let created: i64 = tx.query_row(
        "SELECT count(*) FROM companion_team_actions WHERE run_id=?1 AND kind='create'",
        [actor.run_id],
        |r| r.get(0),
    )?;
    if created >= 4 {
        return Err(CompanionError::TeamAction(
            "companion.team_turn_limit",
            "一个回合最多新建 4 个伙伴",
        ));
    }
    let total: i64 = tx.query_row("SELECT count(*) FROM companion_profiles", [], |r| r.get(0))?;
    if total >= 100 {
        return Err(CompanionError::TeamAction(
            "companion.team_total_limit",
            "伙伴总数最多 100 个",
        ));
    }
    let duty = input.duty.trim();
    if duty.is_empty() {
        return Err(CompanionError::InvalidInput("请提供新伙伴的职责"));
    }
    let new = create_companion_inner(tx, root, None, false, Some(&input.name), None)?;
    save_profile(
        tx,
        &new.profile.id,
        profile_edit(None, Some(duty.to_owned()), input.instructions),
    )?;
    let saved = select_model(
        tx,
        &new.profile.id,
        input.provider_id.as_deref(),
        input.model_ref.as_deref(),
        input.model.as_deref(),
    )?;
    let mut action = actor.record(&saved, Kind::Create, Status::Applied);
    action.after = Some(snapshot(&saved, Kind::Create));
    store(tx, &action)?;
    Ok(action)
}

fn profile_edit(
    name: Option<String>,
    duty: Option<String>,
    instructions: Option<String>,
) -> CompanionProfileSaveInput {
    CompanionProfileSaveInput {
        display_name: name,
        avatar_ref: None,
        duty,
        preference: match instructions {
            None => CompanionPreferenceEdit::Preserve,
            Some(value) if value.trim().is_empty() => CompanionPreferenceEdit::Clear,
            Some(value) => CompanionPreferenceEdit::Replace(value),
        },
    }
}

fn edit_profile(
    tx: &Transaction<'_>,
    actor: &Actor<'_>,
    input: ProfileInput,
) -> Result<CompanionTeamAction> {
    let old = target(tx, &input.target, Kind::Profile)?;
    save_profile(
        tx,
        &old.profile.id,
        profile_edit(input.name, input.duty, input.instructions),
    )?;
    let new = profile(tx, &old.profile.id)?;
    record_edit(tx, actor, &old, &new, Kind::Profile)
}

fn edit_model(
    tx: &Transaction<'_>,
    actor: &Actor<'_>,
    input: ModelInput,
) -> Result<CompanionTeamAction> {
    if input.model.is_none()
        && !matches!(
            (&input.provider_id, &input.model_ref),
            (Some(Some(_)), Some(Some(_))) | (Some(None), Some(None))
        )
    {
        return Err(CompanionError::InvalidInput(
            "请指定模型；跟随全局模型时请将 providerId 和 modelRef 同时设为 null",
        ));
    }
    let old = target(tx, &input.target, Kind::Model)?;
    let new = select_model(
        tx,
        &old.profile.id,
        input.provider_id.flatten().as_deref(),
        input.model_ref.flatten().as_deref(),
        input.model.as_deref(),
    )?;
    record_edit(tx, actor, &old, &new, Kind::Model)
}

fn select_model(
    tx: &Transaction<'_>,
    target_id: &str,
    provider_id: Option<&str>,
    model_ref: Option<&str>,
    model: Option<&str>,
) -> Result<CompanionView> {
    let Some(model) = model else {
        return save_model(tx, target_id, provider_id, model_ref);
    };
    if model_ref.is_some() {
        return Err(CompanionError::InvalidInput(
            "模型名称与模型 ID 只能填写一个",
        ));
    }
    let wanted = model.trim().to_lowercase();
    let mut query = tx.prepare(
        "SELECT m.provider_id, m.id, m.model_id, m.display_name
         FROM provider_models m JOIN model_providers p ON p.id=m.provider_id
         WHERE p.enabled=1 AND (?1 IS NULL OR p.id=?1)",
    )?;
    let rows = query.query_map([provider_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut matches = Vec::new();
    for row in rows {
        let (provider, reference, model_id, display_name) = row?;
        if model_id.to_lowercase() == wanted || display_name.to_lowercase() == wanted {
            matches.push((provider, reference));
        }
    }
    match matches.as_slice() {
        [(provider, reference)] => save_model(tx, target_id, Some(provider), Some(reference)),
        [] => Err(CompanionError::ModelUnavailable(
            "找不到这个已启用的模型，请检查模型名称".into(),
        )),
        _ => Err(CompanionError::ModelUnavailable(
            "多个已启用的模型使用这个名称，请指定模型服务".into(),
        )),
    }
}

fn record_edit(
    tx: &Transaction<'_>,
    actor: &Actor<'_>,
    old: &CompanionView,
    new: &CompanionView,
    kind: Kind,
) -> Result<CompanionTeamAction> {
    let mut before = snapshot(old, kind);
    let mut after = snapshot(new, kind);
    if before == after {
        return Err(CompanionError::InvalidInput("没有需要修改的字段"));
    }
    if kind == Kind::Profile {
        before.retain(|key, value| after.get(key) != Some(value));
        after.retain(|key, _| before.contains_key(key));
    }
    let mut action = actor.record(new, kind, Status::Applied);
    action.before = Some(before);
    action.after = Some(after);
    store(tx, &action)?;
    Ok(action)
}

fn propose(
    tx: &Transaction<'_>,
    root: &Path,
    actor: &Actor<'_>,
    raw: &str,
    kind: Kind,
) -> Result<CompanionTeamAction> {
    let target = target(tx, raw, kind)?;
    require_idle(&target)?;
    let mut action = actor.record(&target, kind, Status::Pending);
    action.preview = if kind == Kind::Delete {
        Some(
            companion_deletions::preview_on(tx, root, &target.profile.id)
                .map_err(deletion_error)?,
        )
    } else {
        None
    };
    store(tx, &action)?;
    Ok(action)
}

fn parse_record(text: &str) -> Result<CompanionTeamAction> {
    serde_json::from_str(text).map_err(|error| CompanionError::Storage(error.to_string()))
}

fn store(tx: &Transaction<'_>, action: &CompanionTeamAction) -> Result<()> {
    let record = serde_json::to_string(action)
        .map_err(|error| CompanionError::Storage(error.to_string()))?;
    tx.execute(
        "INSERT INTO companion_team_actions(id,companion_id,run_id,kind,record_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(id) DO UPDATE SET record_json=excluded.record_json",
        params![
            action.id,
            action.companion_id,
            action.run_id,
            action.kind.key(),
            record,
            action.created_at
        ],
    )?;
    Ok(())
}

fn load(db: &Database, id: &str) -> Result<CompanionTeamAction> {
    let record = db
        .with_connection(|conn| {
            conn.query_row(
                "SELECT record_json FROM companion_team_actions WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .map_err(CompanionError::Storage)?;
    parse_record(&record.ok_or(CompanionError::NotFound)?)
}

pub(crate) fn list(db: &Database, companion_id: &str) -> Result<Vec<CompanionTeamAction>> {
    let records = db.with_connection(|conn| {
        let mut query = conn.prepare(
            "SELECT record_json FROM companion_team_actions WHERE companion_id=?1 ORDER BY created_at, rowid"
        )?;
        let rows = query.query_map([companion_id], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }).map_err(CompanionError::Storage)?;
    records.iter().map(|text| parse_record(text)).collect()
}

pub(crate) fn runtime_actions(
    db: &Database,
    companion_id: &str,
    conversation_id: &str,
) -> Result<Value> {
    db.with_connection(|conn| {
        let scope = "companion_id=?1 AND (json_extract(record_json,'$.conversationId')=?2
            OR run_id IN (SELECT id FROM runs WHERE conversation_id=?2))";
        let total: i64 = conn.query_row(
            &format!("SELECT count(*) FROM companion_team_actions WHERE {scope}"),
            params![companion_id, conversation_id],
            |row| row.get(0),
        )?;
        let mut query = conn.prepare(&format!(
            "SELECT record_json FROM companion_team_actions WHERE {scope}
             ORDER BY json_extract(record_json,'$.updatedAt') DESC,rowid DESC LIMIT 40"
        ))?;
        let records = query.query_map(params![companion_id, conversation_id], |row| row.get::<_, String>(0))?;
        let mut actions = Vec::new();
        for record in records {
            let action: CompanionTeamAction = serde_json::from_str(&record?)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            actions.push(json!({
                "actionId":action.id, "targetId":action.target_id,
                "targetName":safe_label(&action.target_name,40),
                "kind":action.kind, "status":action.status, "updatedAt":action.updated_at,
            }));
        }
        Ok(json!({"totalCount":total,"actions":actions}))
    }).map_err(CompanionError::Storage)
}

pub(crate) fn resolve(
    state: &AppState,
    id: &str,
    decision: Decision,
) -> Result<CompanionTeamAction> {
    let host = &state.runtime_host;
    let _transition = host
        .companion_transition()
        .map_err(CompanionError::Storage)?;
    resolve_on(
        &state.database,
        &state.data_dir,
        id,
        decision,
        |conversation| host.close_conversation_session(conversation),
        |conversation| companion_deletions::purge_sessions(host, conversation),
    )
}

fn resolve_on(
    db: &Database,
    root: &Path,
    id: &str,
    decision: Decision,
    close_session: impl Fn(&str) -> std::result::Result<(), String>,
    purge_sessions: impl FnMut(&str) -> std::result::Result<(), String>,
) -> Result<CompanionTeamAction> {
    let mut action = load(db, id)?;
    action.status = match (action.status, decision, action.kind) {
        (Status::Applied, Decision::Undo, Kind::Profile | Kind::Model) => {
            return db.companion_transaction(|tx| undo_edit(tx, action));
        }
        (Status::Applied, Decision::Undo, Kind::Create) => {
            let preview = db.companion_transaction(|tx| unused_creation(tx, root, &action))?;
            delete_from_preview(db, root, &action.target_id, &preview, purge_sessions)?;
            Status::Undone
        }
        (Status::Pending, Decision::Confirm, Kind::Delete) => {
            let preview = action.preview.as_ref().ok_or_else(stale)?;
            delete_from_preview(db, root, &action.target_id, preview, purge_sessions)?;
            Status::Confirmed
        }
        (Status::Pending, Decision::Confirm, Kind::Archive) => {
            let current = db.companion_profile(&action.target_id)?;
            require_idle(&current)?;
            close_session(&current.conversation_id).map_err(CompanionError::Storage)?;
            for conversation in db
                .companion_background_conversations(&action.target_id)
                .map_err(CompanionError::Storage)?
            {
                close_session(&conversation).map_err(CompanionError::Storage)?;
            }
            db.archive_companion(&action.target_id)?;
            Status::Confirmed
        }
        (Status::Pending, Decision::Dismiss, _) => Status::Dismissed,
        (Status::Undone, Decision::Undo, _)
        | (Status::Confirmed, Decision::Confirm, _)
        | (Status::Dismissed, Decision::Dismiss, _) => return Ok(action),
        _ => return Err(stale()),
    };
    action.updated_at = now_ms();
    db.companion_transaction(|tx| store(tx, &action))?;
    Ok(action)
}

fn stale() -> CompanionError {
    CompanionError::TeamAction(
        "companion.team_action_stale",
        "伙伴已发生变化，无法执行此卡片上的操作",
    )
}

fn deletion_error(error: String) -> CompanionError {
    match error.as_str() {
        "companion.run_active" => CompanionError::RunActive,
        "companion.plan_changed" => {
            CompanionError::TeamAction("companion.plan_changed", "删除清单已变化，请重新提议删除")
        }
        "companion.default_not_deletable" => CompanionError::DefaultProtected(
            "companion.default_not_deletable",
            "小狐是默认伙伴，不能删除",
        ),
        "companion.in_use" => {
            CompanionError::TeamAction("companion.in_use", "定时任务或工作图仍在使用这个私聊")
        }
        _ => CompanionError::Storage(error),
    }
}

fn delete_from_preview(
    db: &Database,
    root: &Path,
    id: &str,
    preview: &Value,
    purge_sessions: impl FnMut(&str) -> std::result::Result<(), String>,
) -> Result<()> {
    let plan_hash = preview
        .get("planHash")
        .and_then(Value::as_str)
        .ok_or_else(stale)?;
    companion_deletions::delete_with_plan_locked(db, root, id, plan_hash, purge_sessions)
        .map_err(deletion_error)?;
    Ok(())
}

fn unused_creation(
    tx: &Transaction<'_>,
    root: &Path,
    action: &CompanionTeamAction,
) -> Result<Value> {
    let current = target(tx, &action.target_id, Kind::Create)?;
    require_idle(&current)?;
    if action.after.as_ref() != Some(&snapshot(&current, Kind::Create)) {
        return Err(stale());
    }
    let used: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE conversation_id=?1)
            OR EXISTS(SELECT 1 FROM companion_tool_grants WHERE companion_id=?2)",
        params![current.conversation_id, current.profile.id],
        |r| r.get(0),
    )?;
    if used {
        return Err(stale());
    }
    let preview =
        companion_deletions::preview_on(tx, root, &action.target_id).map_err(deletion_error)?;
    let plan = &preview["plan"];
    if plan["messages"] != 1
        || plan["generations"] != 0
        || plan["workspaceFiles"] != 0
        || plan["notes"] != 0
        || plan["topics"] != 0
        || plan["sharedMemories"] != 0
        || plan["grantedProjects"] != json!([])
        || plan["files"] != json!([])
        || plan["tasks"] != json!([])
        || plan.get("coordination").is_some()
    {
        return Err(stale());
    }
    Ok(preview)
}

fn undo_edit(tx: &Transaction<'_>, mut action: CompanionTeamAction) -> Result<CompanionTeamAction> {
    let current = target(tx, &action.target_id, action.kind)?;
    let snapshot = snapshot(&current, action.kind);
    let after = action.after.as_ref().ok_or_else(stale)?;
    if after
        .iter()
        .any(|(key, value)| snapshot.get(key) != Some(value))
    {
        return Err(stale());
    }
    let before = action.before.as_ref().ok_or_else(stale)?;
    let string = |key| before.get(key).and_then(Value::as_str);
    if action.kind == Kind::Profile {
        let restored_instructions = before
            .get("instructions")
            .map(|value| value.as_str().unwrap_or("").to_owned());
        save_profile(
            tx,
            &action.target_id,
            profile_edit(
                string("name").map(str::to_owned),
                string("duty").map(str::to_owned),
                restored_instructions,
            ),
        )?;
    } else {
        save_model(
            tx,
            &action.target_id,
            string("providerId"),
            string("modelRef"),
        )?;
    }
    action.status = Status::Undone;
    action.updated_at = now_ms();
    store(tx, &action)?;
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::super::CompanionProfileEdit;
    use super::*;

    struct Fixture {
        db: Database,
        root: std::path::PathBuf,
        owner: CompanionView,
        run: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fox-companion-team-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let db = Database::open(root.join("fox.db")).unwrap();
            let owner = db.seed_default_companion(&root).unwrap();
            let run = insert_running(&db, &owner.conversation_id);
            Self {
                db,
                root,
                owner,
                run,
            }
        }

        fn tool(&self, tool: &str, input: Value) -> Result<Value> {
            execute(
                &self.db,
                &self.root,
                &self.owner.profile.id,
                &self.owner.conversation_id,
                &self.run,
                tool,
                input,
            )
        }

        fn create(&self, name: &str) -> CompanionTeamAction {
            let value = self
                .tool(
                    "companion_create",
                    json!({"name": name, "duty": "整理周报"}),
                )
                .unwrap();
            assert_eq!(value["workspaceReady"], true);
            load(&self.db, value["action"]["id"].as_str().unwrap()).unwrap()
        }

        fn resolve(&self, id: &str, decision: Decision) -> Result<CompanionTeamAction> {
            // This fixture has no live runtime or session mappings.
            resolve_on(&self.db, &self.root, id, decision, |_| Ok(()), |_| Ok(()))
        }

        fn clean(self) {
            drop(self.db);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    #[test]
    fn runtime_action_states_follow_undo_and_dismiss_without_leaking_preview_or_other_chats() {
        let f = Fixture::new();
        let created = f.create("验收核对员");
        assert_eq!(receipt(&created)["actionId"], created.id);
        assert_eq!(receipt(&created)["targetId"], created.target_id);
        assert_ne!(created.id, created.target_id);
        f.resolve(&created.id, Decision::Undo).unwrap();
        let recreated = f.create("验收核对员");
        assert_ne!(created.target_id, recreated.target_id);
        let edited = f.tool("companion_propose_profile", json!({"target":recreated.target_id,"name":"验收数据员"})).unwrap();
        f.resolve(edited["action"]["actionId"].as_str().unwrap(), Decision::Undo).unwrap();
        let mut resolved = vec![created.id.clone(), edited["action"]["actionId"].as_str().unwrap().to_owned()];
        for tool in ["companion_propose_archive", "companion_propose_deletion"] {
            let first = f.tool(tool, json!({"target":recreated.target_id})).unwrap();
            let id = first["action"]["actionId"].as_str().unwrap();
            f.resolve(id, Decision::Dismiss).unwrap();
            f.resolve(id, Decision::Dismiss).unwrap();
            resolved.push(id.to_owned());
            let next = f.tool(tool, json!({"target":recreated.target_id})).unwrap();
            assert_ne!(next["action"]["actionId"], first["action"]["actionId"]);
        }
        let reopened = Database::open(f.root.join("fox.db")).unwrap();
        let state = runtime_actions(&reopened, &f.owner.profile.id, &f.owner.conversation_id).unwrap();
        let actions = state["actions"].as_array().unwrap();
        for (index, id) in resolved.iter().enumerate() {
            let action = actions.iter().find(|item| item["actionId"] == *id).unwrap();
            assert_eq!(action["status"], if index < 2 { "undone" } else { "dismissed" });
            assert!(action.get("preview").is_none());
            assert!(action.get("after").is_none());
            assert!(action.get("before").is_none());
        }
        assert!(state.to_string().find(&f.root.to_string_lossy().to_string()).is_none());
        assert_eq!(runtime_actions(&reopened, &f.owner.profile.id, "other-chat").unwrap()["totalCount"], 0);
        assert_eq!(runtime_actions(&reopened, "other-owner", &f.owner.conversation_id).unwrap()["totalCount"], 0);
        drop(reopened);
        f.clean();
    }

    fn insert_running(db: &Database, chat: &str) -> String {
        let id = Uuid::new_v4().to_string();
        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO runs(id,conversation_id,status,model,last_seq,created_at)
                VALUES(?1,?2,'running','test-model',1,1)",
                params![id, chat],
            )?;
            Ok(())
        })
        .unwrap();
        id
    }

    #[test]
    fn companion_team_create_is_atomic_durable_and_capped() {
        let f = Fixture::new();
        let first = f.create("Weekly Scout");
        let peer = f.db.companion_profile(&first.target_id).unwrap();
        assert_eq!(peer.profile.duty, "整理周报");
        assert_eq!(peer.profile.workspace_state, "ready");
        assert!(f
            .db
            .companion_tool_grants(&first.target_id)
            .unwrap()
            .is_empty());
        let greeting = f.db.load_conversation(&peer.conversation_id).unwrap();
        assert!(greeting.messages[0].content.contains("Weekly Scout"));
        assert_eq!(first.status, Status::Applied);
        assert!(f
            .db
            .companion_transaction(|tx| unused_creation(tx, &f.root, &first))
            .is_ok());
        for (input, error) in [
            (
                json!({"name":"weekly scout","duty":"重复"}),
                "companion.name_conflict",
            ),
            (
                json!({"name":"无权限扩张","duty":"职责","permissions":"full"}),
                "companion.invalid_profile",
            ),
            (
                json!({"name":"太长","duty":"职责","instructions":"字".repeat(4_001)}),
                "companion.invalid_profile",
            ),
            (
                json!({"name":"错误模型","duty":"职责","providerId":"unknown","modelRef":"unknown"}),
                "companion.model_unavailable",
            ),
        ] {
            assert_eq!(
                f.tool("companion_create", input).unwrap_err().response().0,
                error
            );
        }
        assert_eq!(f.db.list_companions().unwrap().companions.len(), 2);
        for name in ["二", "三", "四"] {
            f.create(name);
        }
        let err = f
            .tool("companion_create", json!({"name":"五","duty":"职责"}))
            .unwrap_err();
        assert_eq!(err.response().0, "companion.team_turn_limit");
        f.db.with_connection(|conn| {
            conn.execute("UPDATE runs SET status='cancelled' WHERE id=?1", [&f.run])?;
            Ok(())
        })
        .unwrap();
        let expired = f
            .tool(
                "companion_propose_profile",
                json!({"target": first.target_id, "name": "不应写入"}),
            )
            .unwrap_err();
        assert_eq!(expired.response().0, "companion.team_tool_denied");
        assert_eq!(
            f.db.companion_profile(&first.target_id)
                .unwrap()
                .profile
                .display_name,
            "Weekly Scout"
        );
        let reopened = Database::open(f.root.join("fox.db")).unwrap();
        let cards = list(&reopened, &f.owner.profile.id).unwrap();
        assert_eq!(cards.len(), 4);
        assert_eq!(cards[0].id, first.id);
        assert_eq!(cards[0].after.as_ref().unwrap()["name"], "Weekly Scout");
        drop(reopened);
        f.clean();
    }

    #[test]
    fn companion_team_profile_edit_undo_restores_only_changed_fields() {
        let f = Fixture::new();
        let created = f.create("Weekly Scout");
        let target = &created.target_id;
        let edited = f
            .tool(
                "companion_propose_profile",
                json!({
                    "target":"@weekly_scout","name":"新的名字","instructions":"每周五更新"
                }),
            )
            .unwrap();
        let id = edited["action"]["id"].as_str().unwrap();
        let saved = f.db.companion_profile(target).unwrap();
        assert_eq!(saved.profile.display_name, "新的名字");
        assert_eq!(
            saved.profile.persona_override.as_deref(),
            Some("每周五更新")
        );
        assert_eq!(edited["action"]["after"]["name"], "新的名字");
        assert!(edited["action"].get("before").is_none());
        let card = load(&f.db, id).unwrap();
        assert_eq!(card.before.as_ref().unwrap()["name"], "Weekly Scout");
        assert_eq!(card.before.as_ref().unwrap()["instructions"], Value::Null);
        assert!(!card.before.as_ref().unwrap().contains_key("duty"));
        f.db.edit_companion(
            target,
            &CompanionProfileEdit {
                display_name: None,
                avatar_ref: None,
                duty: Some("用户的新职责".into()),
                persona_override: None,
            },
        )
        .unwrap();
        let undone =
            f.db.companion_transaction(|tx| undo_edit(tx, card))
                .unwrap();
        assert_eq!(undone.status, Status::Undone);
        let restored = f.db.companion_profile(target).unwrap();
        assert_eq!(restored.profile.display_name, "Weekly Scout");
        assert_eq!(restored.profile.duty, "用户的新职责");
        assert!(restored.profile.persona_override.is_none());
        let stale_card = f
            .tool(
                "companion_propose_profile",
                json!({"target":target,"name":"名称二"}),
            )
            .unwrap();
        f.db.edit_companion(
            target,
            &CompanionProfileEdit {
                display_name: Some("用户的名字".into()),
                avatar_ref: None,
                duty: None,
                persona_override: None,
            },
        )
        .unwrap();
        let stale_card = load(&f.db, stale_card["action"]["id"].as_str().unwrap()).unwrap();
        let err =
            f.db.companion_transaction(|tx| undo_edit(tx, stale_card))
                .unwrap_err();
        assert_eq!(err.response().0, "companion.team_action_stale");
        f.clean();
    }

    #[test]
    fn companion_team_resolves_model_name_and_requires_explicit_reset() {
        let f = Fixture::new();
        let target = f.create("周报助手").target_id;
        f.db.execute_raw_sql(
            "INSERT INTO model_providers(id,name,base_url,api_type,enabled,is_default,created_at,updated_at)
            VALUES ('provider','模型服务','https://independent.example.com','openai-completions',1,0,1,1);
            INSERT INTO provider_models(id,provider_id,model_id,display_name,context_window,max_output_tokens,supports_image_input,is_default,created_at,updated_at)
            VALUES ('selected','provider','test-model','模型甲',64000,4096,1,1,1,1);"
        ).unwrap();
        f.tool(
            "companion_propose_model",
            json!({"target":target,"model":"模型甲"}),
        )
        .unwrap();
        assert_eq!(
            f.db.companion_profile(&target)
                .unwrap()
                .profile
                .model_ref
                .as_deref(),
            Some("selected")
        );
        let omitted = f
            .tool("companion_propose_model", json!({"target":target}))
            .unwrap_err();
        assert_eq!(omitted.response().0, "companion.invalid_profile");
        let reset = f
            .tool(
                "companion_propose_model",
                json!({"target":target,"providerId":null,"modelRef":null}),
            )
            .unwrap();
        assert!(f
            .db
            .companion_profile(&target)
            .unwrap()
            .profile
            .model_ref
            .is_none());
        let card = load(&f.db, reset["action"]["id"].as_str().unwrap()).unwrap();
        f.db.companion_transaction(|tx| undo_edit(tx, card))
            .unwrap();
        assert_eq!(
            f.db.companion_profile(&target)
                .unwrap()
                .profile
                .model_ref
                .as_deref(),
            Some("selected")
        );
        f.clean();
    }

    #[test]
    fn companion_team_proposals_are_pending_protect_default_and_refuse_busy_targets() {
        let f = Fixture::new();
        let target = f.create("周报助手");
        let delete = f
            .tool(
                "companion_propose_deletion",
                json!({"target":target.target_id}),
            )
            .unwrap();
        let card = load(&f.db, delete["action"]["id"].as_str().unwrap()).unwrap();
        assert_eq!(card.status, Status::Pending);
        assert!(card.preview.unwrap()["planHash"].as_str().is_some());
        let archive = f
            .tool(
                "companion_propose_archive",
                json!({"target":target.target_id}),
            )
            .unwrap();
        assert_eq!(archive["action"]["status"], "pending");
        let peer = f.db.companion_profile(&target.target_id).unwrap();
        assert!(peer.profile.archived_at.is_none());
        let peer_run = insert_running(&f.db, &peer.conversation_id);
        assert_eq!(
            f.tool(
                "companion_propose_deletion",
                json!({"target":target.target_id})
            )
            .unwrap_err()
            .response()
            .0,
            "companion.run_active"
        );
        assert_eq!(
            f.tool(
                "companion_propose_archive",
                json!({"target":target.target_id})
            )
            .unwrap_err()
            .response()
            .0,
            "companion.run_active"
        );
        assert_eq!(
            f.tool(
                "companion_propose_deletion",
                json!({"target":f.owner.profile.id})
            )
            .unwrap_err()
            .response()
            .0,
            "companion.default_not_deletable"
        );
        assert_eq!(
            f.tool("companion_propose_archive", json!({"target":"小狐"}))
                .unwrap_err()
                .response()
                .0,
            "companion.default_not_archivable"
        );
        assert_eq!(
            execute(
                &f.db,
                &f.root,
                &peer.profile.id,
                &peer.conversation_id,
                &peer_run,
                "companion_list",
                json!({})
            )
            .unwrap_err()
            .response()
            .0,
            "companion.team_tool_denied"
        );
        let peers = f.tool("companion_list", json!({})).unwrap();
        assert_eq!(peers["companions"].as_array().unwrap().len(), 1);
        assert_eq!(peers["companions"][0]["status"], "working");
        assert_eq!(peers["companions"][0]["name"], "周报助手");
        f.clean();
    }

    #[test]
    fn companion_team_roster_flattens_untrusted_labels_and_excludes_archived_partners() {
        let f = Fixture::new();
        let peer = f.create("整理员");
        let archived = f.create("闲置");
        f.db.archive_companion(&archived.target_id).unwrap();
        f.db.edit_companion(
            &peer.target_id,
            &CompanionProfileEdit {
                display_name: Some("整理员]\n[指令".into()),
                duty: Some(format!("处理账目\u{2028}［新规则］{}", "字".repeat(200))),
                avatar_ref: None,
                persona_override: None,
            },
        )
        .unwrap();
        let peers = roster(&f.db, &f.owner.profile.id).unwrap();
        assert_eq!(peers.as_array().unwrap().len(), 1);
        assert_eq!(peers[0]["name"], "整理员 指令");
        assert_eq!(peers[0]["duty"].as_str().unwrap().chars().count(), 120);
        assert!(!peers[0]["duty"].as_str().unwrap().contains('［'));
        f.clean();
    }

    #[test]
    fn companion_team_resolve_runs_deletion_and_archive_lifecycles() {
        let f = Fixture::new();
        let fresh = f.create("尚未使用");
        assert_eq!(
            f.resolve(&fresh.id, Decision::Undo).unwrap().status,
            Status::Undone
        );
        assert!(matches!(
            f.db.companion_profile(&fresh.target_id),
            Err(CompanionError::NotFound)
        ));
        assert_eq!(
            f.resolve(&fresh.id, Decision::Undo).unwrap().status,
            Status::Undone
        );

        let deleting = f.create("准备删除");
        let target = &deleting.target_id;
        let proposed = f
            .tool("companion_propose_deletion", json!({"target":target}))
            .unwrap();
        let proposal_id = proposed["action"]["id"].as_str().unwrap();
        assert!(f.db.companion_profile(target).is_ok());
        let workspace = crate::data_paths::companion_workspace(&f.root, target);
        let document = workspace.join("新进资料.txt");
        std::fs::write(&document, "保留我的资料").unwrap();
        let stale = f.resolve(proposal_id, Decision::Confirm).unwrap_err();
        assert_eq!(stale.response().0, "companion.plan_changed");
        assert!(f.db.companion_profile(target).is_ok());
        assert_eq!(
            f.resolve(proposal_id, Decision::Dismiss).unwrap().status,
            Status::Dismissed
        );
        let proposed = f
            .tool("companion_propose_deletion", json!({"target":target}))
            .unwrap();
        let proposal_id = proposed["action"]["id"].as_str().unwrap();
        assert_eq!(
            f.resolve(proposal_id, Decision::Confirm).unwrap().status,
            Status::Confirmed
        );
        assert!(matches!(
            f.db.companion_profile(target),
            Err(CompanionError::NotFound)
        ));
        assert_eq!(std::fs::read_to_string(document).unwrap(), "保留我的资料");

        let archiving = f.create("准备归档");
        let target = &archiving.target_id;
        let proposed = f
            .tool("companion_propose_archive", json!({"target":target}))
            .unwrap();
        let proposal_id = proposed["action"]["id"].as_str().unwrap();
        let peer = f.db.companion_profile(target).unwrap();
        assert!(peer.profile.archived_at.is_none());
        let new_run = insert_running(&f.db, &peer.conversation_id);
        assert_eq!(
            f.resolve(proposal_id, Decision::Confirm)
                .unwrap_err()
                .response()
                .0,
            "companion.run_active"
        );
        f.db.with_connection(|conn| {
            conn.execute("UPDATE runs SET status='completed' WHERE id=?1", [&new_run])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            f.resolve(proposal_id, Decision::Confirm).unwrap().status,
            Status::Confirmed
        );
        assert!(f
            .db
            .companion_profile(target)
            .unwrap()
            .profile
            .archived_at
            .is_some());
        f.clean();
    }

    #[test]
    fn companion_team_background_card_uses_the_default_private_chat() {
        let f = Fixture::new();
        let task =
            f.db.create_digital_colleague(&crate::database::CreateDigitalColleagueInput {
                name: "定时整理".into(),
                expert_id: "fox-security".into(),
                objective: "整理".into(),
                project_id: None,
                project_root: None,
                knowledge_references: Vec::new(),
                max_runs_per_day: 1,
                max_tokens_per_day: 1000,
                max_duration_ms: 1000,
                max_output_tokens: 100,
                max_tool_calls: 1,
                companion_id: Some(f.owner.profile.id.clone()),
            })
            .unwrap();
        let run = insert_running(&f.db, &task.conversation_id);
        let result = execute(
            &f.db,
            &f.root,
            &f.owner.profile.id,
            &task.conversation_id,
            &run,
            "companion_create",
            json!({"name":"新的助手","duty":"整理资料"}),
        )
        .unwrap();
        let card = load(&f.db, result["action"]["id"].as_str().unwrap()).unwrap();
        assert_eq!(card.conversation_id, f.owner.conversation_id);
        assert_eq!(card.run_id, run);
        f.clean();
    }
}

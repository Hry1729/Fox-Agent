use super::{
    activate_expert_binding, agent_package_snapshot, now_ms, package_snapshot_hash,
    query_agent_record, query_conversation_expert_binding_by_state, Database, DEFAULT_AGENT_ID,
};
use crate::data_paths::{companion_notes_dir, companion_workspace};
use crate::database::{
    AgentRecord, CompanionProfile, ContactBubbleMarkRecord, ContactFavoriteEntry, ContactQuotePreview,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) enum CompanionError {
    NotFound,
    ExpertOnly,
    Archived,
    Deleting,
    RunActive,
    InvalidName,
    NameTaken,
    TemplateChanged,
    BuiltinTemplate,
    ModelUnavailable(String),
    ReminderModeInvalid,
    BubbleNotFound,
    BackfillPending,
    /// C07: no committed bubble of this chat carries that identity yet (live
    /// draft, replaceable preview, foreign chat, or an unknown id).
    BubbleNotDurable,
    NotContactChat,
    ReactionInvalid,
    Storage(String),
}

impl CompanionError {
    pub(crate) fn response(&self) -> (&'static str, &str) {
        match self {
            Self::NotFound => ("companion.not_found", "没有找到联系人或模板"),
            Self::ExpertOnly => ("companion.local_expert_required", "只能添加本机专家"),
            Self::Archived => ("companion.archived", "请先恢复联系人"),
            Self::Deleting => ("companion.deleting", "联系人正在删除"),
            Self::RunActive => ("companion.run_active", "请先停止当前生成"),
            Self::InvalidName => ("companion.invalid_name", "名字需要包含 1 至 40 个字符"),
            Self::NameTaken => ("companion.name_conflict", "已有联系人使用这个名字"),
            Self::TemplateChanged => ("companion.template_changed", "专家模板已变化，请重新查看"),
            Self::BuiltinTemplate => ("companion.builtin_template", "小狐的模板随 Fox 更新"),
            Self::ModelUnavailable(message) => ("companion.model_unavailable", message.as_str()),
            Self::ReminderModeInvalid => ("companion.reminder_mode_invalid", "未知的提醒模式"),
            Self::BubbleNotFound => ("companion.bubble_not_found", "这条消息还不能标为未读"),
            Self::BackfillPending => ("companion.backfill_pending", "私聊历史仍在整理，稍后再试"),
            Self::BubbleNotDurable => ("companion.bubble_not_durable", "这条消息还没有保存，暂时不能引用、收藏或加表情"),
            Self::NotContactChat => ("companion.not_private_chat", "只有联系人私聊支持引用和消息标记"),
            Self::ReactionInvalid => ("companion.reaction_invalid", "未知的表情"),
            Self::Storage(message) => ("storage.error", message),
        }
    }
}

impl From<rusqlite::Error> for CompanionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

type Result<T> = std::result::Result<T, CompanionError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionView {
    #[serde(flatten)]
    pub profile: CompanionProfile,
    pub conversation_id: String,
    pub latest_message_preview: Option<String>,
    pub unread_count: i64,
    pub active_run_status: Option<String>,
    /// K6: the effective bubble read position (converted legacy cursor included).
    pub read_delivery_order: i64,
    /// K6: outstanding approvals + questions across the contact's serial range.
    pub pending_action_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingDeletion {
    companion_id: String,
    display_name: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionsList {
    pub companions: Vec<CompanionView>,
    pub pending_deletions: Vec<PendingDeletion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionProfileEdit {
    pub display_name: Option<String>,
    pub avatar_ref: Option<String>,
    pub duty: Option<String>,
    // An empty override restores the template persona.
    pub persona_override: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTemplateStatus {
    pub bound_hash: Option<String>,
    pub latest_hash: Option<String>,
    pub bound_snapshot: Option<Value>,
    pub latest_snapshot: Option<Value>,
    pub update_available: bool,
    pub dismissed: bool,
}

/// K6: the unread unit is the K1-persisted companion bubble, not the settled
/// assistant message. The effective read position is the converted delivery
/// cursor; before the chat backfill completes (cursor still NULL) it falls back
/// to the legacy ordinal mapped onto bubble delivery order, so a contact never
/// shows a wrong unread count while its history is still being organised.
const EFFECTIVE_READ: &str = "coalesce(p.last_read_delivery_order,
        (SELECT max(b.delivery_order) FROM contact_bubbles b
            WHERE b.conversation_id=c.id AND b.source_ordinal<=p.last_read_ordinal), 0)";

/// K2: the active Run probe covers the contact's whole serial range — the fixed
/// private chat and every bound background execution conversation — so archive,
/// restore and delete cannot see an idle contact while a task is still running.
fn view_sql() -> String {
    format!(
        "SELECT p.*, c.id AS conversation_id,
            (SELECT substr(ltrim(b.content, char(9,10,13,32)), 1, 240)
                FROM contact_bubbles b WHERE b.conversation_id=c.id
                ORDER BY b.delivery_order DESC LIMIT 1) AS latest_message_preview,
            ({effective}) AS read_delivery_order,
            (SELECT count(*) FROM contact_bubbles b WHERE b.conversation_id=c.id AND b.author='companion'
                AND (b.delivery_order > ({effective})
                  OR (p.manual_unread_from_order IS NOT NULL AND b.delivery_order>=p.manual_unread_from_order))) AS unread_count,
            (SELECT r.status FROM runs r
                WHERE r.status IN ('queued','running','cancelling','awaiting_confirmation')
                  AND (r.conversation_id=c.id OR r.conversation_id IN (
                      SELECT dc.conversation_id FROM digital_colleagues dc
                      WHERE dc.companion_id=p.id AND dc.companion_bound=1))
                ORDER BY r.created_at DESC LIMIT 1) AS active_run_status
            FROM companion_profiles p JOIN conversations c ON c.companion_id=p.id",
        effective = EFFECTIVE_READ,
    )
}

fn view(row: &rusqlite::Row<'_>) -> rusqlite::Result<CompanionView> {
    Ok(CompanionView {
        profile: CompanionProfile {
            id: row.get("id")?,
            template_agent_id: row.get("template_agent_id")?,
            display_name: row.get("display_name")?,
            avatar_ref: row.get("avatar_ref")?,
            duty: row.get("duty")?,
            persona_override: row.get("persona_override")?,
            workspace_dir: row.get("workspace_dir")?,
            notes_dir: row.get("notes_dir")?,
            workspace_state: row.get("workspace_state")?,
            workspace_error: row.get("workspace_error")?,
            is_default: row.get("is_default")?,
            pinned_at: row.get("pinned_at")?,
            last_read_ordinal: row.get("last_read_ordinal")?,
            template_notice_dismissed_hash: row.get("template_notice_dismissed_hash")?,
            model_provider_id: row.get("model_provider_id")?,
            model_ref: row.get("model_ref")?,
            reminder_mode: row.get("reminder_mode")?,
            last_read_delivery_order: row.get("last_read_delivery_order")?,
            manual_unread_from_order: row.get("manual_unread_from_order")?,
            manual_unread_at: row.get("manual_unread_at")?,
            archived_at: row.get("archived_at")?,
            deleting_at: row.get("deleting_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        },
        conversation_id: row.get("conversation_id")?,
        latest_message_preview: row.get("latest_message_preview")?,
        unread_count: row.get("unread_count")?,
        active_run_status: row.get("active_run_status")?,
        read_delivery_order: row.get("read_delivery_order")?,
        pending_action_count: 0,
    })
}

/// K6 upgrade: convert a legacy `last_read_ordinal` into a bubble delivery-order
/// read position, but only once the chat's K1 backfill is complete (otherwise the
/// mapping would be premature). Idempotent — runs at migration and again when a
/// backfill finishes. Until converted, the VIEW computes the same position on the
/// fly, so the unread count is correct even mid-backfill.
pub(crate) fn convert_pending_read_cursors(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE companion_profiles SET last_read_delivery_order = coalesce((
            SELECT max(b.delivery_order) FROM contact_bubbles b
            JOIN conversations c ON c.id = b.conversation_id
            WHERE c.companion_id = companion_profiles.id
                AND b.source_ordinal <= companion_profiles.last_read_ordinal), 0)
        WHERE last_read_delivery_order IS NULL AND EXISTS (
            SELECT 1 FROM contact_bubble_backfills f
            JOIN conversations c ON c.id = f.conversation_id
            WHERE c.companion_id = companion_profiles.id)",
        [],
    )?;
    Ok(())
}

/// K6: outstanding approvals + questions across each contact's serial range (the
/// fixed private chat plus bound background execution conversations). Mirrors the
/// pending-question detection in `apply_active_run_activity`: a question stays
/// pending until it is answered, the run terminates, or a later unrelated user
/// message supersedes it.
const PENDING_ACTIONS_SQL: &str = "
WITH contact_chats AS (
    SELECT p.id AS companion_id, c.id AS conversation_id
    FROM companion_profiles p JOIN conversations c ON c.companion_id = p.id
    UNION ALL
    SELECT dc.companion_id, dc.conversation_id
    FROM digital_colleagues dc
    WHERE dc.companion_bound = 1 AND dc.companion_id IS NOT NULL
),
approval_counts AS (
    SELECT cc.companion_id AS companion_id, count(*) AS n
    FROM contact_chats cc
    JOIN runs r ON r.conversation_id = cc.conversation_id
        AND r.status IN ('queued','running','cancelling','awaiting_confirmation')
    JOIN tool_calls t ON t.run_id = r.id
    JOIN approvals a ON a.tool_call_id = t.id AND a.status = 'pending'
    GROUP BY cc.companion_id
),
question_events AS (
    SELECT run_id,
        MAX(CASE WHEN event_type = 'user.question.requested'
            AND CASE WHEN json_valid(event_json) AND json_type(event_json,'$.questions')='array'
                THEN EXISTS(SELECT 1 FROM json_each(event_json,'$.questions') q
                    WHERE CASE WHEN q.type='object' THEN json_type(q.value,'$.question')='text'
                        AND length(trim(json_extract(q.value,'$.question')))>0 ELSE 0 END)
                ELSE 0 END
            THEN seq END) AS requested,
        MAX(CASE WHEN event_type IN ('user.question.responded','run.failed','run.cancelled','run.interrupted')
            OR (event_type='run.completed' AND coalesce(json_extract(
                CASE WHEN json_valid(event_json) THEN event_json ELSE '{}' END,'$.completionReason'),'') <> 'awaiting_user')
            THEN seq END) AS responded
    FROM run_events
    WHERE run_id IN (SELECT r.id FROM runs r JOIN contact_chats cc ON cc.conversation_id=r.conversation_id)
        AND event_type IN ('user.question.requested','user.question.responded','run.completed','run.failed','run.cancelled','run.interrupted')
    GROUP BY run_id
),
question_counts AS (
    SELECT cc.companion_id AS companion_id, count(*) AS n
    FROM question_events q
    JOIN runs r ON r.id = q.run_id
    JOIN contact_chats cc ON cc.conversation_id = r.conversation_id
    JOIN run_events requested ON requested.run_id = q.run_id AND requested.seq = q.requested
    WHERE r.status NOT IN ('failed','cancelled','interrupted')
        AND coalesce(q.requested,-1) > coalesce(q.responded,-1)
        AND NOT EXISTS (
            SELECT 1 FROM messages m WHERE m.conversation_id = r.conversation_id
                AND m.role='user' AND m.created_at > requested.created_at
                AND (m.run_id IS NULL OR m.run_id <> r.id)
                AND NOT EXISTS (
                    SELECT 1 FROM json_each(requested.event_json,'$.questions') question
                    WHERE CASE WHEN question.type='object' AND json_type(question.value,'$.question')='text'
                        AND length(trim(json_extract(question.value,'$.question')))>0
                        THEN instr(m.content, trim(json_extract(question.value,'$.question')) || '：') = 0
                            AND instr(m.content, trim(json_extract(question.value,'$.question')) || ':') = 0
                        ELSE 0 END
                )
        )
    GROUP BY cc.companion_id
)
SELECT companion_id, sum(n) FROM (
    SELECT companion_id, n FROM approval_counts
    UNION ALL
    SELECT companion_id, n FROM question_counts
) GROUP BY companion_id";

fn pending_action_counts(conn: &Connection) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare(PENDING_ACTIONS_SQL)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
    rows.collect()
}

fn profile(conn: &Connection, id: &str) -> Result<CompanionView> {
    let record = conn
        .query_row(&format!("{} WHERE p.id=?1", view_sql()), [id], view)
        .optional()?
        .ok_or(CompanionError::NotFound)?;
    if record.profile.deleting_at.is_some() {
        return Err(CompanionError::Deleting);
    }
    Ok(record)
}

fn name_taken(conn: &Connection, name: &str, id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM companion_profiles WHERE
        archived_at IS NULL AND deleting_at IS NULL AND lower(trim(display_name))=lower(trim(?1))
        AND id<>?2)",
        params![name, id],
        |r| r.get(0),
    )
}

fn available_name(conn: &Connection, desired: &str, id: &str) -> rusqlite::Result<String> {
    let trimmed = desired.trim();
    let base = if trimmed.is_empty() {
        "小 Fox"
    } else {
        trimmed
    };
    let mut name: String = base.chars().take(40).collect();
    let mut index = 2;
    while name_taken(conn, &name, id)? {
        let suffix = format!("（{index}）");
        let prefix: String = base.chars().take(40 - suffix.chars().count()).collect();
        name = format!("{prefix}{suffix}");
        index += 1;
    }
    Ok(name)
}

fn require_idle(record: &CompanionView) -> Result<()> {
    if record.active_run_status.is_some() {
        Err(CompanionError::RunActive)
    } else {
        Ok(())
    }
}

fn restore(tx: &Transaction<'_>, record: CompanionView) -> Result<CompanionView> {
    if record.profile.archived_at.is_none() {
        return Ok(record);
    }
    require_idle(&record)?;
    let id = &record.profile.id;
    let name = available_name(tx, &record.profile.display_name, id)?;
    tx.execute(
        "UPDATE companion_profiles SET archived_at=NULL, display_name=?2, updated_at=?3
        WHERE id=?1",
        params![id, name, now_ms()],
    )?;
    tx.execute(
        "UPDATE conversations SET title=?2 WHERE id=?1",
        params![record.conversation_id, name],
    )?;
    profile(tx, id)
}

fn local_expert(conn: &Connection, id: &str) -> Result<AgentRecord> {
    let agent = query_agent_record(conn, id)?.ok_or(CompanionError::NotFound)?;
    if (
        agent.runtime_type.as_str(),
        agent.agent_kind.as_str(),
        agent.invocation_mode.as_str(),
        agent.visibility.as_str(),
    ) != ("pi", "expert", "inline", "expert_center")
    {
        return Err(CompanionError::ExpertOnly);
    }
    Ok(agent)
}

impl Database {
    fn companion_transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| CompanionError::Storage("数据库锁已损坏".into()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn list_companions(&self) -> Result<CompanionsList> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| CompanionError::Storage("数据库锁已损坏".into()))?;
        let mut query = conn.prepare(&format!(
            "{} ORDER BY p.pinned_at DESC,
            coalesce(c.last_message_at,p.created_at) DESC, p.id",
            view_sql()
        ))?;
        let mut companions: Vec<CompanionView> = query
            .query_map([], view)?
            .collect::<rusqlite::Result<_>>()?;
        // K6: the sidebar's "需要你确认" marker counts outstanding approvals and
        // questions. Computed once for the whole list rather than per profile().
        let pending = pending_action_counts(&conn)?;
        for companion in &mut companions {
            companion.pending_action_count =
                pending.get(&companion.profile.id).copied().unwrap_or(0);
        }
        let mut query = conn.prepare(
            "SELECT companion_id,display_name,error FROM companion_deletions
            WHERE state='deleting' ORDER BY started_at",
        )?;
        let pending_deletions = query
            .query_map([], |r| {
                Ok(PendingDeletion {
                    companion_id: r.get(0)?,
                    display_name: r.get(1)?,
                    error: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(CompanionsList {
            companions,
            pending_deletions,
        })
    }

    pub(crate) fn companion_profile(&self, id: &str) -> Result<CompanionView> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| CompanionError::Storage("数据库锁已损坏".into()))?;
        profile(&conn, id)
    }

    pub(crate) fn add_companion(&self, root: &Path, expert_id: &str) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let deleting = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM companion_deletions
                WHERE template_agent_id=?1 AND state='deleting')",
                [expert_id],
                |r| r.get::<_, bool>(0),
            )?;
            if deleting {
                return Err(CompanionError::Deleting);
            }
            let expert = local_expert(tx, expert_id)?;
            let existing = tx
                .query_row(
                    "SELECT id FROM companion_profiles WHERE template_agent_id=?1",
                    [expert_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            if let Some(id) = existing {
                return restore(tx, profile(tx, &id)?);
            }
            create_companion(tx, root, Some(&expert))
        })
    }

    pub(crate) fn seed_default_companion(&self, root: &Path) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let seeded = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_settings
                WHERE key='companions.default_seeded')",
                [],
                |r| r.get::<_, bool>(0),
            )?;
            let id = tx
                .query_row(
                    "SELECT id FROM companion_profiles WHERE is_default=1",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            let record = match id {
                Some(id) => profile(tx, &id)?,
                None if seeded => {
                    return Err(CompanionError::Storage(
                        "小狐的初始化标记已有记录，但身份缺失".into(),
                    ))
                }
                None => create_companion(tx, root, None)?,
            };
            if !seeded {
                tx.execute(
                    "INSERT INTO app_settings(key,value,updated_at)
                    VALUES ('companions.default_seeded','true',?1)",
                    [now_ms()],
                )?;
            }
            Ok(record)
        })
    }

    pub(crate) fn edit_companion(
        &self,
        id: &str,
        edit: &CompanionProfileEdit,
    ) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            let name = edit.display_name.as_deref().map(str::trim);
            if let Some(name) = name {
                if !(1..=40).contains(&name.chars().count()) {
                    return Err(CompanionError::InvalidName);
                }
                if current.profile.archived_at.is_none() && name_taken(tx, name, id)? {
                    return Err(CompanionError::NameTaken);
                }
                tx.execute(
                    "UPDATE conversations SET title=?2 WHERE id=?1",
                    params![current.conversation_id, name],
                )?;
            }
            tx.execute(
                "UPDATE companion_profiles SET display_name=coalesce(?2,display_name),
                avatar_ref=coalesce(?3,avatar_ref), duty=coalesce(?4,duty),
                persona_override=CASE WHEN ?5 IS NULL THEN persona_override ELSE nullif(?5,'') END,
                updated_at=?6 WHERE id=?1",
                params![
                    id,
                    name,
                    edit.avatar_ref,
                    edit.duty,
                    edit.persona_override,
                    now_ms()
                ],
            )?;
            profile(tx, id)
        })
    }

    /// C04: choose the companion's independent model, or NULL/NULL to follow
    /// the global model again. The choice is validated against the configured
    /// providers at save time; later drift surfaces at send time instead of
    /// silently switching models. Only affects runs created afterwards.
    pub(crate) fn set_companion_model(
        &self,
        id: &str,
        provider_id: Option<&str>,
        model_ref: Option<&str>,
    ) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            profile(tx, id)?;
            match (provider_id, model_ref) {
                (None, None) => {}
                (Some(provider_id), Some(model_ref)) => {
                    let provider: Option<(String, i64)> = tx
                        .query_row(
                            "SELECT name, enabled FROM model_providers WHERE id=?1",
                            [provider_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?;
                    let Some((name, enabled)) = provider else {
                        return Err(CompanionError::ModelUnavailable("所选模型服务已被删除，请重新选择".into()));
                    };
                    if enabled == 0 {
                        return Err(CompanionError::ModelUnavailable(format!("模型服务「{name}」已停用，请先启用或改选其他模型")));
                    }
                    let model_exists: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id=?1 AND provider_id=?2)",
                        params![model_ref, provider_id],
                        |row| row.get(0),
                    )?;
                    if !model_exists {
                        return Err(CompanionError::ModelUnavailable("所选模型已不存在，请重新选择".into()));
                    }
                }
                _ => return Err(CompanionError::ModelUnavailable("模型服务与模型必须同时选择".into())),
            }
            tx.execute(
                "UPDATE companion_profiles SET model_provider_id=?2, model_ref=?3, updated_at=?4 WHERE id=?1",
                params![id, provider_id, model_ref, now_ms()],
            )?;
            profile(tx, id)
        })
    }

    pub(crate) fn pin_companion(&self, id: &str, pinned: bool) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            profile(tx, id)?;
            tx.execute(
                "UPDATE companion_profiles SET pinned_at=CASE WHEN ?2 THEN coalesce(pinned_at,?3)
                ELSE NULL END,updated_at=?3 WHERE id=?1",
                params![id, pinned, now_ms()],
            )?;
            profile(tx, id)
        })
    }

    /// K6: advance the bubble read cursor to a position the user actually saw.
    /// Monotonic (never rewinds), and only a durable bubble of this private chat
    /// can move it — a stale or foreign report is ignored. Reading never touches
    /// pending approvals or questions. A manual unread override clears only when
    /// a *fresh* report (observed after the mark) actually covers its start, so an
    /// in-flight auto-read update cannot immediately cancel a fresh mark-unread.
    pub(crate) fn read_companion_through(
        &self,
        id: &str,
        delivery_order: i64,
        observed_at: i64,
    ) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            let known: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM contact_bubbles WHERE conversation_id=?1 AND delivery_order=?2)",
                params![current.conversation_id, delivery_order],
                |row| row.get(0),
            )?;
            if !known {
                return Ok(current);
            }
            let target = current.read_delivery_order.max(delivery_order);
            tx.execute(
                "UPDATE companion_profiles SET last_read_delivery_order=?2, updated_at=?3 WHERE id=?1",
                params![id, target, now_ms()],
            )?;
            if let Some(from) = current.profile.manual_unread_from_order {
                if delivery_order >= from
                    && observed_at > current.profile.manual_unread_at.unwrap_or(0)
                {
                    tx.execute(
                        "UPDATE companion_profiles SET manual_unread_from_order=NULL, manual_unread_at=NULL, updated_at=?2 WHERE id=?1",
                        params![id, now_ms()],
                    )?;
                }
            }
            profile(tx, id)
        })
    }

    /// K6: mark a contact-sent visible bubble unread from its position onward.
    /// Independent of the auto-read cursor (which never rewinds). Requires the
    /// chat's K1 backfill to be complete so the mark lands on a durable bubble.
    pub(crate) fn mark_companion_unread(&self, id: &str, bubble_id: &str) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            if !super::contact_bubbles::backfill_complete(tx, &current.conversation_id)? {
                return Err(CompanionError::BackfillPending);
            }
            let order: Option<i64> = tx
                .query_row(
                    "SELECT delivery_order FROM contact_bubbles WHERE bubble_id=?1 AND conversation_id=?2 AND author='companion'",
                    params![bubble_id, current.conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(order) = order else {
                return Err(CompanionError::BubbleNotFound);
            };
            let now = now_ms();
            tx.execute(
                "UPDATE companion_profiles SET manual_unread_from_order=?2, manual_unread_at=?3, updated_at=?3 WHERE id=?1",
                params![id, order, now],
            )?;
            profile(tx, id)
        })
    }

    /// C07: the composer's quote draft. The private chat is derived from the
    /// contact, never supplied by the caller, so one contact's bubble id can
    /// never be quoted into another contact's chat.
    pub(crate) fn quote_companion_bubble(&self, id: &str, bubble_id: &str) -> Result<ContactQuotePreview> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            super::contact_marks::quote_preview_on(tx, &current.conversation_id, bubble_id)
        })
    }

    /// C07: the user's own favorite, on a user message or a contact reply.
    pub(crate) fn favorite_companion_bubble(&self, id: &str, bubble_id: &str, favorited: bool) -> Result<ContactBubbleMarkRecord> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            super::contact_marks::set_favorite_on(tx, &current.conversation_id, bubble_id, favorited)
        })
    }

    /// C07: one reaction per bubble — same one clears it, another replaces it.
    pub(crate) fn react_companion_bubble(&self, id: &str, bubble_id: &str, reaction: &str) -> Result<ContactBubbleMarkRecord> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            super::contact_marks::toggle_reaction_on(tx, &current.conversation_id, bubble_id, reaction)
        })
    }

    /// C07: the contact's favorites list (summary + time), newest marked first.
    pub(crate) fn companion_favorites(&self, id: &str, limit: usize) -> Result<Vec<ContactFavoriteEntry>> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            super::contact_marks::favorites_on(tx, &current.conversation_id, limit)
        })
    }

    /// K6: per-contact reminder mode. `follow_global` defers to the global
    /// notification preferences; the other three override them.
    pub(crate) fn set_companion_reminder_mode(&self, id: &str, mode: &str) -> Result<CompanionView> {
        if !matches!(mode, "follow_global" | "all" | "action_only" | "muted") {
            return Err(CompanionError::ReminderModeInvalid);
        }
        self.companion_transaction(|tx| {
            profile(tx, id)?;
            tx.execute(
                "UPDATE companion_profiles SET reminder_mode=?2, updated_at=?3 WHERE id=?1",
                params![id, mode, now_ms()],
            )?;
            profile(tx, id)
        })
    }

    pub(crate) fn archive_companion(&self, id: &str) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let record = profile(tx, id)?;
            require_idle(&record)?;
            let now = now_ms();
            // K2: archiving stops the contact's plans from dispatching, and a
            // restore leaves them off — enabling a plan again is the user's call.
            tx.execute(
                "UPDATE digital_colleague_schedules SET enabled=0, updated_at=?2
                WHERE enabled=1 AND colleague_id IN (
                    SELECT id FROM digital_colleagues WHERE companion_id=?1 AND companion_bound=1)",
                params![id, now],
            )?;
            tx.execute(
                "UPDATE companion_profiles SET archived_at=coalesce(archived_at,?2),updated_at=?2
                WHERE id=?1",
                params![id, now],
            )?;
            profile(tx, id)
        })
    }

    pub(crate) fn restore_companion(&self, id: &str) -> Result<CompanionView> {
        self.companion_transaction(|tx| restore(tx, profile(tx, id)?))
    }

    pub(crate) fn companion_template_status(&self, id: &str) -> Result<CompanionTemplateStatus> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| CompanionError::Storage("数据库锁已损坏".into()))?;
        let record = profile(&conn, id)?;
        if record.profile.is_default {
            return Ok(CompanionTemplateStatus {
                bound_hash: None,
                latest_hash: None,
                bound_snapshot: None,
                latest_snapshot: None,
                update_available: false,
                dismissed: false,
            });
        }
        let binding =
            query_conversation_expert_binding_by_state(&conn, &record.conversation_id, "active")
                .optional()?
                .ok_or_else(|| CompanionError::Storage("私聊的专家绑定缺失".into()))?;
        let expert = local_expert(&conn, &record.profile.template_agent_id)?;
        let latest = agent_package_snapshot(&expert);
        let hash = package_snapshot_hash(&latest);
        Ok(CompanionTemplateStatus {
            update_available: hash != binding.package_hash,
            dismissed: record.profile.template_notice_dismissed_hash.as_ref() == Some(&hash),
            latest_hash: Some(hash),
            bound_hash: Some(binding.package_hash),
            latest_snapshot: Some(latest),
            bound_snapshot: Some(binding.package_snapshot),
        })
    }

    pub(crate) fn apply_companion_template(
        &self,
        id: &str,
        expected: &str,
        dismiss: bool,
    ) -> Result<()> {
        self.companion_transaction(|tx| {
            let record = profile(tx, id)?;
            if record.profile.is_default {
                return Err(CompanionError::BuiltinTemplate);
            }
            let expert = local_expert(tx, &record.profile.template_agent_id)?;
            let hash = package_snapshot_hash(&agent_package_snapshot(&expert));
            if hash != expected {
                return Err(CompanionError::TemplateChanged);
            }
            if !dismiss {
                require_idle(&record)?;
                let bound = query_conversation_expert_binding_by_state(
                    tx,
                    &record.conversation_id,
                    "active",
                )
                .optional()?
                .ok_or_else(|| CompanionError::Storage("私聊的专家绑定缺失".into()))?;
                if bound.package_hash != hash {
                    activate_expert_binding(
                        tx,
                        &record.conversation_id,
                        &expert,
                        "companion_template_update",
                    )?;
                }
            }
            tx.execute(
                "UPDATE companion_profiles SET template_notice_dismissed_hash=?2 WHERE id=?1",
                params![id, dismiss.then_some(hash)],
            )?;
            Ok(())
        })
    }
}

fn create_companion(
    tx: &Transaction<'_>,
    root: &Path,
    expert: Option<&AgentRecord>,
) -> Result<CompanionView> {
    let id = format!("cmp_{}", Uuid::new_v4());
    let conversation_id = Uuid::new_v4().to_string();
    let name = available_name(tx, expert.map_or("小狐", |e| &e.name), &id)?;
    let workspace = companion_workspace(root, &id);
    let notes = companion_notes_dir(root, &id);
    let (template, avatar) = match expert {
        Some(e) => (e.id.as_str(), e.icon.as_deref().unwrap_or("")),
        None => (DEFAULT_AGENT_ID, "/avatars/fox-assistant.png"),
    };
    let now = now_ms();
    tx.execute(
        "INSERT INTO companion_profiles(id,template_agent_id,display_name,avatar_ref,
        workspace_dir,notes_dir,is_default,created_at,updated_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![
            id,
            template,
            name,
            avatar,
            workspace.to_string_lossy(),
            notes.to_string_lossy(),
            expert.is_none(),
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO conversations(id,agent_id,title,project_id,project_root,permission_mode,
        status,lineage_root_id,created_at,updated_at,agent_backend,companion_id)
        VALUES (?1,?2,?3,NULL,?4,'ask','active',?1,?5,?5,'fox',?6)",
        params![
            conversation_id,
            DEFAULT_AGENT_ID,
            name,
            workspace.to_string_lossy(),
            now,
            id
        ],
    )?;
    if let Some(expert) = expert {
        activate_expert_binding(tx, &conversation_id, expert, "companion_create")?;
    }
    let greeting_id = Uuid::new_v4().to_string();
    let introduction = match expert {
        None => "我可以帮你处理文件、整理资料和日常问题。".to_string(),
        Some(expert) => {
            let description = expert.description.split(['。', '！', '？', '\n', '\r']).next().unwrap_or("").trim();
            if description.is_empty() || description.chars().count() > 80 {
                "我可以协助你处理相关工作。".to_string()
            } else {
                format!("我的职责：{description}。")
            }
        }
    };
    let greeting = format!("你好，我是{name}。{introduction}有什么需要我帮忙的？");
    tx.execute(
        "INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
         VALUES (?1,?2,'assistant','text',?3,'completed',1,?4,?4)",
        params![greeting_id, conversation_id, greeting, now],
    )?;
    super::contact_bubbles::project_message_in_transaction(tx, &conversation_id, &greeting_id)?;
    tx.execute(
        "INSERT INTO contact_bubble_backfills(conversation_id, completed_at) VALUES (?1, ?2)",
        params![conversation_id, now],
    )?;
    tx.execute(
        "UPDATE companion_profiles SET last_read_delivery_order=0 WHERE id=?1",
        params![id],
    )?;
    profile(tx, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    struct Fixture {
        root: std::path::PathBuf,
        db: Database,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fox-companion-test-{}", Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let db = Database::open(root.join("fox.db")).unwrap();
            Self { root, db }
        }

        fn add(&self, expert: &str) -> CompanionView {
            self.db.add_companion(&self.root, expert).unwrap()
        }

        fn remove_greeting(&self, chat: &str) {
            self.db.with_connection(|c| {
                c.execute("INSERT INTO companion_deletions(companion_id,template_agent_id,display_name,plan_json,plan_hash,state,started_at)
                    SELECT p.id,p.template_agent_id,p.display_name,'{}','test','deleting',0 FROM companion_profiles p
                    JOIN conversations c ON c.companion_id=p.id WHERE c.id=?1", [chat])?;
                c.execute("DELETE FROM messages WHERE conversation_id=?1 AND role='assistant' AND run_id IS NULL", [chat])?;
                c.execute("DELETE FROM companion_deletions WHERE companion_id=(SELECT companion_id FROM conversations WHERE id=?1)", [chat])?;
                Ok(())
            }).unwrap();
        }

        /// K6 test helper: insert a durable bubble directly so delivery order is
        /// deterministic, independent of the projection pipeline.
        fn bubble(&self, chat: &str, ordinal: i64, author: &str, order: i64, content: &str) -> String {
            let id = format!("bubble-{order}");
            let source = format!("src-{order}");
            let role = if author == "companion" { "assistant" } else { "user" };
            self.db.with_connection(|c| {
                c.execute(
                    "INSERT OR IGNORE INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                     VALUES (?1,?2,?3,'text',?4,'completed',?5,1,1)",
                    params![source, chat, role, content, ordinal],
                )?;
                c.execute(
                    "INSERT OR IGNORE INTO contact_bubbles(bubble_id, conversation_id, run_id, source_message_id, source_ordinal,
                        author, projection, segment_start, segment_end, delivery_order, delivered_at, content, fingerprint, created_at)
                     VALUES (?1, ?2, NULL, ?3, ?4, ?5, 'message', NULL, NULL, ?6, ?7, ?8, ?8, 1)",
                    params![id, chat, source, ordinal, author, order, order, content],
                )?;
                Ok(())
            }).unwrap();
            id
        }

        fn complete_backfill(&self, chat: &str) {
            self.db.with_connection(|c| {
                c.execute("INSERT OR IGNORE INTO contact_bubble_backfills(conversation_id, completed_at) VALUES (?1, 1)", [chat])?;
                Ok(())
            }).unwrap();
        }

        /// C06/D2 起新建联系人事务即写入完成标记与游标 0；需要“迁移遗留、
        /// 未完成回填”前提的测试必须显式回退该状态，而不是依赖新建缺陷。
        fn reset_backfill(&self, chat: &str) {
            self.db.with_connection(|c| {
                c.execute("DELETE FROM contact_bubble_backfills WHERE conversation_id=?1", [chat])?;
                c.execute(
                    "UPDATE companion_profiles SET last_read_delivery_order=NULL
                     WHERE id=(SELECT companion_id FROM conversations WHERE id=?1)",
                    [chat],
                )?;
                Ok(())
            }).unwrap();
        }
    }

    #[test]
    fn new_companions_persist_one_greeting_without_a_run() {
        let f = Fixture::new();
        f.db.execute_raw_sql("UPDATE agents SET description='定位根因、修复缺陷。后续说明' WHERE id='fox-debugger'").unwrap();
        for contact in [f.db.seed_default_companion(&f.root).unwrap(), f.add("fox-debugger")] {
            let rows: Vec<(String, String, Option<String>)> = f.db.with_connection(|c| {
                let mut statement = c.prepare(
                    "SELECT m.content, b.content, m.run_id FROM messages m
                     JOIN contact_bubbles b ON b.source_message_id=m.id
                     WHERE m.conversation_id=?1 AND m.role='assistant'",
                )?;
                let rows = statement.query_map([&contact.conversation_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect()
            }).unwrap();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].0.contains(&contact.profile.display_name));
            if contact.profile.is_default {
                assert!(rows[0].0.contains("处理文件、整理资料和日常问题"));
            } else {
                assert!(rows[0].0.contains("我的职责：定位根因、修复缺陷。"));
                assert!(!rows[0].0.contains("后续说明"));
            }
            assert_eq!(rows[0].0, rows[0].1);
            assert!(rows[0].2.is_none());
            assert_eq!(contact.unread_count, 1);
        }
        f.db.execute_raw_sql(&format!("UPDATE agents SET description='{}' WHERE id='fox-reviewer'", "长".repeat(81))).unwrap();
        let contact = f.add("fox-reviewer");
        let content: String = f.db.with_connection(|c| c.query_row(
            "SELECT content FROM messages WHERE conversation_id=?1", [&contact.conversation_id], |row| row.get(0),
        )).unwrap();
        assert!(content.contains("我可以协助你处理相关工作。"));
    }

    #[test]
    fn companions_default_is_seeded_once_and_stays_archived_after_restart() {
        let f = Fixture::new();
        let default = f.db.seed_default_companion(&f.root).unwrap();
        assert!(default.profile.is_default);
        assert_eq!(default.profile.display_name, "小狐");
        assert_eq!(default.profile.avatar_ref, "/avatars/fox-assistant.png");
        assert!(f
            .db
            .current_conversation_expert_binding(&default.conversation_id)
            .unwrap()
            .is_none());
        f.db.archive_companion(&default.profile.id).unwrap();
        drop(f.db);
        let db = Database::open(f.root.join("fox.db")).unwrap();
        let again = db.seed_default_companion(&f.root).unwrap();
        assert_eq!(again.profile.id, default.profile.id);
        assert!(again.profile.archived_at.is_some());
        assert_eq!(again.conversation_id, default.conversation_id);
        assert_eq!(db.list_companions().unwrap().companions.len(), 1);
        drop(db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn companions_binding_failure_rolls_back_and_concurrent_adds_share_one_private_chat() {
        let f = Fixture::new();
        f.db.execute_raw_sql(
            "CREATE TRIGGER reject_companion_binding BEFORE INSERT ON conversation_expert_bindings
            BEGIN SELECT RAISE(ABORT,'test binding write failed'); END;",
        )
        .unwrap();
        assert!(f.db.add_companion(&f.root, "fox-debugger").is_err());
        let partial:(i64,i64)=f.db.with_connection(|c| c.query_row("SELECT
            (SELECT count(*) FROM companion_profiles), (SELECT count(*) FROM conversations WHERE companion_id IS NOT NULL)",
            [],|r| Ok((r.get(0)?,r.get(1)?)))).unwrap();
        assert_eq!(partial, (0, 0));
        f.db.execute_raw_sql("DROP TRIGGER reject_companion_binding")
            .unwrap();
        assert!(matches!(
            f.db.add_companion(&f.root, DEFAULT_AGENT_ID),
            Err(CompanionError::ExpertOnly)
        ));
        let peer = Database::open(f.root.join("fox.db")).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let start = Arc::clone(&barrier);
        let root = f.root.clone();
        let join = std::thread::spawn(move || {
            start.wait();
            peer.add_companion(&root, "fox-debugger").unwrap()
        });
        barrier.wait();
        let record = f.add("fox-debugger");
        let other = join.join().unwrap();
        assert_eq!(record.profile.id, other.profile.id);
        assert_eq!(record.conversation_id, other.conversation_id);
        // C06/D2: concurrent adds share one creation transaction outcome —
        // exactly one backfill marker and one initialized read cursor.
        let (markers, cursors): (i64, i64) = f.db.with_connection(|c| {
            c.query_row(
                "SELECT (SELECT count(*) FROM contact_bubble_backfills WHERE conversation_id=?1),
                    (SELECT count(*) FROM companion_profiles WHERE id=?2 AND last_read_delivery_order=0)",
                params![record.conversation_id, record.profile.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        }).unwrap();
        assert_eq!((markers, cursors), (1, 1));
        let chat =
            f.db.load_conversation(&record.conversation_id)
                .unwrap()
                .conversation;
        assert!(chat.project_id.is_none());
        assert_eq!(
            chat.project_root.as_deref(),
            Some(record.profile.workspace_dir.as_str())
        );
        assert_eq!(
            f.db.list_conversation_expert_bindings(&chat.id)
                .unwrap()
                .len(),
            1
        );
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn companions_names_persona_pin_unread_and_active_run_survive_archive_and_restore() {
        let f = Fixture::new();
        f.db.execute_raw_sql("UPDATE agents SET name='Fox' WHERE id IN ('fox-debugger','fox-reviewer','fox-security')").unwrap();
        let first = f.add("fox-debugger");
        let second = f.add("fox-reviewer");
        f.remove_greeting(&first.conversation_id);
        let id = &first.profile.id;
        assert_eq!(second.profile.display_name, "Fox（2）");
        let edit = CompanionProfileEdit {
            display_name: Some(" fox ".into()),
            avatar_ref: None,
            duty: None,
            persona_override: None,
        };
        assert!(matches!(
            f.db.edit_companion(&second.profile.id, &edit),
            Err(CompanionError::NameTaken)
        ));
        f.db.pin_companion(id, true).unwrap();
        // K6: unread counts durable companion bubbles, not settled message rows.
        f.bubble(&first.conversation_id, 1, "user", 1000, "");
        f.bubble(&first.conversation_id, 2, "companion", 2000, "第一条回复");
        f.bubble(&first.conversation_id, 3, "companion", 3000, "第二条回复");
        f.complete_backfill(&first.conversation_id);
        f.db.with_connection(|c| {
            c.execute("INSERT INTO runs(id,conversation_id,status,model,created_at)
                VALUES ('test-active',?1,'running','fox',1)",[&first.conversation_id])?;
            Ok(())
        }).unwrap();
        assert_eq!(f.db.companion_profile(id).unwrap().unread_count, 2);
        assert!(matches!(
            f.db.archive_companion(id),
            Err(CompanionError::RunActive)
        ));
        f.db.execute_raw_sql("UPDATE runs SET status='completed' WHERE id='test-active'")
            .unwrap();
        assert_eq!(f.db.read_companion_through(id, 2000, 10).unwrap().unread_count, 1);
        // Monotonic: reporting an earlier bubble never rewinds the cursor.
        assert_eq!(
            f.db.read_companion_through(id, 1000, 11)
                .unwrap()
                .read_delivery_order,
            2000
        );
        f.db.archive_companion(id).unwrap();
        assert_eq!(f.add("fox-security").profile.display_name, "Fox");
        let restored = f.db.restore_companion(id).unwrap();
        assert_eq!(restored.conversation_id, first.conversation_id);
        assert_eq!(restored.profile.display_name, "Fox（3）");
        assert!(restored.profile.pinned_at.is_some());
        assert_eq!(restored.read_delivery_order, 2000);
        assert_eq!(
            f.db.read_companion_through(id, 3000, 12)
                .unwrap()
                .read_delivery_order,
            3000
        );
        assert_eq!(f.db.companion_profile(id).unwrap().unread_count, 0);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn companion_list_previews_the_latest_visible_message_in_each_chat() {
        let f = Fixture::new();
        let active = f.add("fox-debugger");
        let empty = f.add("fox-reviewer");
        f.remove_greeting(&active.conversation_id);
        f.remove_greeting(&empty.conversation_id);
        f.bubble(&active.conversation_id, 2, "companion", 2000, "前一条回复");
        f.bubble(&active.conversation_id, 3, "user", 3000, "  最新用户消息");

        let list = f.db.list_companions().unwrap().companions;
        let preview = |id: &str| list.iter().find(|c| c.profile.id == id).unwrap().latest_message_preview.as_deref();
        assert_eq!(preview(&active.profile.id), Some("最新用户消息"));
        assert_eq!(preview(&empty.profile.id), None);

        let reply = "新".repeat(300);
        f.bubble(&active.conversation_id, 4, "companion", 4000, &reply);
        let updated = f.db.companion_profile(&active.profile.id).unwrap();
        assert_eq!(updated.latest_message_preview, Some("新".repeat(240)));
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_reminder_mode_is_per_contact_and_gates_alerts_independently() {
        use crate::database::repositories::app_management::reminder_allows_alert;
        let f = Fixture::new();
        let a = f.add("fox-debugger");
        let b = f.add("fox-reviewer");
        assert_eq!(a.profile.reminder_mode, "follow_global");
        f.db.set_companion_reminder_mode(&a.profile.id, "muted").unwrap();
        let list = f.db.list_companions().unwrap().companions;
        let ra = list.iter().find(|c| c.profile.id == a.profile.id).unwrap();
        let rb = list.iter().find(|c| c.profile.id == b.profile.id).unwrap();
        assert_eq!(ra.profile.reminder_mode, "muted");
        // 静音互不影响：the other contact keeps following global.
        assert_eq!(rb.profile.reminder_mode, "follow_global");
        assert!(!reminder_allows_alert("muted", "approval"));
        assert!(!reminder_allows_alert("muted", "message"));
        assert!(reminder_allows_alert("action_only", "approval"));
        assert!(reminder_allows_alert("action_only", "question"));
        assert!(!reminder_allows_alert("action_only", "message"));
        assert!(!reminder_allows_alert("action_only", "completed"));
        assert!(reminder_allows_alert("all", "message"));
        assert!(reminder_allows_alert("follow_global", "approval"));
        assert!(matches!(
            f.db.set_companion_reminder_mode(&a.profile.id, "loud"),
            Err(CompanionError::ReminderModeInvalid)
        ));
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_mid_run_delivered_bubbles_stay_unread_after_cancel() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        f.remove_greeting(&c.conversation_id);
        let chat = c.conversation_id.clone();
        f.db.with_connection(|conn| {
            conn.execute("INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES ('r1',?1,'running','fox',1)", [&chat])?;
            Ok(())
        }).unwrap();
        f.bubble(&chat, 2, "companion", 2000, "中途一");
        f.bubble(&chat, 3, "companion", 3000, "中途二");
        f.db.execute_raw_sql(&format!("UPDATE contact_bubbles SET run_id='r1' WHERE conversation_id='{chat}'")).unwrap();
        assert_eq!(f.db.companion_profile(&c.profile.id).unwrap().unread_count, 2);
        // 整轮取消不撤销已送达气泡的未读资格。
        f.db.execute_raw_sql("UPDATE runs SET status='cancelled' WHERE id='r1'").unwrap();
        let after = f.db.companion_profile(&c.profile.id).unwrap();
        assert_eq!(after.unread_count, 2);
        assert!(after.active_run_status.is_none());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_manual_unread_survives_restart_and_clears_only_on_fresh_read() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.bubble(&chat, 2, "companion", 2000, "一");
        f.bubble(&chat, 3, "companion", 3000, "二");
        f.complete_backfill(&chat);
        f.db.read_companion_through(&id, 3000, 100).unwrap();
        assert_eq!(f.db.companion_profile(&id).unwrap().unread_count, 0);
        let marked = f.db.mark_companion_unread(&id, "bubble-2000").unwrap();
        assert_eq!(marked.profile.manual_unread_from_order, Some(2000));
        assert_eq!(marked.unread_count, 2);
        // 独立覆盖起点，不倒退自动已读游标。
        assert_eq!(marked.read_delivery_order, 3000);
        let mark_at = marked.profile.manual_unread_at.unwrap();
        // 自动已读更新不能立即抵消手动覆盖（observed_at 不晚于设置时刻）。
        let stale = f.db.read_companion_through(&id, 3000, mark_at).unwrap();
        assert_eq!(stale.profile.manual_unread_from_order, Some(2000));
        assert_eq!(stale.unread_count, 2);
        drop(f.db);
        let db = Database::open(f.root.join("fox.db")).unwrap();
        let after = db.companion_profile(&id).unwrap();
        assert_eq!(after.profile.manual_unread_from_order, Some(2000));
        assert_eq!(after.unread_count, 2);
        // 实际阅读对应位置后清除覆盖。
        let cleared = db.read_companion_through(&id, 3000, mark_at + 1).unwrap();
        assert_eq!(cleared.profile.manual_unread_from_order, None);
        assert_eq!(cleared.unread_count, 0);
        drop(db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_mark_unread_requires_durable_companion_bubble_and_backfill() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.reset_backfill(&chat);
        f.bubble(&chat, 2, "companion", 2000, "一");
        // 回填未完成时不能永久标记。
        assert!(matches!(f.db.mark_companion_unread(&id, "bubble-2000"), Err(CompanionError::BackfillPending)));
        f.complete_backfill(&chat);
        // 用户气泡或未知气泡不可标记。
        assert!(matches!(f.db.mark_companion_unread(&id, "missing"), Err(CompanionError::BubbleNotFound)));
        f.bubble(&chat, 1, "user", 1000, "");
        assert!(matches!(f.db.mark_companion_unread(&id, "bubble-1000"), Err(CompanionError::BubbleNotFound)));
        assert!(f.db.mark_companion_unread(&id, "bubble-2000").is_ok());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_reading_does_not_clear_pending_actions() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.bubble(&chat, 2, "companion", 2000, "回复");
        f.complete_backfill(&chat);
        f.db.with_connection(|conn| {
            conn.execute("INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES ('r1',?1,'awaiting_confirmation','fox',1)", [&chat])?;
            conn.execute("INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,status,started_at,updated_at)
                VALUES ('t1','rt1','r1',?1,'write_file','pending',1,1)", [&chat])?;
            conn.execute("INSERT INTO approvals(id,tool_call_id,status,requested_action,requested_at) VALUES ('a1','t1','pending','write_file',1)", [])?;
            Ok(())
        }).unwrap();
        let before = f.db.list_companions().unwrap().companions.into_iter().find(|x| x.profile.id == id).unwrap();
        assert_eq!(before.pending_action_count, 1);
        f.db.read_companion_through(&id, 2000, 100).unwrap();
        let after = f.db.list_companions().unwrap().companions.into_iter().find(|x| x.profile.id == id).unwrap();
        // 阅读不消除待处理审批。
        assert_eq!(after.pending_action_count, 1);
        assert_eq!(after.unread_count, 0);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn c06_legacy_read_ordinal_converts_to_bubble_position() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.bubble(&chat, 1, "companion", 1000, "一");
        f.bubble(&chat, 2, "companion", 2000, "二");
        f.bubble(&chat, 3, "companion", 3000, "三");
        f.db.execute_raw_sql(&format!("UPDATE companion_profiles SET last_read_ordinal=2, last_read_delivery_order=NULL WHERE id='{id}'")).unwrap();
        // 转换前 VIEW 仍按旧 ordinal 映射出正确位置。
        let before = f.db.companion_profile(&id).unwrap();
        assert_eq!(before.read_delivery_order, 2000);
        assert_eq!(before.unread_count, 1);
        f.complete_backfill(&chat);
        f.db.with_connection(|conn| { super::convert_pending_read_cursors(conn)?; Ok(()) }).unwrap();
        let after = f.db.companion_profile(&id).unwrap();
        assert_eq!(after.profile.last_read_delivery_order, Some(2000));
        assert_eq!(after.read_delivery_order, 2000);
        // 旧游标之后的已送达气泡按新规则计未读。
        assert_eq!(after.unread_count, 1);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn companions_template_adoption_checks_reviewed_hash_and_keeps_user_fields() {
        let f = Fixture::new();
        let record = f.add("fox-debugger");
        let id = &record.profile.id;
        let initial = f.db.companion_template_status(id).unwrap();
        assert!(!initial.update_available);
        f.db.edit_companion(
            id,
            &CompanionProfileEdit {
                display_name: None,
                avatar_ref: None,
                duty: Some("用户职责".into()),
                persona_override: Some("用户人设".into()),
            },
        )
        .unwrap();
        f.db.execute_raw_sql("UPDATE agents SET system_prompt='新版专家' WHERE id='fox-debugger'")
            .unwrap();
        let changed = f.db.companion_template_status(id).unwrap();
        assert!(changed.update_available);
        let latest = changed.latest_hash.unwrap();
        assert!(matches!(
            f.db.apply_companion_template(id, &initial.bound_hash.unwrap(), false),
            Err(CompanionError::TemplateChanged)
        ));
        f.db.apply_companion_template(id, &latest, true).unwrap();
        assert!(f.db.companion_template_status(id).unwrap().dismissed);
        f.db.with_connection(|c| {
            c.execute(
                "INSERT INTO runs(id,conversation_id,status,model,created_at)
            VALUES ('test-active',?1,'queued','fox',1)",
                [&record.conversation_id],
            )
        })
        .unwrap();
        assert!(matches!(
            f.db.apply_companion_template(id, &latest, false),
            Err(CompanionError::RunActive)
        ));
        f.db.execute_raw_sql("UPDATE runs SET status='completed' WHERE id='test-active'")
            .unwrap();
        f.db.apply_companion_template(id, &latest, false).unwrap();
        f.db.apply_companion_template(id, &latest, false).unwrap();
        let history =
            f.db.list_conversation_expert_bindings(&record.conversation_id)
                .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].state, "replaced");
        assert_eq!(history[1].activation_source, "companion_template_update");
        assert_eq!(history[1].package_snapshot["systemPrompt"], "新版专家");
        let updated = f.db.companion_profile(id).unwrap();
        assert_eq!(updated.profile.duty, "用户职责");
        assert_eq!(
            updated.profile.persona_override.as_deref(),
            Some("用户人设")
        );
        assert!(!f.db.companion_template_status(id).unwrap().update_available);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn contact_bubble_backfill_resumes_after_a_mid_batch_failure() {
        let f = Fixture::new();
        let first = f.add("fox-reviewer");
        let second = f.add("fox-debugger");
        f.remove_greeting(&first.conversation_id);
        f.remove_greeting(&second.conversation_id);
        // 显式构造旧库遗留状态：真实历史未完成回填，新建事务的初始化被回退。
        f.reset_backfill(&first.conversation_id);
        f.reset_backfill(&second.conversation_id);
        for (chat, run) in [
            (first.conversation_id.as_str(), "run-a"),
            (second.conversation_id.as_str(), "run-b"),
        ] {
            f.db.with_connection(|c| {
                c.execute(
                    "INSERT INTO runs(id,conversation_id,status,model,created_at)
                     VALUES (?1,?2,'completed','fox',1)",
                    params![run, chat],
                )?;
                c.execute(
                    "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
                     VALUES (?1,?2,NULL,'user','text','问题','completed',1,1,1)",
                    params![format!("{run}-user"), chat],
                )?;
                c.execute(
                    "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
                     VALUES (?1,?2,?3,'assistant','text','回答','completed',2,2,2)",
                    params![format!("assistant-{run}"), chat, run],
                )?;
                c.execute(
                    "INSERT INTO run_events(id,run_id,seq,event_type,event_json,created_at)
                     VALUES (?1,?2,1,'message.delta','{\"delta\":\"回答\"}',1)",
                    params![format!("{run}-event"), run],
                )?;
                Ok(())
            })
            .unwrap();
        }
        let complete = |db: &Database, chat: &str| {
            db.with_connection(|c| crate::database::repositories::contact_bubbles::backfill_complete(c, chat))
                .unwrap()
        };
        assert!(!complete(&f.db, &first.conversation_id));
        assert!(!complete(&f.db, &second.conversation_id));

        // A durable write failure mid-backfill must leave the failed chat
        // unmarked so the next batch retries it; already-finished chats keep
        // their marker and are never projected again.
        f.db.execute_raw_sql(
            "CREATE TRIGGER fail_backfill BEFORE INSERT ON contact_bubbles
             WHEN NEW.run_id = 'run-b'
             BEGIN SELECT RAISE(ABORT, 'injected backfill failure'); END;",
        )
        .unwrap();
        assert!(f.db.backfill_contact_bubbles_batch(8).is_err());
        assert!(!complete(&f.db, &second.conversation_id));
        f.db.execute_raw_sql("DROP TRIGGER fail_backfill;").unwrap();

        // UUID ordering decides which chat failed first, so only the total is
        // deterministic: every unmarked chat is finished exactly once.
        let mut finished = 0;
        loop {
            let batch = f.db.backfill_contact_bubbles_batch(1).unwrap();
            if batch == 0 {
                break;
            }
            finished += batch;
        }
        assert!((1..=2).contains(&finished));
        assert_eq!(f.db.backfill_contact_bubbles_batch(8).unwrap(), 0);
        assert!(complete(&f.db, &first.conversation_id));
        assert!(complete(&f.db, &second.conversation_id));
        for (chat, run) in [
            (first.conversation_id.as_str(), "run-a"),
            (second.conversation_id.as_str(), "run-b"),
        ] {
            let bubbles = f
                .db
                .with_connection(|c| crate::database::repositories::contact_bubbles::query_window(c, chat, 0, None))
                .unwrap();
            assert_eq!(bubbles.len(), 2);
            assert_eq!(bubbles[0].id, format!("{run}-user"));
            assert_eq!(bubbles[1].id, format!("{run}:response:1"));
        }
        let markers: i64 = f
            .db
            .with_connection(|c| {
                c.query_row("SELECT COUNT(*) FROM contact_bubble_backfills", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(markers, 2, "ordinary conversations never get a marker");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn terminal_transitions_project_bubbles_inside_the_write_transaction() {
        let f = Fixture::new();
        let companion = f.add("fox-debugger");
        f.remove_greeting(&companion.conversation_id);
        let bubbles = |db: &Database, chat: &str| {
            db.with_connection(|c| {
                crate::database::repositories::contact_bubbles::query_window(c, chat, 0, None)
            })
            .unwrap()
        };
        let started = f.db.create_run(&companion.conversation_id, "你好", None).unwrap();
        let run = started.run.id.clone();
        // The user bubble exists as soon as the message is inserted.
        let rows = bubbles(&f.db, &companion.conversation_id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, started.user_message.id);
        assert_eq!(rows[0].author, "user");

        f.db.apply_runtime_event(&run, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        f.db.apply_runtime_event(&run, 2, &serde_json::json!({"type":"message.started"})).unwrap();
        f.db.apply_runtime_event(&run, 3, &serde_json::json!({"type":"message.delta","delta":"第一段"})).unwrap();
        f.db.apply_runtime_event(&run, 4, &serde_json::json!({"type":"tool.started","toolCallId":"t","tool":"read","input":{}})).unwrap();
        f.db.apply_runtime_event(&run, 5, &serde_json::json!({"type":"message.delta","delta":"第二段"})).unwrap();
        f.db.apply_runtime_event(&run, 6, &serde_json::json!({"type":"message.completed"})).unwrap();
        let rows = bubbles(&f.db, &companion.conversation_id);
        assert_eq!(rows.len(), 3, "message.completed projects without waiting for the Run");
        assert_eq!(rows[1].id, format!("{run}:response:3"));
        assert_eq!(rows[2].id, format!("{run}:response:5"));
        f.db.apply_runtime_event(&run, 7, &serde_json::json!({"type":"run.completed"})).unwrap();
        let again = bubbles(&f.db, &companion.conversation_id);
        assert_eq!(
            rows.iter().map(|b| &b.id).collect::<Vec<_>>(),
            again.iter().map(|b| &b.id).collect::<Vec<_>>(),
            "the terminal Run transition never renumbers existing bubbles"
        );

        // An ordinary conversation pays only the companion check: no rows.
        let plain = f.db.create_conversation("fox-general", Some("普通对话"), None, None).unwrap();
        let plain_run = f.db.create_run(&plain.id, "普通问题", None).unwrap();
        let id = &plain_run.run.id;
        f.db.apply_runtime_event(id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        f.db.apply_runtime_event(id, 2, &serde_json::json!({"type":"message.started"})).unwrap();
        f.db.apply_runtime_event(id, 3, &serde_json::json!({"type":"message.delta","delta":"回答"})).unwrap();
        f.db.apply_runtime_event(id, 4, &serde_json::json!({"type":"message.completed"})).unwrap();
        f.db.apply_runtime_event(id, 5, &serde_json::json!({"type":"run.completed"})).unwrap();
        assert!(bubbles(&f.db, &plain.id).is_empty());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    // C04/K5: independent model resolution and the frozen run snapshot.

    fn global_service(model_id: &str, supports_image_input: bool) -> crate::database::ModelServiceRecord {
        crate::database::ModelServiceRecord {
            name: "全局服务".to_owned(),
            base_url: "https://global.example.com".to_owned(),
            model_id: model_id.to_owned(),
            api_type: "openai-completions".to_owned(),
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_image_input,
            enabled: true,
            credential_configured: false,
            connection_type: "remote".to_owned(),
            last_status: "unknown".to_owned(),
            last_latency_ms: None,
            last_checked_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn seed_provider(db: &Database, enabled: bool, supports_image_input: bool) {
        db.execute_raw_sql(&format!(
            "INSERT INTO model_providers(id,name,base_url,api_type,enabled,is_default,created_at,updated_at)
             VALUES ('prov','独立服务','https://independent.example.com','openai-completions',{},0,1,1)",
            enabled as i64
        )).unwrap();
        db.execute_raw_sql(&format!(
            "INSERT INTO provider_models(id,provider_id,model_id,display_name,context_window,max_output_tokens,supports_image_input,is_default,created_at,updated_at)
             VALUES ('prov-model','prov','independent-model','independent-model',64000,4096,{},1,1,1)",
            supports_image_input as i64
        )).unwrap();
    }

    #[test]
    fn companion_model_override_wins_and_the_run_snapshot_is_write_once() {
        let f = Fixture::new();
        let companion = f.add("fox-debugger");
        f.db.save_model_service(&global_service("global-model", false)).unwrap();
        seed_provider(&f.db, true, true);
        let global = f.db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap();
        assert_eq!(global.model_id, "global-model", "无覆盖时跟随全局");

        f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")).unwrap();
        let resolved = f.db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap();
        assert_eq!(resolved.model_id, "independent-model");
        assert!(resolved.supports_image_input);
        assert_eq!(resolved.context_window, 64_000);
        assert_eq!(resolved.max_output_tokens, 4_096);

        let run = f.db.create_run(&companion.conversation_id, "用独立模型", None).unwrap();
        assert_eq!(run.run.model, "independent-model", "Run 的展示模型也是覆盖值");
        f.db.freeze_run_model_snapshot(&run.run.id, &resolved).unwrap();
        f.db.save_model_service(&global_service("global-model-v2", true)).unwrap();
        f.db.execute_raw_sql("UPDATE provider_models SET model_id='renamed', supports_image_input=0").unwrap();
        let snapshot = f.db.run_model_snapshot(&run.run.id).unwrap().unwrap();
        assert_eq!(snapshot.model_id, "independent-model", "快照不随全局或供应商变化");
        assert!(snapshot.supports_image_input);
        f.db.freeze_run_model_snapshot(&run.run.id, &global_service("attempted-replacement", false)).unwrap();
        assert_eq!(f.db.run_model_snapshot(&run.run.id).unwrap().unwrap().model_id, "independent-model", "冻结只写一次");
        assert_eq!(f.db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap().model_id, "renamed", "新发送重新解析当前配置");

        f.db.set_companion_model(&companion.profile.id, None, None).unwrap();
        assert_eq!(f.db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap().model_id, "global-model-v2");
        f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")).unwrap();
        drop(f.db);
        let db = Database::open(f.root.join("fox.db")).unwrap();
        let view = db.companion_profile(&companion.profile.id).unwrap();
        assert_eq!(view.profile.model_provider_id.as_deref(), Some("prov"));
        assert_eq!(view.profile.model_ref.as_deref(), Some("prov-model"));
        assert_eq!(db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap().model_id, "renamed");
        assert_eq!(db.run_model_snapshot(&run.run.id).unwrap().unwrap().model_id, "independent-model", "重开后快照仍在");
        drop(db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    /// K2: a contact background task resolves the same effective model as the
    /// private chat, and the Run it dispatches carries that model before the
    /// Host starts it.
    #[test]
    fn contact_background_task_shares_the_contact_model() {
        let f = Fixture::new();
        let companion = f.add("fox-debugger");
        f.db.save_model_service(&global_service("global-model", false)).unwrap();
        seed_provider(&f.db, true, true);
        f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")).unwrap();
        let colleague = f.db.create_digital_colleague(&crate::database::CreateDigitalColleagueInput {
            name: "后台模型".into(),
            expert_id: "fox-debugger".into(),
            objective: "整理发布证据".into(),
            project_id: None,
            project_root: None,
            knowledge_references: Vec::new(),
            max_runs_per_day: 2,
            max_tokens_per_day: 10_000,
            max_duration_ms: 60_000,
            max_output_tokens: 1_024,
            max_tool_calls: 8,
            companion_id: Some(companion.profile.id.clone()),
        }).unwrap();
        assert_eq!(
            f.db.resolve_effective_model(&colleague.conversation_id).unwrap().unwrap().model_id,
            "independent-model",
            "后台执行会话与私聊共用联系人的有效模型"
        );
        let accepted = f.db.accept_digital_colleague_trigger(
            &colleague.id, "manual", None, "manual:model", &serde_json::json!({}), None, "user",
        ).unwrap();
        let run = accepted.prepared.expect("a bound task is admitted").started.run;
        assert_eq!(run.model, "independent-model", "派发前解析出的模型写进 Run");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn a_broken_companion_model_override_errors_instead_of_falling_back() {
        let f = Fixture::new();
        let companion = f.add("fox-debugger");
        f.db.save_model_service(&global_service("global-model", false)).unwrap();
        seed_provider(&f.db, true, false);
        assert!(matches!(
            f.db.set_companion_model(&companion.profile.id, Some("missing"), Some("m")),
            Err(CompanionError::ModelUnavailable(_))
        ));
        assert!(matches!(
            f.db.set_companion_model(&companion.profile.id, Some("prov"), None),
            Err(CompanionError::ModelUnavailable(_))
        ));
        assert!(matches!(
            f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("missing")),
            Err(CompanionError::ModelUnavailable(_))
        ));
        f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")).unwrap();

        f.db.execute_raw_sql("UPDATE model_providers SET enabled=0").unwrap();
        assert!(matches!(
            f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")),
            Err(CompanionError::ModelUnavailable(_))
        ), "停用服务不能新选");
        assert!(f.db.resolve_effective_model(&companion.conversation_id).unwrap_err().contains("停用"));
        f.db.execute_raw_sql("UPDATE model_providers SET enabled=1").unwrap();
        f.db.execute_raw_sql("DELETE FROM provider_models").unwrap();
        assert!(f.db.resolve_effective_model(&companion.conversation_id).unwrap_err().contains("移除"));
        f.db.execute_raw_sql("DELETE FROM model_providers").unwrap();
        assert!(f.db.resolve_effective_model(&companion.conversation_id).unwrap_err().contains("删除"));

        let plain = f.db.create_conversation("fox-general", Some("普通对话"), None, None).unwrap();
        assert_eq!(f.db.resolve_effective_model(&plain.id).unwrap().unwrap().model_id, "global-model", "普通对话不受影响");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn a_companion_with_an_override_resolves_without_any_global_model() {
        let f = Fixture::new();
        let companion = f.add("fox-debugger");
        seed_provider(&f.db, true, true);
        f.db.set_companion_model(&companion.profile.id, Some("prov"), Some("prov-model")).unwrap();
        assert!(f.db.get_model_service().unwrap().is_none());
        let resolved = f.db.resolve_effective_model(&companion.conversation_id).unwrap().unwrap();
        assert_eq!(resolved.model_id, "independent-model");
        let plain = f.db.create_conversation("fox-general", Some("普通对话"), None, None).unwrap();
        assert!(f.db.resolve_effective_model(&plain.id).unwrap().is_none(), "无覆盖且无全局时仍是未配置");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    // C06/D2: runtime-created contacts must initialize backfill facts inside
    // the creation transaction — no restart scan, no manual marker repair.

    fn marker_count(f: &Fixture, chat: &str) -> i64 {
        f.db.with_connection(|c| {
            c.query_row(
                "SELECT count(*) FROM contact_bubble_backfills WHERE conversation_id=?1",
                [chat],
                |r| r.get(0),
            )
        })
        .unwrap()
    }

    fn chat_bubbles(f: &Fixture, chat: &str) -> Vec<crate::database::ContactBubbleRecord> {
        f.db.with_connection(|c| {
            super::super::contact_bubbles::query_window(c, chat, 0, None)
        })
        .unwrap()
    }

    #[test]
    fn runtime_added_companion_can_use_bubble_actions_without_restart() {
        let f = Fixture::new();
        // 启动扫描已结束（此时没有任何私聊），之后的添加不会再被扫描唤醒。
        assert_eq!(f.db.backfill_contact_bubbles_batch(8).unwrap(), 0);
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        assert_eq!(marker_count(&f, &chat), 1, "新建事务即写入完成标记");
        assert_eq!(c.profile.last_read_delivery_order, Some(0));
        // 真实运行事件形成完成回复，不调用 complete_backfill、不手工补标。
        let started = f.db.create_run(&chat, "你好", None).unwrap();
        let run = started.run.id.clone();
        f.db.apply_runtime_event(&run, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        f.db.apply_runtime_event(&run, 2, &serde_json::json!({"type":"message.started"})).unwrap();
        f.db.apply_runtime_event(&run, 3, &serde_json::json!({"type":"message.delta","delta":" runtime 回复"})).unwrap();
        f.db.apply_runtime_event(&run, 4, &serde_json::json!({"type":"message.completed"})).unwrap();
        f.db.apply_runtime_event(&run, 5, &serde_json::json!({"type":"run.completed"})).unwrap();
        let reply = format!("{run}:response:3");
        let marked = f.db.mark_companion_unread(&id, &reply).unwrap();
        assert!(marked.profile.manual_unread_from_order.is_some());
        let preview = f.db.quote_companion_bubble(&id, &reply).unwrap();
        assert!(preview.summary.contains("runtime 回复"));
        f.db.favorite_companion_bubble(&id, &reply, true).unwrap();
        f.db.favorite_companion_bubble(&id, &reply, true).unwrap();
        let favorites = f.db.companion_favorites(&id, 10).unwrap();
        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].bubble_id, reply);
        assert_eq!(marker_count(&f, &chat), 1, "运行投影不新增/覆盖标记");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn new_default_companion_has_empty_history_backfill_completed() {
        let f = Fixture::new();
        let default = f.db.seed_default_companion(&f.root).unwrap();
        let chat = default.conversation_id.clone();
        assert_eq!(marker_count(&f, &chat), 1);
        assert_eq!(default.profile.last_read_delivery_order, Some(0));
        let completed_at: i64 = f.db.with_connection(|c| {
            c.query_row("SELECT completed_at FROM contact_bubble_backfills WHERE conversation_id=?1", [&chat], |r| r.get(0))
        }).unwrap();
        f.bubble(&chat, 1, "companion", 1000, "一");
        f.db.read_companion_through(&default.profile.id, 1000, 10).unwrap();
        // 再次 seed 不重建、不改已初始化的阅读事实与标记时间。
        let again = f.db.seed_default_companion(&f.root).unwrap();
        assert_eq!(again.profile.id, default.profile.id);
        assert_eq!(again.profile.last_read_delivery_order, Some(1000));
        let after: i64 = f.db.with_connection(|c| {
            c.query_row("SELECT completed_at FROM contact_bubble_backfills WHERE conversation_id=?1", [&chat], |r| r.get(0))
        }).unwrap();
        assert_eq!(after, completed_at);
        assert_eq!(marker_count(&f, &chat), 1);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn restoring_existing_companion_does_not_complete_legacy_backfill() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        // 迁移遗留：旧消息、部分持久气泡、缺完成标记、游标未转换。
        f.reset_backfill(&chat);
        f.bubble(&chat, 1, "companion", 1000, "旧回复");
        f.db.archive_companion(&id).unwrap();
        let restored = f.add("fox-debugger");
        assert_eq!(restored.conversation_id, chat, "重复添加走 restore，不是真新建");
        assert_eq!(marker_count(&f, &chat), 0, "restore 不得补历史完成标记");
        assert!(matches!(f.db.mark_companion_unread(&id, "bubble-1000"), Err(CompanionError::BackfillPending)));
        assert!(matches!(f.db.quote_companion_bubble(&id, "bubble-1000"), Err(CompanionError::BackfillPending)));
        assert!(matches!(f.db.favorite_companion_bubble(&id, "bubble-1000", true), Err(CompanionError::BackfillPending)));
        // 只有真实启动回填续跑完成后才放行。
        loop {
            if f.db.backfill_contact_bubbles_batch(8).unwrap() == 0 {
                break;
            }
        }
        assert_eq!(marker_count(&f, &chat), 1);
        assert!(f.db.mark_companion_unread(&id, "bubble-1000").is_ok());
        assert!(f.db.quote_companion_bubble(&id, "bubble-1000").is_ok());
        assert!(f.db.favorite_companion_bubble(&id, "bubble-1000", true).is_ok());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn restoring_completed_companion_preserves_read_and_backfill_facts() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.bubble(&chat, 1, "companion", 1000, "一");
        f.bubble(&chat, 2, "companion", 2000, "二");
        f.db.read_companion_through(&id, 2000, 10).unwrap();
        f.db.mark_companion_unread(&id, "bubble-1000").unwrap();
        f.db.favorite_companion_bubble(&id, "bubble-2000", true).unwrap();
        let completed_at: i64 = f.db.with_connection(|c| {
            c.query_row("SELECT completed_at FROM contact_bubble_backfills WHERE conversation_id=?1", [&chat], |r| r.get(0))
        }).unwrap();
        f.db.archive_companion(&id).unwrap();
        let restored = f.add("fox-debugger");
        assert_eq!(restored.conversation_id, chat);
        assert_eq!(restored.profile.last_read_delivery_order, Some(2000));
        assert_eq!(restored.profile.manual_unread_from_order, Some(1000));
        let after: i64 = f.db.with_connection(|c| {
            c.query_row("SELECT completed_at FROM contact_bubble_backfills WHERE conversation_id=?1", [&chat], |r| r.get(0))
        }).unwrap();
        assert_eq!(after, completed_at, "restore 不覆盖原完成时间");
        assert_eq!(f.db.companion_favorites(&id, 10).unwrap().len(), 1);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn companion_creation_rolls_back_if_backfill_initialization_fails() {
        let f = Fixture::new();
        let empty = || {
            f.db.with_connection(|c| {
                c.query_row(
                    "SELECT (SELECT count(*) FROM companion_profiles),
                        (SELECT count(*) FROM conversations WHERE companion_id IS NOT NULL),
                        (SELECT count(*) FROM conversation_expert_bindings),
                        (SELECT count(*) FROM contact_bubble_backfills)",
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)),
                )
            }).unwrap()
        };
        // 标记 INSERT 失败：整个创建回滚，不返回半成品联系人。
        f.db.execute_raw_sql(
            "CREATE TRIGGER fail_marker BEFORE INSERT ON contact_bubble_backfills
             BEGIN SELECT RAISE(ABORT,'test marker write failed'); END;",
        ).unwrap();
        assert!(f.db.add_companion(&f.root, "fox-debugger").is_err());
        assert_eq!(empty(), (0, 0, 0, 0));
        f.db.execute_raw_sql("DROP TRIGGER fail_marker").unwrap();
        // 游标 UPDATE 失败：同样整体回滚。
        f.db.execute_raw_sql(
            "CREATE TRIGGER fail_cursor BEFORE UPDATE OF last_read_delivery_order ON companion_profiles
             WHEN NEW.last_read_delivery_order IS NOT NULL
             BEGIN SELECT RAISE(ABORT,'test cursor write failed'); END;",
        ).unwrap();
        assert!(f.db.add_companion(&f.root, "fox-debugger").is_err());
        assert_eq!(empty(), (0, 0, 0, 0));
        f.db.execute_raw_sql("DROP TRIGGER fail_cursor").unwrap();
        let c = f.add("fox-debugger");
        assert_eq!(marker_count(&f, &c.conversation_id), 1);
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn legacy_partial_backfill_stays_pending_until_projection_completes() {
        let f = Fixture::new();
        let c = f.add("fox-debugger");
        f.remove_greeting(&c.conversation_id);
        let chat = c.conversation_id.clone();
        let id = c.profile.id.clone();
        f.reset_backfill(&chat);
        // 部分历史已投影（持久气泡），第二段尚未投影；旧游标按 ordinal=1 已读。
        f.bubble(&chat, 1, "companion", 1000, "第一段");
        f.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                 VALUES ('m2',?1,'assistant','text','第二段','completed',2,2,2)",
                [&chat],
            )?;
            Ok(())
        }).unwrap();
        f.db.execute_raw_sql(&format!(
            "UPDATE companion_profiles SET last_read_ordinal=1 WHERE id='{id}'"
        )).unwrap();
        f.db.execute_raw_sql(
            "CREATE TRIGGER fail_second BEFORE INSERT ON contact_bubbles
             WHEN NEW.source_ordinal = 2
             BEGIN SELECT RAISE(ABORT,'injected partial backfill failure'); END;",
        ).unwrap();
        assert!(f.db.backfill_contact_bubbles_batch(8).is_err());
        assert_eq!(marker_count(&f, &chat), 0, "部分投影失败不得提前标完成");
        assert_eq!(chat_bubbles(&f, &chat).len(), 1);
        assert!(matches!(f.db.mark_companion_unread(&id, "bubble-1000"), Err(CompanionError::BackfillPending)));
        assert!(matches!(f.db.quote_companion_bubble(&id, "bubble-1000"), Err(CompanionError::BackfillPending)));
        assert!(matches!(f.db.favorite_companion_bubble(&id, "bubble-1000", true), Err(CompanionError::BackfillPending)));
        f.db.execute_raw_sql("DROP TRIGGER fail_second").unwrap();
        // 真实续跑完成后才标完成，且旧 ordinal 游标正确转换为 delivery order。
        loop {
            if f.db.backfill_contact_bubbles_batch(8).unwrap() == 0 {
                break;
            }
        }
        assert_eq!(marker_count(&f, &chat), 1);
        assert_eq!(chat_bubbles(&f, &chat).len(), 2);
        let after = f.db.companion_profile(&id).unwrap();
        assert_eq!(after.profile.last_read_delivery_order, Some(1000));
        assert_eq!(after.read_delivery_order, 1000);
        assert!(f.db.mark_companion_unread(&id, "bubble-1000").is_ok());
        assert!(f.db.quote_companion_bubble(&id, "bubble-1000").is_ok());
        assert!(f.db.favorite_companion_bubble(&id, "bubble-1000", true).is_ok());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }
}

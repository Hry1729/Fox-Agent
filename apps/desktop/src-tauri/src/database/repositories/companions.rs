use super::{
    activate_expert_binding, agent_package_snapshot, now_ms, package_snapshot_hash,
    query_agent_record, query_conversation_expert_binding_by_state, Database, DEFAULT_AGENT_ID,
};
use crate::data_paths::{companion_notes_dir, companion_workspace};
use crate::database::{AgentRecord, CompanionProfile};
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
    pub unread_count: i64,
    pub active_run_status: Option<String>,
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

const VIEW: &str = "SELECT p.*, c.id AS conversation_id,
    (SELECT count(*) FROM messages m WHERE m.conversation_id=c.id AND m.ordinal>p.last_read_ordinal
        AND m.role='assistant' AND m.status='completed') AS unread_count,
    (SELECT r.status FROM runs r WHERE r.conversation_id=c.id
        AND r.status IN ('queued','running','cancelling','awaiting_confirmation')
        ORDER BY r.created_at DESC LIMIT 1) AS active_run_status
    FROM companion_profiles p JOIN conversations c ON c.companion_id=p.id";

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
            archived_at: row.get("archived_at")?,
            deleting_at: row.get("deleting_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        },
        conversation_id: row.get("conversation_id")?,
        unread_count: row.get("unread_count")?,
        active_run_status: row.get("active_run_status")?,
    })
}

fn profile(conn: &Connection, id: &str) -> Result<CompanionView> {
    let record = conn
        .query_row(&format!("{VIEW} WHERE p.id=?1"), [id], view)
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
            "{VIEW} ORDER BY p.pinned_at DESC,
            coalesce(c.last_message_at,p.created_at) DESC, p.id"
        ))?;
        let companions = query
            .query_map([], view)?
            .collect::<rusqlite::Result<_>>()?;
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

    pub(crate) fn read_companion_through(&self, id: &str, ordinal: i64) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let current = profile(tx, id)?;
            tx.execute(
                "UPDATE companion_profiles SET last_read_ordinal=max(last_read_ordinal,
                coalesce((SELECT max(ordinal) FROM messages WHERE conversation_id=?3 AND ordinal<=?2
                    AND role='assistant' AND status='completed'),0)) WHERE id=?1",
                params![id, ordinal, current.conversation_id],
            )?;
            profile(tx, id)
        })
    }

    pub(crate) fn archive_companion(&self, id: &str) -> Result<CompanionView> {
        self.companion_transaction(|tx| {
            let record = profile(tx, id)?;
            require_idle(&record)?;
            tx.execute(
                "UPDATE companion_profiles SET archived_at=coalesce(archived_at,?2),updated_at=?2
                WHERE id=?1",
                params![id, now_ms()],
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
        f.db.with_connection(|c| {
            for (ord,role,status) in [(1,"user","completed"),(2,"assistant","completed"),(3,"assistant","completed"),(4,"assistant","streaming")] {
                c.execute("INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                    VALUES (?1,?2,?3,'text','message',?4,?5,1,1)",
                    params![Uuid::new_v4().to_string(),first.conversation_id,role,status,ord])?;
            }
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
        assert_eq!(f.db.read_companion_through(id, 2).unwrap().unread_count, 1);
        assert_eq!(
            f.db.read_companion_through(id, 1)
                .unwrap()
                .profile
                .last_read_ordinal,
            2
        );
        f.db.archive_companion(id).unwrap();
        assert_eq!(f.add("fox-security").profile.display_name, "Fox");
        let restored = f.db.restore_companion(id).unwrap();
        assert_eq!(restored.conversation_id, first.conversation_id);
        assert_eq!(restored.profile.display_name, "Fox（3）");
        assert!(restored.profile.pinned_at.is_some());
        assert_eq!(restored.profile.last_read_ordinal, 2);
        assert_eq!(
            f.db.read_companion_through(id, 1000)
                .unwrap()
                .profile
                .last_read_ordinal,
            3
        );
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
}

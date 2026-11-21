// Ported from OpenMausBot @ 1a4e4da9f380804ad98c7fd1b141bb0dbfafced4: server/runtime/{types,runtime,tree,questions}.ts and server/runtime-coordinator.ts (Apache-2.0).
use super::{
    companion_ownership::resolve_run_on, create_run_in_transaction, now_ms, Database,
    DEFAULT_AGENT_ID,
};
use crate::database::{CoordinationConversation, StartRunResult};
use rusqlite::{
    params, Connection, OptionalExtension, Params, Row, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

const MAX_DEPTH: i64 = 4;
const MAX_REQUESTS: i64 = 24;
const MAX_EXECUTIONS: i64 = 48;
const MAX_WAKES: i64 = 48;
const MINUTE: i64 = 60_000;
const QUEUE_MS: i64 = 24 * 60 * MINUTE;
const STALL_MS: i64 = 4 * 60 * MINUTE;
const TURN_STALL_MS: i64 = 20 * MINUTE;
const NO_PROGRESS_STRIKES: usize = 2;
const SAME_ERROR_STRIKES: usize = 3;
const OPEN: &str = "('queued','running','needs_input','waiting','resume')";
const TERMINAL: &str = "('completed','failed','cancelled')";
const RUN_ACTIVE: &str = "('queued','running','cancelling','awaiting_confirmation')";
const COLUMNS: &str =
    "a.id,a.root_run_id,a.parent_id,a.depth,a.source_conversation_id,a.source_run_id,
    a.source_companion_id,a.source_name,a.recipient_conversation_id,a.recipient_companion_id,
    a.recipient_name,a.reply,a.consult,a.rework,a.title,a.instructions,a.done_when,a.status,
    a.delivery_status,a.recipient_run_id,a.resume_run_id,a.result,a.question,a.default_answer,
    a.permission_mode,a.created_at,a.updated_at,a.resume_error";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    #[default]
    Resume,
    None,
}

impl Reply {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CoordinationRequest {
    pub targets: Vec<String>,
    pub instructions: String,
    #[serde(default)]
    pub reply: Reply,
    #[serde(default)]
    pub consult: bool,
    #[serde(default)]
    pub rework: bool,
    pub title: Option<String>,
    pub done_when: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignmentQuestion {
    pub prompt: String,
    pub default_answer: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assignment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<AssignmentMessage>>,
    pub id: String,
    pub root_run_id: String,
    pub parent_id: Option<String>,
    pub depth: i64,
    pub source_conversation_id: String,
    pub source_run_id: String,
    pub source_companion_id: Option<String>,
    pub source_name: String,
    pub recipient_conversation_id: String,
    pub recipient_companion_id: String,
    pub recipient_name: String,
    pub reply: String,
    pub consult: bool,
    pub rework: bool,
    pub title: String,
    pub instructions: String,
    pub done_when: Option<String>,
    pub status: String,
    pub delivery_status: String,
    pub recipient_run_id: Option<String>,
    pub resume_run_id: Option<String>,
    pub resume_error: Option<String>,
    pub result: Option<String>,
    pub question: Option<AssignmentQuestion>,
    pub permission_mode: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Assignment {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            messages: None,
            id: row.get(0)?,
            root_run_id: row.get(1)?,
            parent_id: row.get(2)?,
            depth: row.get(3)?,
            source_conversation_id: row.get(4)?,
            source_run_id: row.get(5)?,
            source_companion_id: row.get(6)?,
            source_name: row.get(7)?,
            recipient_conversation_id: row.get(8)?,
            recipient_companion_id: row.get(9)?,
            recipient_name: row.get(10)?,
            reply: row.get(11)?,
            consult: row.get(12)?,
            rework: row.get(13)?,
            title: row.get(14)?,
            instructions: row.get(15)?,
            done_when: row.get(16)?,
            status: row.get(17)?,
            delivery_status: row.get(18)?,
            recipient_run_id: row.get(19)?,
            resume_run_id: row.get(20)?,
            result: row.get(21)?,
            question: row
                .get::<_, Option<String>>(22)?
                .map(|prompt| {
                    Ok::<_, rusqlite::Error>(AssignmentQuestion {
                        prompt,
                        default_answer: row.get(23)?,
                    })
                })
                .transpose()?,
            permission_mode: row.get(24)?,
            created_at: row.get(25)?,
            updated_at: row.get(26)?,
            resume_error: row.get(27)?,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignmentMessage {
    id: String,
    direction: String,
    text: String,
    created_at: i64,
}

fn assignment_messages(conn: &Connection, assignment: &Assignment) -> rusqlite::Result<Vec<AssignmentMessage>> {
    let mut messages = vec![AssignmentMessage {
        id: format!("{}:sent", assignment.id), direction: "sent".into(),
        text: assignment.instructions.clone(), created_at: assignment.created_at,
    }];
    let mut query = conn.prepare(
        "SELECT c.run_id,c.answer,r.created_at,m.id,m.content,m.created_at
         FROM companion_coordination_runs c JOIN runs r ON r.id=c.run_id
         LEFT JOIN messages m ON r.status='completed' AND m.id=(SELECT id FROM messages
           WHERE run_id=r.id AND role='assistant' AND length(trim(content))>0
           ORDER BY ordinal DESC LIMIT 1)
         WHERE c.assignment_id=?1 AND r.conversation_id=?2
         ORDER BY r.created_at,r.rowid"
    )?;
    let rows = query.query_map(params![assignment.id, assignment.recipient_conversation_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<i64>>(5)?))
    })?;
    for row in rows {
        let (run, answer, started, id, text, created) = row?;
        if let Some(text) = answer {
            messages.push(AssignmentMessage { id: format!("{run}:answer"), direction: "sent".into(), text, created_at: started });
        }
        if let (Some(id), Some(text), Some(created_at)) = (id, text, created) {
            if assignment.reply != "none" {
                let text = needs_input(&text).map(|question| question.prompt).unwrap_or(text);
                messages.push(AssignmentMessage { id, direction: "received".into(), text, created_at });
            }
        }
    }
    Ok(messages)
}

pub(crate) struct CoordinationRunContext {
    pub root_run_id: String,
    pub incoming: Option<Assignment>,
    pub returned: Vec<Assignment>,
    pub consult: bool,
    pub permission_mode: String,
    pub answer: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CoordinationDispatch {
    pub started: StartRunResult,
    pub assignment_ids: Vec<String>,
}

struct Recipient {
    id: String,
    name: String,
    workspace: String,
    permission: String,
}

fn refusal(message: &'static str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(format!("companion.coordination: {message}"))
}

fn assignment_on(conn: &Connection, id: &str) -> rusqlite::Result<Option<Assignment>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM companion_assignments a WHERE id=?1"),
        [id],
        Assignment::from_row,
    )
    .optional()
}

fn assignments_on(
    conn: &Connection,
    clause: &str,
    values: impl Params,
) -> rusqlite::Result<Vec<Assignment>> {
    let mut query = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM companion_assignments a WHERE {clause} ORDER BY a.created_at,a.rowid"
    ))?;
    let rows = query.query_map(values, Assignment::from_row)?;
    rows.collect()
}

pub(crate) fn thread_on(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<CoordinationConversation>> {
    conn.query_row(
        "SELECT source_conversation_id,source_companion_id,source_name,recipient_companion_id
         FROM companion_coordination_threads WHERE conversation_id=?1",
        [id],
        |row| {
            Ok(CoordinationConversation {
                source_conversation_id: row.get(0)?,
                source_companion_id: row.get(1)?,
                source_name: row.get(2)?,
                recipient_companion_id: row.get(3)?,
            })
        },
    )
    .optional()
}

pub(crate) fn usage_root_on(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT usage_root_id FROM run_usage_roots WHERE run_id=?1 AND coordinated",
        [run_id],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn permission_mode_on(
    conn: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT cr.permission_mode FROM runs r JOIN companion_coordination_runs cr
         ON cr.run_id=coalesce(r.root_run_id,r.id) WHERE r.id=?1",
        [run_id],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn consultation_on(conn: &Connection, run_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs r JOIN companion_coordination_runs cr
         ON cr.run_id=coalesce(r.root_run_id,r.id)
         JOIN companion_assignments a ON a.id=cr.assignment_id
         WHERE r.id=?1 AND a.consult=1)",
        [run_id],
        |row| row.get(0),
    )
}

fn validate(input: &CoordinationRequest) -> rusqlite::Result<()> {
    if !(1..=4).contains(&input.targets.len())
        || input.targets.iter().any(|target| target.trim().is_empty())
    {
        return Err(refusal("请提供 1 到 4 位伙伴"));
    }
    if input.instructions.trim().is_empty() || input.instructions.chars().count() > 4000 {
        return Err(refusal("派活说明需要 1 到 4000 字"));
    }
    if input
        .title
        .as_ref()
        .is_some_and(|title| title.chars().count() > 80)
        || input
            .done_when
            .as_ref()
            .is_some_and(|done| done.chars().count() > 1000)
    {
        return Err(refusal("标题最多 80 字，完成标准最多 1000 字"));
    }
    Ok(())
}

fn folded(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('@')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn recipients(conn: &Connection, requested: &[String]) -> rusqlite::Result<Vec<Recipient>> {
    let mut query = conn.prepare(
        "SELECT p.id,p.display_name,p.workspace_dir,c.permission_mode
         FROM companion_profiles p JOIN conversations c ON c.companion_id=p.id
         WHERE p.archived_at IS NULL AND p.deleting_at IS NULL",
    )?;
    let peers = query
        .query_map([], |row| {
            Ok(Recipient {
                id: row.get(0)?,
                name: row.get(1)?,
                workspace: row.get(2)?,
                permission: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut selected = Vec::new();
    let mut unique = HashSet::new();
    for target in requested {
        let raw = target.trim();
        let peer = if let Some(peer) = peers.iter().find(|peer| peer.id == raw) {
            peer
        } else {
            let wanted = folded(raw);
            let mut matches = peers.iter().filter(|peer| folded(&peer.name) == wanted);
            let resolved = matches
                .next()
                .ok_or_else(|| refusal("找不到活动伙伴，请用名单中的伙伴 ID"))?;
            if matches.next().is_some() {
                return Err(refusal("有多个同名伙伴，请用伙伴 ID"));
            }
            resolved
        };
        if !unique.insert(peer.id.as_str()) {
            return Err(refusal("同一次派活不能重复指定同一个伙伴"));
        }
        selected.push(Recipient {
            id: peer.id.clone(),
            name: peer.name.clone(),
            workspace: peer.workspace.clone(),
            permission: peer.permission.clone(),
        });
    }
    Ok(selected)
}

fn permission_cap(source: &str, target: &str, consult: bool) -> &'static str {
    if consult || source == "read_only" || target == "read_only" {
        "read_only"
    } else if source == "ask" || target == "ask" {
        "ask"
    } else {
        "allow"
    }
}

fn standing_conversation(
    tx: &Transaction<'_>,
    source_chat: &str,
    source_id: Option<&str>,
    source_name: &str,
    recipient: &Recipient,
) -> rusqlite::Result<String> {
    let source_key = match source_id {
        Some(id) => format!("companion:{id}"),
        None => format!("conversation:{source_chat}"),
    };
    let existing = tx.query_row(
        "SELECT conversation_id FROM companion_coordination_threads WHERE source_key=?1 AND recipient_companion_id=?2",
        params![source_key, recipient.id], |row| row.get::<_, String>(0),
    ).optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    let title = format!(
        "@{} · 协作",
        source_name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(40)
            .collect::<String>()
    );
    let now = now_ms();
    tx.execute(
        "INSERT INTO conversations(id,agent_id,title,project_root,permission_mode,status,
         lineage_root_id,created_at,updated_at,agent_backend)
         VALUES(?1,?2,?3,?4,?5,'active',?1,?6,?6,'fox')",
        params![
            id,
            DEFAULT_AGENT_ID,
            title,
            recipient.workspace,
            recipient.permission,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO companion_coordination_threads(conversation_id,source_key,source_conversation_id,
             source_companion_id,source_name,recipient_companion_id) VALUES(?1,?2,
             COALESCE((SELECT id FROM conversations WHERE companion_id=?4),?3),?4,?5,?6)",
        params![id, source_key, source_chat, source_id, source_name, recipient.id],
    )?;
    Ok(id)
}

fn ensure_destinations(
    conn: &Connection,
    source_id: Option<&str>,
    parent_id: Option<&str>,
    recipient: &str,
    root: &str,
    source_run: &str,
    instructions: &str,
    rework: bool,
) -> rusqlite::Result<()> {
    if source_id == Some(recipient) {
        return Err(refusal("不能把工作派给自己"));
    }
    let previous: Option<String> = conn.query_row(
        "SELECT status FROM companion_assignments WHERE root_run_id=?1 AND recipient_companion_id=?2
         AND (parent_id IS ?3 OR source_run_id=?4) AND instructions=?5
         ORDER BY created_at DESC,rowid DESC LIMIT 1",
        params![root, recipient, parent_id, source_run, instructions], |row| row.get(0),
    ).optional()?;
    if previous
        .as_deref()
        .is_some_and(|status| !rework || !is_terminal(status))
    {
        return Err(refusal(
            "已经派过同样的工作；已结束的工作需要明确 rework 才能重派",
        ));
    }
    if let Some(parent_id) = parent_id {
        let ancestor = conn.query_row(
            "WITH RECURSIVE ancestors AS (
                SELECT id,parent_id,source_companion_id,recipient_companion_id FROM companion_assignments WHERE id=?1
                UNION ALL
                SELECT a.id,a.parent_id,a.source_companion_id,a.recipient_companion_id
                FROM companion_assignments a JOIN ancestors n ON a.id=n.parent_id
             ) SELECT EXISTS(SELECT 1 FROM ancestors WHERE source_companion_id=?2 OR recipient_companion_id=?2)",
            params![parent_id, recipient], |row| row.get::<_, bool>(0),
        )?;
        if ancestor {
            return Err(refusal("不能把工作派回上游，成果会自动返回"));
        }
    }
    if let Some(source_id) = source_id {
        let crossed = conn.query_row(
            &format!("WITH RECURSIVE waiting(id) AS (
              SELECT ?1 UNION
              SELECT a.recipient_companion_id FROM companion_assignments a JOIN waiting w ON a.source_companion_id=w.id
              WHERE a.reply='resume' AND a.status IN {OPEN}
            ) SELECT EXISTS(SELECT 1 FROM waiting WHERE id=?2)"),
            params![recipient, source_id], |row| row.get::<_, bool>(0),
        )?;
        if crossed {
            return Err(refusal("接收方已经在等待你的工作，不能交叉等待"));
        }
    }
    Ok(())
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

impl Database {
    pub(crate) fn enqueue_companion_coordination(
        &self,
        source_conversation_id: &str,
        source_run_id: &str,
        request: CoordinationRequest,
    ) -> Result<Vec<Assignment>, String> {
        validate(&request).map_err(|error| error.to_string())?;
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let assignments = enqueue(&tx, source_conversation_id, source_run_id, request)?;
            tx.commit()?;
            Ok(assignments)
        })
    }

    pub(crate) fn companion_assignment_status(
        &self,
        conversation_id: &str,
        current_run_id: &str,
        id: Option<&str>,
    ) -> Result<Vec<Assignment>, String> {
        self.with_connection(|conn| {
            let early = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM companion_assignments WHERE source_conversation_id=?1 AND source_run_id=?2 AND id=?3)",
                params![conversation_id, current_run_id, id], |row| row.get::<_, bool>(0),
            )?;
            if early {
                return Err(refusal("同一回合刚派出的工作会自动回报，请勿轮询"));
            }
            assignments_on(conn, &format!(
                "a.source_conversation_id=?1 AND a.source_run_id<>?2 AND
                 ((?3 IS NOT NULL AND a.id=?3) OR (?3 IS NULL AND a.status IN {OPEN}))"
            ), params![conversation_id,current_run_id,id])
        })
    }

    pub(crate) fn companion_assignments_list(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<Assignment>, String> {
        self.with_connection(|conn| {
            let mut assignments = assignments_on(
                conn,
                "a.source_conversation_id=?1 OR a.recipient_conversation_id=?1 OR EXISTS(
              SELECT 1 FROM conversations c WHERE c.id=?1 AND
              c.companion_id IN (a.source_companion_id,a.recipient_companion_id)
            )",
                [conversation_id],
            )?;
            for assignment in &mut assignments {
                assignment.messages = Some(assignment_messages(conn, assignment)?);
            }
            Ok(assignments)
        })
    }

    pub(crate) fn companion_coordination_context(
        &self,
        run_id: &str,
    ) -> Result<Option<CoordinationRunContext>, String> {
        self.with_connection(|conn| {
            let Some((root_run_id, incoming_id, permission_mode, answer)) = conn.query_row(
                "SELECT root_run_id,assignment_id,permission_mode,answer FROM companion_coordination_runs WHERE run_id=?1",
                [run_id], |row| Ok((row.get(0)?,row.get::<_, Option<String>>(1)?,row.get(2)?,row.get(3)?)),
            ).optional()? else { return Ok(None); };
            let incoming = incoming_id.map(|id| assignment_on(conn, &id)).transpose()?.flatten();
            let returned = assignments_on(conn, "a.resume_run_id=?1", [run_id])?;
            Ok(Some(CoordinationRunContext {
                root_run_id, consult: incoming.as_ref().is_some_and(|assignment| assignment.consult),
                incoming, returned, permission_mode, answer,
            }))
        })
    }
}

fn enqueue(
    tx: &Transaction<'_>,
    source_chat: &str,
    source_run: &str,
    request: CoordinationRequest,
) -> rusqlite::Result<Vec<Assignment>> {
    let (source_permission, child): (String, bool) = tx
        .query_row(
            "SELECT c.permission_mode,r.run_kind='child' OR r.parent_run_id IS NOT NULL
         FROM runs r JOIN conversations c ON c.id=r.conversation_id
         WHERE r.id=?1 AND c.id=?2 AND r.status='running'",
            params![source_run, source_chat],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| refusal("发起回合已结束"))?;
    let scope = resolve_run_on(tx, source_run).map_err(rusqlite::Error::InvalidParameterName)?;
    if child
        || scope.as_ref().is_some_and(|scope| scope.is_child)
        || consultation_on(tx, source_run)?
    {
        return Err(refusal("子运行和只读咨询不能派活"));
    }
    let source_companion_id = scope.as_ref().map(|scope| scope.companion_id.as_str());
    let source_name = if let Some(id) = source_companion_id {
        tx.query_row(
            "SELECT display_name FROM companion_profiles WHERE id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )?
    } else {
        "工作对话".to_owned()
    };
    let root_run_id = usage_root_on(tx, source_run)?.unwrap_or_else(|| source_run.to_owned());
    let source_permission = permission_mode_on(tx, source_run)?.unwrap_or(source_permission);
    let parent_id = tx
        .query_row(
            "SELECT assignment_id FROM companion_coordination_runs WHERE run_id=?1",
            [source_run],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let depth = if let Some(parent_id) = &parent_id {
        tx.query_row(
            "SELECT depth+1 FROM companion_assignments WHERE id=?1",
            [parent_id],
            |row| row.get::<_, i64>(0),
        )?
    } else {
        1
    };
    if depth > MAX_DEPTH {
        return Err(refusal("派活链最多 4 层"));
    }
    let wanted = recipients(tx, &request.targets)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM companion_assignments WHERE root_run_id=?1",
        [&root_run_id],
        |row| row.get(0),
    )?;
    if count + wanted.len() as i64 > MAX_REQUESTS {
        return Err(refusal("同一个源请求最多派活 24 次"));
    }
    let title = request
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("伙伴协作");
    let brief = request.instructions.trim();
    let now = now_ms();
    let mut assignments = Vec::new();
    for target in wanted {
        ensure_destinations(
            tx,
            source_companion_id,
            parent_id.as_deref(),
            &target.id,
            &root_run_id,
            source_run,
            brief,
            request.rework,
        )?;
        let target_chat =
            standing_conversation(tx, source_chat, source_companion_id, &source_name, &target)?;
        let id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO companion_assignments(id,root_run_id,parent_id,depth,source_conversation_id,source_run_id,
             source_companion_id,source_name,recipient_conversation_id,recipient_companion_id,recipient_name,
             permission_mode,reply,consult,rework,title,instructions,done_when,source_permission_mode,
             status,delivery_status,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,'queued','queued',?20,?20)",
            params![id,root_run_id,parent_id,depth,source_chat,source_run,source_companion_id,source_name,target_chat,
                target.id,target.name,permission_cap(&source_permission,&target.permission,request.consult),request.reply.as_str(),
                request.consult,request.rework,title,brief,request.done_when,source_permission,now],
        )?;
        assignments.push(assignment_on(tx, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?);
    }
    Ok(assignments)
}

#[derive(Debug, Default)]
pub(crate) struct CoordinationReconcile {
    pub changed: Vec<Assignment>,
    pub cancel_run_ids: Vec<String>,
}

impl Database {
    pub(crate) fn pending_companion_assignments(&self) -> Result<Vec<Assignment>, String> {
        self.with_connection(|conn| assignments_on(conn,
            "(a.status='queued' AND a.recipient_run_id IS NULL) OR (a.status='resume' AND a.answer IS NOT NULL)",
            [],
        ))
    }

    pub(crate) fn claim_companion_assignment(
        &self,
        id: &str,
    ) -> Result<Option<CoordinationDispatch>, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = claim_delivery(&tx, id)?;
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn fail_companion_assignment(&self, id: &str, reason: &str) -> Result<(), String> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE companion_assignments SET status='failed', delivery_status=CASE
                 WHEN delivery_status='queued' THEN 'failed' ELSE delivery_status END,
                 result=?2,updated_at=?3 WHERE id=?1 AND status IN ('queued','resume','running')",
                params![id, clipped(reason, 12_000), now_ms()],
            )?;
            Ok(())
        })
    }

    pub(crate) fn pending_companion_resumes(&self) -> Result<Vec<Vec<Assignment>>, String> {
        self.with_connection(|conn| ready_resume_batches(conn))
    }

    pub(crate) fn claim_companion_resume(
        &self,
        ids: &[String],
    ) -> Result<Option<CoordinationDispatch>, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = claim_resume(&tx, ids)?;
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn answer_companion_assignment(
        &self,
        id: &str,
        answer: &str,
    ) -> Result<Assignment, String> {
        let answer = answer.trim();
        if answer.is_empty() || answer.chars().count() > 4000 {
            return Err(refusal("回答需要 1 到 4000 字").to_string());
        }
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed = tx.execute(
                "UPDATE companion_assignments SET answer=?2,question=NULL,default_answer=NULL,
                 recipient_run_id=NULL,status='resume',updated_at=?3
                 WHERE id=?1 AND status='needs_input' AND question IS NOT NULL",
                params![id, answer, now_ms()],
            )?;
            if changed == 0 {
                return Err(refusal("此协作任务没有等待回答的提问"));
            }
            let assignment = assignment_on(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            tx.commit()?;
            Ok(assignment)
        })
    }

    pub(crate) fn cancel_companion_assignment(&self, id: &str) -> Result<Assignment, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let assignment = assignment_on(&tx, id)?.ok_or_else(|| refusal("没有找到此协作任务"))?;
            if assignment.status == "cancelled" {
                return Ok(assignment);
            }
            let cancellable = (assignment.status == "queued" && assignment.recipient_run_id.is_none())
                || (assignment.status == "needs_input" && assignment.question.is_some());
            let active = tx.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status IN {RUN_ACTIVE})"),
                [assignment.recipient_run_id.as_deref()],
                |row| row.get::<_, bool>(0),
            )?;
            if !cancellable || active {
                return Err(refusal("只能取消尚未投递或正在等待补答的协作；执行和审批中的任务请先停止运行"));
            }
            stop_assignment(&tx, id, "cancelled", "用户已取消本次协作", now_ms())?;
            let updated = assignment_on(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            tx.commit()?;
            Ok(updated)
        })
    }

    pub(crate) fn settle_companion_assignment_run(
        &self,
        run_id: &str,
        status: &str,
        text: &str,
    ) -> Result<bool, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = settle_run(&tx, run_id, status, text, now_ms())?.is_some();
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn reconcile_companion_assignments(
        &self,
        now: i64,
    ) -> Result<CoordinationReconcile, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = reconcile(&tx, now)?;
            tx.commit()?;
            Ok(result)
        })
    }
}

fn chat_idle(conn: &Connection, chat: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        &format!("SELECT NOT EXISTS(SELECT 1 FROM runs WHERE conversation_id=?1 AND status IN {RUN_ACTIVE})"),
        [chat], |row| row.get(0),
    )
}

fn root_usage(conn: &Connection, root: &str) -> rusqlite::Result<(i64, i64)> {
    conn.query_row(
        "SELECT 1+count(*),COUNT(*) FILTER(WHERE kind='resume')
         FROM companion_coordination_runs WHERE root_run_id=?1",
        [root],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

fn create_internal_run(
    tx: &Transaction<'_>,
    chat: &str,
    prompt: &str,
) -> rusqlite::Result<StartRunResult> {
    let mut started = create_run_in_transaction(tx, chat, prompt, None)?;
    tx.execute(
        "UPDATE messages SET role='system',kind='internal' WHERE id=?1",
        [&started.user_message.id],
    )?;
    tx.execute(
        "DELETE FROM contact_bubbles WHERE source_message_id=?1",
        [&started.user_message.id],
    )?;
    started.user_message.role = "system".into();
    started.user_message.kind = "internal".into();
    Ok(started)
}

fn claim_delivery(
    tx: &Transaction<'_>,
    id: &str,
) -> rusqlite::Result<Option<CoordinationDispatch>> {
    let Some(assignment) = assignment_on(tx, id)? else {
        return Ok(None);
    };
    let answer: Option<String> = tx.query_row(
        "SELECT answer FROM companion_assignments WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let eligible = (assignment.status == "queued" && assignment.recipient_run_id.is_none())
        || (assignment.status == "resume" && answer.is_some());
    if !eligible || !chat_idle(tx, &assignment.recipient_conversation_id)? {
        return Ok(None);
    }
    if root_usage(tx, &assignment.root_run_id)?.0 >= MAX_EXECUTIONS {
        return Err(refusal("协作链已达到 48 次执行上限"));
    }
    let target_permission: String = tx.query_row(
        "SELECT c.permission_mode FROM companion_profiles p JOIN conversations c ON c.companion_id=p.id
         WHERE p.id=?1 AND p.archived_at IS NULL AND p.deleting_at IS NULL",
        [&assignment.recipient_companion_id], |r| r.get(0),
    ).optional()?.ok_or_else(|| refusal("接收伙伴已不可用"))?;
    let cap = permission_cap(
        &assignment.permission_mode,
        &target_permission,
        assignment.consult,
    );
    let started = create_internal_run(
        tx,
        &assignment.recipient_conversation_id,
        "处理伙伴派来的协作事项",
    )?;
    tx.execute(
        "INSERT INTO companion_coordination_runs(run_id,root_run_id,assignment_id,kind,permission_mode,answer)
         VALUES (?1,?2,?3,'delivery',?4,?5)",
        params![started.run.id,assignment.root_run_id,id,cap,answer],
    )?;
    tx.execute(
        "UPDATE companion_assignments SET recipient_run_id=?2,status='queued',answer=NULL,updated_at=?3 WHERE id=?1",
        params![id,started.run.id,now_ms()],
    )?;
    Ok(Some(CoordinationDispatch {
        started,
        assignment_ids: vec![id.to_owned()],
    }))
}

fn resume_batch(conn: &Connection, source_run: &str) -> rusqlite::Result<Option<Vec<Assignment>>> {
    let result = assignments_on(
        conn,
        "a.source_run_id=?1 AND a.reply='resume' AND a.resume_run_id IS NULL AND a.resume_error IS NULL
         AND EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status IN ('completed','failed'))
         AND (a.parent_id IS NULL OR EXISTS(SELECT 1 FROM companion_assignments p
             WHERE p.id=a.parent_id AND p.status='waiting' AND p.recipient_run_id=?1))",
        [source_run],
    )?;
    if result.is_empty()
        || result
            .iter()
            .any(|assignment| !is_terminal(&assignment.status))
    {
        return Ok(None);
    }
    Ok(Some(result))
}

fn ready_resume_batches(conn: &Connection) -> rusqlite::Result<Vec<Vec<Assignment>>> {
    let mut query = conn.prepare(
        "SELECT a.source_run_id FROM companion_assignments a JOIN runs r ON r.id=a.source_run_id
         WHERE a.reply='resume' AND a.resume_run_id IS NULL AND a.resume_error IS NULL
           AND r.status IN ('completed','failed')
         GROUP BY a.source_run_id ORDER BY min(a.created_at)",
    )?;
    let runs = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut groups = Vec::new();
    for run in runs {
        if let Some(group) = resume_batch(conn, &run)? {
            groups.push(group);
        }
    }
    Ok(groups)
}

fn resume_refusal(
    conn: &Connection,
    batch: &[Assignment],
    now: i64,
) -> rusqlite::Result<Option<&'static str>> {
    let first = &batch[0];
    let (executions, wakes) = root_usage(conn, &first.root_run_id)?;
    if executions >= MAX_EXECUTIONS || wakes >= MAX_WAKES {
        return Ok(Some("协作链已达到执行或唤醒上限"));
    }
    let active = conn
        .query_row(
            "SELECT c.archived=0 AND c.trashed_at IS NULL AND NOT EXISTS(
            SELECT 1 FROM companion_profiles p WHERE p.id=?2
              AND (p.archived_at IS NOT NULL OR p.deleting_at IS NOT NULL)
         ) FROM conversations c WHERE c.id=?1",
            params![first.source_conversation_id, first.source_companion_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if !active {
        return Ok(Some("发起对话或伙伴已关闭"));
    }
    let last_update = batch
        .iter()
        .map(|item| item.updated_at)
        .max()
        .unwrap_or(now);
    Ok((now.saturating_sub(last_update) >= STALL_MS).then_some("回报已等待自动恢复超过 4 小时"))
}

fn suppress_resume(
    tx: &Transaction<'_>,
    batch: &[Assignment],
    reason: &str,
    now: i64,
) -> rusqlite::Result<Vec<Assignment>> {
    let mut changed = Vec::new();
    for item in batch {
        tx.execute(
            "UPDATE companion_assignments SET resume_error=?2,updated_at=?3 WHERE id=?1",
            params![item.id, reason, now],
        )?;
        changed.extend(assignment_on(tx, &item.id)?);
    }
    if let Some(parent) = &batch[0].parent_id {
        let updated = tx.execute(
            "UPDATE companion_assignments SET status='failed',result=?2,updated_at=?3
                                  WHERE id=?1 AND status='waiting'",
            params![parent, reason, now],
        )?;
        if updated != 0 {
            changed.extend(assignment_on(tx, parent)?);
        }
    }
    Ok(changed)
}

fn claim_resume(
    tx: &Transaction<'_>,
    ids: &[String],
) -> rusqlite::Result<Option<CoordinationDispatch>> {
    let Some(first_id) = ids.first() else {
        return Ok(None);
    };
    let Some(first) = assignment_on(tx, first_id)? else {
        return Ok(None);
    };
    let Some(batch) = resume_batch(tx, &first.source_run_id)? else {
        return Ok(None);
    };
    if batch.len() != ids.len() || batch.iter().any(|assignment| !ids.contains(&assignment.id)) {
        return Ok(None);
    }
    let now = now_ms();
    if let Some(reason) = resume_refusal(tx, &batch, now)? {
        suppress_resume(tx, &batch, reason, now)?;
        return Ok(None);
    }
    if !chat_idle(tx, &first.source_conversation_id)? {
        return Ok(None);
    }
    let current_permission: String = tx.query_row(
        "SELECT permission_mode FROM conversations WHERE id=?1",
        [&first.source_conversation_id],
        |row| row.get(0),
    )?;
    let source_permission: String = tx.query_row(
        "SELECT source_permission_mode FROM companion_assignments WHERE id=?1",
        [&first.id],
        |row| row.get(0),
    )?;
    let mode = permission_cap(&source_permission, &current_permission, false);
    let started = create_internal_run(
        tx,
        &first.source_conversation_id,
        "继续处理伙伴已回报的协作事项",
    )?;
    tx.execute(
        "INSERT INTO companion_coordination_runs(run_id,root_run_id,assignment_id,kind,permission_mode)
         VALUES (?1,?2,?3,'resume',?4)",
        params![started.run.id,first.root_run_id,first.parent_id,mode],
    )?;
    for item in &batch {
        tx.execute(
            "UPDATE companion_assignments SET resume_run_id=?2,updated_at=?3 WHERE id=?1",
            params![item.id, started.run.id, now],
        )?;
    }
    if let Some(parent) = first.parent_id {
        tx.execute(
            "UPDATE companion_assignments SET recipient_run_id=?2,status='resume',updated_at=?3
                    WHERE id=?1 AND status='waiting'",
            params![parent, started.run.id, now],
        )?;
    }
    Ok(Some(CoordinationDispatch {
        started,
        assignment_ids: ids.to_owned(),
    }))
}

fn clipped(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn needs_input(text: &str) -> Option<AssignmentQuestion> {
    let request = text.lines().map(str::trim).find_map(|line| {
        let (label, body) = line.split_once([':', '：', '—'])?;
        let label = label.trim();
        if label.eq_ignore_ascii_case("needs input") || label == "需要输入" {
            Some(body.trim())
        } else {
            None
        }
    })?;
    let same_line = request
        .split_once("默认：")
        .or_else(|| request.split_once("默认:"))
        .or_else(|| {
            let index = request.to_ascii_lowercase().find("default:")?;
            Some((&request[..index], &request[index + "default:".len()..]))
        });
    let (prompt, inline_default) = same_line.unwrap_or((request, ""));
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return None;
    }
    let default_answer = if inline_default.trim().is_empty() {
        text.lines().map(str::trim).find_map(|line| {
            let (label, body) = line.split_once([':', '：'])?;
            if label.eq_ignore_ascii_case("default") || label == "默认" {
                Some(body.trim())
            } else {
                None
            }
        })
    } else {
        Some(inline_default.trim())
    };
    Some(AssignmentQuestion {
        prompt: clipped(prompt, 2000),
        default_answer: default_answer
            .filter(|value| !value.is_empty())
            .map(|value| clipped(value, 1000)),
    })
}

#[derive(Default, Deserialize, Serialize)]
struct LoopGuard {
    stale: usize,
    errors: Vec<String>,
}

fn continuation_guard(
    tx: &Transaction<'_>,
    assignment_id: &str,
    run_id: &str,
) -> rusqlite::Result<Option<String>> {
    let stored: String = tx.query_row(
        "SELECT guard_json FROM companion_assignments WHERE id=?1",
        [assignment_id],
        |r| r.get(0),
    )?;
    let mut guard: LoopGuard = serde_json::from_str(&stored).unwrap_or_default();
    let progressed = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_calls WHERE run_id=?1 AND status='completed'
         AND tool_name NOT IN ('companion_coordinate','companion_assignment_status'))",
        [run_id],
        |r| r.get::<_, bool>(0),
    )?;
    let mut query = tx.prepare(
        "SELECT error_message FROM tool_calls
         WHERE run_id=?1 AND error_message IS NOT NULL AND trim(error_message)<>''
         ORDER BY started_at DESC,id DESC LIMIT 3",
    )?;
    let err = query
        .query_map([run_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    guard.stale = if progressed { 0 } else { guard.stale + 1 };
    guard
        .errors
        .extend(err.iter().rev().map(|text| clipped(text.trim(), 200)));
    if guard.errors.len() > SAME_ERROR_STRIKES {
        guard
            .errors
            .drain(..guard.errors.len() - SAME_ERROR_STRIKES);
    }
    let serialized = serde_json::to_string(&guard)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    tx.execute(
        "UPDATE companion_assignments SET guard_json=?2 WHERE id=?1",
        params![assignment_id, serialized],
    )?;
    if guard.errors.len() == SAME_ERROR_STRIKES
        && guard.errors.windows(2).all(|pair| pair[0] == pair[1])
    {
        Ok(Some(format!(
            "连续 3 次出现相同错误，已结束协作：{}",
            guard.errors[0]
        )))
    } else if guard.stale >= NO_PROGRESS_STRIKES {
        Ok(Some(
            "连续 2 次继续执行都没有可观察的进展，已结束协作".into(),
        ))
    } else {
        Ok(None)
    }
}

fn settle_run(
    tx: &Transaction<'_>,
    run_id: &str,
    status: &str,
    text: &str,
    now: i64,
) -> rusqlite::Result<Option<Assignment>> {
    let current = assignments_on(
        tx,
        "a.recipient_run_id=?1 AND (a.status IN ('queued','running','resume') OR
         (a.status='needs_input' AND a.question IS NULL))",
        [run_id],
    )?;
    let Some(assignment) = current.first() else {
        return Ok(None);
    };
    let has_children = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM companion_assignments
         WHERE source_run_id=?1 AND reply='resume' AND resume_run_id IS NULL)",
        [run_id],
        |r| r.get::<_, bool>(0),
    )?;
    let result = clipped(text.trim(), 12_000);
    let question = (status == "completed").then(|| needs_input(&result)).flatten();
    let (next, next_result, question) = match status {
        "completed" if question.is_some() && assignment.source_companion_id.is_some() => {
            ("failed", Some(result), None)
        }
        "completed" if question.is_some() => ("needs_input", None, question),
        "completed" | "failed" if has_children => {
            match continuation_guard(tx, &assignment.id, run_id)? {
                Some(stopped) => ("failed", Some(stopped), None),
                None => ("waiting", None, None),
            }
        }
        "completed" if result.is_empty() => (
            "failed",
            Some("接收伙伴结束运行时没有返回可用结果".into()),
            None,
        ),
        "completed" => ("completed", Some(result), None),
        "cancelled" => (
            "cancelled",
            Some(if result.is_empty() {
                "协作已取消".into()
            } else {
                result
            }),
            None,
        ),
        "interrupted" | "failed" => (
            "failed",
            Some(if result.is_empty() {
                "接收伙伴未能完成协作".into()
            } else {
                result
            }),
            None,
        ),
        _ => return Ok(None),
    };
    let (prompt, default_answer) = question
        .as_ref()
        .map(|q| (Some(q.prompt.as_str()), q.default_answer.as_deref()))
        .unwrap_or((None, None));
    tx.execute(
        "UPDATE companion_assignments SET status=?2,result=?3,question=?4,default_answer=?5,
         delivery_status=CASE WHEN delivery_status='queued' AND ?2 IN ('failed','cancelled') THEN 'failed'
                              ELSE 'delivered' END,updated_at=?6 WHERE id=?1",
        params![assignment.id,next,next_result,prompt,default_answer,now],
    )?;
    assignment_on(tx, &assignment.id)
}

struct RunActivity {
    status: String,
    progress_at: i64,
    pending_user: bool,
    result: String,
}

fn run_activity(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<RunActivity>> {
    conn.query_row(
        "SELECT r.status,coalesce((SELECT max(created_at) FROM run_events WHERE run_id=r.id),r.started_at,r.created_at),
          EXISTS(SELECT 1 FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
                 WHERE t.run_id=r.id AND a.status='pending')
          OR coalesce((SELECT event_type='user.question.requested' FROM run_events WHERE run_id=r.id
               AND event_type IN ('user.question.requested','user.question.responded') ORDER BY seq DESC LIMIT 1),0),
          coalesce(CASE WHEN r.status='completed' THEN (
              SELECT content FROM messages WHERE run_id=r.id AND role='assistant'
                AND length(trim(content))>0 ORDER BY ordinal DESC LIMIT 1
          ) ELSE r.error_message END,'')
         FROM runs r WHERE r.id=?1",
        [run_id], |r| Ok(RunActivity {status:r.get(0)?,progress_at:r.get(1)?,pending_user:r.get(2)?,result:r.get(3)?}),
    ).optional()
}

fn fail_stalled_roots(
    tx: &Transaction<'_>,
    now: i64,
    outcome: &mut CoordinationReconcile,
) -> rusqlite::Result<()> {
    let mut query = tx.prepare(
        "SELECT a.root_run_id FROM companion_assignments a
         WHERE a.root_run_id IN (
             SELECT root_run_id FROM companion_assignments WHERE status IN ('waiting','resume')
         )
         GROUP BY a.root_run_id
         HAVING max(a.status='needs_input')=0
            AND max(max(a.updated_at, coalesce((SELECT max(e.created_at) FROM run_events e
                WHERE e.run_id IN (a.source_run_id,a.recipient_run_id,a.resume_run_id)),0)))<=?1",
    )?;
    let roots = query
        .query_map([now.saturating_sub(STALL_MS)], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for root in roots {
        let assignments = assignments_on(
            tx,
            &format!("a.root_run_id=?1 AND a.status IN {OPEN}"),
            [&root],
        )?;
        for item in assignments {
            if let Some(run) = item.recipient_run_id.as_deref() {
                let active = tx.query_row(
                    &format!("SELECT status IN {RUN_ACTIVE} FROM runs WHERE id=?1"),
                    [run],
                    |row| row.get::<_, bool>(0),
                )?;
                if active {
                    outcome.cancel_run_ids.push(run.to_owned());
                }
            }
            stop_assignment(tx, &item.id, "failed", "整条协作链超过 4 小时没有进展", now)?;
            outcome.changed.extend(assignment_on(tx, &item.id)?);
        }
    }
    Ok(())
}

fn stop_assignment(
    tx: &Transaction<'_>,
    id: &str,
    status: &str,
    message: &str,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE companion_assignments SET status=?2,result=?3,
         delivery_status=CASE WHEN delivery_status='queued' THEN 'failed' ELSE delivery_status END,
         question=NULL,default_answer=NULL,answer=NULL,updated_at=?4 WHERE id=?1",
        params![id, status, message, now],
    )?;
    Ok(())
}

fn reconcile(tx: &Transaction<'_>, now: i64) -> rusqlite::Result<CoordinationReconcile> {
    let pending = assignments_on(tx, &format!("a.status IN {OPEN}"), [])?;
    let mut outcome = CoordinationReconcile::default();
    for assignment in pending {
        let run = assignment.recipient_run_id.as_deref();
        let activity = run.map(|id| run_activity(tx, id)).transpose()?.flatten();
        let active = activity.as_ref().is_some_and(|record| {
            matches!(
                record.status.as_str(),
                "queued" | "running" | "cancelling" | "awaiting_confirmation"
            )
        });
        let cancelled = tx.query_row(
            "SELECT status='cancelled' FROM runs WHERE id=?1",
            [&assignment.source_run_id],
            |r| r.get::<_, bool>(0),
        )?;
        let parent_finished = assignment
            .parent_id
            .as_ref()
            .map(|parent| {
                tx.query_row(
                    &format!("SELECT status IN {TERMINAL} FROM companion_assignments WHERE id=?1"),
                    [parent],
                    |r| r.get::<_, bool>(0),
                )
            })
            .transpose()?
            .unwrap_or(false);
        let reason = if cancelled || parent_finished {
            Some(("cancelled", "发起方已停止本次协作"))
        } else if assignment.status == "queued"
            && now.saturating_sub(assignment.updated_at) >= QUEUE_MS
        {
            Some(("failed", "已排队 24 小时，接收伙伴未能开始"))
        } else if assignment.status == "running"
            && activity.as_ref().is_some_and(|record| {
                !record.pending_user
                    && record.status == "running"
                    && now.saturating_sub(record.progress_at) >= TURN_STALL_MS
            })
        {
            Some(("failed", "本次执行连续 20 分钟没有活动"))
        } else {
            None
        };
        if let Some((status, message)) = reason {
            stop_assignment(tx, &assignment.id, status, message, now)?;
            if active {
                outcome.cancel_run_ids.extend(run.map(str::to_owned));
            }
            if let Some(record) = assignment_on(tx, &assignment.id)? {
                outcome.changed.push(record);
            }
            continue;
        }
        let Some((run, activity)) = run.zip(activity) else {
            continue;
        };
        if matches!(
            activity.status.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        ) {
            if let Some(settled) = settle_run(tx, run, &activity.status, &activity.result, now)? {
                outcome.changed.push(settled);
            }
            continue;
        }
        let next = match activity.status.as_str() {
            "awaiting_confirmation" => "needs_input",
            "running" if activity.pending_user => "needs_input",
            "running" => "running",
            _ => continue,
        };
        if next != assignment.status {
            tx.execute(
                "UPDATE companion_assignments SET status=?2,delivery_status='delivered',updated_at=?3 WHERE id=?1",
                params![assignment.id,next,now],
            )?;
            if let Some(record) = assignment_on(tx, &assignment.id)? {
                outcome.changed.push(record);
            }
        }
    }
    fail_stalled_roots(tx, now, &mut outcome)?;
    for group in ready_resume_batches(tx)? {
        if let Some(reason) = resume_refusal(tx, &group, now)? {
            outcome
                .changed
                .extend(suppress_resume(tx, &group, reason, now)?);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::CreateDigitalColleagueInput;
    use serde_json::json;
    use std::{fs, path::PathBuf};

    struct Fixture {
        root: PathBuf,
        db: Database,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("fox-companion-coordination-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let db = Database::open(root.join("fox.db")).unwrap();
            Self { root, db }
        }

        fn peer(&self) -> crate::database::companions::CompanionView {
            self.db.create_blank_companion(&self.root).unwrap()
        }

        fn work_chat(&self) -> String {
            self.db
                .create_conversation(DEFAULT_AGENT_ID, Some("工作对话"), None, Some("read_only"))
                .unwrap()
                .id
        }

        fn run(&self, chat: &str) -> String {
            let started = self.db.create_run(chat, "请协调伙伴", None).unwrap();
            self.start(&started.run.id);
            started.run.id
        }

        fn start(&self, id: &str) {
            self.db
                .apply_runtime_event(id, 1, &json!({"type":"run.started"}))
                .unwrap();
        }

        fn finish(&self, id: &str, text: &str) {
            self.db.with_connection(|conn| {
                conn.execute("UPDATE runs SET status='completed',finished_at=?2 WHERE id=?1",params![id,now_ms()])?;
                conn.execute(
                    "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
                     SELECT ?2,r.conversation_id,r.id,'assistant','text',?3,'completed',
                        coalesce((SELECT max(m.ordinal) FROM messages m WHERE m.conversation_id=r.conversation_id),0)+1,?4,?4
                     FROM runs r WHERE id=?1",
                    params![id,Uuid::new_v4().to_string(),text,now_ms()],
                )?;
                Ok(())
            }).unwrap();
        }

        fn enqueue(
            &self,
            chat: &str,
            run: &str,
            targets: &[&str],
            instructions: &str,
        ) -> Vec<Assignment> {
            self.db
                .enqueue_companion_coordination(chat, run, request(targets, instructions))
                .unwrap()
        }

        fn clean(self) {
            drop(self.db);
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn request(targets: &[&str], instructions: &str) -> CoordinationRequest {
        CoordinationRequest {
            targets: targets.iter().map(|id| id.to_string()).collect(),
            instructions: instructions.to_owned(),
            reply: Reply::Resume,
            consult: false,
            rework: false,
            title: None,
            done_when: None,
        }
    }

    #[test]
    fn internal_coordination_run_projects_its_runtime_records_and_pending_approval() {
        let f = Fixture::new();
        let source = f.peer();
        let recipient = f.peer();
        let run = f.run(&source.conversation_id);
        let sent = f
            .enqueue(&source.conversation_id, &run, &[&recipient.profile.id], "核对文件")
            .remove(0);
        let delivery = f.db.claim_companion_assignment(&sent.id).unwrap().unwrap();
        let target_chat = &delivery.started.run.conversation_id;
        let recipient_run = &delivery.started.run.id;
        assert_eq!(delivery.started.user_message.role, "system");
        assert_eq!(delivery.started.user_message.kind, "internal");
        f.start(recipient_run);
        let tool = f
            .db
            .create_host_tool_call(
                recipient_run,
                "coordination-write",
                "write_file",
                &json!({"path":"TEST-103.txt","content":"approved"}),
                "pending",
                true,
            )
            .unwrap();
        let approval = f
            .db
            .create_approval(&tool.id, "创建 TEST-103.txt", &json!({"target":"TEST-103.txt"}))
            .unwrap();

        let detail = f.db.load_conversation(target_chat).unwrap();
        assert!(detail.messages.iter().any(|message| {
            message.run_id.as_deref() == Some(recipient_run)
                && message.role == "system"
                && message.kind == "internal"
        }));
        assert!(detail.runs.iter().any(|item| item.id == *recipient_run));
        assert!(detail.runtime_events.iter().any(|event| {
            event.run_id == *recipient_run && event.event_type == "run.started"
        }));
        assert!(detail.tool_calls.iter().any(|item| item.id == tool.id));
        assert!(detail.approvals.iter().any(|item| item.id == approval.id));

        for index in 0..130 {
            f.db.with_connection(|conn| {
                conn.execute(
                    "INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                     VALUES(?1,?2,'user','text',?3,'completed',?4,?5,?5)",
                    params![
                        Uuid::new_v4().to_string(),
                        target_chat,
                        format!("later message {index}"),
                        index + 1,
                        now_ms()
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        }
        let paged = f.db.load_conversation(target_chat).unwrap();
        assert_eq!(paged.messages.len(), 120);
        assert!(!paged.messages.iter().any(|message| {
            message.run_id.as_deref() == Some(recipient_run)
        }));
        assert!(paged.approvals.iter().any(|item| item.id == approval.id));
        f.clean();
    }

    #[test]
    fn needs_input_recognizes_explicit_line_markers_without_treating_quotations_as_questions() {
        for text in [
            "NEEDS INPUT: 正式还是轻松？\n默认：正式",
            "needs input：正式还是轻松？\ndefault: 正式",
            "需要输入: 正式还是轻松？\n默认：正式",
            "NEEDS INPUT — 正式还是轻松？\n默认：正式",
        ] {
            let question = needs_input(text).expect(text);
            assert_eq!(question.prompt, "正式还是轻松？");
            assert_eq!(question.default_answer.as_deref(), Some("正式"));
        }
        for text in [
            "NEEDS INPUT — ",
            "请把 NEEDS INPUT: 正式还是轻松？ 当作普通引用",
            "> NEEDS INPUT: 正式还是轻松？",
            "\"NEEDS INPUT: 正式还是轻松？\"",
            "任务已完成，正文没有提问。",
        ] {
            assert!(needs_input(text).is_none(), "{text}");
        }
    }

    #[test]
    fn cancellation_is_safe_idempotent_and_returns_through_the_existing_batch() {
        let f = Fixture::new();
        let source = f.peer();
        let recipient = f.peer();
        let run = f.run(&source.conversation_id);
        let sent = f.enqueue(&source.conversation_id, &run, &[&recipient.profile.id], "核对数字").remove(0);
        f.finish(&run, "已排队");
        let cancelled = f.db.cancel_companion_assignment(&sent.id).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.result.as_deref(), Some("用户已取消本次协作"));
        assert!(f.db.claim_companion_assignment(&sent.id).unwrap().is_none());
        assert!(f.db.answer_companion_assignment(&sent.id, "不能再答").is_err());
        assert_eq!(f.db.cancel_companion_assignment(&sent.id).unwrap().updated_at, cancelled.updated_at);
        let resume = f.db.claim_companion_resume(&[sent.id.clone()]).unwrap().unwrap();
        let context = f.db.companion_coordination_context(&resume.started.run.id).unwrap().unwrap();
        assert_eq!(context.returned.len(), 1);
        assert_eq!(context.returned[0].status, "cancelled");
        assert!(f.db.claim_companion_resume(&[sent.id]).unwrap().is_none());
        f.clean();
    }

    #[test]
    fn cancellation_refuses_claimed_and_active_runs_but_can_close_a_parked_question() {
        let f = Fixture::new();
        let source = f.work_chat();
        let recipient = f.peer();
        let run = f.run(&source);
        let sent = f.enqueue(&source, &run, &[&recipient.profile.id], "先询问语气").remove(0);
        let delivery = f.db.claim_companion_assignment(&sent.id).unwrap().unwrap();
        assert!(f.db.cancel_companion_assignment(&sent.id).is_err());
        f.start(&delivery.started.run.id);
        assert!(f.db.cancel_companion_assignment(&sent.id).is_err());
        f.finish(&delivery.started.run.id, "NEEDS INPUT: 正式还是轻松？\n默认：正式");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        f.db.sync_app_notifications().unwrap();
        let cancelled = f.db.cancel_companion_assignment(&sent.id).unwrap();
        assert!(cancelled.question.is_none());
        assert_eq!(cancelled.status, "cancelled");
        assert!(f.db.answer_companion_assignment(&sent.id, "正式").is_err());
        f.db.sync_app_notifications().unwrap();
        assert!(!f.db.list_app_notifications(false, 100).unwrap().iter()
            .any(|item| item.merge_key == format!("companion-assignment:{}", sent.id)));
        f.clean();
    }

    #[test]
    fn a_submitted_answer_wins_against_a_late_cancel_and_completed_work_cannot_be_cancelled() {
        let f = Fixture::new();
        let source = f.work_chat();
        let recipient = f.peer();
        let run = f.run(&source);
        let sent = f.enqueue(&source, &run, &[&recipient.profile.id], "先询问语气").remove(0);
        let delivery = f.db.claim_companion_assignment(&sent.id).unwrap().unwrap();
        f.finish(&delivery.started.run.id, "NEEDS INPUT: 正式还是轻松？");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        f.db.answer_companion_assignment(&sent.id, "正式").unwrap();
        assert!(f.db.cancel_companion_assignment(&sent.id).is_err());
        let retry = f.db.claim_companion_assignment(&sent.id).unwrap().unwrap();
        assert_eq!(f.db.companion_coordination_context(&retry.started.run.id).unwrap().unwrap().answer.as_deref(), Some("正式"));
        f.finish(&retry.started.run.id, "核对通过。");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        assert!(f.db.cancel_companion_assignment(&sent.id).is_err());
        f.clean();
    }

    #[test]
    fn coordination_delivers_into_private_recipient_context_and_wakes_batch_once() {
        let f = Fixture::new();
        let first = f.peer();
        let second = f.peer();
        f.db.execute_raw_sql(
            "INSERT INTO model_providers(id,name,base_url,api_type,enabled,is_default,created_at,updated_at)
             VALUES('recipient-provider','接收方服务','http://127.0.0.1','openai-completions',1,0,1,1);
             INSERT INTO provider_models(id,provider_id,model_id,display_name,context_window,max_output_tokens,supports_image_input,is_default,created_at,updated_at)
             VALUES('recipient-model','recipient-provider','recipient-fixture-model','接收方模型',64000,512,0,1,1,1);",
        ).unwrap();
        f.db.set_companion_model(
            &first.profile.id,
            Some("recipient-provider"),
            Some("recipient-model"),
        )
        .unwrap();
        let ordinary =
            f.db.create_conversation(
                DEFAULT_AGENT_ID,
                Some("工作区对话"),
                None,
                Some("read_only"),
            )
            .unwrap();
        let source = f.run(&ordinary.id);
        let batch = f.enqueue(
            &ordinary.id,
            &source,
            &[&first.profile.id, &second.profile.id],
            "请查本周进度",
        );
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].delivery_status, "queued");
        assert!(batch[0].source_companion_id.is_none());
        assert!(f
            .db
            .companion_assignment_status(&ordinary.id, &source, Some(&batch[0].id))
            .is_err());
        assert!(f
            .db
            .companion_assignment_status(&second.conversation_id, "another", Some(&batch[0].id))
            .unwrap()
            .is_empty());
        let target_chat = &batch[0].recipient_conversation_id;
        assert_ne!(target_chat, &first.conversation_id);
        // Distinct agent IDs make a wrong usage owner observable.
        f.db.with_connection(|conn| {
            conn.execute(
                "UPDATE conversations SET agent_id='fox-debugger' WHERE id=?1",
                [target_chat],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            f.db.load_conversation(target_chat)
                .unwrap()
                .coordination
                .unwrap()
                .recipient_companion_id,
            first.profile.id
        );
        assert_eq!(
            f.db.companion_assignments_list(&first.conversation_id)
                .unwrap()
                .len(),
            1
        );
        let received =
            f.db.claim_companion_assignment(&batch[0].id)
                .unwrap()
                .unwrap();
        assert_eq!(received.started.run.model, "recipient-fixture-model");
        assert_eq!(
            f.db.resolve_effective_model(target_chat)
                .unwrap()
                .unwrap()
                .model_id,
            "recipient-fixture-model"
        );
        assert!(f
            .db
            .claim_companion_assignment(&batch[0].id)
            .unwrap()
            .is_none());
        assert_eq!(received.started.user_message.role, "system");
        assert!(!received.started.user_message.content.contains("本周进度"));
        let first_run = received.started.run.id;
        let owner = f.db.run_companion_scope(&first_run).unwrap().unwrap();
        assert!(!owner.is_child);
        assert_eq!(owner.companion_id, first.profile.id);
        let execution =
            f.db.companion_coordination_context(&first_run)
                .unwrap()
                .unwrap();
        assert_eq!(execution.root_run_id, source);
        assert_eq!(execution.permission_mode, "read_only");
        assert_eq!(execution.incoming.unwrap().instructions, "请查本周进度");
        f.start(&first_run);
        f.db.apply_runtime_event(&first_run,2,&json!({"type":"usage.updated","inputTokens":9,"outputTokens":3,"totalTokens":12,"cacheReadTokens":0,"cacheWriteTokens":0})).unwrap();
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        f.finish(&first_run, "进度一");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        f.finish(&source, "等待伙伴报告");
        assert!(f.db.pending_companion_resumes().unwrap().is_empty());

        let second_run =
            f.db.claim_companion_assignment(&batch[1].id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&second_run);
        f.finish(&second_run, "进度二");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        let groups = f.db.pending_companion_resumes().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        let ids: Vec<_> = groups[0].iter().map(|item| item.id.clone()).collect();
        let resume = f.db.claim_companion_resume(&ids).unwrap().unwrap();
        assert_eq!(resume.started.run.conversation_id, ordinary.id);
        assert_eq!(
            f.db.with_connection(|conn| usage_root_on(conn, &resume.started.run.id))
                .unwrap()
                .as_deref(),
            Some(source.as_str())
        );
        f.start(&resume.started.run.id);
        f.db.apply_runtime_event(&resume.started.run.id,2,&json!({"type":"usage.updated","inputTokens":5,"outputTokens":3,"totalTokens":8,"cacheReadTokens":0,"cacheWriteTokens":0})).unwrap();
        let context =
            f.db.companion_coordination_context(&resume.started.run.id)
                .unwrap()
                .unwrap();
        assert!(context.incoming.is_none());
        assert_eq!(context.returned.len(), 2);
        assert_eq!(context.returned[0].result.as_deref(), Some("进度一"));
        assert_eq!(context.returned[1].result.as_deref(), Some("进度二"));
        let stats = f.db.usage_statistics().unwrap();
        assert_eq!(stats.total_tokens, 20);
        assert_eq!(
            stats
                .agents
                .iter()
                .find(|agent| agent.agent_id == DEFAULT_AGENT_ID)
                .unwrap()
                .total_tokens,
            20
        );
        assert_eq!(
            stats
                .agents
                .iter()
                .find(|agent| agent.agent_id == "fox-debugger")
                .unwrap()
                .total_tokens,
            0
        );
        assert!(f.db.claim_companion_resume(&ids).unwrap().is_none());
        assert!(f.db.pending_companion_resumes().unwrap().is_empty());
        f.clean();
    }

    #[test]
    fn peer_consult_returns_its_question_to_the_sender_without_a_user_notice() {
        let f = Fixture::new();
        let sender = f.peer();
        let recipient = f.peer();
        let source = f.run(&sender.conversation_id);
        let mut input = request(&[&recipient.profile.id], "请仅回答");
        input.consult = true;
        let sent =
            f.db.enqueue_companion_coordination(&sender.conversation_id, &source, input)
                .unwrap()
                .remove(0);
        let run =
            f.db.claim_companion_assignment(&sent.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&run);
        assert!(f
            .db
            .with_connection(|conn| consultation_on(conn, &run))
            .unwrap());
        let binding = f.db.freeze_legacy_run_control(&run, "legacy").unwrap();
        assert_eq!(
            binding.permission.mode,
            fox_engine_protocol::PermissionMode::ReadOnly
        );
        assert!(f
            .db
            .ensure_companion_tool_granted(
                &run,
                "write_file",
                Some(&json!({"path":"forbidden.txt","content":"never"}))
            )
            .is_err());
        let refuses = f.db.enqueue_companion_coordination(
            &sent.recipient_conversation_id,
            &run,
            request(&[&sender.profile.id], "不能转派"),
        );
        assert!(refuses.unwrap_err().contains("只读咨询"));
        f.finish(&run, "NEEDS INPUT — 选哪个月份？\n默认：本月");
        f.finish(&source, "等回复");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        let parked =
            f.db.companion_assignment_status(&sender.conversation_id, "later", Some(&sent.id))
                .unwrap()
                .remove(0);
        assert_eq!(parked.status, "failed");
        assert!(parked.question.is_none());
        assert!(parked.result.as_deref().unwrap().contains("选哪个月份？"));
        assert!(f.db.claim_companion_assignment(&sent.id).unwrap().is_none());
        let merge_key = format!("companion-assignment:{}", sent.id);
        let mut batches = f.db.pending_companion_resumes().unwrap();
        assert_eq!(batches.remove(0)[0].id, sent.id);
        let retry = f.db.claim_companion_resume(&[sent.id.clone()]).unwrap().unwrap();
        let resumed =
            f.db.companion_coordination_context(&retry.started.run.id)
                .unwrap()
                .unwrap();
        assert!(resumed.incoming.is_none());
        assert_eq!(resumed.returned[0].id, sent.id);
        assert!(resumed.returned[0].result.as_deref().unwrap().contains("选哪个月份？"));
        assert!(f
            .db
            .answer_companion_assignment(&sent.id, "上个月")
            .is_err());
        let ordinary = f.run(&f.work_chat());
        f.finish(&ordinary, "普通工作结果");
        f.db.sync_app_notifications().unwrap();
        let notices = f.db.list_app_notifications(false, 100).unwrap();
        for key in [
            merge_key,
            format!("run:{source}"),
            format!("run:{run}"),
            format!("run:{}", retry.started.run.id),
        ] {
            assert!(!notices.iter().any(|item| item.merge_key == key));
        }
        assert!(notices.iter().any(|item| item.merge_key == format!("run:{ordinary}")));
        f.clean();
    }

    #[test]
    fn coordination_refuses_upstream_crossed_duplication_and_caps_every_path() {
        let f = Fixture::new();
        let peers: Vec<_> = (0..6).map(|_| f.peer()).collect();
        let run = f.run(&peers[0].conversation_id);
        let first = f
            .enqueue(
                &peers[0].conversation_id,
                &run,
                &[&peers[1].profile.id],
                "推进验收",
            )
            .remove(0);
        assert!(f
            .db
            .enqueue_companion_coordination(
                &peers[0].conversation_id,
                &run,
                request(&[&peers[1].profile.id], "推进验收")
            )
            .unwrap_err()
            .contains("同样"));
        let cross = f.run(&peers[1].conversation_id);
        assert!(f
            .db
            .enqueue_companion_coordination(
                &peers[1].conversation_id,
                &cross,
                request(&[&peers[0].profile.id], "另开协作")
            )
            .unwrap_err()
            .contains("交叉"));
        let first_run =
            f.db.claim_companion_assignment(&first.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&first_run);
        assert!(f
            .db
            .enqueue_companion_coordination(
                &first.recipient_conversation_id,
                &first_run,
                request(&[&peers[0].profile.id], "上游不应接到")
            )
            .unwrap_err()
            .contains("上游"));
        let mut assignment = first;
        let mut source_run = first_run;
        for peer in &peers[2..5] {
            assignment = f
                .enqueue(
                    &assignment.recipient_conversation_id,
                    &source_run,
                    &[&peer.profile.id],
                    "推动下一层",
                )
                .remove(0);
            source_run =
                f.db.claim_companion_assignment(&assignment.id)
                    .unwrap()
                    .unwrap()
                    .started
                    .run
                    .id;
            f.start(&source_run);
        }
        assert_eq!(assignment.depth, 4);
        assert!(f
            .db
            .enqueue_companion_coordination(
                &assignment.recipient_conversation_id,
                &source_run,
                request(&[&peers[5].profile.id], "超过深度")
            )
            .unwrap_err()
            .contains("4 层"));
        for i in 0..20 {
            f.enqueue(
                &peers[0].conversation_id,
                &run,
                &[&peers[5].profile.id],
                &format!("不同任务 {i}"),
            );
        }
        assert!(f
            .db
            .enqueue_companion_coordination(
                &peers[0].conversation_id,
                &run,
                request(&[&peers[5].profile.id], "再交一个")
            )
            .unwrap_err()
            .contains("24 次"));
        f.clean();
    }

    #[test]
    fn coordination_expires_queue_and_running_without_fabricating_delivery() {
        let f = Fixture::new();
        let source = f.peer();
        let recipient = f.peer();
        let root = f.run(&source.conversation_id);
        let queued = f
            .enqueue(
                &source.conversation_id,
                &root,
                &[&recipient.profile.id],
                "会超时",
            )
            .remove(0);
        let timed =
            f.db.reconcile_companion_assignments(queued.updated_at + QUEUE_MS)
                .unwrap();
        assert_eq!(timed.changed[0].status, "failed");
        assert_eq!(timed.changed[0].delivery_status, "failed");
        assert!(timed.cancel_run_ids.is_empty());
        let pending = f
            .enqueue(
                &source.conversation_id,
                &root,
                &[&recipient.profile.id],
                "执行后停滞",
            )
            .remove(0);
        let run =
            f.db.claim_companion_assignment(&pending.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&run);
        let marked = f.db.reconcile_companion_assignments(now_ms()).unwrap();
        assert_eq!(marked.changed[0].delivery_status, "delivered");
        let timed =
            f.db.reconcile_companion_assignments(now_ms() + TURN_STALL_MS)
                .unwrap();
        assert_eq!(timed.changed[0].status, "failed");
        assert_eq!(timed.changed[0].delivery_status, "delivered");
        assert_eq!(timed.cancel_run_ids, vec![run]);
        f.clean();
    }

    #[test]
    fn coordination_usage_is_charged_to_the_original_scheduled_run() {
        let f = Fixture::new();
        let origin = f.peer();
        let recipient = f.peer();
        let colleague =
            f.db.create_digital_colleague(&CreateDigitalColleagueInput {
                name: "定时工作".into(),
                expert_id: "fox-debugger".into(),
                objective: "每晚测量进度".into(),
                project_id: None,
                project_root: None,
                knowledge_references: Vec::new(),
                max_runs_per_day: 1,
                max_tokens_per_day: 5000,
                max_duration_ms: 60_000,
                max_output_tokens: 1000,
                max_tool_calls: 16,
                companion_id: Some(origin.profile.id.clone()),
            })
            .unwrap();
        let accepted =
            f.db.accept_digital_colleague_trigger(
                &colleague.id,
                "manual",
                None,
                "coordination:usage",
                &json!({}),
                None,
                "user",
            )
            .unwrap();
        let root = accepted.trigger.run_id.unwrap();
        f.start(&root);
        let assignment = f
            .enqueue(
                &colleague.conversation_id,
                &root,
                &[&recipient.profile.id],
                "记录数据",
            )
            .remove(0);
        let delivery =
            f.db.claim_companion_assignment(&assignment.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&delivery);
        f.db.apply_runtime_event(&delivery,2,&json!({"type":"usage.updated","inputTokens":42,"outputTokens":9,"totalTokens":51,"cacheReadTokens":0,"cacheWriteTokens":0})).unwrap();
        f.finish(&delivery, "完成");
        f.finish(&root, "等待汇总");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        let resume =
            f.db.claim_companion_resume(&[assignment.id])
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&resume);
        f.db.apply_runtime_event(&resume,2,&json!({"type":"usage.updated","inputTokens":16,"outputTokens":4,"totalTokens":20,"cacheReadTokens":0,"cacheWriteTokens":0})).unwrap();
        assert_eq!(
            f.db.with_connection(|conn| usage_root_on(conn, &resume))
                .unwrap()
                .as_deref(),
            Some(root.as_str())
        );
        let record =
            f.db.list_digital_colleague_triggers(&colleague.id, 10)
                .unwrap()
                .remove(0);
        assert_eq!(record.input_tokens, 58);
        assert_eq!(record.output_tokens, 13);
        assert_eq!(record.total_tokens, 71);
        f.clean();
    }

    #[test]
    fn coordination_nested_work_settles_inner_before_waking_root() {
        let f = Fixture::new();
        let origin = f.peer();
        let worker = f.peer();
        let reviewer = f.peer();
        let root = f.run(&origin.conversation_id);
        let first = f
            .enqueue(
                &origin.conversation_id,
                &root,
                &[&worker.profile.id],
                "准备稿件",
            )
            .remove(0);
        let worker_run =
            f.db.claim_companion_assignment(&first.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&worker_run);
        let nested = f
            .enqueue(
                &first.recipient_conversation_id,
                &worker_run,
                &[&reviewer.profile.id],
                "检查稿件",
            )
            .remove(0);
        assert_eq!(nested.parent_id.as_deref(), Some(first.id.as_str()));
        let reviewer_run =
            f.db.claim_companion_assignment(&nested.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&reviewer_run);
        f.finish(&root, "等待稿件");
        f.finish(&worker_run, "等审阅");
        f.db.with_connection(|conn| {
            conn.execute("UPDATE runs SET status='failed' WHERE id=?1", [&worker_run])?;
            Ok(())
        })
        .unwrap();
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        assert_eq!(
            f.db.companion_assignment_status(&origin.conversation_id, "later", Some(&first.id))
                .unwrap()[0]
                .status,
            "waiting"
        );
        assert!(f.db.pending_companion_resumes().unwrap().is_empty());
        f.finish(&reviewer_run, "审阅已完成");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        let worker_resume =
            f.db.claim_companion_resume(&[nested.id])
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&worker_resume);
        let context =
            f.db.companion_coordination_context(&worker_resume)
                .unwrap()
                .unwrap();
        assert_eq!(context.incoming.unwrap().id, first.id);
        assert_eq!(context.returned[0].result.as_deref(), Some("审阅已完成"));
        assert!(f.db.pending_companion_resumes().unwrap().is_empty());
        f.finish(&worker_resume, "稿件已改完");
        f.db.reconcile_companion_assignments(now_ms()).unwrap();
        let root_resume = f.db.claim_companion_resume(&[first.id]).unwrap().unwrap();
        assert_eq!(
            root_resume.started.run.conversation_id,
            origin.conversation_id
        );
        assert_eq!(
            f.db.companion_coordination_context(&root_resume.started.run.id)
                .unwrap()
                .unwrap()
                .returned[0]
                .result
                .as_deref(),
            Some("稿件已改完")
        );
        f.clean();
    }

    #[test]
    fn coordination_keeps_reports_when_wake_budget_is_exhausted() {
        let f = Fixture::new();
        let source = f.peer();
        let recipient = f.peer();
        let root = f.run(&source.conversation_id);
        let item = f
            .enqueue(
                &source.conversation_id,
                &root,
                &[&recipient.profile.id],
                "保留既有成果",
            )
            .remove(0);
        let target =
            f.db.claim_companion_assignment(&item.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&target);
        f.finish(&target, "成果仍然可读");
        f.finish(&root, "等回报");
        f.db.with_connection(|conn| {
            for index in 0..46 {
                let fake = format!("budget-member-{index}");
                conn.execute("INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES(?1,?2,'completed','fixture',1)",
                    params![fake,item.recipient_conversation_id])?;
                conn.execute("INSERT INTO companion_coordination_runs(run_id,root_run_id,kind,permission_mode)
                    VALUES(?1,?2,'delivery','ask')",params![fake,root])?;
            }
            Ok(())
        }).unwrap();
        let changed =
            f.db.reconcile_companion_assignments(now_ms())
                .unwrap()
                .changed;
        let suppressed = changed
            .iter()
            .find(|record| record.resume_error.is_some())
            .unwrap();
        assert_eq!(suppressed.id, item.id);
        assert_eq!(suppressed.status, "completed");
        assert_eq!(suppressed.result.as_deref(), Some("成果仍然可读"));
        assert!(suppressed
            .resume_error
            .as_deref()
            .unwrap()
            .contains("执行或唤醒上限"));
        assert!(f.db.pending_companion_resumes().unwrap().is_empty());
        assert!(f.db.claim_companion_resume(&[item.id]).unwrap().is_none());
        assert!(f
            .db
            .reconcile_companion_assignments(now_ms())
            .unwrap()
            .changed
            .is_empty());
        f.clean();
    }

    #[test]
    fn coordination_root_stall_observes_nested_activity_and_waits_for_user_input() {
        let f = Fixture::new();
        let sender = f.peer();
        let receiver = f.peer();
        let reviewer = f.peer();
        let root = f.run(&sender.conversation_id);
        let parent = f
            .enqueue(
                &sender.conversation_id,
                &root,
                &[&receiver.profile.id],
                "写提纲",
            )
            .remove(0);
        let parent_run =
            f.db.claim_companion_assignment(&parent.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&parent_run);
        let child = f
            .enqueue(
                &parent.recipient_conversation_id,
                &parent_run,
                &[&reviewer.profile.id],
                "核对数据",
            )
            .remove(0);
        let child_run =
            f.db.claim_companion_assignment(&child.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&child_run);
        f.finish(&parent_run, "等数据");
        f.finish(&root, "等提纲");
        let now = now_ms();
        f.db.reconcile_companion_assignments(now).unwrap();
        let old = now - STALL_MS - 1;
        f.db.with_connection(|conn| {
            conn.execute(
                "UPDATE companion_assignments SET updated_at=?1 WHERE root_run_id=?2",
                params![old, root],
            )?;
            conn.execute(
                "UPDATE run_events SET created_at=?1 WHERE run_id IN (?2,?3)",
                params![old, root, parent_run],
            )?;
            Ok(())
        })
        .unwrap();
        let status = |item: &Assignment| {
            f.db.companion_assignment_status(&item.source_conversation_id, "later", Some(&item.id))
                .unwrap()[0]
                .status
                .clone()
        };
        assert!(f
            .db
            .reconcile_companion_assignments(now)
            .unwrap()
            .changed
            .is_empty());
        assert_eq!(status(&parent), "waiting");
        f.finish(&child_run, "NEEDS INPUT: 数据日期是？");
        f.db.with_connection(|conn| {
            conn.execute(
                "UPDATE companion_assignments SET status='needs_input', question='数据日期是？' WHERE id=?1",
                [&child.id],
            )?;
            Ok(())
        }).unwrap();
        f.db.reconcile_companion_assignments(now).unwrap();
        assert_eq!(status(&child), "needs_input");
        assert!(f
            .db
            .reconcile_companion_assignments(now + STALL_MS + 1)
            .unwrap()
            .changed
            .is_empty());
        assert_eq!(status(&parent), "waiting");
        assert_eq!(status(&child), "needs_input");

        f.db.answer_companion_assignment(&child.id, "本月").unwrap();
        let stalled =
            f.db.reconcile_companion_assignments(now_ms() + STALL_MS + 1)
                .unwrap();
        assert_eq!(stalled.changed.len(), 2);
        assert_eq!(status(&parent), "failed");
        assert_eq!(status(&child), "failed");
        f.clean();
    }

    #[test]
    fn coordination_uses_and_updates_only_the_recipient_memory() {
        let f = Fixture::new();
        let source = f.peer();
        let target = f.peer();
        let root = f.run(&source.conversation_id);
        let source_note=f.db.companion_notes_tool(&source.conversation_id,&root,"memory_write",&json!({
            "target":"notes","action":"add","canonicalKey":"private-origin","content":"原伙伴独有记忆"
        })).unwrap();
        let item = f
            .enqueue(
                &source.conversation_id,
                &root,
                &[&target.profile.id],
                "核对资料",
            )
            .remove(0);
        let run =
            f.db.claim_companion_assignment(&item.id)
                .unwrap()
                .unwrap()
                .started
                .run
                .id;
        f.start(&run);
        let local=f.db.companion_notes_tool(&item.recipient_conversation_id,&run,"memory_write",&json!({
            "target":"notes","action":"add","canonicalKey":"private-recipient","content":"目标伙伴独有记忆"
        })).unwrap();
        assert!(local["memoryId"].as_str().is_some());
        let notes =
            f.db.companion_runtime_notes(&item.recipient_conversation_id, "记忆")
                .unwrap()
                .unwrap();
        assert_eq!(notes["companionId"], target.profile.id);
        assert!(notes["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note["content"] == "目标伙伴独有记忆"));
        assert!(!notes.to_string().contains("原伙伴独有记忆"));
        let rejected=f.db.companion_notes_tool(&item.recipient_conversation_id,&run,"memory_write",&json!({
            "target":"notes","action":"patch","memoryId":source_note["memoryId"],"content":"尝试跨伙伴覆盖"
        })).unwrap_err();
        assert!(rejected.contains("memory.not_found"), "{rejected}");
        let rejected=f.db.with_connection(|conn|conn.execute(
            "INSERT INTO memory_entities(id,scope,scope_key,kind,canonical_key,content,state,source_conversation_id,created_by,created_at,updated_at)
             VALUES('foreign-attempt','agent',?1,'fact','foreign','尝试跨伙伴插入','candidate',?2,'agent',1,1)",
            params![source.profile.id,item.recipient_conversation_id],
        )).unwrap_err();
        assert!(
            rejected.contains("memory_companion_owner_mismatch"),
            "{rejected}"
        );
        f.clean();
    }
}

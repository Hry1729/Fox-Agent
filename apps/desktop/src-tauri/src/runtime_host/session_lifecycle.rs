//! Stage two: the Pi-native session lifecycle.
//!
//! A `runtime_sessions` row maps one Fox conversation onto one Pi session. The
//! Host owns the directory, the runtime owns the file, and the two facts are
//! deliberately kept apart:
//!
//! * `allocated` — the Host booked a path; Pi has not written the file yet
//!   (its JSONL is created on the first appended message).
//! * `ready` — the file exists with real content and can be reopened.
//! * `unavailable` — a reopen attempt failed. The conversation keeps reporting
//!   a recovery problem until an explicit reset; it is never silently given a
//!   fresh context.
//!
//! Everything that decides *what to do* is pure (`open_decision`,
//! `event_proves_persistence`); everything that talks to the runtime goes
//! through `SessionRuntimeLink`, so the decisions are testable without a live
//! Pi process.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use super::{response_error, resume_plan, ResumePlan, RuntimeEnvelope, RuntimeRequest};
use crate::database::{Database, RuntimeSessionRecord, RuntimeSessionStatus};

/// A session the Host may use: the runtime session id and Pi's exact file.
pub(crate) type OpenedSession = (String, PathBuf);

/// Why opening a session failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionOpenError {
    /// The conversation has a Pi-native context that cannot be reopened.
    ///
    /// Nothing was replaced and no new session was created: the mapping keeps
    /// reporting the problem until an explicit reset. This is the difference
    /// between "restore failed" and "start over".
    RecoveryProblem { code: String, message: String },
    /// Any other failure: unusable runtime, filesystem, database.
    Failed(String),
}

impl SessionOpenError {
    /// The user-facing text. A recovery problem names its stable code and says
    /// what the user has to do next, so a substitute empty context can never be
    /// mistaken for a successful restore.
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::RecoveryProblem { code, message } => format!(
                "会话上下文无法恢复（{code}）：{message}。请先按提示重置会话上下文再继续，历史与资料绑定不会被删除。"
            ),
            Self::Failed(message) => message.clone(),
        }
    }
}

/// The Host's channel to the runtime for `create_session` / `resume_session`.
///
/// A seam on purpose: the lifecycle's decisions (reopen, refuse, create,
/// compensate) are exercised without a live Pi process, and the only thing a
/// test fakes is the protocol exchange.
pub(crate) trait SessionRuntimeLink {
    fn send_session_request(
        &mut self,
        request: RuntimeRequest,
        timeout: Duration,
    ) -> Result<RuntimeEnvelope, String>;
}

/// A reset that was refused (or failed) with a stable code the command can
/// show verbatim. Never a bare string: "a run is active" is a refusal the user
/// has to act on, not a generic storage error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResetRefusal {
    pub code: String,
    pub message: String,
}

impl ResetRefusal {
    pub(crate) fn failed(message: impl Into<String>) -> Self {
        Self::new(RESET_FAILED, message)
    }

    pub(crate) fn run_active() -> Self {
        Self::new(RESET_RUN_ACTIVE, "当前对话正在运行，请先停止生成再重置会话上下文")
    }

    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }

    fn from_open(error: SessionOpenError) -> Self {
        Self::new(RESET_FAILED, error.user_message())
    }
}

/// The new generation a successful reset installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResetOutcome {
    pub runtime_session_id: String,
    pub session_path: PathBuf,
    pub context_generation: i64,
}

/// A permanent delete was refused for one of the conversation's session files.
/// Nothing was deleted: a refusal is all-or-nothing on purpose, so a doubtful
/// row can never leave the conversation half-cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionFileRefusal {
    pub code: String,
    pub message: String,
}

impl SessionFileRefusal {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }
}

/// The stable code for "a Run is active, so a reset was refused".
pub(crate) const RESET_RUN_ACTIVE: &str = "session.reset_run_active";
/// The stable code for "the reset failed; the previous generation is intact".
pub(crate) const RESET_FAILED: &str = "session.reset_failed";
/// A recorded session file did not canonicalize into the Fox session directory.
pub(crate) const SESSION_FILE_OUTSIDE_DIRECTORY: &str = "session.file_outside_directory";
/// A recorded session path exists but is not a regular file.
pub(crate) const SESSION_FILE_NOT_REGULAR: &str = "session.file_not_regular";
/// Another conversation still references the file or its runtime session id.
pub(crate) const SESSION_FILE_OTHER_OWNER: &str = "session.file_owned_by_another_conversation";
/// The ownership of a recorded session file could not be established (the
/// database read failed). Deleting on an unknown state is never allowed.
pub(crate) const SESSION_FILE_REFUSED_UNKNOWN: &str = "session.file_ownership_unknown";
/// The file passed every ownership check but the filesystem refused the delete.
pub(crate) const SESSION_FILE_DELETE_FAILED: &str = "session.file_delete_failed";

/// A reset is unavailable for this conversation, and why. Stable code + text
/// so the UI can mirror the Host instead of guessing.
pub(crate) const RESET_UNSUPPORTED_RUNTIME: &str = "session.reset_unsupported_runtime";
/// A legacy `format_version 1` conversation has no Pi session to reset yet.
pub(crate) const RESET_LEGACY_CONTEXT: &str = "session.reset_legacy_context";

/// Whether this conversation's context may be reset at all.
///
/// One rule, shared by the Host's refusal and the UI's menu item, so the two
/// cannot disagree about where reset is offered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResetAvailability {
    pub supported: bool,
    pub code: Option<String>,
    pub reason: Option<String>,
}

impl ResetAvailability {
    pub(crate) fn supported() -> Self {
        Self { supported: true, code: None, reason: None }
    }

    pub(crate) fn unsupported(code: &str, reason: impl Into<String>) -> Self {
        Self {
            supported: false,
            code: Some(code.to_owned()),
            reason: Some(reason.into()),
        }
    }

    /// The refusal the command must return when the UI (or any other caller)
    /// asks for an unsupported reset anyway.
    pub(crate) fn refusal(&self) -> Option<ResetRefusal> {
        if self.supported {
            return None;
        }
        Some(ResetRefusal {
            code: self.code.clone().unwrap_or_else(|| RESET_FAILED.to_owned()),
            message: self.reason.clone().unwrap_or_default(),
        })
    }
}

/// Where a context reset is available.
///
/// * Yuxi conversations keep their context in the remote knowledge service;
///   there is no Pi session for this command to replace.
/// * A legacy `format_version 1` row is a Fox JSON snapshot, not a Pi session:
///   resetting it would be a migration, not a reset, and the old snapshot is
///   still the conversation's only context.
/// * A `format_version >= 2` row (or no row yet, which a reset creates) is the
///   Pi-native model this command was built for.
pub(crate) fn reset_availability(
    runtime_type: &str,
    mapping: Option<&RuntimeSessionRecord>,
) -> ResetAvailability {
    if runtime_type == "yuxi" {
        return ResetAvailability::unsupported(
            RESET_UNSUPPORTED_RUNTIME,
            "玉溪对话的上下文由远程知识库服务持有，不支持重置会话上下文",
        );
    }
    if let Some(record) = mapping {
        if record.format_version < 2 {
            return ResetAvailability::unsupported(
                RESET_LEGACY_CONTEXT,
                "该对话仍使用旧版 Fox 会话快照（format_version 1），尚未迁移到 Pi 原生会话，暂不支持重置上下文",
            );
        }
    }
    ResetAvailability::supported()
}

/// What `open` must do with the mapping it found.
///
/// Pure and unit tested: getting this wrong either loses a conversation's
/// context or hands it an empty one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenDecision {
    /// No mapping at all: a Pi-native session may be created.
    Create,
    /// Reopen the recorded session (native or legacy, per `resume_plan`).
    Resume,
    /// Refuse, reporting a recovery problem. Never a fresh context.
    Refuse { code: &'static str, detail: &'static str },
    /// An `allocated` native session whose file Pi never wrote and whose
    /// worker is gone: there is no context to lose, so the session Pi creates
    /// now may be adopted *into the same generation* instead of forcing a
    /// second reset.
    Adopt,
    /// An `allocated` native session that the live runtime worker still holds.
    /// Pi has simply not written its JSONL yet, so nothing was lost: the
    /// recorded session is used as-is, without a runtime request and without
    /// any context replay. This is what makes a just-reset generation usable on
    /// the very next Run.
    ReuseLive,
}

/// Is this an `allocated` mapping whose session the live runtime worker still
/// holds?
///
/// The evidence has to be exact: the same runtime session id the runtime was
/// last given. A worker restart clears that id, so a session Pi never wrote
/// still refuses afterwards (it cannot be restored) instead of silently
/// becoming a fresh context.
pub(crate) fn allocated_mapping_is_live(
    mapping: &RuntimeSessionRecord,
    live_runtime_session_id: Option<&str>,
) -> bool {
    mapping.is_native()
        && matches!(mapping.status_kind(), Some(RuntimeSessionStatus::Allocated))
        && live_runtime_session_id.is_some_and(|live| live == mapping.runtime_session_id)
}

/// May this mapping adopt the session Pi creates now?
///
/// Deliberately narrow: only an `allocated` row (Pi booked a path but never
/// appended a message), only when that recorded file does not exist at all, and
/// only for a Pi-native conversation. A `ready` row whose file is missing, an
/// empty-but-present file, a corrupt file or an identity mismatch never
/// qualifies: those are recovery problems, and they keep refusing exactly as
/// before.
pub(crate) fn adoption_is_safe(
    mapping: &RuntimeSessionRecord,
    session_file_exists: bool,
) -> bool {
    mapping.is_native()
        && matches!(mapping.status_kind(), Some(RuntimeSessionStatus::Allocated))
        && mapping.native_path().is_some()
        && !session_file_exists
}

/// The stable code for "the conversation's Pi context could not be restored".
pub(crate) const SESSION_UNAVAILABLE: &str = "session.native_unavailable";
/// The stable code for "Pi never wrote the session file, so there is nothing to
/// restore". Distinct from a failed restore: the mapping stays `allocated`.
pub(crate) const SESSION_NOT_PERSISTED: &str = "session.native_not_persisted";

/// Decide how an existing mapping is reopened.
///
/// * A native row is only resumed when it is `ready`, or when it is
///   `allocated` *and* Pi's file really exists (`native_file_persisted`).
/// * A native row that failed to restore keeps reporting the problem; it never
///   becomes a fresh session.
/// * Legacy rows keep their pre-stage-two behaviour exactly: a snapshot that
///   was marked unavailable falls back to a fresh Pi-native session, because
///   that is what those conversations did before stage two existed.
pub(crate) fn open_decision(
    mapping: Option<&RuntimeSessionRecord>,
    native_file_persisted: bool,
) -> OpenDecision {
    let Some(record) = mapping else {
        return OpenDecision::Create;
    };
    if record.is_native() {
        return match record.status_kind() {
            Some(RuntimeSessionStatus::Ready) => OpenDecision::Resume,
            Some(RuntimeSessionStatus::Allocated) if native_file_persisted => {
                OpenDecision::Resume
            }
            Some(RuntimeSessionStatus::Allocated) => OpenDecision::Refuse {
                code: SESSION_NOT_PERSISTED,
                detail: "Pi 还没有把该会话写入磁盘（进程在第一次写入前就结束了），没有可恢复的上下文",
            },
            // `unavailable`, or a status this build does not know: both are
            // "do not pretend this can be reopened".
            _ => OpenDecision::Refuse {
                code: SESSION_UNAVAILABLE,
                detail: "该会话的 Pi 上下文此前恢复失败，需要显式重置后才能继续",
            },
        };
    }
    if record.status.trim() == RuntimeSessionStatus::Unavailable.as_str() {
        return OpenDecision::Create;
    }
    OpenDecision::Resume
}

/// Runtime events that prove Pi has written its session file.
///
/// `run.started` is emitted *before* the first message is appended, so it is
/// not proof of anything. An assistant message that completed, or any terminal
/// run event, is: by then the manager has flushed at least one entry.
pub(crate) fn event_proves_persistence(event_type: &str) -> bool {
    matches!(
        event_type,
        "message.completed"
            | "run.completed"
            | "run.failed"
            | "run.cancelled"
            | "run.interrupted"
    )
}

/// True when Pi's file exists with real content.
///
/// An empty file is not a session: Pi's manager refuses to load one, so
/// treating it as recoverable would promise a restore that cannot happen.
pub(crate) fn native_session_file_is_persisted(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// One conversation's session mapping, read and written against the database
/// and the Fox session directory.
pub(crate) struct SessionLifecycle<'a> {
    database: &'a Database,
    sessions_dir: &'a Path,
    runtime_version: Option<String>,
}

impl<'a> SessionLifecycle<'a> {
    pub(crate) fn new(
        database: &'a Database,
        sessions_dir: &'a Path,
        runtime_version: Option<String>,
    ) -> Self {
        Self {
            database,
            sessions_dir,
            runtime_version,
        }
    }

    /// Open (or create) the conversation's Pi session.
    pub(crate) fn open(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<OpenedSession, SessionOpenError> {
        self.open_with_live_session(conversation_id, link, None)
    }

    /// Bind the queued Run even when opening only reuses or adopts a session
    /// and therefore does not rewrite the session mapping.
    pub(crate) fn open_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        link: &mut dyn SessionRuntimeLink,
        live_runtime_session_id: Option<&str>,
    ) -> Result<OpenedSession, SessionOpenError> {
        let opened = self.open_with_live_session(conversation_id, link, live_runtime_session_id)?;
        self.database
            .associate_run_runtime_session(run_id, conversation_id, &opened.0)
            .map_err(SessionOpenError::Failed)?;
        Ok(opened)
    }

    /// Open (or create) the conversation's Pi session, knowing which runtime
    /// session the live worker currently holds.
    ///
    /// `live_runtime_session_id` is the only extra fact the production Host has
    /// and the pure decision does not: a just-created (or just-reset) native
    /// session is `allocated` until Pi appends its first message, and it is
    /// perfectly usable while the worker that created it is still alive.
    pub(crate) fn open_with_live_session(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
        live_runtime_session_id: Option<&str>,
    ) -> Result<OpenedSession, SessionOpenError> {
        let mapping = self
            .database
            .get_runtime_session(conversation_id)
            .map_err(SessionOpenError::Failed)?;
        // Pi writes its JSONL lazily, so an `allocated` mapping has to be
        // re-checked against the filesystem before it is called recoverable.
        // `recorded_file_exists` distinguishes "Pi never wrote anything" (the
        // only case that may adopt) from "a file is there but unusable".
        let (persisted, recorded_file_exists) = match mapping.as_ref() {
            Some(record) if mapping_is_allocated(record) => {
                let exists = record
                    .native_path()
                    .is_some_and(|path| std::fs::symlink_metadata(path).is_ok());
                (self.promote_if_persisted(conversation_id, record)?, exists)
            }
            _ => (false, false),
        };
        let decision = match open_decision(mapping.as_ref(), persisted) {
            OpenDecision::Refuse { code: SESSION_NOT_PERSISTED, .. }
                if mapping.as_ref().is_some_and(|record| {
                    allocated_mapping_is_live(record, live_runtime_session_id)
                }) =>
            {
                OpenDecision::ReuseLive
            }
            // The worker that created the generation is gone, so the live id
            // proves nothing — but Pi still never wrote this generation's file,
            // so there is no context to lose: adopt the session Pi creates now
            // instead of forcing a second reset.
            OpenDecision::Refuse { code: SESSION_NOT_PERSISTED, .. }
                if mapping.as_ref().is_some_and(|record| {
                    adoption_is_safe(record, recorded_file_exists)
                }) =>
            {
                OpenDecision::Adopt
            }
            other => other,
        };
        match decision {
            OpenDecision::Create => self.create(conversation_id, link),
            OpenDecision::Resume => {
                let record = mapping.expect("a resume decision always comes from a mapping");
                self.resume(conversation_id, &record, link)
            }
            OpenDecision::ReuseLive => {
                let record = mapping.expect("a reuse decision always comes from a mapping");
                let path = record.native_path().ok_or_else(|| {
                    SessionOpenError::Failed("会话映射缺少 Pi 会话文件路径".to_owned())
                })?;
                Ok((record.runtime_session_id.clone(), path))
            }
            OpenDecision::Adopt => {
                let record = mapping.expect("an adopt decision always comes from a mapping");
                self.adopt(conversation_id, &record, link)
            }
            OpenDecision::Refuse { code, detail } => Err(SessionOpenError::RecoveryProblem {
                code: code.to_owned(),
                message: detail.to_owned(),
            }),
        }
    }

    /// Explicit verification: promote an `allocated` mapping to `ready` once Pi
    /// has really written the file. Never creates, rewrites or truncates it, and
    /// never turns a missing file into a "recoverable" claim.
    pub(crate) fn verify(
        &self,
        conversation_id: &str,
    ) -> Result<Option<RuntimeSessionStatus>, SessionOpenError> {
        let Some(record) = self
            .database
            .get_runtime_session(conversation_id)
            .map_err(SessionOpenError::Failed)?
        else {
            return Ok(None);
        };
        if !mapping_is_allocated(&record) {
            return Ok(record.status_kind());
        }
        if self.promote_if_persisted(conversation_id, &record)? {
            Ok(Some(RuntimeSessionStatus::Ready))
        } else {
            Ok(Some(RuntimeSessionStatus::Allocated))
        }
    }

    /// Create a Pi-native session and record exactly where the runtime put it.
    ///
    /// Order matters: the runtime names the file, the Host validates that it is
    /// inside its own session directory, then the mapping is committed in one
    /// transaction. Only after that transaction has definitely not committed is
    /// the orphan file removed — otherwise a row could point at a deleted file.
    /// A session created here is a *new generation*, not a reset of an old
    /// one: the generation is allocated above every generation this
    /// conversation has used (live or retired), and the row this replaces is
    /// retired into the ledger by the same transaction. The edit/rerun
    /// `format_version 1` fallback goes through this path, and it must not
    /// leave the snapshot it replaces without an owner.
    pub(crate) fn create(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<OpenedSession, SessionOpenError> {
        let context_generation = self
            .database
            .next_runtime_session_generation(conversation_id)
            .map_err(SessionOpenError::Failed)?;
        let (runtime_session_id, file, native_ref, status) =
            self.allocate(
                conversation_id,
                link,
                context_generation,
                Uuid::new_v4().to_string(),
            )?;
        if let Err(error) = self.database.ensure_native_runtime_session(
            conversation_id,
            &runtime_session_id,
            self.runtime_version.as_deref(),
            file.to_string_lossy().as_ref(),
            native_ref.as_deref(),
            context_generation,
            status,
        ) {
            // The mapping write is all-or-nothing, so a failure means no row
            // references this file. Prove it before deleting: a row that
            // survived would point at a file that no longer exists.
            if !self.mapping_survived(conversation_id, &runtime_session_id, &file) {
                let _ = std::fs::remove_file(&file);
            }
            return Err(SessionOpenError::Failed(error));
        }
        Ok((runtime_session_id, file))
    }

    /// Ask the runtime for a new Pi-native session in `context_generation` and
    /// validate the file it named.
    ///
    /// Nothing is recorded here: the caller owns the mapping transaction, so a
    /// failed write can compensate by removing exactly the file no row
    /// references. The runtime session id is the caller's, because an adoption
    /// must keep the id the live row already carries.
    fn allocate(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
        context_generation: i64,
        runtime_session_id: String,
    ) -> Result<(String, PathBuf, Option<String>, RuntimeSessionStatus), SessionOpenError> {
        std::fs::create_dir_all(self.sessions_dir)
            .map_err(|error| SessionOpenError::Failed(format!("会话目录不可用: {error}")))?;
        let create_request = RuntimeRequest::new("create_session")
            .with_conversation(conversation_id)
            .with_runtime_session(&runtime_session_id)
            .with_payload(json!({
                "sessionFormat": "pi-native",
                "sessionDir": self.sessions_dir.to_string_lossy(),
                "cwd": self.sessions_dir.to_string_lossy(),
                "contextGeneration": context_generation,
            }));
        let response = link
            .send_session_request(create_request, super::RESPONSE_TIMEOUT)
            .map_err(SessionOpenError::Failed)?;
        if response.r#type != "session_created" {
            return Err(SessionOpenError::Failed(response_error(
                &response,
                "runtime session could not be created",
            )));
        }
        let payload = response.payload.as_ref();
        let reported = payload
            .and_then(|payload| payload.get("nativeSessionPath"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                SessionOpenError::Failed(
                    "运行时没有返回 Pi 会话文件路径，已拒绝把无法恢复的会话记入数据库".to_owned(),
                )
            })?;
        let file = self.owned_session_file(reported)?;
        // The generation is the Host's fact, not the runtime's: a runtime that
        // renamed it would make the ledger and the file disagree.
        if let Some(echoed) = payload
            .and_then(|payload| payload.get("contextGeneration"))
            .and_then(Value::as_i64)
        {
            if echoed != context_generation {
                return Err(SessionOpenError::Failed(
                    "运行时返回的上下文代次与请求不一致，已拒绝记录".to_owned(),
                ));
            }
        }
        let native_ref = payload
            .and_then(|payload| payload.get("nativeSessionId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        // The honest status: Pi has either persisted the file already or only
        // booked the path. A mapping may not claim `ready` before it is.
        let status = if native_session_file_is_persisted(&file) {
            RuntimeSessionStatus::Ready
        } else {
            RuntimeSessionStatus::Allocated
        };
        Ok((runtime_session_id, file, native_ref, status))
    }

    /// Reset the conversation's model context: keep the chat, start a new Pi
    /// session in the next generation.
    ///
    /// The old file is never touched and never reopened. The previous mapping is
    /// retired (so permanent delete keeps finding its file) only in the same
    /// transaction that installs the new generation: if that transaction does
    /// not commit, the old mapping survives untouched and the old generation
    /// stays openable.
    pub(crate) fn reset(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<ResetOutcome, ResetRefusal> {
        // A reset must never race a live Run: the Run owns the session the
        // reset would retire. The Host re-checks this under its run transition
        // lock; the check here keeps the lifecycle honest on its own.
        let active = self
            .database
            .conversation_has_active_run(conversation_id)
            .map_err(|error| ResetRefusal::new(RESET_FAILED, error))?;
        if active {
            return Err(ResetRefusal::run_active());
        }
        // The same support boundary the UI mirrors: a Yuxi context is remote and
        // a `format_version 1` snapshot is not a Pi session. The command is
        // refused here even if a caller (or a stale UI) asks for it anyway.
        let runtime_type = self
            .database
            .conversation_runtime_type(conversation_id)
            .map_err(|error| ResetRefusal::new(RESET_FAILED, error))?;
        let current = self
            .database
            .get_runtime_session(conversation_id)
            .map_err(|error| ResetRefusal::new(RESET_FAILED, error))?;
        if let Some(refusal) = reset_availability(&runtime_type, current.as_ref()).refusal() {
            return Err(refusal);
        }
        // Allocated above the live *and* every retired generation: a number the
        // ledger already holds would overwrite a retired session's ownership
        // record. The transaction re-checks this before it writes.
        let context_generation = self
            .database
            .next_runtime_session_generation(conversation_id)
            .map_err(|error| ResetRefusal::new(RESET_FAILED, error))?;
        let (runtime_session_id, file, native_ref, status) =
            self.allocate(
                conversation_id,
                link,
                context_generation,
                Uuid::new_v4().to_string(),
            )
            .map_err(ResetRefusal::from_open)?;
        if let Err(error) = self.database.reset_native_runtime_session(
            conversation_id,
            &runtime_session_id,
            self.runtime_version.as_deref(),
            file.to_string_lossy().as_ref(),
            native_ref.as_deref(),
            context_generation,
            status,
        ) {
            // The previous mapping survived (the transaction rolled back), so
            // only the new, unreferenced file may be removed.
            if !self.mapping_survived(conversation_id, &runtime_session_id, &file) {
                let _ = std::fs::remove_file(&file);
            }
            return Err(ResetRefusal::new(RESET_FAILED, error));
        }
        Ok(ResetOutcome {
            runtime_session_id,
            session_path: file,
            context_generation,
        })
    }

    /// Continue an `allocated` generation Pi never persisted.
    ///
    /// The user already paid for this generation by resetting; the app restart
    /// (or the switch to another conversation and back) only lost the live
    /// runtime id. Pi still has nothing on disk for it, so the Host re-creates
    /// the Pi session *inside the same generation* — same conversation, same
    /// generation, same Fox runtime session id — and records the file Pi now
    /// owns. Nothing is replayed and the previous generation is never reopened.
    ///
    /// This cannot mask real data loss:
    ///
    /// * it is reachable only for `allocated` rows (`ready` rows whose file
    ///   exists are resumed, and a `ready` row whose file is missing still
    ///   refuses — that is a restore failure, not a fresh start);
    /// * the recorded file must not exist at the moment of adoption, re-checked
    ///   right before the write, so no real file can lose its owner;
    /// * a corrupt file or an identity mismatch is a *resume* path and still
    ///   refuses;
    /// * the generation adopted is the one the user just created and Pi never
    ///   wrote anything into, so the empty context is the truth about it, not a
    ///   substitute for lost history.
    fn adopt(
        &self,
        conversation_id: &str,
        existing: &RuntimeSessionRecord,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<OpenedSession, SessionOpenError> {
        let Some(recorded) = existing.native_path() else {
            return Err(SessionOpenError::Failed(
                "会话映射缺少 Pi 会话文件路径".to_owned(),
            ));
        };
        if std::fs::symlink_metadata(&recorded).is_ok() {
            // The file appeared between the decision and the write. Adopting
            // would drop the owner of a real file, so keep refusing.
            return Err(SessionOpenError::RecoveryProblem {
                code: SESSION_NOT_PERSISTED.to_owned(),
                message:
                    "Pi 还没有把该会话写入磁盘（进程在第一次写入前就结束了），没有可恢复的上下文"
                        .to_owned(),
            });
        }
        let context_generation = existing.context_generation;
        let (runtime_session_id, file, native_ref, status) = self.allocate(
            conversation_id,
            link,
            context_generation,
            existing.runtime_session_id.clone(),
        )?;
        let adopted = self
            .database
            .adopt_native_runtime_session(
                conversation_id,
                &runtime_session_id,
                file.to_string_lossy().as_ref(),
                native_ref.as_deref(),
                context_generation,
            )
            .map_err(SessionOpenError::Failed)?;
        if !adopted {
            // The row moved (another reset, or another open) while Pi was
            // creating the session: refuse rather than record a session the
            // conversation no longer owns.
            return Err(SessionOpenError::Failed(
                "会话映射在采纳新的 Pi 会话时发生了变化，请重试".to_owned(),
            ));
        }
        if matches!(status, RuntimeSessionStatus::Ready) {
            self.database
                .confirm_native_runtime_session_ready(
                    conversation_id,
                    file.to_string_lossy().as_ref(),
                )
                .map_err(SessionOpenError::Failed)?;
        }
        Ok((runtime_session_id, file))
    }

    fn resume(
        &self,
        conversation_id: &str,
        existing: &RuntimeSessionRecord,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<OpenedSession, SessionOpenError> {
        let plan = resume_plan(existing, self.legacy_session_path(&existing.runtime_session_id));
        let native = matches!(plan, ResumePlan::Native { .. });
        let (payload, path) = match &plan {
            ResumePlan::Native { path, session_id } => (
                json!({
                    "sessionFormat": "pi-native",
                    "nativeSessionPath": path.to_string_lossy(),
                    "nativeSessionId": session_id,
                    "contextGeneration": existing.context_generation,
                }),
                path.clone(),
            ),
            ResumePlan::Legacy { path } => (json!({ "sessionPath": path }), path.clone()),
        };
        let resume_request = RuntimeRequest::new("resume_session")
            .with_conversation(conversation_id)
            .with_runtime_session(&existing.runtime_session_id)
            .with_payload(payload);
        let response = link
            .send_session_request(resume_request, super::RESPONSE_TIMEOUT)
            .map_err(SessionOpenError::Failed)?;
        if response.r#type == "session_created" {
            if native {
                // Pi opened the file it was told to open, so the mapping is
                // provably recoverable again.
                self.database
                    .ensure_native_runtime_session(
                        conversation_id,
                        &existing.runtime_session_id,
                        existing.runtime_version.as_deref(),
                        path.to_string_lossy().as_ref(),
                        existing.native_session_ref.as_deref(),
                        existing.context_generation,
                        RuntimeSessionStatus::Ready,
                    )
                    .map_err(SessionOpenError::Failed)?;
            } else {
                self.database
                    .ensure_runtime_session(
                        conversation_id,
                        &existing.runtime_session_id,
                        existing.runtime_version.as_deref(),
                        path.to_str(),
                    )
                    .map_err(SessionOpenError::Failed)?;
            }
            return Ok((existing.runtime_session_id.clone(), path));
        }
        let code = response
            .payload
            .as_ref()
            .and_then(|payload| payload.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("session.resume_failed")
            .to_owned();
        let message = response_error(&response, "Pi session could not be resumed");
        self.database
            .mark_runtime_session_unavailable(conversation_id)
            .map_err(SessionOpenError::Failed)?;
        if native {
            // A Pi-native conversation whose file cannot be reopened is a
            // recovery problem. Starting an empty context here would silently
            // drop the conversation; the user has to reset it explicitly.
            return Err(SessionOpenError::RecoveryProblem { code, message });
        }
        // Legacy rows keep their pre-stage-two behaviour: a Fox snapshot that
        // cannot be reopened starts a fresh Pi-native session.
        self.create(conversation_id, link)
    }

    /// Promote an `allocated` mapping to `ready` when Pi's file is on disk.
    /// Returns whether the file is really persisted.
    fn promote_if_persisted(
        &self,
        conversation_id: &str,
        record: &RuntimeSessionRecord,
    ) -> Result<bool, SessionOpenError> {
        let Some(path) = record.native_path() else {
            return Ok(false);
        };
        if !native_session_file_is_persisted(&path) {
            return Ok(false);
        }
        self.database
            .confirm_native_runtime_session_ready(conversation_id, path.to_string_lossy().as_ref())
            .map_err(SessionOpenError::Failed)?;
        Ok(true)
    }

    /// Validate that the runtime's file really lives in the Host-owned session
    /// directory, and return the canonical path that will be recorded.
    fn owned_session_file(&self, reported: &str) -> Result<PathBuf, SessionOpenError> {
        let directory = std::fs::canonicalize(self.sessions_dir)
            .map_err(|error| SessionOpenError::Failed(format!("会话目录不可用: {error}")))?;
        let reported = PathBuf::from(reported);
        let parent = reported
            .parent()
            .ok_or_else(|| SessionOpenError::Failed("Pi 会话文件路径没有父目录".to_owned()))?;
        let canonical_parent = std::fs::canonicalize(parent)
            .map_err(|error| SessionOpenError::Failed(format!("Pi 会话目录不可用: {error}")))?;
        let file_name = reported
            .file_name()
            .ok_or_else(|| SessionOpenError::Failed("Pi 会话文件路径没有文件名".to_owned()))?;
        if canonical_parent != directory {
            return Err(SessionOpenError::Failed(
                "Pi 会话文件不在 Fox 会话目录内，已拒绝记录".to_owned(),
            ));
        }
        Ok(canonical_parent.join(file_name))
    }

    /// Did a mapping row survive the failed write?
    ///
    /// A read failure answers `true` on purpose: an unknown state must never
    /// license deleting a file that something might still reference.
    fn mapping_survived(
        &self,
        conversation_id: &str,
        runtime_session_id: &str,
        file: &Path,
    ) -> bool {
        let row = match self.database.get_runtime_session(conversation_id) {
            Ok(Some(row)) => row,
            // Nothing references the file: the compensation may remove it.
            Ok(None) => return false,
            // A read failure must not license a delete.
            Err(_) => return true,
        };
        if row.runtime_session_id == runtime_session_id {
            return true;
        }
        row.native_session_path
            .as_deref()
            .is_some_and(|recorded| Path::new(recorded.trim()) == file)
    }

    /// The legacy snapshot path for a row that recorded no explicit path.
    fn legacy_session_path(&self, runtime_session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{runtime_session_id}.json"))
    }
}

fn mapping_is_allocated(record: &RuntimeSessionRecord) -> bool {
    record.is_native()
        && matches!(
            record.status_kind(),
            Some(RuntimeSessionStatus::Allocated)
        )
}

/// Remove every session file this conversation owns — the live generation and
/// every retired one.
///
/// This is a permanent delete, so it has to be provably safe rather than
/// thorough. Before a single byte disappears, each row is checked:
///
/// * the row belongs to this conversation;
/// * no other conversation still references the file or its runtime session id;
/// * the path canonicalizes into the Host's session directory (never a symlink
///   that escapes it, never a sibling directory);
/// * it is a regular file.
///
/// Pi-native rows are all-or-nothing: a refusal deletes nothing at all, because
/// a half-cleaned conversation is worse than a retryable refusal. Legacy
/// `format_version` 1 snapshots keep their pre-stage-two behaviour exactly:
/// delete a regular file that really is inside the directory, silently skip
/// anything that is not.
///
/// A missing file is not a refusal: there is nothing to delete, and a retired
/// generation Pi never persisted legitimately has no file.
pub(crate) fn purge_owned_session_files(
    database: &Database,
    sessions_dir: &Path,
    conversation_id: &str,
) -> Result<Vec<PathBuf>, SessionFileRefusal> {
    let refused = |code: &str, message: String| SessionFileRefusal::new(code, message);
    let directory = match std::fs::canonicalize(sessions_dir) {
        Ok(directory) => directory,
        // No directory means nothing this conversation owns can exist.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(refused(
                SESSION_FILE_OUTSIDE_DIRECTORY,
                format!("无法校验 Fox 会话目录，已拒绝删除任何会话文件：{error}"),
            ))
        }
    };
    let records = database
        .conversation_runtime_session_files(conversation_id)
        .map_err(|error| refused(SESSION_FILE_REFUSED_UNKNOWN, error))?;
    let mut planned: Vec<PathBuf> = Vec::new();
    for record in records {
        // The row identity is re-checked here, not assumed from the query: a
        // row that names another conversation is never deleted.
        if record.conversation_id != conversation_id {
            return Err(refused(
                SESSION_FILE_OTHER_OWNER,
                "会话文件记录属于另一个对话，已拒绝删除".to_owned(),
            ));
        }
        let Some(path) = record.file_path() else {
            continue;
        };
        let other_owner = database
            .other_conversation_referencing_session(
                conversation_id,
                path.to_string_lossy().as_ref(),
                &record.runtime_session_id,
            )
            .map_err(|error| refused(SESSION_FILE_REFUSED_UNKNOWN, error))?;
        if record.is_native() {
            let canonical = match path.canonicalize() {
                Ok(canonical) => canonical,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(refused(
                        SESSION_FILE_OUTSIDE_DIRECTORY,
                        format!("无法确认会话文件位置，已拒绝删除：{error}"),
                    ))
                }
            };
            if canonical.parent() != Some(directory.as_path()) {
                return Err(refused(
                    SESSION_FILE_OUTSIDE_DIRECTORY,
                    format!(
                        "会话文件不在 Fox 会话目录内，已拒绝删除：{}",
                        canonical.to_string_lossy()
                    ),
                ));
            }
            if !canonical.is_file() {
                return Err(refused(
                    SESSION_FILE_NOT_REGULAR,
                    format!("会话路径不是普通文件，已拒绝删除：{}", canonical.to_string_lossy()),
                ));
            }
            if let Some(other) = other_owner {
                return Err(refused(
                    SESSION_FILE_OTHER_OWNER,
                    format!("会话文件仍被对话 {other} 引用，已拒绝删除"),
                ));
            }
            if !planned.contains(&canonical) {
                planned.push(canonical);
            }
            continue;
        }
        // Legacy snapshot: unchanged pre-stage-two behaviour.
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if canonical.parent() != Some(directory.as_path()) || !canonical.is_file() {
            continue;
        }
        if other_owner.is_some() {
            continue;
        }
        if !planned.contains(&canonical) {
            planned.push(canonical);
        }
    }
    let mut removed = Vec::new();
    for file in planned {
        match std::fs::remove_file(&file) {
            Ok(()) => removed.push(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(refused(
                    SESSION_FILE_DELETE_FAILED,
                    format!("删除会话文件失败：{error}"),
                ))
            }
        }
    }
    Ok(removed)
}

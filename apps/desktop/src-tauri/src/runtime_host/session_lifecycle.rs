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
        let mapping = self
            .database
            .get_runtime_session(conversation_id)
            .map_err(SessionOpenError::Failed)?;
        // Pi writes its JSONL lazily, so an `allocated` mapping has to be
        // re-checked against the filesystem before it is called recoverable.
        let persisted = match mapping.as_ref() {
            Some(record) if mapping_is_allocated(record) => {
                self.promote_if_persisted(conversation_id, record)?
            }
            _ => false,
        };
        match open_decision(mapping.as_ref(), persisted) {
            OpenDecision::Create => self.create(conversation_id, link),
            OpenDecision::Resume => {
                let record = mapping.expect("a resume decision always comes from a mapping");
                self.resume(conversation_id, &record, link)
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
    pub(crate) fn create(
        &self,
        conversation_id: &str,
        link: &mut dyn SessionRuntimeLink,
    ) -> Result<OpenedSession, SessionOpenError> {
        let runtime_session_id = Uuid::new_v4().to_string();
        std::fs::create_dir_all(self.sessions_dir)
            .map_err(|error| SessionOpenError::Failed(format!("会话目录不可用: {error}")))?;
        let create_request = RuntimeRequest::new("create_session")
            .with_conversation(conversation_id)
            .with_runtime_session(&runtime_session_id)
            .with_payload(json!({
                "sessionFormat": "pi-native",
                "sessionDir": self.sessions_dir.to_string_lossy(),
                "cwd": self.sessions_dir.to_string_lossy(),
                "contextGeneration": 1,
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
        let native_ref = payload
            .and_then(|payload| payload.get("nativeSessionId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let generation = payload
            .and_then(|payload| payload.get("contextGeneration"))
            .and_then(Value::as_i64)
            .unwrap_or(1);
        // The honest status: Pi has either persisted the file already or only
        // booked the path. A mapping may not claim `ready` before it is.
        let status = if native_session_file_is_persisted(&file) {
            RuntimeSessionStatus::Ready
        } else {
            RuntimeSessionStatus::Allocated
        };
        if let Err(error) = self.database.ensure_native_runtime_session(
            conversation_id,
            &runtime_session_id,
            self.runtime_version.as_deref(),
            file.to_string_lossy().as_ref(),
            native_ref.as_deref(),
            generation,
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

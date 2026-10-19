//! Host-side ACP v1 client (plan 6.7) on the official `agent-client-protocol`
//! crate. One process per bound session (6.1): spawned with an allowlisted
//! environment, in its own process group on Unix, and killed as a tree on
//! close. The Desktop Host additionally wraps the child in its existing Windows
//! Job Object when it integrates this module.
//!
//! What this module guarantees, matching the port rules:
//! * `session/cancel` answers every pending permission request of that session
//!   with `cancelled` (ACP spec requirement; plan 6.4 rule 1).
//! * Permission answers are reported as written, never as acknowledged: ACP v1
//!   has no acknowledgement for a permission response (plan 6.3 rule 3).
//! * Capabilities are read from `initialize` and default to "unsupported".

use crate::launch::{child_environment, LaunchSpec};
use crate::port::{CapabilityDeclaration, SessionCapabilities};
use crate::reverse::WriteOutcome;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Something the backend told the Host.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    Initialized {
        capabilities: CapabilityDeclaration,
        initialize_response: Value,
    },
    /// A `session/update` notification, kept as JSON for the Host mapper.
    Update { session_id: String, update: Value },
    /// The agent asks for permission. Answer with [`AcpConnection::answer_permission`].
    PermissionRequested {
        native_request_id: String,
        session_id: String,
        option_ids: Vec<String>,
        request: Value,
    },
    TurnEnded { session_id: String, stop_reason: Value },
    Failed(String),
    Closed,
}

enum Command {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Cancel {
        session_id: String,
    },
}

type PendingPermissions = Arc<Mutex<HashMap<String, (String, Responder<RequestPermissionResponse>)>>>;

pub struct AcpConnection {
    commands: mpsc::UnboundedSender<Command>,
    pub events: mpsc::UnboundedReceiver<BackendEvent>,
    pending: PendingPermissions,
    child_id: Option<u32>,
    child: Arc<tokio::sync::Mutex<tokio::process::Child>>,
}

/// Map an ACP `initialize` response onto the port's capability declaration.
/// Anything the agent does not declare stays unsupported.
pub fn capabilities_from_initialize(response: &Value) -> CapabilityDeclaration {
    let caps = response.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    let session_caps = caps.get("sessionCapabilities").cloned().unwrap_or(Value::Null);
    let present = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    let mut declared = CapabilityDeclaration::unverified();
    declared.session = SessionCapabilities {
        load: present(&caps, "loadSession"),
        resume: present(&session_caps, "resume"),
        close: present(&session_caps, "close"),
        list: present(&session_caps, "list"),
        steer: false,
    };
    declared.approval_answer_acknowledged = false;
    declared
}

fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

impl AcpConnection {
    /// Spawn the backend and complete `initialize`. Returns after the
    /// `Initialized` event has been queued.
    pub async fn spawn(spec: &LaunchSpec, cwd: &std::path::Path) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(cwd)
            .env_clear()
            .envs(child_environment(spec, std::env::vars()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", spec.program))?;
        let stdin = child.stdin.take().ok_or("backend stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("backend stdout unavailable")?;
        let child_id = child.id();
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

        let (event_tx, events) = mpsc::unbounded_channel();
        let (commands, mut command_rx) = mpsc::unbounded_channel::<Command>();
        let pending: PendingPermissions = Arc::default();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        let notify_tx = event_tx.clone();
        let permission_tx = event_tx.clone();
        let permission_pending = Arc::clone(&pending);
        let main_tx = event_tx.clone();
        let next_request = Arc::new(std::sync::atomic::AtomicU64::new(0));

        tokio::spawn(async move {
            let result = Client
                .builder()
                .name("fox-host")
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        let update = serde_json::to_value(&notification.update).unwrap_or(Value::Null);
                        let _ = notify_tx.send(BackendEvent::Update {
                            session_id: notification.session_id.to_string(),
                            update,
                        });
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder: Responder<RequestPermissionResponse>, _cx| {
                        let id = format!(
                            "perm-{}",
                            next_request.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        );
                        let session_id = request.session_id.to_string();
                        let option_ids = request.options.iter().map(|option| option.option_id.to_string()).collect();
                        let raw = serde_json::to_value(&request).unwrap_or(Value::Null);
                        permission_pending
                            .lock()
                            .expect("permission registry poisoned")
                            .insert(id.clone(), (session_id.clone(), responder));
                        let _ = permission_tx.send(BackendEvent::PermissionRequested {
                            native_request_id: id,
                            session_id,
                            option_ids,
                            request: raw,
                        });
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                    let init = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await;
                    let init = match init {
                        Ok(init) => init,
                        Err(error) => {
                            let _ = ready_tx.send(Err(format!("initialize failed: {error}")));
                            return Ok(());
                        }
                    };
                    let initialize_response = serde_json::to_value(&init).unwrap_or(Value::Null);
                    let _ = main_tx.send(BackendEvent::Initialized {
                        capabilities: capabilities_from_initialize(&initialize_response),
                        initialize_response,
                    });
                    let _ = ready_tx.send(Ok(()));
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            Command::NewSession { cwd, reply } => {
                                let created = connection
                                    .send_request(NewSessionRequest::new(cwd))
                                    .block_task()
                                    .await
                                    .map(|response| response.session_id.to_string())
                                    .map_err(|error| error.to_string());
                                let _ = reply.send(created);
                            }
                            Command::Prompt { session_id, text } => {
                                // Prompt completion arrives on its own task so
                                // cancel and permission answers keep flowing.
                                let connection = connection.clone();
                                let tx = main_tx.clone();
                                tokio::spawn(async move {
                                    let outcome = connection
                                        .send_request(PromptRequest::new(
                                            session_id.clone(),
                                            vec![ContentBlock::Text(TextContent::new(text))],
                                        ))
                                        .block_task()
                                        .await;
                                    let _ = tx.send(match outcome {
                                        Ok(response) => BackendEvent::TurnEnded {
                                            session_id,
                                            stop_reason: serde_json::to_value(&response.stop_reason)
                                                .unwrap_or(Value::Null),
                                        },
                                        Err(error) => BackendEvent::Failed(format!("prompt failed: {error}")),
                                    });
                                });
                            }
                            Command::Cancel { session_id } => {
                                let _ = connection.send_notification(CancelNotification::new(session_id));
                            }
                        }
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                let _ = event_tx.send(BackendEvent::Failed(error.to_string()));
            }
            let _ = event_tx.send(BackendEvent::Closed);
        });

        match ready_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if let Some(pid) = child_id {
                    kill_tree(pid);
                }
                return Err(error);
            }
            Err(_) => {
                if let Some(pid) = child_id {
                    kill_tree(pid);
                }
                return Err("backend closed before initialize completed".into());
            }
        }
        Ok(Self {
            commands,
            events,
            pending,
            child_id,
            child: Arc::new(tokio::sync::Mutex::new(child)),
        })
    }

    pub async fn new_session(&self, cwd: PathBuf) -> Result<String, String> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::NewSession { cwd, reply })
            .map_err(|_| "backend connection closed".to_string())?;
        response.await.map_err(|_| "backend connection closed".to_string())?
    }

    /// Start a turn. Its end is reported as `TurnEnded` or `Failed`.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<(), String> {
        self.commands
            .send(Command::Prompt {
                session_id: session_id.to_owned(),
                text: text.to_owned(),
            })
            .map_err(|_| "backend connection closed".to_string())
    }

    /// Send `session/cancel` and answer every pending permission request of
    /// the session with `cancelled`. Returns the native ids that were settled.
    pub fn cancel(&self, session_id: &str) -> Vec<String> {
        let _ = self.commands.send(Command::Cancel {
            session_id: session_id.to_owned(),
        });
        let mut registry = self.pending.lock().expect("permission registry poisoned");
        let ids: Vec<String> = registry
            .iter()
            .filter(|(_, (session, _))| session == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            if let Some((_, responder)) = registry.remove(id) {
                let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            }
        }
        ids
    }

    /// Answer a permission request with one of the offered option ids, or
    /// `None` to reject as cancelled. ACP has no acknowledgement for this
    /// response, so success is `Written`, never acknowledged; a closed
    /// transport is `Unconfirmed` (outcome unknown).
    pub fn answer_permission(&self, native_request_id: &str, option_id: Option<&str>) -> Result<WriteOutcome, String> {
        let (_, responder) = self
            .pending
            .lock()
            .expect("permission registry poisoned")
            .remove(native_request_id)
            .ok_or_else(|| "permission request already answered or no longer pending".to_string())?;
        let outcome = match option_id {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id.to_owned())),
            None => RequestPermissionOutcome::Cancelled,
        };
        Ok(match responder.respond(RequestPermissionResponse::new(outcome)) {
            Ok(()) => WriteOutcome::Written,
            Err(_) => WriteOutcome::Unconfirmed,
        })
    }

    /// Number of permission requests still waiting for the user.
    pub fn pending_permissions(&self) -> usize {
        self.pending.lock().map(|registry| registry.len()).unwrap_or(0)
    }

    /// Terminate the backend process tree. Pending permission requests are
    /// dropped with the connection; the Host marks them per `reverse` rules.
    pub async fn close(self) {
        if let Some(pid) = self.child_id {
            kill_tree(pid);
        }
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_to_unsupported_and_read_declared_fields() {
        let empty = capabilities_from_initialize(&serde_json::json!({"protocolVersion": 1}));
        assert_eq!(empty.session, SessionCapabilities::default());
        let declared = capabilities_from_initialize(&serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}, "close": {}, "list": null}
            }
        }));
        assert!(declared.session.load && declared.session.resume && declared.session.close);
        assert!(!declared.session.list);
        assert!(!declared.approval_answer_acknowledged);
    }
}

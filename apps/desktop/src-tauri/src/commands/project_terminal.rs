use super::{project_root, ApiResponse, AppState};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{ipc::Channel, State, WebviewWindow};

#[derive(Clone, Default)]
pub struct ProjectTerminals(Arc<Mutex<HashMap<String, Arc<TerminalSession>>>>);

struct TerminalSession {
    window: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl TerminalSession {
    fn write(&self, data: &str) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(data.as_bytes())?;
        writer.flush()
    }
}

impl ProjectTerminals {
    fn get(&self, id: &str, window: &str) -> Option<Arc<TerminalSession>> {
        self.0
            .lock()
            .unwrap()
            .get(id)
            .filter(|session| session.window == window)
            .cloned()
    }

    fn close(&self, id: &str, window: &str) {
        let session = {
            let mut sessions = self.0.lock().unwrap();
            if sessions
                .get(id)
                .is_some_and(|session| session.window == window)
            {
                sessions.remove(id)
            } else {
                None
            }
        };
        if let Some(session) = session {
            let _ = session.killer.lock().unwrap().kill();
        }
    }

    pub fn close_all(&self) {
        let sessions = std::mem::take(&mut *self.0.lock().unwrap());
        for session in sessions.into_values() {
            let _ = session.killer.lock().unwrap().kill();
        }
    }
}

#[derive(Deserialize)]
pub struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl From<TerminalSize> for PtySize {
    fn from(size: TerminalSize) -> Self {
        Self {
            cols: size.cols,
            rows: size.rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTerminalRequest {
    conversation_id: String,
    size: TerminalSize,
}

#[derive(Clone, Serialize)]
#[cfg_attr(test, derive(Debug, Deserialize))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TerminalEvent {
    Output { data: Vec<u8> },
    Exited { code: Option<u32> },
}

#[tauri::command]
pub async fn project_terminal_start(
    state: State<'_, AppState>,
    terminals: State<'_, ProjectTerminals>,
    window: WebviewWindow,
    request: StartTerminalRequest,
    output: Channel<TerminalEvent>,
) -> Result<ApiResponse<String>, String> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "project.root_unavailable",
                error,
                false,
            ))
        }
    };
    let terminals = terminals.inner().clone();
    let window = window.label().to_owned();
    let result = tauri::async_runtime::spawn_blocking(move || {
        start_terminal(terminals, window, &root, request.size.into(), output)
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(match result {
        Ok(id) => ApiResponse::success(id),
        Err(error) => ApiResponse::failure("terminal.start_failed", error, true),
    })
}

fn start_terminal(
    terminals: ProjectTerminals,
    window: String,
    root: &Path,
    size: PtySize,
    output: Channel<TerminalEvent>,
) -> Result<String, String> {
    let (session, mut child, mut reader) = open_powershell(window, root, size)?;
    let id = uuid::Uuid::new_v4().to_string();
    terminals
        .0
        .lock()
        .unwrap()
        .insert(id.clone(), Arc::new(session));

    let stream = output.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0
                || stream
                    .send(TerminalEvent::Output {
                        data: buffer[..count].to_vec(),
                    })
                    .is_err()
            {
                break;
            }
        }
    });

    let ended_id = id.clone();
    std::thread::spawn(move || {
        let code = child.wait().ok().map(|status| status.exit_code());
        // Closing the master releases the output pipe. Drain it before sending the exit status.
        let session = terminals.0.lock().unwrap().remove(&ended_id);
        drop(session);
        let _ = reader_thread.join();
        let _ = output.send(TerminalEvent::Exited { code });
    });
    Ok(id)
}

type OpenTerminal = (
    TerminalSession,
    Box<dyn Child + Send + Sync>,
    Box<dyn Read + Send>,
);

fn open_powershell(window: String, root: &Path, size: PtySize) -> Result<OpenTerminal, String> {
    let pair = portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|error| format!("无法创建终端：{error}"))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;
    let mut command = CommandBuilder::new(powershell_path());
    command.args(["-NoLogo", "-NoProfile"]);
    command.cwd(root);
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动 PowerShell：{error}"))?;
    let session = TerminalSession {
        window,
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(child.clone_killer()),
    };
    Ok((session, child, reader))
}

fn powershell_path() -> PathBuf {
    #[cfg(windows)]
    {
        let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        PathBuf::from(system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
    }
    #[cfg(not(windows))]
    PathBuf::from("pwsh")
}

#[tauri::command]
pub async fn project_terminal_write(
    terminals: State<'_, ProjectTerminals>,
    window: WebviewWindow,
    id: String,
    data: String,
) -> Result<ApiResponse<()>, String> {
    let session = terminals
        .get(&id, window.label())
        .ok_or_else(|| "终端已关闭".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || session.write(&data))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(ApiResponse::success(()))
}

#[tauri::command]
pub async fn project_terminal_resize(
    terminals: State<'_, ProjectTerminals>,
    window: WebviewWindow,
    id: String,
    size: TerminalSize,
) -> Result<ApiResponse<()>, String> {
    if let Some(session) = terminals.get(&id, window.label()) {
        session
            .master
            .lock()
            .unwrap()
            .resize(size.into())
            .map_err(|error| error.to_string())?;
    }
    Ok(ApiResponse::success(()))
}

#[tauri::command]
pub async fn project_terminal_close(
    terminals: State<'_, ProjectTerminals>,
    window: WebviewWindow,
    id: String,
) -> Result<ApiResponse<()>, String> {
    let terminals = terminals.inner().clone();
    let window = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || terminals.close(&id, &window))
        .await
        .map_err(|error| error.to_string())?;
    Ok(ApiResponse::success(()))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn powershell_streams_in_project_directory_and_reports_exit() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .canonicalize()
            .unwrap();
        let terminals = ProjectTerminals::default();
        let (events, read) = mpsc::channel();
        let output = Channel::new(move |body| {
            let event = body.deserialize::<TerminalEvent>().unwrap();
            let _ = events.send(event);
            Ok(())
        });
        let id = start_terminal(
            terminals.clone(),
            "test".into(),
            &root,
            PtySize {
                cols: 160,
                rows: 32,
                pixel_width: 0,
                pixel_height: 0,
            },
            output,
        )
        .unwrap();
        let next = |deadline: Instant| {
            read.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|error| error.to_string())
        };
        let write = |data: &str| {
            terminals
                .get(&id, "test")
                .ok_or("终端已退出")?
                .write(data)
                .map_err(|error| error.to_string())
        };
        let collect_until = |matches: &dyn Fn(&str) -> bool| -> Result<String, String> {
            let mut bytes = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                match next(deadline).map_err(|error| {
                    format!("{error}; output: {:?}", String::from_utf8_lossy(&bytes))
                })? {
                    TerminalEvent::Output { data } => bytes.extend(data),
                    event => return Err(format!("等待流式输出时收到 {event:?}")),
                }
                let transcript = String::from_utf8_lossy(&bytes);
                if matches(&transcript) {
                    return Ok(transcript.into_owned());
                }
            }
        };
        let result = (|| -> Result<(), String> {
            let initial =
                collect_until(&|output| output.contains("\x1b[6n") || output.contains("> "))?;
            if initial.contains("\x1b[6n") {
                write("\x1b[1;1R")?;
                collect_until(&|output| output.contains("> "))?;
            }
            write("Write-Output ((Get-Location).Path); Write-Output ('FOX_STREAM_' + '中')\r")?;
            let expected_root = root.to_string_lossy();
            let expected_root = expected_root.trim_start_matches(r"\\?\").to_lowercase();
            collect_until(&|output| {
                output.contains("FOX_STREAM_中") && output.to_lowercase().contains(&expected_root)
            })?;
            write("exit 7\r")?;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let TerminalEvent::Exited { code } = next(deadline)? {
                    return if code == Some(7) {
                        Ok(())
                    } else {
                        Err(format!("退出码为 {code:?}"))
                    };
                }
            }
        })();
        terminals.close(&id, "test");
        result.unwrap();
        assert!(terminals.get(&id, "test").is_none());
    }
}

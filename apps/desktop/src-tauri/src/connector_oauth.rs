use crate::app_state::AppState;
use crate::commands::{external_url_open, OpenExternalUrlRequest};
use crate::connector_catalog::{preset, Connector};
use crate::database::{ApiResponse, McpServerRecord};
use keyring::{Entry, Error as KeyError};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use url::Url;

static HELPER: OnceLock<PathBuf> = OnceLock::new();
static REFRESH: Mutex<()> = Mutex::new(());

pub fn setup(resources: &Path) {
    let _ = HELPER.set(resources.join("agent-runtime").join(format!(
        "fox-connector-auth{}",
        std::env::consts::EXE_SUFFIX
    )));
}

fn credential(id: &str) -> Result<Entry, String> {
    Entry::new("com.fox.agent.connector-oauth", id).map_err(|e| e.to_string())
}

fn manifest(id: &str) -> Result<Option<(String, usize)>, String> {
    match credential(id)?.get_password() {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|_| "本机授权记录损坏，请重新连接".to_owned()),
        Err(KeyError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取系统凭证库：{error}")),
    }
}

fn read(id: &str) -> Result<Option<Value>, String> {
    let Some((generation, count)) = manifest(id)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    for index in 0..count {
        bytes.extend(
            credential(&format!("{id}/{generation}/{index}"))?
                .get_secret()
                .map_err(|e| e.to_string())?,
        );
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "本机授权记录损坏，请重新连接".to_owned())
}

fn remove_parts(id: &str, generation: &str, count: usize) {
    for index in 0..count {
        if let Ok(part) = credential(&format!("{id}/{generation}/{index}")) {
            let _ = part.delete_credential();
        }
    }
}

fn store(id: &str, grant: &Value) -> Result<(), String> {
    // Windows Credential Manager limits each binary secret to 2560 bytes.
    // Publish the new manifest only after all parts of the rotated grant exist.
    let previous = manifest(id)?;
    let generation = uuid::Uuid::new_v4().to_string();
    let bytes = serde_json::to_vec(grant).map_err(|e| e.to_string())?;
    let count = bytes.chunks(2400).len();
    let save = || -> Result<(), String> {
        for (index, chunk) in bytes.chunks(2400).enumerate() {
            credential(&format!("{id}/{generation}/{index}"))?
                .set_secret(chunk)
                .map_err(|e| e.to_string())?;
        }
        credential(id)?
            .set_password(&json!([generation, count]).to_string())
            .map_err(|e| e.to_string())
    };
    if let Err(error) = save() {
        remove_parts(id, &generation, count);
        return Err(error);
    }
    if let Some((old, n)) = previous {
        remove_parts(id, &old, n);
    }
    Ok(())
}

pub fn clear(id: &str) -> Result<(), String> {
    if !preset(id).is_some_and(|p| p.setup.oauth) {
        return Ok(());
    }
    let _guard = REFRESH.lock().map_err(|_| "授权刷新状态不可用")?;
    let previous = manifest(id)?;
    match credential(id)?.delete_credential() {
        Ok(()) | Err(KeyError::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }?;
    if let Some((old, n)) = previous {
        remove_parts(id, &old, n);
    }
    Ok(())
}

pub fn configured(id: &str) -> bool {
    preset(id).is_some_and(|p| p.setup.oauth) && manifest(id).ok().flatten().is_some()
}

fn setting(registration: &str, field: &str) -> Option<String> {
    std::env::var(format!("FOX_CONNECTOR_{registration}_{field}"))
        .ok()
        .or_else(|| {
            let config: Value =
                serde_json::from_str(option_env!("FOX_CONNECTOR_REGISTRATIONS_JSON")?).ok()?;
            config
                .get(registration)?
                .get(field)?
                .as_str()
                .map(str::to_owned)
        })
        .filter(|value| !value.trim().is_empty())
}

#[tauri::command]
pub fn connector_authorization_ready(id: String) -> ApiResponse<bool> {
    let ready = preset(&id).is_some_and(|item| {
        item.setup.oauth
            && item.setup.registration.as_ref().is_none_or(|key| {
                setting(key, "CLIENT_ID").is_some()
                    && (key != "FEISHU" || setting(key, "TOKEN_URL").is_some())
            })
    });
    ApiResponse::success(ready)
}

pub(crate) fn helper() -> Result<Command, String> {
    if let Some(path) = HELPER.get().filter(|path| path.is_file()) {
        return Ok(Command::new(path));
    }
    #[cfg(debug_assertions)]
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../services/agent-runtime/src/connector-auth.mjs");
        if path.is_file() {
            let mut cmd = Command::new("node");
            cmd.arg(path);
            return Ok(cmd);
        }
    }
    Err("本次安装缺少 Fox 授权组件".to_owned())
}

fn authenticate(
    item: &Connector,
    prior: Option<Value>,
    interactive: bool,
) -> Result<Value, String> {
    let mut request = json!({"endpoint":item.setup.url,"session":prior,"interactive":interactive});
    let own_key = item.id.replace('-', "_").to_ascii_uppercase();
    let registered = item
        .setup
        .registration
        .as_ref()
        .or_else(|| setting(&own_key, "CLIENT_ID").map(|_| &own_key));
    if let Some(registration) = registered {
        request["clientId"] = json!(setting(registration, "CLIENT_ID")
            .ok_or_else(|| format!("Fox 尚未配置 {} 的应用注册", item.name))?);
        request["port"] = json!(setting(registration, "CALLBACK_PORT")
            .unwrap_or_else(|| "42181".to_owned())
            .parse::<u16>()
            .map_err(|_| "回调端口无效")?);
        if let Some(scope) = setting(registration, "SCOPE") {
            request["scope"] = json!(scope);
        }
        request["registration"] = json!(registration);
        if registration == "FEISHU" || setting(registration, "TOKEN_URL").is_some() {
            let broker =
                setting(registration, "TOKEN_URL").ok_or("Fox 尚未配置飞书授权服务地址")?;
            if !Url::parse(&broker).is_ok_and(|u| u.scheme() == "https") {
                return Err("Fox 授权服务需要 HTTPS 地址".to_owned());
            }
            request["tokenUrl"] = json!(broker);
            if registration != "FEISHU" {
                request["issuerTokenUrl"] =
                    json!(setting(registration, "ISSUER_TOKEN_URL").ok_or("缺少平台令牌端点")?);
            }
        }
    } else if let Some(metadata) = setting(&own_key, "METADATA_URL") {
        request["clientMetadataUrl"] = json!(metadata);
        request["port"] = json!(setting(&own_key, "CALLBACK_PORT")
            .unwrap_or_else(|| "42181".into())
            .parse::<u16>()
            .map_err(|_| "回调端口无效")?);
    }
    let mut cmd = helper()?;
    fox_agent_backend::console::HideConsole::hide_console(&mut cmd);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("无法启动 Fox 授权组件：{error}"))?;
    let result = (|| {
        child
            .stdin
            .take()
            .ok_or("授权组件无法接收请求")?
            .write_all(request.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        for line in BufReader::new(child.stdout.take().ok_or("授权组件无法返回结果")?).lines()
        {
            let message: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                .map_err(|_| "授权组件返回格式无效")?;
            if let Some(url) = message["url"].as_str() {
                if !interactive || !Url::parse(url).is_ok_and(|u| u.scheme() == "https") {
                    return Err("无效的授权地址".to_owned());
                }
                if !external_url_open(OpenExternalUrlRequest {
                    url: url.to_owned(),
                })
                .ok
                {
                    return Err("无法打开系统浏览器".to_owned());
                }
            } else if let Some(grant) = message.get("session") {
                return Ok(grant.clone());
            } else if let Some(code) = message["error"].as_str() {
                return Err(format!(
                    "平台授权未完成（{code}），请检查账号、客户端注册和授权范围"
                ));
            }
        }
        Err("授权窗口已关闭、超时或组件未启动".to_owned())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

pub fn bearer(id: &str, endpoint: &Url) -> Result<Option<String>, String> {
    let Some(item) = preset(id).filter(|p| p.setup.oauth) else {
        return Ok(None);
    };
    if item
        .setup
        .url
        .as_deref()
        .and_then(|u| Url::parse(u).ok())
        .as_ref()
        != Some(endpoint)
    {
        return Ok(None);
    }
    let _guard = REFRESH.lock().map_err(|_| "授权刷新状态不可用")?;
    let Some(mut grant) = read(id)? else {
        return Ok(None);
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry = grant["expiresAt"].as_u64().unwrap_or(0);
    if expiry != 0 && expiry <= now + 30 {
        if grant["tokens"]["refresh_token"].as_str().is_none() {
            return Err("平台登录已过期，请重新授权".to_owned());
        }
        grant = authenticate(item, Some(grant), false)?;
        store(id, &grant)?;
    }
    grant["tokens"]["access_token"]
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| "授权记录缺少访问令牌".to_owned())
}

#[tauri::command]
pub async fn connector_authorize(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResponse<McpServerRecord>, String> {
    let db = state.database.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let work = || -> Result<McpServerRecord, String> {
            let item = preset(&id)
                .filter(|p| p.setup.oauth)
                .ok_or("目录条目不支持浏览器授权")?;
            let grant = authenticate(item, None, true)?;
            {
                let _guard = REFRESH.lock().map_err(|_| "授权刷新状态不可用")?;
                store(&id, &grant)?;
            }
            crate::mcp::clear_environment(&id)?;
            let server = db.save_mcp_server(
                &id,
                &item.name,
                "",
                &[],
                "streamable_http",
                item.setup.url.as_deref(),
                None,
            )?;
            crate::mcp::invalidate_connection(&id);
            Ok(server)
        };
        match work() {
            Ok(server) => ApiResponse::success(server),
            Err(e) => ApiResponse::failure("connector.authorization", e, true),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

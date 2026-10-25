use crate::database::ModelConnectionTest;
use keyring::Entry;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use url::Url;

const CREDENTIAL_SERVICE: &str = "com.fox.agent.model";

#[derive(Clone)]
pub struct ModelServiceClient {
    http: Client,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelItem>,
}

#[derive(Deserialize)]
struct ModelItem {
    id: String,
}

#[derive(Serialize)]
struct AnthropicProbe<'a> {
    model: &'a str,
    max_tokens: u16,
    messages: Vec<AnthropicProbeMessage<'a>>,
}

#[derive(Serialize)]
struct AnthropicProbeMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicProbeResponse {
    #[serde(default)]
    model: String,
}

impl ModelServiceClient {
    pub async fn test_output(&self, base_url: &str, api_key: Option<&str>, api_type: &str, model: &str) -> Result<(), String> {
        let base = normalize_model_base_url(base_url)?;
        if model.trim().is_empty() { return Err("请先设置供应商的默认模型".into()); }
        let (endpoint, body) = match api_type {
            "openai-responses" => (format!("{base}/responses"), serde_json::json!({"model": model, "input": "Reply only OK", "max_output_tokens": 1024, "store": false})),
            "anthropic-messages" => (anthropic_messages_url(&base)?.to_string(), serde_json::json!({"model": model, "max_tokens": 1024, "messages": [{"role": "user", "content": "Reply only OK"}]})),
            "openai-completions" => (format!("{base}/chat/completions"), serde_json::json!({"model": model, "max_tokens": 1024, "messages": [{"role": "user", "content": "Reply only OK"}]})),
            _ => return Err("不支持此 API 协议".into()),
        };
        let mut request = self.http.post(endpoint).timeout(Duration::from_secs(90)).json(&body);
        if api_type == "anthropic-messages" {
            request = request.header("anthropic-version", "2023-06-01");
            if let Some(key) = api_key { request = request.header("x-api-key", key); }
        } else if let Some(key) = api_key { request = request.bearer_auth(key); }
        let response = request.send().await.map_err(|e| connection_error(&e))?;
        if !response.status().is_success() { return Err(format!("模型服务返回 HTTP {}", response.status())); }
        let value: serde_json::Value = response.json().await.map_err(|_| "模型服务没有返回有效 JSON")?;
        if !completed_output(api_type, &value) { return Err("请求未返回完整的模型输出，入口尚未通过验证".into()); }
        Ok(())
    }

    pub fn new() -> Result<Self, String> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("Fox/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { http })
    }

    pub async fn test(
        &self,
        base_url: &str,
        api_key: Option<&str>,
        api_type: &str,
        model_id: &str,
    ) -> Result<ModelConnectionTest, String> {
        let normalized = normalize_model_base_url(base_url)?;
        let requested_model_id = normalize_model_id(&normalized, model_id);
        let started = Instant::now();
        let response = match api_type {
            "openai-completions" => {
                let endpoint = models_url(&normalized)?;
                let mut request = self.http.get(endpoint);
                if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
                    request = request.bearer_auth(api_key);
                }
                request.send().await
            }
            "openai-responses" => {
                if requested_model_id.is_empty() { return Err("Responses 连接测试需要模型 ID".into()); }
                let mut request = self.http.post(format!("{normalized}/responses"))
                    .json(&serde_json::json!({"model": requested_model_id, "input": "Reply OK", "max_output_tokens": 32, "store": false}));
                if let Some(key) = api_key.filter(|v| !v.trim().is_empty()) { request = request.bearer_auth(key); }
                request.send().await
            }
            "anthropic-messages" => {
                let model_id = requested_model_id.as_str();
                if model_id.is_empty() {
                    return Err("Anthropic Messages 连接测试需要填写模型 ID".to_owned());
                }
                let api_key = api_key
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "Anthropic Messages 连接测试需要 API Key".to_owned())?;
                self.http
                    .post(anthropic_messages_url(&normalized)?)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&AnthropicProbe {
                        model: model_id,
                        max_tokens: 1,
                        messages: vec![AnthropicProbeMessage {
                            role: "user",
                            content: "Hi",
                        }],
                    })
                    .send()
                    .await
            }
            _ => return Err("不支持的模型 API 协议".to_owned()),
        }
        .map_err(|error| connection_error(&error))?;
        let latency_ms = started.elapsed().as_millis() as i64;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("模型服务返回 HTTP {status}"));
        }
        let models = if api_type == "openai-responses" {
            let value: serde_json::Value = response.json().await.map_err(|_| "响应不是有效的 Responses JSON".to_owned())?;
            if !completed_output("openai-responses", &value) {
                return Err("Responses 请求未返回完成的模型输出".into());
            }
            vec![requested_model_id.clone()]
        } else if api_type == "anthropic-messages" {
            let returned_model = response
                .json::<AnthropicProbeResponse>()
                .await
                .map_err(|_| "响应不是有效的 Anthropic Messages 响应".to_owned())?
                .model;
            vec![if returned_model.is_empty() {
                requested_model_id.clone()
            } else {
                returned_model
            }]
        } else {
            let models = response
                .json::<ModelsResponse>()
                .await
                .map_err(|_| "响应不是有效的 OpenAI-compatible 模型列表".to_owned())?
                .data
                .into_iter()
                .map(|model| model.id)
                .collect::<Vec<_>>();
            if !requested_model_id.is_empty()
                && !models.iter().any(|model| model == &requested_model_id)
            {
                let available = models
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、");
                let suffix = if available.is_empty() {
                    "服务没有返回可用模型".to_owned()
                } else {
                    format!("当前可用模型：{available}")
                };
                return Err(format!(
                    "模型 ID“{requested_model_id}”不在服务返回的模型列表中；{suffix}"
                ));
            }
            models
        };
        Ok(ModelConnectionTest {
            ok: true,
            base_url: normalized.clone(),
            connection_type: connection_type(&normalized),
            status: "connected".to_owned(),
            latency_ms,
            models,
        })
    }
}

fn completed_output(api: &str, value: &serde_json::Value) -> bool {
    let text = |v: &serde_json::Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    match api {
        "openai-responses" => value["object"] == "response" && value["status"] == "completed"
            && value["output"].as_array().is_some_and(|items| items.iter().any(|item| item["type"] == "message" && item["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["type"] == "output_text" && text(&part["text"]))))),
        "anthropic-messages" => value["type"] == "message" && value["stop_reason"] == "end_turn"
            && value["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["type"] == "text" && text(&part["text"]))),
        "openai-completions" => value["choices"].as_array().is_some_and(|choices| choices.iter().any(|choice| choice["finish_reason"] == "stop" && text(&choice["message"]["content"]))),
        _ => false,
    }
}

pub fn normalize_model_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("请输入模型 API 地址".to_owned());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "模型 API 地址格式无效".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("模型 API 地址只支持 http 或 https".to_owned());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "模型 API 地址缺少主机名".to_owned())?;
    if url.scheme() == "http" && !is_private_or_local_host(host) {
        return Err("远程模型服务必须使用 HTTPS；HTTP 仅允许本机或局域网地址".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请勿在模型 API 地址中包含用户名或密码".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("模型 API 地址不能包含查询参数或片段".to_owned());
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(if path.is_empty() { "/" } else { &path });
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Error code and user-facing text for a model service whose base URL is
/// already used by another service (`model_providers.base_url` is UNIQUE).
pub const DUPLICATE_BASE_URL_CODE: &str = "model.duplicate_base_url";
pub const DUPLICATE_BASE_URL_MESSAGE: &str = "这个服务地址已经添加过了";

/// Id of another provider (not `own_id`) that already uses `base_url`.
/// Both sides are compared after trimming whitespace and trailing slashes,
/// case-insensitively (stored URLs are already normalized).
pub fn duplicate_base_url_owner<'a>(
    providers: impl IntoIterator<Item = (&'a str, &'a str)>,
    own_id: &str,
    base_url: &str,
) -> Option<&'a str> {
    let key = |value: &str| value.trim().trim_end_matches('/').to_ascii_lowercase();
    let wanted = key(base_url);
    providers
        .into_iter()
        .find(|(id, url)| *id != own_id && key(url) == wanted)
        .map(|(id, _)| id)
}

/// True when a storage error is SQLite's UNIQUE violation on the provider
/// base URL. Callers map it to [`DUPLICATE_BASE_URL_CODE`] so the raw SQL
/// text never reaches the user.
pub fn is_duplicate_base_url_error(error: &str) -> bool {
    error.contains("UNIQUE constraint failed") && error.contains("model_providers.base_url")
}

/// Preserve provider-defined model IDs, while correcting known cases where a
/// human-facing SenseNova display name was pasted into the machine ID field.
pub fn normalize_model_id(base_url: &str, value: &str) -> String {
    let trimmed = value.trim();
    let is_sensenova_token_api = Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| host.eq_ignore_ascii_case("token.sensenova.cn"));
    if !is_sensenova_token_api {
        return trimmed.to_owned();
    }

    let mut alias = String::with_capacity(trimmed.len());
    let mut previous_separator = false;
    for character in trimmed.chars() {
        if character.is_ascii_alphanumeric() || character == '.' {
            alias.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator && !alias.is_empty() {
            alias.push('-');
            previous_separator = true;
        }
    }
    let alias = alias.trim_end_matches('-');
    if matches!(
        alias,
        "sensenova-6.7-flash-lite" | "sensenova-6.8-flash-lite"
    ) {
        alias.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn set_api_key(base_url: &str, api_key: &str) -> Result<(), String> {
    credential_entry(base_url)?
        .set_password(api_key)
        .map_err(|error| error.to_string())
}

pub fn get_api_key(base_url: &str) -> Option<String> {
    credential_entry(base_url)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.is_empty())
}

pub fn clear_api_key(base_url: &str) -> Result<(), String> {
    match credential_entry(base_url)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The OS credential store behind `fox_agent_backend::credential_save`, so a
/// provider save snapshots, writes and undoes Keys in the tested order.
pub struct KeyringStore;

impl fox_agent_backend::credential_save::KeyStore for KeyringStore {
    fn read(&mut self, id: &str) -> Result<Option<String>, String> {
        match credential_entry(id)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn write(&mut self, id: &str, key: &str) -> Result<(), String> {
        set_api_key(id, key)
    }
    fn delete(&mut self, id: &str) -> Result<(), String> {
        clear_api_key(id)
    }
}

pub fn api_key_configured(base_url: &str) -> bool {
    get_api_key(base_url).is_some()
}

fn credential_entry(base_url: &str) -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, base_url).map_err(|error| error.to_string())
}

fn models_url(base_url: &str) -> Result<Url, String> {
    let mut url = Url::parse(base_url).map_err(|_| "模型 API 地址格式无效".to_owned())?;
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/models"));
    Ok(url)
}

fn anthropic_messages_url(base_url: &str) -> Result<Url, String> {
    let mut url = Url::parse(base_url).map_err(|_| "模型 API 地址格式无效".to_owned())?;
    let path = url.path().trim_end_matches('/');
    let endpoint_path = if path.ends_with("/v1") {
        format!("{path}/messages")
    } else {
        format!("{path}/v1/messages")
    };
    url.set_path(&endpoint_path);
    Ok(url)
}

fn is_private_or_local_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || !host.contains('.') {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
        }
    })
}

fn connection_type(base_url: &str) -> String {
    Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .map(|host| {
            if host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            {
                "local"
            } else if is_private_or_local_host(&host) {
                "lan"
            } else {
                "remote"
            }
        })
        .unwrap_or("remote")
        .to_owned()
}

fn connection_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "连接模型服务超时，请检查地址和网络".to_owned()
    } else if error.is_connect() {
        "无法连接模型服务，请确认服务已启动且当前设备可以访问该地址".to_owned()
    } else {
        format!("连接模型服务失败：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn duplicate_base_url_owner_ignores_trailing_slash_case_and_self() {
        let providers = [("a", "https://api.example.com/v1"), ("b", "https://other.example.com")];
        assert_eq!(duplicate_base_url_owner(providers, "new", "https://api.example.com/v1/"), Some("a"));
        assert_eq!(duplicate_base_url_owner(providers, "new", " HTTPS://API.example.com/v1 "), Some("a"));
        assert_eq!(duplicate_base_url_owner(providers, "a", "https://api.example.com/v1"), None);
        assert_eq!(duplicate_base_url_owner(providers, "new", "https://api.example.com/v2"), None);
    }

    #[test]
    fn duplicate_base_url_error_matches_only_the_provider_unique_index() {
        assert!(is_duplicate_base_url_error("UNIQUE constraint failed: model_providers.base_url"));
        assert!(!is_duplicate_base_url_error("UNIQUE constraint failed: provider_models.provider_id, provider_models.model_id"));
        assert!(!is_duplicate_base_url_error("database is locked"));
    }

    #[test]
    fn validates_local_and_remote_model_urls() {
        assert_eq!(
            normalize_model_base_url("http://127.0.0.1:11434/v1/").unwrap(),
            "http://127.0.0.1:11434/v1"
        );
        assert_eq!(
            normalize_model_base_url("https://api.example.com/v1").unwrap(),
            "https://api.example.com/v1"
        );
        assert!(normalize_model_base_url("http://api.example.com/v1").is_err());
        assert!(normalize_model_base_url("https://user:pass@example.com/v1").is_err());
    }

    #[test]
    fn normalizes_sensenova_display_names_to_api_model_ids() {
        assert_eq!(
            normalize_model_id("https://token.sensenova.cn/v1", "SenseNova 6.8 Flash Lite"),
            "sensenova-6.8-flash-lite"
        );
        assert_eq!(
            normalize_model_id("https://token.sensenova.cn/v1", "SenseNova-6.7-Flash-Lite"),
            "sensenova-6.7-flash-lite"
        );
        assert_eq!(
            normalize_model_id("https://api.example.com/v1", "My Model"),
            "My Model"
        );
    }

    #[test]
    fn tests_openai_compatible_models_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 2048];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            let body = r#"{"data":[{"id":"test-model"}]}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let client = ModelServiceClient::new().expect("create client");
        let result = tauri::async_runtime::block_on(client.test(
            &format!("http://{address}/v1"),
            None,
            "openai-completions",
            "test-model",
        ))
        .expect("test connection");
        assert_eq!(result.models, vec!["test-model"]);
        server.join().expect("join server");
    }

    #[test]
    fn rejects_an_openai_model_id_missing_from_the_provider_catalog() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 2048];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            let body = r#"{"data":[{"id":"sensenova-6.8-flash-lite"}]}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let client = ModelServiceClient::new().expect("create client");
        let error = tauri::async_runtime::block_on(client.test(
            &format!("http://{address}/v1"),
            None,
            "openai-completions",
            "wrong-model",
        ))
        .expect_err("missing model ID must fail the connection test");
        assert!(error.contains("wrong-model"));
        assert!(error.contains("sensenova-6.8-flash-lite"));
        server.join().expect("join server");
    }

    #[test]
    fn builds_anthropic_messages_endpoint() {
        assert_eq!(
            anthropic_messages_url("https://api.minimaxi.com/anthropic")
                .unwrap()
                .as_str(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.example.com/v1")
                .unwrap()
                .as_str(),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn tests_anthropic_messages_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with("POST /v1/messages HTTP/1.1"));
            assert!(request.to_ascii_lowercase().contains("x-api-key: test-key"));
            assert!(request.contains("\"model\":\"MiniMax-M3\""));
            let body = r#"{"id":"msg-1","type":"message","role":"assistant","model":"MiniMax-M3","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let client = ModelServiceClient::new().expect("create client");
        let result = tauri::async_runtime::block_on(client.test(
            &format!("http://{address}"),
            Some("test-key"),
            "anthropic-messages",
            "MiniMax-M3",
        ))
        .expect("test connection");
        assert_eq!(result.models, vec!["MiniMax-M3"]);
        server.join().expect("join server");
    }

    #[test]
    fn responses_probe_requires_completed_output_and_rejects_http_errors() {
        for (status, body, success) in [
            ("200 OK", r#"{"object":"response","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}]}"#, true),
            ("200 OK", r#"{"object":"response","status":"failed","output":[]}"#, false),
            ("200 OK", r#"{"object":"response","status":"completed","output":[{"type":"reasoning","summary":[] }]}"#, false),
            ("200 OK", r#"{"object":"response","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":""}]}]}"#, false),
            ("200 OK", r#"{"data":[{"id":"test-model"}]}"#, false),
            ("422 Unprocessable Entity", r#"{"error":"unsupported"}"#, false),
            ("401 Unauthorized", r#"{"error":"auth"}"#, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    let request = String::from_utf8_lossy(&bytes);
                    if let Some((headers, content)) = request.split_once("\r\n\r\n") {
                        let length: usize = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned)).unwrap().trim().parse().unwrap();
                        if content.len() >= length { break; }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
                assert!(request.to_ascii_lowercase().contains("authorization: bearer test-key"));
                assert!(request.contains("\"model\":\"test-model\""));
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = tauri::async_runtime::block_on(ModelServiceClient::new().unwrap().test(&format!("http://{address}/v1"), Some("test-key"), "openai-responses", "test-model"));
            assert_eq!(result.is_ok(), success, "{status}: {result:?}");
            server.join().unwrap();
        }
    }

    #[test]
    fn protocol_output_requires_terminal_success_and_nonempty_text() {
        let message = serde_json::json!({"type":"message", "stop_reason":"end_turn", "content":[{"type":"text", "text":"OK"}]});
        assert!(completed_output("anthropic-messages", &message));
        let mut truncated = message.clone(); truncated["stop_reason"] = serde_json::json!("max_tokens");
        assert!(!completed_output("anthropic-messages", &truncated));
        assert!(!completed_output("anthropic-messages", &serde_json::json!({"model":"some-model"})));
        let chat = serde_json::json!({"choices":[{"finish_reason":"stop", "message":{"content":"OK"}}]});
        assert!(completed_output("openai-completions", &chat));
        let mut truncated = chat.clone(); truncated["choices"][0]["finish_reason"] = serde_json::json!("length");
        assert!(!completed_output("openai-completions", &truncated));
        assert!(!completed_output("openai-completions", &serde_json::json!({"data":[{"id":"some-model"}]})));
    }

    #[test]
    fn first_use_validation_posts_to_each_protocol_and_rejects_partial_output() {
        for (api, endpoint, body, success) in [
            ("openai-completions", "/v1/chat/completions", r#"{"choices":[{"finish_reason":"stop","message":{"content":"OK"}}]}"#, true),
            ("openai-completions", "/v1/chat/completions", r#"{"choices":[{"finish_reason":"length","message":{"content":"OK"}}]}"#, false),
            ("anthropic-messages", "/v1/messages", r#"{"type":"message","stop_reason":"end_turn","content":[{"type":"text","text":"OK"}]}"#, true),
            ("anthropic-messages", "/v1/messages", r#"{"type":"message","stop_reason":"max_tokens","content":[{"type":"text","text":"OK"}]}"#, false),
            ("openai-responses", "/v1/responses", r#"{"object":"response","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}]}"#, true),
            ("openai-responses", "/v1/responses", r#"{"object":"response","status":"incomplete","output":[]}"#, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap(); assert!(n > 0); bytes.extend_from_slice(&chunk[..n]);
                    let request = String::from_utf8_lossy(&bytes);
                    if let Some((headers, content)) = request.split_once("\r\n\r\n") {
                        let length: usize = headers.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned)).unwrap().trim().parse().unwrap();
                        if content.len() >= length { break; }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with(&format!("POST {endpoint} HTTP/1.1")));
                assert!(request.to_ascii_lowercase().contains(if api == "anthropic-messages" { "x-api-key: test-key" } else { "authorization: bearer test-key" }));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = tauri::async_runtime::block_on(ModelServiceClient::new().unwrap().test_output(&format!("http://{address}/v1"), Some("test-key"), api, "model"));
            assert_eq!(result.is_ok(), success, "{api}: {result:?}");
            server.join().unwrap();
        }
    }
}

pub(crate) fn external_login(database: &crate::database::Database, backend: fox_agent_backend::port::AgentBackend) -> Option<fox_agent_backend::unified_login::UnifiedLogin> {
    crate::external_model::external_login(database, backend)
}

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
        let models = if api_type == "anthropic-messages" {
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
}

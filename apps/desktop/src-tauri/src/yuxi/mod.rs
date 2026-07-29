use crate::database::{
    AgentResourceRecord, AgentResourcesRecord, KnowledgeBaseRecord, KnowledgeBindingRecord,
    KnowledgeDetailRecord, KnowledgeDocumentRecord, KnowledgeDocumentSourceMetadata,
    YuxiAgentRecord, YuxiConnectionTest, YuxiModelRecord, YuxiUserRecord,
};
use keyring::Entry;
use reqwest::{multipart, Client, Method, Response};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;

const CREDENTIAL_SERVICE: &str = "com.fox.agent.yuxi";

#[derive(Clone)]
pub struct YuxiClient {
    http: Client,
}

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
    message: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct YuxiRunStart {
    pub run_id: String,
    pub thread_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct YuxiAttachmentUpload {
    pub file_id: String,
}

impl YuxiClient {
    pub fn new() -> Result<Self, String> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("Fox/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { http })
    }

    pub async fn test(
        &self,
        base_url: &str,
        access_token: Option<&str>,
    ) -> Result<YuxiConnectionTest, String> {
        let normalized = normalize_base_url(base_url)?;
        let endpoint = health_url(&normalized)?;
        let started = Instant::now();
        let response = self
            .http
            .get(endpoint)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        let latency_ms = started.elapsed().as_millis() as i64;
        let status_code = response.status();
        if !status_code.is_success() {
            return Err(format!("知识库服务返回 HTTP {status_code}"));
        }
        let health: HealthResponse = response
            .json()
            .await
            .map_err(|_| "响应不是有效的知识库健康检查数据".to_owned())?;
        let ok = health.status.eq_ignore_ascii_case("ok");
        if !ok {
            return Err(health
                .message
                .unwrap_or_else(|| "知识库服务状态异常".to_owned()));
        }

        let token = access_token
            .map(str::trim)
            .filter(|token| !token.is_empty());
        let (authenticated, auth_status) = if let Some(token) = token {
            self.get_json::<Value>(&normalized, "/api/agent", Some(token))
                .await
                .map_err(|error| format!("知识库服务可达，但凭证验证失败：{error}"))?;
            (true, "authenticated".to_owned())
        } else {
            (false, "not_configured".to_owned())
        };

        Ok(YuxiConnectionTest {
            ok,
            connection_type: connection_type(&normalized),
            base_url: normalized,
            status: "connected".to_owned(),
            authenticated,
            auth_status,
            version: health.version,
            message: if authenticated {
                "服务正常运行，访问凭证已验证".to_owned()
            } else {
                health.message.unwrap_or_else(|| "服务正常运行".to_owned())
            },
            latency_ms,
        })
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        access_token: Option<&str>,
    ) -> Result<T, String> {
        let response = self
            .request(Method::GET, base_url, path, access_token, None)
            .await?;
        response
            .json::<T>()
            .await
            .map_err(|_| "知识库服务返回了无法解析的 JSON 数据".to_owned())
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        access_token: Option<&str>,
        body: &Value,
    ) -> Result<T, String> {
        let response = self
            .request(Method::POST, base_url, path, access_token, Some(body))
            .await?;
        response
            .json::<T>()
            .await
            .map_err(|_| "知识库服务返回了无法解析的 JSON 数据".to_owned())
    }

    pub async fn put_json<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        access_token: Option<&str>,
        body: &Value,
    ) -> Result<T, String> {
        let response = self
            .request(Method::PUT, base_url, path, access_token, Some(body))
            .await?;
        response
            .json::<T>()
            .await
            .map_err(|_| "知识库服务返回了无法解析的 JSON 数据".to_owned())
    }

    pub async fn delete_json<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        access_token: Option<&str>,
    ) -> Result<T, String> {
        let response = self
            .request(Method::DELETE, base_url, path, access_token, None)
            .await?;
        response
            .json::<T>()
            .await
            .map_err(|_| "知识库服务返回了无法解析的 JSON 数据".to_owned())
    }

    async fn request(
        &self,
        method: Method,
        base_url: &str,
        path: &str,
        access_token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Response, String> {
        let normalized = normalize_base_url(base_url)?;
        let endpoint = api_url(&normalized, path)?;
        let mut request = self.http.request(method, endpoint);
        if let Some(token) = access_token
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.json::<Value>().await.ok().and_then(|body| {
                body.get("detail")
                    .or_else(|| body.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            return Err(match status.as_u16() {
                401 => "访问凭证无效或已过期".to_owned(),
                403 => "当前用户无权访问该知识库资源".to_owned(),
                423 => "当前知识库用户已被锁定".to_owned(),
                _ => detail.unwrap_or_else(|| format!("知识库服务返回 HTTP {status}")),
            });
        }
        Ok(response)
    }

    pub async fn login(
        &self,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<(String, YuxiUserRecord), String> {
        let normalized = normalize_base_url(base_url)?;
        let endpoint = api_url(&normalized, "/api/auth/token")?;
        let response = self
            .http
            .post(endpoint)
            .form(&[("username", username.trim()), ("password", password)])
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|_| "知识库登录响应格式无效".to_owned())?;
        if !status.is_success() {
            return Err(body
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("知识库登录失败")
                .to_owned());
        }
        let token = string_field(&body, &["access_token"])?;
        let user = YuxiUserRecord {
            uid: string_field(&body, &["uid"])?,
            username: string_field(&body, &["username"])?,
            avatar: optional_string_field(&body, &["avatar"]),
            role: optional_string_field(&body, &["role"]).unwrap_or_else(|| "user".to_owned()),
            department_id: body.get("department_id").and_then(Value::as_i64),
            department_name: optional_string_field(&body, &["department_name"]),
        };
        Ok((token, user))
    }

    pub async fn current_user(
        &self,
        base_url: &str,
        token: &str,
    ) -> Result<YuxiUserRecord, String> {
        let body: Value = self.get_json(base_url, "/api/auth/me", Some(token)).await?;
        Ok(YuxiUserRecord {
            uid: string_field(&body, &["uid"])?,
            username: string_field(&body, &["username"])?,
            avatar: optional_string_field(&body, &["avatar"]),
            role: optional_string_field(&body, &["role"]).unwrap_or_else(|| "user".to_owned()),
            department_id: body.get("department_id").and_then(Value::as_i64),
            department_name: optional_string_field(&body, &["department_name"]),
        })
    }

    pub async fn list_agents(
        &self,
        base_url: &str,
        token: &str,
    ) -> Result<Vec<YuxiAgentRecord>, String> {
        let body: Value = self.get_json(base_url, "/api/agent", Some(token)).await?;
        let default: Value = self
            .get_json(base_url, "/api/agent/default", Some(token))
            .await
            .unwrap_or(Value::Null);
        let default_slug = default
            .get("agent")
            .and_then(|value| value.get("slug"))
            .and_then(Value::as_str);
        let items = body
            .get("agents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut agents = Vec::new();
        for summary in items {
            let Some(slug) = optional_string_field(&summary, &["slug", "id"]) else {
                continue;
            };
            let detail: Value = self
                .get_json(base_url, &format!("/api/agent/{slug}"), Some(token))
                .await
                .unwrap_or(Value::Null);
            let item = detail.get("agent").unwrap_or(&summary);
            let config = item
                .get("config_json")
                .and_then(|value| value.get("context"))
                .unwrap_or(&Value::Null);
            let configurable_items = item
                .get("configurable_items")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let resources = map_agent_resources(config, &configurable_items);
            agents.push(YuxiAgentRecord {
                id: format!("yuxi:{slug}"),
                slug: slug.clone(),
                name: optional_string_field(item, &["name"]).unwrap_or_else(|| slug.clone()),
                description: optional_string_field(item, &["description"]).unwrap_or_default(),
                icon: optional_string_field(item, &["icon"]),
                backend_id: optional_string_field(item, &["backend_id"])
                    .unwrap_or_else(|| "ChatbotAgent".to_owned()),
                default_model: optional_string_field(config, &["model"])
                    .unwrap_or_else(|| "知识库默认模型".to_owned()),
                capabilities: json!({
                    "tools": resources.tools.len(),
                    "knowledges": resources.knowledges.len(),
                    "mcps": resources.mcps.len(),
                    "skills": resources.skills.len(),
                }),
                resources,
                configurable_items,
                is_default: item
                    .get("is_default")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || default_slug == Some(slug.as_str()),
                available: true,
            });
        }
        Ok(agents)
    }

    pub async fn list_models(
        &self,
        base_url: &str,
        token: &str,
    ) -> Result<Vec<YuxiModelRecord>, String> {
        let body: Value = self
            .get_json(
                base_url,
                "/api/system/model-providers/models/v2",
                Some(token),
            )
            .await?;
        let groups = body
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| "知识库模型列表响应缺少 data".to_owned())?;
        let mut models = Vec::new();
        for (fallback_provider_id, group) in groups {
            let provider_id = optional_string_field(group, &["provider_id"])
                .unwrap_or_else(|| fallback_provider_id.clone());
            let provider_display_name = optional_string_field(group, &["provider_display_name"])
                .unwrap_or_else(|| provider_id.clone());
            for model in group
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(spec) = optional_string_field(model, &["spec"]) else {
                    continue;
                };
                let model_id =
                    optional_string_field(model, &["model_id"]).unwrap_or_else(|| spec.clone());
                let display_name = optional_string_field(model, &["display_name"])
                    .unwrap_or_else(|| model_id.clone());
                models.push(YuxiModelRecord {
                    spec,
                    model_id,
                    display_name,
                    provider_id: provider_id.clone(),
                    provider_display_name: provider_display_name.clone(),
                });
            }
        }
        Ok(models)
    }

    pub async fn list_knowledge_bases(
        &self,
        base_url: &str,
        token: &str,
    ) -> Result<Vec<KnowledgeBaseRecord>, String> {
        let body: Value = self
            .get_json(base_url, "/api/knowledge/databases/accessible", Some(token))
            .await?;
        let items = body
            .get("databases")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut records = Vec::new();
        for item in items {
            let Some(id) = optional_string_field(&item, &["kb_id", "id"]) else {
                continue;
            };
            let tree = self
                .knowledge_tree(base_url, token, &id)
                .await
                .unwrap_or_else(|_| json!({ "entries": [] }));
            let documents = map_knowledge_documents(&tree);
            records.push(map_knowledge_base(&item, &id, &documents));
        }
        Ok(records)
    }

    pub async fn knowledge_detail(
        &self,
        base_url: &str,
        token: &str,
        id: &str,
    ) -> Result<KnowledgeDetailRecord, String> {
        let accessible: Value = self
            .get_json(base_url, "/api/knowledge/databases/accessible", Some(token))
            .await?;
        let item = accessible
            .get("databases")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    optional_string_field(item, &["kb_id", "id"]).as_deref() == Some(id)
                })
            })
            .ok_or_else(|| "当前用户无权访问该知识库".to_owned())?;
        let tree = self.knowledge_tree(base_url, token, id).await?;
        let documents = map_knowledge_documents(&tree);
        let database = map_knowledge_base(item, id, &documents);
        Ok(KnowledgeDetailRecord {
            database,
            documents,
        })
    }

    pub async fn knowledge_document_content(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        doc_id: &str,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/workspace/knowledge/file")?;
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("file_id", doc_id)
            .append_pair("variant", "parsed");
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await
    }

    pub async fn knowledge_document_download(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        doc_id: &str,
    ) -> Result<Response, String> {
        let mut endpoint = api_url(base_url, "/api/workspace/knowledge/download")?;
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("file_id", doc_id)
            .append_pair("variant", "original");
        let download_client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Fox/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        let response = download_client
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!("知识库文件下载返回 HTTP {}", response.status()))
        }
    }

    pub async fn knowledge_document_source_metadata(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        doc_id: &str,
    ) -> Result<KnowledgeDocumentSourceMetadata, String> {
        let mut endpoint = api_url(base_url, "/api/workspace/knowledge/download")?;
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("file_id", doc_id)
            .append_pair("variant", "original");
        let response = self
            .http
            .head(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_knowledge_source_metadata(response)
    }

    pub async fn knowledge_document_range(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        doc_id: &str,
        start: u64,
        end: u64,
        source_revision: &str,
    ) -> Result<Vec<u8>, String> {
        if end < start {
            return Err("知识库文件范围结束位置不能小于开始位置".to_owned());
        }
        const MAX_RANGE_BYTES: u64 = 4 * 1024 * 1024;
        let requested = end
            .checked_sub(start)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| "知识库文件范围过大".to_owned())?;
        if requested > MAX_RANGE_BYTES {
            return Err(format!("单次知识库文件范围不能超过 {MAX_RANGE_BYTES} 字节"));
        }
        let mut endpoint = api_url(base_url, "/api/workspace/knowledge/download")?;
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("file_id", doc_id)
            .append_pair("variant", "original");
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .header(reqwest::header::IF_RANGE, source_revision)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!(
                "知识库服务未按范围返回文件内容（HTTP {}），请升级知识库服务",
                response.status()
            ));
        }
        validate_content_range(response.headers(), start, end)?;
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("读取知识库文件范围失败：{error}"))?;
        if bytes.len() as u64 != requested {
            return Err(format!(
                "知识库文件范围长度不一致：预期 {requested} 字节，实际 {} 字节",
                bytes.len()
            ));
        }
        Ok(bytes.to_vec())
    }

    pub async fn query_knowledge(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        query: &str,
    ) -> Result<Value, String> {
        let _ = (base_url, token, kb_id, query);
        Err(
            "当前普通用户 API 暂未提供独立知识库检索；请选择智能体后在对话中绑定知识库提问"
                .to_owned(),
        )
    }

    pub async fn graph_subgraph(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        keyword: &str,
        max_depth: i64,
        max_nodes: i64,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/graph/subgraph")?;
        let node_label = if keyword.trim().is_empty() {
            "*"
        } else {
            keyword.trim()
        };
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("node_label", node_label)
            .append_pair("max_depth", &max_depth.to_string())
            .append_pair("max_nodes", &max_nodes.to_string())
            .append_pair("exclude_chunk", "true");
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await
    }

    pub async fn graph_labels(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/graph/labels")?;
        endpoint.query_pairs_mut().append_pair("kb_id", kb_id);
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await
    }

    pub async fn graph_stats(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/graph/stats")?;
        endpoint.query_pairs_mut().append_pair("kb_id", kb_id);
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await
    }

    async fn knowledge_tree(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/workspace/knowledge/tree")?;
        endpoint
            .query_pairs_mut()
            .append_pair("kb_id", kb_id)
            .append_pair("recursive", "true");
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await
    }

    pub async fn create_thread(
        &self,
        base_url: &str,
        token: &str,
        agent_id: &str,
        title: &str,
    ) -> Result<String, String> {
        let body: Value = self
            .post_json(
                base_url,
                "/api/chat/thread",
                Some(token),
                &json!({
                    "agent_id": agent_id, "title": title, "metadata": { "client": "fox-desktop" }
                }),
            )
            .await?;
        string_field(&body, &["id"])
    }

    pub async fn rename_thread(
        &self,
        base_url: &str,
        token: &str,
        thread_id: &str,
        title: &str,
    ) -> Result<(), String> {
        let _: Value = self
            .put_json(
                base_url,
                &format!("/api/chat/thread/{thread_id}"),
                Some(token),
                &json!({ "title": title }),
            )
            .await?;
        Ok(())
    }

    pub async fn delete_thread(
        &self,
        base_url: &str,
        token: &str,
        thread_id: &str,
    ) -> Result<(), String> {
        let _: Value = self
            .delete_json(
                base_url,
                &format!("/api/chat/thread/{thread_id}"),
                Some(token),
            )
            .await?;
        Ok(())
    }

    pub async fn create_run(
        &self,
        base_url: &str,
        token: &str,
        agent_id: &str,
        thread_id: &str,
        query: &str,
        model: Option<&str>,
        attachment_file_ids: &[String],
        knowledge_bindings: &[KnowledgeBindingRecord],
    ) -> Result<YuxiRunStart, String> {
        let request_body = create_run_request_body(
            query,
            agent_id,
            thread_id,
            model,
            attachment_file_ids,
            knowledge_bindings,
        );
        let body: Value = self
            .post_json(base_url, "/api/agent/runs", Some(token), &request_body)
            .await?;
        Ok(YuxiRunStart {
            run_id: string_field(&body, &["run_id"])?,
            thread_id: optional_string_field(&body, &["thread_id"])
                .unwrap_or_else(|| thread_id.to_owned()),
            status: optional_string_field(&body, &["status"])
                .unwrap_or_else(|| "pending".to_owned()),
        })
    }

    pub async fn create_resume_run(
        &self,
        base_url: &str,
        token: &str,
        agent_id: &str,
        thread_id: &str,
        parent_run_id: &str,
        answers: &Value,
        knowledge_bindings: &[KnowledgeBindingRecord],
    ) -> Result<YuxiRunStart, String> {
        let resume_request_id = Uuid::new_v4().to_string();
        let knowledge_bases = knowledge_binding_metadata(knowledge_bindings);
        let knowledge_base_ids = knowledge_bases
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        let body: Value = self
            .post_json(
                base_url,
                "/api/agent/runs",
                Some(token),
                &json!({
                    "query": Value::Null,
                    "agent_id": agent_id,
                    "thread_id": thread_id,
                    "resume": answers,
                    "parent_run_id": parent_run_id,
                    "resume_request_id": resume_request_id,
                    "meta": {
                        "client": "fox-desktop",
                        "knowledge_base_ids": knowledge_base_ids,
                        "knowledge_bases": knowledge_bases,
                    },
                }),
            )
            .await?;
        Ok(YuxiRunStart {
            run_id: string_field(&body, &["run_id"])?,
            thread_id: optional_string_field(&body, &["thread_id"])
                .unwrap_or_else(|| thread_id.to_owned()),
            status: optional_string_field(&body, &["status"])
                .unwrap_or_else(|| "pending".to_owned()),
        })
    }

    pub async fn upload_thread_attachment(
        &self,
        base_url: &str,
        token: &str,
        thread_id: &str,
        filename: &str,
        media_type: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<YuxiAttachmentUpload, String> {
        let normalized = normalize_base_url(base_url)?;
        let endpoint = api_url(
            &normalized,
            &format!("/api/chat/thread/{thread_id}/attachments"),
        )?;
        let mut part = multipart::Part::bytes(bytes).file_name(filename.to_owned());
        if let Some(media_type) = media_type.filter(|value| !value.trim().is_empty()) {
            part = part
                .mime_str(media_type)
                .map_err(|error| format!("附件媒体类型无效: {error}"))?;
        }
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(token)
            .multipart(multipart::Form::new().part("file", part))
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        parse_response_json(response).await.and_then(|value| {
            serde_json::from_value(value).map_err(|_| "知识库附件上传响应格式无效".to_owned())
        })
    }

    pub async fn cancel_run(
        &self,
        base_url: &str,
        token: &str,
        run_id: &str,
    ) -> Result<(), String> {
        let _: Value = self
            .post_json(
                base_url,
                &format!("/api/agent/runs/{run_id}/cancel"),
                Some(token),
                &json!({}),
            )
            .await?;
        Ok(())
    }

    pub async fn get_run(
        &self,
        base_url: &str,
        token: &str,
        run_id: &str,
    ) -> Result<Value, String> {
        self.get_json(base_url, &format!("/api/agent/runs/{run_id}"), Some(token))
            .await
    }

    pub async fn run_event_response(
        &self,
        base_url: &str,
        token: &str,
        run_id: &str,
        cursor: Option<&str>,
    ) -> Result<Response, String> {
        let mut endpoint = api_url(base_url, &format!("/api/agent/runs/{run_id}/events"))?;
        endpoint.query_pairs_mut().append_pair("verbose", "false");
        let mut request = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .header("Accept", "text/event-stream");
        if let Some(cursor) = cursor.filter(|value| !value.is_empty() && *value != "0-0") {
            request = request.header("Last-Event-ID", cursor);
        }
        let response = request
            .send()
            .await
            .map_err(|error| connection_error(&error))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!("知识库任务事件流返回 HTTP {}", response.status()))
        }
    }
}

async fn parse_response_json(response: Response) -> Result<Value, String> {
    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .map_err(|_| "知识库服务返回了无法解析的 JSON 数据".to_owned())?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(body
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("知识库请求失败")
            .to_owned())
    }
}

fn parse_knowledge_source_metadata(
    response: Response,
) -> Result<KnowledgeDocumentSourceMetadata, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("知识库原文件元数据返回 HTTP {status}"));
    }
    let headers = response.headers();
    let size = headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "知识库原文件元数据缺少有效的 Content-Length".to_owned())?;
    let source_revision = required_header(headers, "x-source-revision")?;
    let filename = decoded_file_name_header(headers).unwrap_or_else(|| "knowledge-file".to_owned());
    let media_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let accepts_ranges = headers
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
    if !accepts_ranges {
        return Err("知识库服务尚未声明字节范围读取能力".to_owned());
    }
    Ok(KnowledgeDocumentSourceMetadata {
        filename,
        media_type,
        size,
        source_revision,
        version_id: header_string(headers, "x-version-id"),
        etag: headers
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        last_modified: headers
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        accepts_ranges,
        preview_api_version: header_string(headers, "x-preview-api-version")
            .and_then(|value| value.parse::<u32>().ok()),
        supported_preview_variants: comma_separated_header(headers, "x-supported-preview-variants"),
        available_variants: comma_separated_header(headers, "x-available-variants"),
    })
}

fn comma_separated_header(headers: &reqwest::header::HeaderMap, name: &str) -> Vec<String> {
    let mut variants = header_string(headers, name)
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    variants.sort();
    variants.dedup();
    variants
}

fn header_string(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn decoded_file_name_header(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let value = header_string(headers, "x-file-name")?;
    if !header_string(headers, "x-file-name-encoding")
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("percent"))
    {
        return Some(value);
    }
    decode_percent_encoded_utf8(&value)
}

fn decode_percent_encoded_utf8(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((decode_hex_digit(high)? << 4) | decode_hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn decode_hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn required_header(headers: &reqwest::header::HeaderMap, name: &str) -> Result<String, String> {
    header_string(headers, name).ok_or_else(|| format!("知识库原文件元数据缺少 {name}"))
}

fn validate_content_range(
    headers: &reqwest::header::HeaderMap,
    expected_start: u64,
    expected_end: u64,
) -> Result<(), String> {
    let value = headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "知识库范围响应缺少 Content-Range".to_owned())?;
    let range = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split('/').next())
        .ok_or_else(|| "知识库范围响应的 Content-Range 无效".to_owned())?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| "知识库范围响应的 Content-Range 无效".to_owned())?;
    let start = start
        .parse::<u64>()
        .map_err(|_| "知识库范围响应的开始位置无效".to_owned())?;
    let end = end
        .parse::<u64>()
        .map_err(|_| "知识库范围响应的结束位置无效".to_owned())?;
    if start != expected_start || end != expected_end {
        return Err(format!(
            "知识库范围响应不匹配：请求 {expected_start}-{expected_end}，返回 {start}-{end}"
        ));
    }
    Ok(())
}

fn string_field(value: &Value, keys: &[&str]) -> Result<String, String> {
    optional_string_field(value, keys)
        .ok_or_else(|| format!("知识库响应缺少字段 {}", keys.join("/")))
}

fn optional_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| match item {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
    })
}

fn knowledge_binding_metadata(bindings: &[KnowledgeBindingRecord]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    bindings
        .iter()
        .filter(|binding| binding.enabled)
        .filter_map(|binding| {
            let id = binding.knowledge_base_id.trim();
            if id.is_empty() || !seen.insert(id.to_owned()) {
                return None;
            }
            Some(json!({
                "id": id,
                "name": binding
                    .knowledge_base_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty()),
            }))
        })
        .collect()
}

fn create_run_request_body(
    query: &str,
    agent_id: &str,
    thread_id: &str,
    model: Option<&str>,
    attachment_file_ids: &[String],
    knowledge_bindings: &[KnowledgeBindingRecord],
) -> Value {
    let knowledge_bases = knowledge_binding_metadata(knowledge_bindings);
    let knowledge_base_ids = knowledge_bases
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    json!({
        "query": query,
        "agent_id": agent_id,
        "thread_id": thread_id,
        "meta": {
            "client": "fox-desktop",
            "attachment_file_ids": attachment_file_ids,
            "knowledge_base_ids": knowledge_base_ids,
            "knowledge_bases": knowledge_bases,
        },
        "model_spec": model,
    })
}

fn map_agent_resources(context: &Value, configurable_items: &Value) -> AgentResourcesRecord {
    AgentResourcesRecord {
        tools: map_agent_resource_kind(context, configurable_items, "tools"),
        knowledges: map_agent_resource_kind(context, configurable_items, "knowledges"),
        mcps: map_agent_resource_kind(context, configurable_items, "mcps"),
        skills: map_agent_resource_kind(context, configurable_items, "skills"),
    }
}

fn map_agent_resource_kind(
    context: &Value,
    configurable_items: &Value,
    kind: &str,
) -> Vec<AgentResourceRecord> {
    let options = configurable_items
        .get(kind)
        .and_then(|item| item.get("options"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let selected = context.get(kind).and_then(Value::as_array);

    options
        .into_iter()
        .filter_map(|option| {
            let id = resource_option_id(&option)?;
            if selected.is_some_and(|selected| {
                !selected
                    .iter()
                    .any(|value| resource_selection_matches(value, &id))
            }) {
                return None;
            }
            Some(AgentResourceRecord {
                id: id.clone(),
                name: optional_string_field(&option, &["name", "label"])
                    .unwrap_or_else(|| id.clone()),
                description: optional_string_field(&option, &["description"]).unwrap_or_default(),
            })
        })
        .collect()
}

fn resource_option_id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Object(_) => optional_string_field(
            value,
            &["key", "id", "value", "name", "db_id", "slug", "label"],
        ),
        _ => None,
    }
}

fn resource_selection_matches(value: &Value, option_id: &str) -> bool {
    resource_option_id(value).as_deref() == Some(option_id)
}

fn map_knowledge_documents(value: &Value) -> Vec<KnowledgeDocumentRecord> {
    value
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = optional_string_field(item, &["file_id", "id"])?;
            Some(KnowledgeDocumentRecord {
                id,
                name: optional_string_field(item, &["name"])
                    .unwrap_or_else(|| "未命名文档".to_owned()),
                parent_id: optional_string_field(item, &["parent_id"]),
                is_folder: item
                    .get("is_dir")
                    .or_else(|| item.get("is_folder"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                status: optional_string_field(item, &["status"]),
                size: item.get("size").and_then(Value::as_i64).unwrap_or(0),
                created_at: optional_string_field(item, &["created_at"]),
                updated_at: optional_string_field(item, &["modified_at", "updated_at"]),
            })
        })
        .collect()
}

fn map_knowledge_base(
    value: &Value,
    fallback_id: &str,
    documents: &[KnowledgeDocumentRecord],
) -> KnowledgeBaseRecord {
    let files = documents.iter().filter(|item| !item.is_folder);
    let file_count = files.clone().count() as i64;
    let processed_count = files
        .filter(|item| {
            item.status.as_deref().is_some_and(|status| {
                matches!(
                    status,
                    "done" | "ready" | "active" | "completed" | "success"
                )
            })
        })
        .count() as i64;
    KnowledgeBaseRecord {
        id: optional_string_field(value, &["kb_id", "id"])
            .unwrap_or_else(|| fallback_id.to_owned()),
        name: optional_string_field(value, &["name"]).unwrap_or_else(|| "未命名知识库".to_owned()),
        description: optional_string_field(value, &["description"]).unwrap_or_default(),
        kb_type: optional_string_field(value, &["kb_type"]),
        status: optional_string_field(value, &["status"]).or_else(|| {
            Some(
                if file_count == processed_count {
                    "active"
                } else {
                    "processing"
                }
                .to_owned(),
            )
        }),
        file_count,
        processed_count,
        row_count: value.get("row_count").and_then(Value::as_i64).unwrap_or(0),
        created_at: optional_string_field(value, &["created_at"]),
        updated_at: optional_string_field(value, &["updated_at"]),
    }
}

pub fn normalize_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("请输入知识库 API 地址".to_owned());
    }
    let mut url = Url::parse(trimmed).map_err(|_| "知识库地址格式无效".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("知识库地址只支持 http 或 https".to_owned());
    }
    if url.host_str().is_none() {
        return Err("知识库地址缺少主机名".to_owned());
    }
    let host = url.host_str().unwrap_or_default();
    if url.scheme() == "http" && !is_private_or_local_host(host) {
        return Err("远程知识库服务必须使用 HTTPS；HTTP 仅允许本机或局域网地址".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请勿在知识库地址中包含用户名或密码".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("知识库地址不能包含查询参数或片段".to_owned());
    }
    let mut path = url.path().trim_end_matches('/').to_owned();
    if path.ends_with("/api") {
        path.truncate(path.len() - 4);
    } else if path == "/api" {
        path.clear();
    }
    url.set_path(if path.is_empty() { "/" } else { &path });
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn set_access_token(base_url: &str, token: &str) -> Result<(), String> {
    let entry = credential_entry(base_url)?;
    entry.set_password(token).map_err(|error| error.to_string())
}

pub fn clear_access_token(base_url: &str) -> Result<(), String> {
    let entry = credential_entry(base_url)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn access_token_configured(base_url: &str) -> bool {
    get_access_token(base_url).is_some()
}

pub fn get_access_token(base_url: &str) -> Option<String> {
    credential_entry(base_url)
        .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
        .ok()
        .filter(|token| !token.is_empty())
}

fn credential_entry(base_url: &str) -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, base_url).map_err(|error| error.to_string())
}

fn health_url(base_url: &str) -> Result<Url, String> {
    api_url(base_url, "/api/system/health")
}

fn api_url(base_url: &str, path: &str) -> Result<Url, String> {
    let mut url = Url::parse(base_url).map_err(|_| "知识库地址格式无效".to_owned())?;
    let base_path = url.path().trim_end_matches('/');
    let suffix = path.trim_start_matches('/');
    url.set_path(&format!("{base_path}/{suffix}"));
    Ok(url)
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

fn connection_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "连接知识库超时，请检查地址和网络".to_owned()
    } else if error.is_connect() {
        "无法连接知识库，请确认服务已启动且当前设备可以访问该地址".to_owned()
    } else {
        format!("连接知识库失败：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn validates_exact_content_ranges() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 1024-2047/4096"),
        );
        assert!(validate_content_range(&headers, 1024, 2047).is_ok());
        assert!(validate_content_range(&headers, 0, 1023).is_err());
    }

    #[test]
    fn rejects_missing_source_revision_headers() {
        let headers = HeaderMap::new();
        assert!(required_header(&headers, "x-source-revision").is_err());
    }

    #[test]
    fn decodes_percent_encoded_knowledge_file_names() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-file-name",
            HeaderValue::from_static("%E8%BD%A8%E9%81%93%E5%90%8A.pdf"),
        );
        headers.insert("x-file-name-encoding", HeaderValue::from_static("percent"));
        assert_eq!(
            decoded_file_name_header(&headers).as_deref(),
            Some("轨道吊.pdf")
        );
    }

    #[test]
    fn reads_source_metadata_from_the_authenticated_head_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with(
                "HEAD /api/workspace/knowledge/download?kb_id=kb-1&file_id=file-1&variant=original HTTP/1.1"
            ));
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: 4096\r\nAccept-Ranges: bytes\r\nX-Source-Revision: sha256:abc\r\nX-File-Name: manual.pdf\r\nX-Preview-Api-Version: 1\r\nX-Supported-Preview-Variants: \r\nX-Available-Variants: original\r\nETag: \"opaque\"\r\nConnection: close\r\n\r\n",
                )
                .expect("write response");
        });

        let client = YuxiClient::new().expect("create client");
        let metadata = tauri::async_runtime::block_on(client.knowledge_document_source_metadata(
            &format!("http://{address}"),
            "test-token",
            "kb-1",
            "file-1",
        ))
        .expect("read metadata");
        assert_eq!(metadata.size, 4096);
        assert_eq!(metadata.filename, "manual.pdf");
        assert_eq!(metadata.media_type, "application/pdf");
        assert_eq!(metadata.source_revision, "sha256:abc");
        assert!(metadata.accepts_ranges);
        assert_eq!(metadata.preview_api_version, Some(1));
        assert!(metadata.supported_preview_variants.is_empty());
        assert_eq!(metadata.available_variants, vec!["original"]);
        server.join().expect("join test server");
    }

    #[test]
    fn reads_only_the_requested_authenticated_byte_range() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with(
                "GET /api/workspace/knowledge/download?kb_id=kb-1&file_id=file-1&variant=original HTTP/1.1"
            ));
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("range: bytes=4-7")));
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("if-range: sha256:abc")));
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
            stream
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Length: 4\r\nContent-Range: bytes 4-7/16\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n4567",
                )
                .expect("write response");
        });

        let client = YuxiClient::new().expect("create client");
        let bytes = tauri::async_runtime::block_on(client.knowledge_document_range(
            &format!("http://{address}"),
            "test-token",
            "kb-1",
            "file-1",
            4,
            7,
            "sha256:abc",
        ))
        .expect("read range");
        assert_eq!(bytes, b"4567");
        server.join().expect("join test server");
    }

    #[test]
    fn refuses_servers_that_ignore_range_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\n0123456789abcdef",
                )
                .expect("write response");
        });

        let client = YuxiClient::new().expect("create client");
        let error = tauri::async_runtime::block_on(client.knowledge_document_range(
            &format!("http://{address}"),
            "test-token",
            "kb-1",
            "file-1",
            4,
            7,
            "sha256:abc",
        ))
        .expect_err("reject full response");
        assert!(error.contains("未按范围返回"));
        server.join().expect("join test server");
    }

    #[test]
    fn serializes_enabled_knowledge_bindings_for_remote_run_metadata() {
        let bindings = vec![
            KnowledgeBindingRecord {
                conversation_id: "conversation-1".to_owned(),
                service_connection_id: "knowledge-service".to_owned(),
                knowledge_base_id: "kb-1".to_owned(),
                knowledge_base_name: Some("产品资料".to_owned()),
                enabled: true,
                created_at: 0,
                updated_at: 0,
            },
            KnowledgeBindingRecord {
                conversation_id: "conversation-1".to_owned(),
                service_connection_id: "knowledge-service".to_owned(),
                knowledge_base_id: "kb-1".to_owned(),
                knowledge_base_name: Some("重复项".to_owned()),
                enabled: true,
                created_at: 0,
                updated_at: 0,
            },
            KnowledgeBindingRecord {
                conversation_id: "conversation-1".to_owned(),
                service_connection_id: "knowledge-service".to_owned(),
                knowledge_base_id: "kb-2".to_owned(),
                knowledge_base_name: None,
                enabled: false,
                created_at: 0,
                updated_at: 0,
            },
        ];

        assert_eq!(
            knowledge_binding_metadata(&bindings),
            vec![json!({ "id": "kb-1", "name": "产品资料" })]
        );

        let body = create_run_request_body(
            "查询产品参数",
            "assistant",
            "thread-1",
            Some("provider:model"),
            &["file-1".to_owned()],
            &bindings,
        );
        assert_eq!(body["query"], "查询产品参数");
        assert_eq!(body["meta"]["knowledge_base_ids"], json!(["kb-1"]));
        assert_eq!(
            body["meta"]["knowledge_bases"],
            json!([{ "id": "kb-1", "name": "产品资料" }])
        );
        assert_eq!(body["meta"]["attachment_file_ids"], json!(["file-1"]));
    }

    #[test]
    fn normalizes_local_and_remote_urls() {
        assert_eq!(
            normalize_base_url(" http://127.0.0.1:5050/ ").unwrap(),
            "http://127.0.0.1:5050"
        );
        assert_eq!(
            normalize_base_url("https://agent.example.com/yuxi/").unwrap(),
            "https://agent.example.com/yuxi"
        );
        assert_eq!(
            normalize_base_url("https://agent.example.com/yuxi/api/").unwrap(),
            "https://agent.example.com/yuxi"
        );
        assert_eq!(
            normalize_base_url("http://192.168.1.20:5050").unwrap(),
            "http://192.168.1.20:5050"
        );
        assert_eq!(connection_type("http://192.168.1.20:5050"), "lan");
    }

    #[test]
    fn rejects_unsafe_base_url_shapes() {
        assert!(normalize_base_url("file:///tmp/yuxi").is_err());
        assert!(normalize_base_url("https://user:pass@example.com").is_err());
        assert!(normalize_base_url("https://example.com?token=secret").is_err());
        assert!(normalize_base_url("http://agent.example.com").is_err());
    }

    #[test]
    fn maps_workspace_knowledge_tree_entries() {
        let documents = map_knowledge_documents(&json!({
            "entries": [
                {"file_id":"folder-1","name":"资料","is_dir":true,"status":"done"},
                {"file_id":"doc-1","parent_id":"folder-1","name":"说明.md","is_dir":false,"size":128,"status":"done","modified_at":"2026-07-17T12:00:00Z"},
                {"file_id":"doc-2","name":"待处理.pdf","is_dir":false,"size":256,"status":"processing"}
            ]
        }));
        assert_eq!(documents.len(), 3);
        assert!(documents[0].is_folder);
        assert_eq!(documents[1].parent_id.as_deref(), Some("folder-1"));

        let database = map_knowledge_base(
            &json!({"kb_id":"kb-1","name":"测试知识库","description":"说明"}),
            "fallback",
            &documents,
        );
        assert_eq!(database.id, "kb-1");
        assert_eq!(database.file_count, 2);
        assert_eq!(database.processed_count, 1);
        assert_eq!(database.status.as_deref(), Some("processing"));
    }

    #[test]
    fn maps_selected_and_default_all_agent_resources() {
        let configurable = json!({
            "tools": {"options": [
                {"key":"read","name":"读取文件","description":"读取文件内容"},
                {"key":"write","name":"写入文件","description":"写入文件内容"}
            ]},
            "knowledges": {"options": [
                {"kb_id":"kb-1","name":"产品知识库"}
            ]},
            "mcps": {"options": [
                {"id":"mcp-1","name":"浏览器 MCP"}
            ]},
            "skills": {"options": [
                {"slug":"research","name":"研究技能"}
            ]}
        });
        let resources = map_agent_resources(
            &json!({"tools":["read"],"knowledges":null,"mcps":[],"skills":null}),
            &configurable,
        );
        assert_eq!(resources.tools.len(), 1);
        assert_eq!(resources.tools[0].id, "read");
        assert_eq!(resources.knowledges.len(), 1);
        assert!(resources.mcps.is_empty());
        assert_eq!(resources.skills.len(), 1);
    }

    #[test]
    fn tests_the_real_yuxi_health_path() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 2048];
            let length = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with("GET /api/system/health HTTP/1.1"));
            let body = r#"{"status":"ok","message":"ready","version":"0.7.0"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let client = YuxiClient::new().expect("create Yuxi client");
        let result =
            tauri::async_runtime::block_on(client.test(&format!("http://{address}"), None))
                .expect("test Yuxi connection");
        assert!(result.ok);
        assert!(!result.authenticated);
        assert_eq!(result.auth_status, "not_configured");
        assert_eq!(result.version.as_deref(), Some("0.7.0"));
        assert_eq!(result.connection_type, "local");
        server.join().expect("join test server");
    }

    #[test]
    fn verifies_access_token_against_the_agent_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test address");
        let server = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept test request");
                let mut request = [0_u8; 4096];
                let length = stream.read(&mut request).expect("read request");
                let request = String::from_utf8_lossy(&request[..length]);
                let body = if request_index == 0 {
                    assert!(request.starts_with("GET /api/system/health HTTP/1.1"));
                    r#"{"status":"ok","message":"ready","version":"0.7.0"}"#
                } else {
                    assert!(request.starts_with("GET /api/agent HTTP/1.1"));
                    assert!(request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token")));
                    r#"{"agents":[]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        let client = YuxiClient::new().expect("create Yuxi client");
        let result = tauri::async_runtime::block_on(
            client.test(&format!("http://{address}"), Some("test-token")),
        )
        .expect("verify Yuxi access token");
        assert!(result.ok);
        assert!(result.authenticated);
        assert_eq!(result.auth_status, "authenticated");
        assert_eq!(result.message, "服务正常运行，访问凭证已验证");
        server.join().expect("join test server");
    }

    #[test]
    fn tests_configured_live_yuxi_when_requested() {
        let Ok(base_url) = std::env::var("FOX_TEST_YUXI_URL") else {
            return;
        };
        let client = YuxiClient::new().expect("create Yuxi client");
        let token = std::env::var("FOX_TEST_YUXI_TOKEN")
            .ok()
            .or_else(|| get_access_token(&base_url));
        let result = tauri::async_runtime::block_on(client.test(&base_url, token.as_deref()))
            .expect("connect to configured live Yuxi");
        assert!(result.ok);
        assert!(!result.version.as_deref().unwrap_or_default().is_empty());
        if token.is_some() {
            assert!(result.authenticated);
        }
    }

    #[test]
    fn tests_configured_live_knowledge_original_gateway_when_requested() {
        if std::env::var("FOX_TEST_YUXI_PREVIEW").as_deref() != Ok("1") {
            return;
        }
        let base_url = std::env::var("FOX_TEST_YUXI_URL")
            .expect("FOX_TEST_YUXI_URL is required for the live preview test");
        let token = std::env::var("FOX_TEST_YUXI_TOKEN")
            .ok()
            .or_else(|| get_access_token(&base_url))
            .expect("Fox has no saved knowledge-service credential for this URL");
        let client = YuxiClient::new().expect("create Yuxi client");
        let databases =
            tauri::async_runtime::block_on(client.list_knowledge_bases(&base_url, &token))
                .expect("list accessible knowledge bases");
        let requested_kb = std::env::var("FOX_TEST_YUXI_KB_ID").ok();
        let database = databases
            .iter()
            .find(|item| requested_kb.as_deref().is_none_or(|id| item.id == id))
            .expect("no matching accessible knowledge base");
        let detail = tauri::async_runtime::block_on(client.knowledge_detail(
            &base_url,
            &token,
            &database.id,
        ))
        .expect("load knowledge-base documents");
        let requested_file = std::env::var("FOX_TEST_YUXI_FILE_ID").ok();
        let document = detail
            .documents
            .iter()
            .find(|item| {
                !item.is_folder
                    && requested_file.as_deref().is_none_or(|id| item.id == id)
                    && item.name.rsplit_once('.').is_some_and(|(_, extension)| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "docx" | "xlsx" | "xls" | "ods" | "pptx" | "odp"
                        )
                    })
            })
            .expect("no matching Office document is available for preview verification");
        let metadata = tauri::async_runtime::block_on(client.knowledge_document_source_metadata(
            &base_url,
            &token,
            &database.id,
            &document.id,
        ))
        .expect("read authenticated original metadata");
        assert!(metadata.size > 0);
        assert!(metadata.accepts_ranges);
        let original_end = metadata.size.min(32) - 1;
        let original = tauri::async_runtime::block_on(client.knowledge_document_range(
            &base_url,
            &token,
            &database.id,
            &document.id,
            0,
            original_end,
            &metadata.source_revision,
        ))
        .expect("read exact original range");
        assert_eq!(original.len() as u64, original_end + 1);
    }
}
pub mod runtime;

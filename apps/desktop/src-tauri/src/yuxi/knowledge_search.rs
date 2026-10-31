//! 远程知识库只走服务端原生索引检索；索引不可用时直接报错，不降级为文档关键词扫描。
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::{api_url, YuxiClient};
use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const SEARCH_DURATION: Duration = Duration::from_secs(25);

fn remote_http_error(status: StatusCode) -> String {
    let (code, message) = match status.as_u16() {
        401 => (
            "authentication_required",
            "知识库登录已失效，请重新登录知识库服务",
        ),
        403 => (
            "access_denied",
            "知识库服务拒绝当前账号读取该资料；这不表示会话没有绑定知识库",
        ),
        400 | 404 | 422 => (
            "document_unavailable",
            "知识文档或解析文本暂不可读，请检查文档是否解析完成",
        ),
        429 | 500..=599 => ("retrieval_unavailable", "知识库服务暂时不可用"),
        _ => ("protocol_invalid", "知识库服务返回了不支持的响应"),
    };
    format!("[remote_knowledge.{code}] {message} (HTTP {status})")
}

async fn bounded_body(mut response: Response) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(remote_http_error(response.status()));
    }
    let too_large =
        || "[remote_knowledge.response_too_large] 知识文档超过单次读取大小上限".to_owned();
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "[remote_knowledge.retrieval_unavailable] 读取知识库响应失败".to_owned())?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn normalize_indexed_results(
    payload: Value,
    kb_id: &str,
    query: &str,
    limit: usize,
    max_chars: usize,
) -> Result<Value, String> {
    let invalid =
        || "[remote_knowledge.protocol_invalid] 知识库检索响应缺少有效结果或来源".to_owned();
    if payload.get("status").is_some_and(|s| s != "success") {
        return Err(invalid());
    }
    let items = payload
        .as_array()
        .or_else(|| payload.get("result").and_then(Value::as_array))
        .ok_or_else(invalid)?;
    let mut remaining = max_chars;
    let mut truncated = items.len() > limit;
    let mut results = Vec::new();
    for item in items.iter().take(limit) {
        if remaining == 0 {
            truncated = true;
            break;
        }
        let text = item
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let mut metadata = item
            .get("metadata")
            .filter(|m| m.is_object())
            .cloned()
            .ok_or_else(invalid)?;
        let file_id = metadata
            .get("file_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?
            .to_owned();
        let chunk_id = metadata
            .get("chunk_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?
            .to_owned();
        let content: String = text.chars().take(remaining).collect();
        remaining -= content.chars().count();
        truncated |= content.len() < text.len();
        metadata["sourceType"] = json!("remote");
        metadata["retrievalMode"] = json!("server_indexed");
        results.push(json!({"id": chunk_id, "anchor": chunk_id,
            "kb_id": kb_id, "file_id": file_id, "documentId": file_id,
            "content": content, "metadata": metadata, "score": item.get("score"),
            "distance": item.get("distance")}));
    }
    Ok(json!({"kb_id": kb_id, "query": query, "results": results,
        "retrievalMode": "server_indexed", "partial": false, "degraded": false,
        "truncated": truncated, "returnedChunks": items.len()}))
}

fn query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for word in query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_lowercase();
        if !terms.contains(&word) {
            terms.push(word.clone());
        }
        // Chinese questions often arrive without spaces. Preserve the phrase and
        // also match short terms without requiring an external tokenizer/model.
        let chars = word.chars().collect::<Vec<_>>();
        if chars.len() > 3
            && chars
                .iter()
                .all(|ch| ('\u{3400}'..='\u{9fff}').contains(ch))
        {
            for pair in chars.windows(2) {
                let term: String = pair.iter().collect();
                if !terms.contains(&term) {
                    terms.push(term);
                }
                if terms.len() >= 32 {
                    break;
                }
            }
        }
        if terms.len() >= 32 {
            break;
        }
    }
    terms.truncate(32);
    terms
}

impl YuxiClient {
    async fn indexed_knowledge_search(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        query: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Value, String> {
        let mut endpoint = api_url(base_url, "/api/knowledge/databases/")?;
        endpoint
            .path_segments_mut()
            .map_err(|_| "Invalid knowledge URL".to_owned())?
            .pop_if_empty()
            .push(kb_id)
            .push("query");
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(token)
            .json(&json!({"query": query, "meta": {}}))
            .send()
            .await
            .map_err(|_| {
                "[remote_knowledge.retrieval_unavailable] 无法连接知识库检索服务".to_owned()
            })?;
        // 403/404/405/501 表示当前账号没有原生索引，或服务没有该接口。
        // 必须直接失败，禁止再退回文档关键词扫描。
        if matches!(response.status().as_u16(), 403 | 404 | 405 | 501) {
            let status = response.status().as_u16();
            return Err(format!(
                "[remote_knowledge.index_unavailable] 远程知识库的原生索引检索不可用（HTTP {status}）。Fox 不会改用文档关键词扫描；请让知识库管理员为当前账号开放检索接口，或改用本地资料库。"
            ));
        }
        let bytes = bounded_body(response).await?;
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| {
            "[remote_knowledge.protocol_invalid] 知识库检索响应不是有效 JSON".to_owned()
        })?;
        normalize_indexed_results(payload, kb_id, query, limit, max_chars)
    }

    async fn knowledge_bytes(
        &self,
        base_url: &str,
        token: &str,
        path: &str,
        pairs: &[(&str, &str)],
    ) -> Result<Vec<u8>, String> {
        let mut endpoint = api_url(base_url, path)?;
        endpoint
            .query_pairs_mut()
            .extend_pairs(pairs.iter().copied());
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| {
                "[remote_knowledge.retrieval_unavailable] 无法连接知识库服务".to_owned()
            })?;
        if response.status().is_success() && path.ends_with("/download") {
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if !matches!(
                content_type,
                "text/markdown" | "text/plain" | "application/octet-stream"
            ) {
                return Err("[remote_knowledge.protocol_invalid] 知识库未返回解析文本".to_owned());
            }
        }
        bounded_body(response).await
    }

    async fn parsed_knowledge_text(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        file_id: &str,
    ) -> Result<String, String> {
        let bytes = self
            .knowledge_bytes(
                base_url,
                token,
                "/api/workspace/knowledge/download",
                &[
                    ("kb_id", kb_id),
                    ("file_id", file_id),
                    ("variant", "parsed"),
                ],
            )
            .await?;
        String::from_utf8(bytes).map_err(|_| {
            "[remote_knowledge.protocol_invalid] 知识文档解析结果不是 UTF-8 文本".to_owned()
        })
    }

    /// Model-facing text reads must use parsed downloads, not binary UI previews.
    pub async fn knowledge_document_text(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        file_id: &str,
    ) -> Result<Value, String> {
        let text = self
            .parsed_knowledge_text(base_url, token, kb_id, file_id)
            .await?;
        let content = text.chars().take(60_000).collect::<String>();
        Ok(
            json!({ "kb_id": kb_id, "file_id": file_id, "documentId": file_id,
            "truncated": content.len() < text.len(), "content": content, "source": "remote" }),
        )
    }

    pub async fn query_knowledge_with_limits(
        &self,
        base_url: &str,
        token: &str,
        kb_id: &str,
        query: &str,
        top_k: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<Value, String> {
        let query = query.trim();
        if kb_id.trim().is_empty() || query.is_empty() || query.chars().count() > 1000 {
            return Err(
                "[remote_knowledge.invalid_query] 知识库编号和不超过 1000 字符的查询不能为空"
                    .to_owned(),
            );
        }
        let terms = query_terms(query);
        if terms.is_empty() {
            return Err("[remote_knowledge.invalid_query] 查询需要包含文字或数字".to_owned());
        }
        let limit = top_k.unwrap_or(8).clamp(1, 20);
        let max_chars = max_chars.unwrap_or(16_000).clamp(1, 40_000);
        match tokio::time::timeout(
            SEARCH_DURATION,
            self.indexed_knowledge_search(base_url, token, kb_id, query, limit, max_chars),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err("[remote_knowledge.retrieval_unavailable] 知识库检索超时".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    #[test]
    #[ignore = "requires an explicitly authorized live test account in FOX_TEST_YUXI_* environment variables"]
    fn live_remote_knowledge_search_and_read() {
        let base = std::env::var("FOX_TEST_YUXI_BASE_URL").expect("live service URL is required");
        let username = std::env::var("FOX_TEST_YUXI_USERNAME").expect("live username is required");
        let password = std::env::var("FOX_TEST_YUXI_PASSWORD").expect("live password is required");
        let kb_id =
            std::env::var("FOX_TEST_YUXI_KB_ID").expect("authorized knowledge base ID is required");
        tauri::async_runtime::block_on(async {
            let client = YuxiClient::new().unwrap();
            let (token, user) = client
                .login(&base, &username, &password)
                .await
                .expect("live login failed");
            println!("live login succeeded; role={}", user.role);
            let detail = client
                .knowledge_detail(&base, &token, &kb_id)
                .await
                .expect("bound knowledge base must be readable");
            println!(
                "live knowledge base: {}; documents={}",
                detail.database.name,
                detail.documents.len()
            );
            let query = std::env::var("FOX_TEST_YUXI_QUERY")
                .unwrap_or_else(|_| "如何给轨道吊起升赋值".to_owned());
            for query in [query.as_str()] {
                let result = client
                    .query_knowledge_with_limits(
                        &base,
                        &token,
                        &kb_id,
                        query,
                        Some(10),
                        Some(16_000),
                    )
                    .await
                    .expect("live search failed");
                let hits = result["results"].as_array().expect("search results array");
                println!(
                    "query={query}; hits={}; mode={}; partial={}",
                    hits.len(),
                    result["retrievalMode"],
                    result["partial"]
                );
                assert!(
                    !hits.is_empty(),
                    "known crane query must retrieve actual evidence"
                );
                assert_eq!(result["retrievalMode"], "server_indexed");
                if query == "箱量记录" {
                    assert!(hits
                        .iter()
                        .any(|hit| hit["content"].as_str().unwrap_or("").contains("72.3")));
                    assert!(hits
                        .iter()
                        .any(|hit| hit["content"].as_str().unwrap_or("").contains("28536")));
                }
                let first = &hits[0];
                println!(
                    "source={}; file_id={}; excerpt={}",
                    first["metadata"]["source"],
                    first["file_id"],
                    first["content"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(700)
                        .collect::<String>()
                );
                let file_id = first["file_id"].as_str().expect("real document ID");
                assert_ne!(file_id, kb_id);
                let document = client
                    .knowledge_document_text(&base, &token, &kb_id, file_id)
                    .await
                    .expect("live parsed document read failed");
                let content = document["content"].as_str().expect("parsed document text");
                assert!(!content.is_empty());
                println!(
                    "document read succeeded; chars={}; truncated={}",
                    content.chars().count(),
                    document["truncated"]
                );
            }
        });
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let size = stream.read(&mut buffer).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&buffer[..size]);
        }
        String::from_utf8(request).unwrap()
    }

    fn server(
        responses: Vec<(String, u16, &'static str, Vec<u8>)>,
    ) -> (String, thread::JoinHandle<()>) {
        let (address, _requests, handle) = serve(responses, false);
        (address, handle)
    }

    /// 登记脚本化响应，并在结束后短窗口内记录额外连接，用来断言没有 tree/download。
    fn recording_server(
        responses: Vec<(String, u16, &'static str, Vec<u8>)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        serve(responses, true)
    }

    fn serve(
        responses: Vec<(String, u16, &'static str, Vec<u8>)>,
        watch_for_extra_requests: bool,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for (expected, status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                recorded
                    .lock()
                    .unwrap()
                    .push(request.lines().next().unwrap_or("").to_owned());
                let expected = if expected.starts_with("POST ") {
                    expected
                } else {
                    format!("GET {expected}")
                };
                assert!(
                    request.starts_with(&format!("{expected} HTTP/1.1")),
                    "unexpected endpoint: {request}"
                );
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer fixture-token\r\n"));
                let headers = format!("HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(headers.as_bytes()).unwrap();
                let _ = stream.write_all(&body); // Bounded readers can close oversized responses early.
            }
            if !watch_for_extra_requests {
                return;
            }
            // 客户端应在唯一一次 query 返回后结束。再等一小段，抓住迟到的 tree/download。
            let _ = listener.set_nonblocking(true);
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_millis(100) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_http_request(&mut stream);
                        recorded
                            .lock()
                            .unwrap()
                            .push(request.lines().next().unwrap_or("").to_owned());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        (address, requests, handle)
    }

    #[test]
    fn indexed_search_preserves_semantic_chunks_and_enforces_output_limits() {
        let items = json!([
            {"content":"最高月吞吐量72.3万标准箱", "metadata":{"source":"企业文化.xlsx", "file_id":"file-1", "chunk_id":"file-1_chunk_79"}, "score":0.56},
            {"content":"最高昼夜28536TEU", "metadata":{"source":"企业文化.xlsx", "file_id":"file-1", "chunk_id":"file-1_chunk_78"}, "score":0.53}
        ]);
        for payload in [items.clone(), json!({"status":"success", "result":items})] {
            let (base, handle) = server(vec![(
                "POST /api/knowledge/databases/kb-1/query".to_owned(),
                200,
                "application/json",
                payload.canonical_string().into_bytes(),
            )]);
            let result = tauri::async_runtime::block_on(
                YuxiClient::new().unwrap().query_knowledge_with_limits(
                    &base,
                    "fixture-token",
                    "kb-1",
                    "箱量记录",
                    Some(1),
                    Some(8),
                ),
            )
            .unwrap();
            handle.join().unwrap();
            assert_eq!(result["retrievalMode"], "server_indexed");
            assert_eq!(result["partial"], false);
            assert_eq!(result["truncated"], true);
            assert_eq!(result["results"].as_array().unwrap().len(), 1);
            assert_eq!(result["results"][0]["id"], "file-1_chunk_79");
            assert_eq!(result["results"][0]["file_id"], "file-1");
            assert_eq!(
                result["results"][0]["content"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count(),
                8
            );
        }
        let empty = normalize_indexed_results(json!([]), "kb-1", "箱量", 5, 100).unwrap();
        assert!(empty["results"].as_array().unwrap().is_empty());
        assert!(normalize_indexed_results(
            json!({"status":"error", "result":[]}),
            "kb-1",
            "箱量",
            5,
            100
        )
        .is_err());
        assert!(
            normalize_indexed_results(json!([{"content":"来源丢失"}]), "kb-1", "箱量", 5, 100)
                .is_err()
        );
    }

    #[test]
    fn indexed_failures_do_not_silently_become_lexical_no_hits() {
        for (status, code) in [
            (401, "authentication_required"),
            (429, "retrieval_unavailable"),
            (500, "retrieval_unavailable"),
        ] {
            let (base, handle) = server(vec![(
                "POST /api/knowledge/databases/kb-1/query".to_owned(),
                status,
                "application/json",
                b"{}".to_vec(),
            )]);
            let error = tauri::async_runtime::block_on(YuxiClient::new().unwrap().query_knowledge(
                &base,
                "fixture-token",
                "kb-1",
                "箱量",
            ))
            .unwrap_err();
            handle.join().unwrap();
            assert!(error.starts_with(&format!("[remote_knowledge.{code}]")));
        }
    }

    fn file_path(id: &str) -> String {
        format!("/api/workspace/knowledge/download?kb_id=kb-1&file_id={id}&variant=parsed")
    }

    #[test]
    fn remote_search_without_native_index_fails_with_index_unavailable() {
        for status in [403_u16, 404, 405, 501] {
            let (base, requests, handle) = recording_server(vec![(
                "POST /api/knowledge/databases/kb-1/query".to_owned(),
                status,
                "application/json",
                b"{}".to_vec(),
            )]);
            let error = tauri::async_runtime::block_on(
                YuxiClient::new().unwrap().query_knowledge_with_limits(
                    &base,
                    "fixture-token",
                    "kb-1",
                    "箱量",
                    Some(5),
                    Some(100),
                ),
            )
            .unwrap_err();
            handle.join().unwrap();
            assert!(
                error.starts_with("[remote_knowledge.index_unavailable]"),
                "{error}"
            );
            assert!(error.contains(&format!("HTTP {status}")), "{error}");
            let recorded = requests.lock().unwrap();
            assert_eq!(
                recorded.len(),
                1,
                "只能请求原生索引，不能再访问目录或下载：{recorded:?}"
            );
            assert!(
                recorded[0].starts_with("POST /api/knowledge/databases/kb-1/query "),
                "{}",
                recorded[0]
            );
            assert!(
                recorded
                    .iter()
                    .all(|line| !line.contains("/api/workspace/knowledge/tree")
                        && !line.contains("/download")),
                "{recorded:?}"
            );
        }
    }

    #[test]
    fn remote_access_denial_and_missing_parsed_documents_are_errors_not_missing_bindings() {
        // 403 与“解析文档缺失”原先依赖目录扫描降级。那条路径已删除。
        // 401 与 503 打在原生 query 上仍然是明确错误，而不是会话未绑定。
        for (status, code) in [
            (401_u16, "authentication_required"),
            (503, "retrieval_unavailable"),
        ] {
            let (base, handle) = server(vec![(
                "POST /api/knowledge/databases/kb-1/query".to_owned(),
                status,
                "application/json",
                b"{}".to_vec(),
            )]);
            let client = YuxiClient::new().unwrap();
            let error = tauri::async_runtime::block_on(client.query_knowledge(
                &base,
                "fixture-token",
                "kb-1",
                "起升",
            ))
            .unwrap_err();
            handle.join().unwrap();
            assert!(error.starts_with(&format!("[remote_knowledge.{code}]")));
        }
    }

    #[test]
    fn remote_document_read_uses_parsed_text_and_rejects_binary_previews_and_oversized_bodies() {
        for (kind, body, expected) in [
            ("text/markdown", "# 起升赋值".as_bytes().to_vec(), None),
            (
                "application/pdf",
                b"%PDF-1.7".to_vec(),
                Some("protocol_invalid"),
            ),
            (
                "text/markdown",
                vec![b'x'; MAX_RESPONSE_BYTES + 1],
                Some("response_too_large"),
            ),
        ] {
            let (base, handle) = server(vec![(file_path("file-1"), 200, kind, body)]);
            let result =
                tauri::async_runtime::block_on(YuxiClient::new().unwrap().knowledge_document_text(
                    &base,
                    "fixture-token",
                    "kb-1",
                    "file-1",
                ));
            handle.join().unwrap();
            if let Some(code) = expected {
                assert!(result
                    .unwrap_err()
                    .starts_with(&format!("[remote_knowledge.{code}]")));
            } else {
                assert_eq!(result.unwrap()["content"], "# 起升赋值");
            }
        }
    }
}

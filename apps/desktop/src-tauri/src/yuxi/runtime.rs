use super::{get_access_token, YuxiClient};
use crate::{
    database::{
        AttachmentRecord, ConversationRuntimeRecord, Database, KnowledgeBindingRecord,
        StartRunResult,
    },
    runtime_host::RuntimeEventNotification,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter};

const MAX_SSE_FRAME_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct YuxiRuntimeHost {
    app: AppHandle,
    database: Database,
    client: YuxiClient,
    active_runs: Arc<Mutex<HashMap<String, String>>>,
    event_lock: Arc<Mutex<()>>,
}

impl YuxiRuntimeHost {
    pub fn new(app: AppHandle, database: Database, client: YuxiClient) -> Self {
        Self {
            app,
            database,
            client,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            event_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn start_run_detached(
        &self,
        started: StartRunResult,
        text: String,
        attachments: Vec<AttachmentRecord>,
    ) {
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = host.start_run(&started, &text, &attachments).await {
                let recoverable = host
                    .database
                    .external_run(&started.run.id)
                    .ok()
                    .flatten()
                    .and_then(|run| run.external_run_id)
                    .is_some();
                host.emit_event(
                    &started.run.id,
                    &started.run.conversation_id,
                    None,
                    if recoverable {
                        json!({ "type": "run.interrupted", "code": "yuxi.connection_interrupted", "message": error })
                    } else {
                        json!({ "type": "run.failed", "code": "yuxi.run_failed", "message": error })
                    },
                );
            }
        });
    }

    pub fn resume_interrupted_run_detached(
        &self,
        started: StartRunResult,
        runtime: ConversationRuntimeRecord,
        parent_remote_run_id: String,
        answers: Value,
    ) {
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = host
                .start_resumed_run(&started, runtime, &parent_remote_run_id, &answers)
                .await
            {
                host.emit_event(
                    &started.run.id,
                    &started.run.conversation_id,
                    None,
                    json!({ "type": "run.failed", "code": "yuxi.resume_failed", "message": error }),
                );
            }
        });
    }

    pub fn recover_pending_runs_detached(&self) {
        self.recover_runs_detached(None);
    }

    pub fn recover_conversation_detached(&self, conversation_id: &str) {
        self.recover_runs_detached(Some(conversation_id));
    }

    fn recover_runs_detached(&self, conversation_id: Option<&str>) {
        let Ok(runs) = self.database.recoverable_yuxi_runs() else {
            return;
        };
        for run in runs
            .into_iter()
            .filter(|run| conversation_id.is_none_or(|id| run.conversation_id == id))
        {
            let already_active = self
                .active_runs
                .lock()
                .ok()
                .is_some_and(|runs| runs.contains_key(&run.local_run_id));
            if already_active {
                continue;
            }
            if let Ok(mut active) = self.active_runs.lock() {
                active.insert(run.local_run_id.clone(), run.external_run_id.clone());
            }
            let host = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = host
                    .resume_run(
                        &run.local_run_id,
                        &run.conversation_id,
                        &run.external_run_id,
                        &run.remote_thread_id,
                        run.external_cursor.as_deref(),
                    )
                    .await;
                if let Err(error) = result {
                    if let Ok(mut active) = host.active_runs.lock() {
                        active.remove(&run.local_run_id);
                    }
                    host.emit_event(
                        &run.local_run_id,
                        &run.conversation_id,
                        Some(&run.remote_thread_id),
                        json!({ "type": "run.interrupted", "code": "yuxi.recovery_failed", "message": error }),
                    );
                }
            });
        }
    }

    async fn start_run(
        &self,
        started: &StartRunResult,
        text: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<(), String> {
        let service = self
            .database
            .get_yuxi_service()?
            .ok_or_else(|| "请先配置知识库服务".to_owned())?;
        let token =
            get_access_token(&service.base_url).ok_or_else(|| "请先登录知识库".to_owned())?;
        let runtime = self
            .database
            .conversation_runtime(&started.run.conversation_id)?;
        let agent_id = runtime
            .remote_agent_id
            .ok_or_else(|| "远程智能体映射缺失".to_owned())?;
        let thread_id = runtime
            .remote_thread_id
            .ok_or_else(|| "远程会话映射缺失".to_owned())?;
        let knowledge_bindings = self
            .database
            .conversation_knowledge_bindings(&started.run.conversation_id)?;
        let knowledge_scope = self
            .database
            .conversation_agent_knowledge_scope(&started.run.conversation_id)?;
        validate_knowledge_scope(&knowledge_bindings, knowledge_scope.as_deref())?;
        let remote_query = build_yuxi_query(text, &knowledge_bindings);
        let mut remote_attachment_ids = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            let bytes = std::fs::read(&attachment.storage_path)
                .map_err(|error| format!("读取附件 {} 失败: {error}", attachment.display_name))?;
            let uploaded = self
                .client
                .upload_thread_attachment(
                    &service.base_url,
                    &token,
                    &thread_id,
                    &attachment.display_name,
                    attachment.media_type.as_deref(),
                    bytes,
                )
                .await?;
            remote_attachment_ids.push(uploaded.file_id);
        }
        let remote = self
            .client
            .create_run(
                &service.base_url,
                &token,
                &agent_id,
                &thread_id,
                &remote_query,
                (!started.run.model.is_empty()).then_some(started.run.model.as_str()),
                &remote_attachment_ids,
                &knowledge_bindings,
            )
            .await?;
        self.database
            .set_external_run(&started.run.id, &remote.run_id)?;
        self.active_runs
            .lock()
            .map_err(|_| "知识库 Runtime 状态锁异常".to_owned())?
            .insert(started.run.id.clone(), remote.run_id.clone());
        self.emit_event(
            &started.run.id,
            &started.run.conversation_id,
            Some(&thread_id),
            json!({
                "type": "run.started",
                "model": started.run.model,
                "runtime": "yuxi",
                "knowledgeBaseCount": knowledge_bindings.len(),
            }),
        );

        self.stream_run(
            &service.base_url,
            &token,
            &started.run.id,
            &started.run.conversation_id,
            &remote.run_id,
            &thread_id,
            self.database
                .external_run(&started.run.id)?
                .and_then(|run| run.external_cursor)
                .as_deref(),
        )
        .await
    }

    async fn start_resumed_run(
        &self,
        started: &StartRunResult,
        runtime: ConversationRuntimeRecord,
        parent_remote_run_id: &str,
        answers: &Value,
    ) -> Result<(), String> {
        let service = self
            .database
            .get_yuxi_service()?
            .ok_or_else(|| "请先配置知识库服务".to_owned())?;
        let token =
            get_access_token(&service.base_url).ok_or_else(|| "请先登录知识库".to_owned())?;
        let agent_id = runtime
            .remote_agent_id
            .ok_or_else(|| "远程智能体映射缺失".to_owned())?;
        let thread_id = runtime
            .remote_thread_id
            .ok_or_else(|| "远程会话映射缺失".to_owned())?;
        let knowledge_bindings = self
            .database
            .conversation_knowledge_bindings(&started.run.conversation_id)?;
        let remote = self
            .client
            .create_resume_run(
                &service.base_url,
                &token,
                &agent_id,
                &thread_id,
                parent_remote_run_id,
                answers,
                &knowledge_bindings,
            )
            .await?;
        self.database
            .set_external_run(&started.run.id, &remote.run_id)?;
        self.active_runs
            .lock()
            .map_err(|_| "知识库 Runtime 状态锁异常".to_owned())?
            .insert(started.run.id.clone(), remote.run_id.clone());
        self.emit_event(
            &started.run.id,
            &started.run.conversation_id,
            Some(&thread_id),
            json!({
                "type": "run.started",
                "runtime": "yuxi",
                "resumed": true,
                "knowledgeBaseCount": knowledge_bindings.len(),
            }),
        );
        self.stream_run(
            &service.base_url,
            &token,
            &started.run.id,
            &started.run.conversation_id,
            &remote.run_id,
            &thread_id,
            None,
        )
        .await
    }

    async fn resume_run(
        &self,
        local_run_id: &str,
        conversation_id: &str,
        remote_run_id: &str,
        thread_id: &str,
        cursor: Option<&str>,
    ) -> Result<(), String> {
        let service = self
            .database
            .get_yuxi_service()?
            .ok_or_else(|| "请先配置知识库服务".to_owned())?;
        let token =
            get_access_token(&service.base_url).ok_or_else(|| "请先登录知识库".to_owned())?;
        let status = self
            .client
            .get_run(&service.base_url, &token, remote_run_id)
            .await?;
        let status = status
            .get("run")
            .and_then(|run| run.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("running");
        // Re-open the local run before replaying remote events. A completed
        // remote run can still have message/tool events that Fox missed while
        // the SSE connection or Yuxi worker was unavailable.
        self.emit_event(
            local_run_id,
            conversation_id,
            Some(thread_id),
            json!({ "type": "run.started", "runtime": "yuxi", "resumed": true, "remoteStatus": status }),
        );
        self.active_runs
            .lock()
            .map_err(|_| "知识库 Runtime 状态锁异常".to_owned())?
            .insert(local_run_id.to_owned(), remote_run_id.to_owned());
        self.stream_run(
            &service.base_url,
            &token,
            local_run_id,
            conversation_id,
            remote_run_id,
            thread_id,
            cursor,
        )
        .await
    }

    async fn stream_run(
        &self,
        base_url: &str,
        token: &str,
        local_run_id: &str,
        conversation_id: &str,
        remote_run_id: &str,
        thread_id: &str,
        initial_cursor: Option<&str>,
    ) -> Result<(), String> {
        let mut buffer = Vec::new();
        let mut state = StreamState::default();
        let mut cursor = initial_cursor.map(str::to_owned);
        let mut last_error = None;
        let mut terminal_status = None;
        for attempt in 0..5 {
            buffer.clear();
            let draining_terminal_run = terminal_status.is_some();
            let response = self
                .client
                .run_event_response(base_url, token, remote_run_id, cursor.as_deref())
                .await;
            let mut response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 4 {
                        retry_delay(attempt).await;
                        continue;
                    }
                    break;
                }
            };
            let mut response_reached_eof = false;
            loop {
                let chunk = match response.chunk().await {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) => {
                        response_reached_eof = true;
                        break;
                    }
                    Err(error) => {
                        last_error = Some(format!("知识库事件流中断：{error}"));
                        break;
                    }
                };
                buffer.extend_from_slice(&chunk);
                for event in parse_sse_buffer(&mut buffer, false)? {
                    if let Some(next_cursor) = event.id.as_deref() {
                        cursor = Some(next_cursor.to_owned());
                        let _ = self.database.set_external_cursor(local_run_id, next_cursor);
                    }
                    for payload in map_yuxi_event(&event.event, &event.data, &mut state) {
                        self.emit_event(local_run_id, conversation_id, Some(thread_id), payload);
                    }
                    if state.terminal {
                        break;
                    }
                }
                if state.terminal {
                    break;
                }
            }
            if state.terminal {
                break;
            }
            if response_reached_eof {
                for event in parse_sse_buffer(&mut buffer, true)? {
                    if let Some(next_cursor) = event.id.as_deref() {
                        cursor = Some(next_cursor.to_owned());
                        let _ = self.database.set_external_cursor(local_run_id, next_cursor);
                    }
                    for payload in map_yuxi_event(&event.event, &event.data, &mut state) {
                        self.emit_event(local_run_id, conversation_id, Some(thread_id), payload);
                    }
                    if state.terminal {
                        break;
                    }
                }
            }
            if state.terminal {
                break;
            }
            // A completed status response is only a snapshot. Reconnect once
            // with the latest cursor before synthesizing terminal events so
            // Redis entries committed just before completion are not skipped.
            if draining_terminal_run && response_reached_eof {
                if let Some(run) = terminal_status.take() {
                    for event in map_yuxi_event("status", &run, &mut state) {
                        self.emit_event(local_run_id, conversation_id, Some(thread_id), event);
                    }
                }
                if state.terminal {
                    break;
                }
            }
            match self.client.get_run(base_url, token, remote_run_id).await {
                Ok(run) => {
                    if event_status(&run).is_some_and(is_terminal_status) {
                        terminal_status = Some(run);
                    } else {
                        for event in map_yuxi_event("status", &run, &mut state) {
                            self.emit_event(local_run_id, conversation_id, Some(thread_id), event);
                        }
                    }
                }
                Err(error) => last_error = Some(error),
            }
            if last_error.is_none() {
                last_error = Some("知识库事件流在运行完成前断开".to_owned());
            }
            if attempt < 4 {
                retry_delay(attempt).await;
            }
        }
        if !state.terminal {
            if let Some(run) = terminal_status.take() {
                for event in map_yuxi_event("status", &run, &mut state) {
                    self.emit_event(local_run_id, conversation_id, Some(thread_id), event);
                }
            }
        }
        self.active_runs
            .lock()
            .ok()
            .map(|mut runs| runs.remove(local_run_id));
        if !state.terminal {
            return Err(last_error.unwrap_or_else(|| "知识库事件流在运行完成前断开".to_owned()));
        }
        Ok(())
    }

    pub async fn cancel_run(&self, local_run_id: &str) -> Result<bool, String> {
        let remote_run_id = self
            .database
            .external_run(local_run_id)?
            .and_then(|run| run.external_run_id)
            .or_else(|| {
                self.active_runs
                    .lock()
                    .ok()
                    .and_then(|runs| runs.get(local_run_id).cloned())
            });
        let Some(remote_run_id) = remote_run_id else {
            return Ok(false);
        };
        let service = self
            .database
            .get_yuxi_service()?
            .ok_or_else(|| "请先配置知识库服务".to_owned())?;
        let token =
            get_access_token(&service.base_url).ok_or_else(|| "请先登录知识库".to_owned())?;
        self.client
            .cancel_run(&service.base_url, &token, &remote_run_id)
            .await?;
        Ok(true)
    }

    fn emit_event(
        &self,
        run_id: &str,
        conversation_id: &str,
        session_id: Option<&str>,
        event: Value,
    ) {
        let Ok(_event_guard) = self.event_lock.lock() else {
            return;
        };
        let Ok(Some(seq)) = self.database.record_external_event(run_id, &event) else {
            return;
        };
        let _ = self.app.emit(
            "fox://runtime-event",
            RuntimeEventNotification {
                conversation_id: conversation_id.to_owned(),
                runtime_session_id: session_id.map(str::to_owned),
                run_id: run_id.to_owned(),
                seq,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                event,
            },
        );
    }
}

async fn retry_delay(attempt: usize) {
    tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1) as u64)).await;
}

#[derive(Default)]
struct StreamState {
    message_started: bool,
    terminal: bool,
    tools: HashMap<String, String>,
    completed_tools: HashSet<String>,
    content_buffer: String,
    in_reasoning: bool,
    question_emitted: bool,
}

#[derive(Debug)]
struct SseEvent {
    id: Option<String>,
    event: String,
    data: Value,
}

fn frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    match (crlf, lf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
}

fn parse_sse_buffer(buffer: &mut Vec<u8>, flush_remainder: bool) -> Result<Vec<SseEvent>, String> {
    let mut events = Vec::new();
    while let Some((index, delimiter_len)) = frame_end(buffer) {
        if index > MAX_SSE_FRAME_BYTES {
            buffer.clear();
            return Err(format!(
                "Knowledge event frame exceeds the {} MiB safety limit",
                MAX_SSE_FRAME_BYTES / 1024 / 1024
            ));
        }
        let frame = buffer[..index].to_vec();
        buffer.drain(..index + delimiter_len);
        let frame = String::from_utf8(frame)
            .map_err(|error| format!("知识库事件流包含无效 UTF-8：{error}"))?;
        if let Some(event) = parse_sse_frame(&frame) {
            events.push(event);
        }
    }
    if buffer.len() > MAX_SSE_FRAME_BYTES {
        buffer.clear();
        return Err(format!(
            "Knowledge event frame exceeds the {} MiB safety limit",
            MAX_SSE_FRAME_BYTES / 1024 / 1024
        ));
    }
    if flush_remainder {
        let frame = std::mem::take(buffer);
        if frame.iter().any(|byte| !byte.is_ascii_whitespace()) {
            let frame = String::from_utf8(frame)
                .map_err(|error| format!("知识库事件流尾部包含无效 UTF-8：{error}"))?;
            if let Some(event) = parse_sse_frame(&frame) {
                events.push(event);
            }
        }
    }
    Ok(events)
}

fn parse_sse_frame(frame: &str) -> Option<SseEvent> {
    if frame.trim().is_empty() || frame.trim_start().starts_with(':') {
        return None;
    }
    let mut id = None;
    let mut event = "message".to_owned();
    let mut data = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("event:") {
            event = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        }
    }
    let data = serde_json::from_str(&data.join("\n")).ok()?;
    Some(SseEvent { id, event, data })
}

fn map_yuxi_event(event_type: &str, envelope: &Value, state: &mut StreamState) -> Vec<Value> {
    let mut events = Vec::new();
    let payload = envelope.get("payload").unwrap_or(envelope);
    let mut chunks = Vec::new();
    if let Some(items) = payload.get("items").and_then(Value::as_array) {
        chunks.extend(items.iter());
    }
    if let Some(chunk) = payload.get("chunk") {
        chunks.push(chunk);
    }
    for chunk in &chunks {
        map_chunk(chunk, state, &mut events);
    }
    if !state.question_emitted {
        if let Some(question) = find_user_question(envelope) {
            let questions = question
                .get("questions")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            if questions.as_array().is_some_and(|items| !items.is_empty()) {
                state.question_emitted = true;
                if !state.tools.values().any(|tool| tool == "ask_user_question") {
                    state.tools.insert(
                        "yuxi-ask-user-question".to_owned(),
                        "ask_user_question".to_owned(),
                    );
                }
                events.push(json!({
                    "type": "user.question.requested",
                    "source": question.get("source").and_then(Value::as_str).unwrap_or("ask_user_question"),
                    "questions": questions,
                }));
            }
        }
    }
    let status = event_status(envelope).or_else(|| event_status(payload));
    if event_type == "end" || (event_type == "status" && status.is_some_and(is_terminal_status)) {
        events.extend(terminal_events(
            status.unwrap_or("failed"),
            state,
            event_error_message(envelope),
        ));
    } else if event_type == "error" && chunks.is_empty() {
        events.push(json!({ "type": "run.failed", "code": "yuxi.stream_error", "message": payload.get("message").and_then(Value::as_str).unwrap_or("知识库事件流失败") }));
        state.terminal = true;
    }
    events
}

fn find_user_question(value: &Value) -> Option<&Value> {
    if value.get("status").and_then(Value::as_str) == Some("ask_user_question_required")
        && value.get("questions").and_then(Value::as_array).is_some()
    {
        return Some(value);
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        for item in items {
            if let Some(found) = find_user_question(item) {
                return Some(found);
            }
        }
    }
    for key in ["payload", "chunk", "data"] {
        if let Some(child) = value.get(key) {
            if let Some(found) = find_user_question(child) {
                return Some(found);
            }
        }
    }
    None
}

fn event_status(value: &Value) -> Option<&str> {
    value
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("run")
                .and_then(|run| run.get("status"))
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("data").and_then(event_status))
        .or_else(|| value.get("payload").and_then(event_status))
        .or_else(|| value.get("chunk").and_then(event_status))
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed"
            | "finished"
            | "success"
            | "succeeded"
            | "failed"
            | "error"
            | "cancelled"
            | "canceled"
            | "interrupted"
    )
}

fn event_error_message(value: &Value) -> Option<&str> {
    value
        .get("error_message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .or_else(|| value.get("run").and_then(event_error_message))
        .or_else(|| value.get("payload").and_then(event_error_message))
        .or_else(|| value.get("chunk").and_then(event_error_message))
}

fn terminal_events(status: &str, state: &mut StreamState, message: Option<&str>) -> Vec<Value> {
    if state.terminal {
        return Vec::new();
    }
    let mut events = Vec::new();
    flush_stream_content(state, &mut events);
    let normalized = status.to_ascii_lowercase();
    let awaiting_user =
        normalized == "interrupted" && state.tools.values().any(|tool| tool == "ask_user_question");
    if awaiting_user {
        for (tool_call_id, tool) in &state.tools {
            if tool == "ask_user_question" && !state.completed_tools.contains(tool_call_id) {
                events.push(json!({
                    "type": "tool.completed",
                    "toolCallId": tool_call_id,
                    "tool": tool,
                    "result": { "status": "awaiting_user" },
                    "isError": false,
                }));
            }
        }
    }
    if state.message_started {
        events.push(json!({ "type": "message.completed" }));
    }
    events.push(if awaiting_user {
        json!({ "type": "run.completed", "completionReason": "awaiting_user" })
    } else {
        match normalized.as_str() {
        "completed" | "finished" | "success" | "succeeded" => json!({ "type": "run.completed" }),
        "cancelled" | "canceled" => json!({ "type": "run.cancelled" }),
        "interrupted" => json!({ "type": "run.interrupted", "code": "yuxi.interrupted", "message": message.unwrap_or("远程任务等待用户处理或已中断") }),
        _ => json!({ "type": "run.failed", "code": "yuxi.run_failed", "message": message.unwrap_or("远程任务执行失败") }),
        }
    });
    state.terminal = true;
    events
}

fn map_chunk(chunk: &Value, state: &mut StreamState, events: &mut Vec<Value>) {
    let stream_event = chunk.get("stream_event").or_else(|| {
        chunk
            .get("event")
            .filter(|event| event.get("method").is_none() && event.get("type").is_some())
    });
    if let Some(stream_event) = stream_event {
        match stream_event.get("type").and_then(Value::as_str) {
            Some("message_delta") => {
                for key in ["reasoning_content", "additional_reasoning_content"] {
                    if let Some(delta) = stream_event
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        events.push(
                            json!({ "type": "reasoning.delta", "delta": delta, "source": "yuxi" }),
                        );
                    }
                }
                if let Some(delta) = stream_event
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    map_stream_content(delta, state, events);
                }
            }
            Some("tool_call") | Some("tool_call_delta") => {
                let id = stream_event
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("yuxi-tool");
                let name = stream_event
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                if !state.tools.contains_key(id) {
                    state.tools.insert(id.to_owned(), name.to_owned());
                    events.push(json!({ "type": "tool.started", "toolCallId": id, "tool": name, "input": stream_event.get("args").cloned().unwrap_or(Value::Null) }));
                } else if let Some(delta) = stream_event.get("args_delta") {
                    events.push(json!({ "type": "tool.updated", "toolCallId": id, "tool": name, "update": delta }));
                }
            }
            _ => {}
        }
    }
    if let Some(tool_event) = chunk
        .get("event")
        .filter(|event| event.get("method").and_then(Value::as_str) == Some("tools"))
    {
        let data = tool_event.get("data").unwrap_or(tool_event);
        let id = data
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("yuxi-tool");
        let name = data
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| state.tools.get(id).cloned())
            .unwrap_or_else(|| "tool".to_owned());
        match data.get("event").and_then(Value::as_str) {
            Some("tool-started") if !state.tools.contains_key(id) => {
                state.tools.insert(id.to_owned(), name.clone());
                events.push(
                    json!({ "type": "tool.started", "toolCallId": id, "tool": name, "input": {} }),
                );
            }
            Some("tool-finished") => {
                let output = data.get("output").cloned().unwrap_or(Value::Null);
                state.completed_tools.insert(id.to_owned());
                if matches!(name.as_str(), "query_kb" | "search_knowledge") {
                    events.extend(knowledge_sources(&output));
                }
                events.push(json!({
                    "type": "tool.completed", "toolCallId": id, "tool": name,
                    "result": output,
                    "isError": data.get("error").is_some(),
                }));
            }
            _ => {}
        }
    }
}

fn emit_stream_text(text: &str, reasoning: bool, state: &mut StreamState, events: &mut Vec<Value>) {
    if text.is_empty() {
        return;
    }
    if reasoning {
        events.push(json!({ "type": "reasoning.delta", "delta": text, "source": "yuxi" }));
        return;
    }
    if !state.message_started {
        events.push(json!({ "type": "message.started" }));
        state.message_started = true;
    }
    for delta in text_chunks(text, 24) {
        events.push(json!({ "type": "message.delta", "delta": delta }));
    }
}

fn text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() || max_chars == 0 {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if current.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn partial_marker_suffix_len(text: &str, marker: &str) -> usize {
    (1..marker.len())
        .rev()
        .find(|length| text.ends_with(&marker[..*length]))
        .unwrap_or(0)
}

fn map_stream_content(delta: &str, state: &mut StreamState, events: &mut Vec<Value>) {
    state.content_buffer.push_str(delta);
    loop {
        let marker = if state.in_reasoning {
            "</think>"
        } else {
            "<think>"
        };
        if let Some(index) = state.content_buffer.find(marker) {
            let text = state.content_buffer[..index].to_owned();
            state.content_buffer.drain(..index + marker.len());
            emit_stream_text(&text, state.in_reasoning, state, events);
            state.in_reasoning = !state.in_reasoning;
            continue;
        }

        let retained = partial_marker_suffix_len(&state.content_buffer, marker);
        let emitted = state.content_buffer.len().saturating_sub(retained);
        if emitted > 0 {
            let text = state.content_buffer[..emitted].to_owned();
            state.content_buffer.drain(..emitted);
            emit_stream_text(&text, state.in_reasoning, state, events);
        }
        break;
    }
}

fn flush_stream_content(state: &mut StreamState, events: &mut Vec<Value>) {
    if state.content_buffer.is_empty() {
        return;
    }
    let text = std::mem::take(&mut state.content_buffer);
    emit_stream_text(&text, state.in_reasoning, state, events);
    state.in_reasoning = false;
}

fn build_yuxi_query(user_text: &str, bindings: &[KnowledgeBindingRecord]) -> String {
    let mut seen = HashSet::new();
    let enabled = bindings
        .iter()
        .filter(|binding| binding.enabled)
        .filter_map(|binding| {
            let id = binding.knowledge_base_id.trim();
            if id.is_empty() || !seen.insert(id.to_owned()) {
                return None;
            }
            let name = binding
                .knowledge_base_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some((id.to_owned(), name.to_owned()))
        })
        .collect::<Vec<_>>();

    if enabled.is_empty() {
        return user_text.to_owned();
    }

    let resources = enabled
        .iter()
        .map(|(id, name)| format!("- {name} (kb_id: {id})"))
        .collect::<Vec<_>>()
        .join("\n");
    let mentions = enabled
        .iter()
        .map(|(_, name)| format_knowledge_mention(name))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "[Fox 会话知识库约束]\n\
当前会话已启用以下知识库：\n{resources}\n\
Yuxi 原生知识库引用：{mentions}\n\n\
处理本次请求时：\n\
1. 对事实、业务资料或文档内容的问题，必须先实际调用 query_kb，并使用上方准确的 kb_id；需要上下文时继续调用 open_kb_document 或 find_kb_document。\n\
2. 只能查询上方列出的知识库，不要改查其他知识库，也不要在未调用工具时声称已经查询。\n\
3. 如果信息已经足以形成检索词，不要先调用 ask_user_question；先检索，再仅对仍然缺失的关键条件提问。\n\
4. 本段是 Fox 注入的运行约束，不是用户正文。\n\n\
[用户问题]\n{user_text}"
    )
}

fn validate_knowledge_scope(
    bindings: &[KnowledgeBindingRecord],
    allowed_ids: Option<&[String]>,
) -> Result<(), String> {
    let Some(allowed_ids) = allowed_ids else {
        return Ok(());
    };
    let allowed = allowed_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let unavailable = bindings
        .iter()
        .filter(|binding| binding.enabled && !binding.knowledge_base_id.trim().is_empty())
        .filter(|binding| !allowed.contains(binding.knowledge_base_id.trim()))
        .map(|binding| {
            binding
                .knowledge_base_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(binding.knowledge_base_id.as_str())
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "当前智能体未启用所选知识库：{}。请在智能体管理中启用后重试，或选择该智能体可用的知识库。",
            unavailable.join("、")
        ))
    }
}

fn format_knowledge_mention(name: &str) -> String {
    if name
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | '\\'))
    {
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        format!("@knowledge:\"{escaped}\"")
    } else {
        format!("@knowledge:{name}")
    }
}

fn knowledge_sources(output: &Value) -> Vec<Value> {
    let parsed_content = output
        .get("content")
        .and_then(Value::as_str)
        .and_then(|content| serde_json::from_str::<Value>(content).ok());
    let root = parsed_content.as_ref().unwrap_or(output);
    let Some((knowledge_base_id, results)) = find_knowledge_results(root, 0) else {
        return Vec::new();
    };

    results
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let metadata = item.get("metadata").filter(|value| value.is_object());
            let document_id = first_non_empty_string(&[
                item.get("file_id"),
                metadata.and_then(|value| value.get("file_id")),
            ])
            .unwrap_or_default();
            let id = first_non_empty_string(&[
                item.get("id"),
                item.get("chunk_id"),
                metadata.and_then(|value| value.get("chunk_id")),
            ])
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{knowledge_base_id}-{}", index + 1));
            let title = first_non_empty_string(&[
                metadata.and_then(|value| value.get("source")),
                metadata.and_then(|value| value.get("title")),
                metadata.and_then(|value| value.get("filename")),
                item.get("title"),
                item.get("name"),
                item.get("file_name"),
            ])
            .unwrap_or(if document_id.is_empty() {
                "知识库来源"
            } else {
                document_id
            });
            let excerpt = item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(320)
                .collect::<String>();
            let page = first_positive_u64(&[
                item.get("page"),
                item.get("page_number"),
                metadata.and_then(|value| value.get("page")),
                metadata.and_then(|value| value.get("page_number")),
                metadata.and_then(|value| value.get("page_num")),
            ]);
            let anchor = first_non_empty_string(&[
                item.get("anchor"),
                metadata.and_then(|value| value.get("anchor")),
                metadata.and_then(|value| value.get("section")),
                metadata.and_then(|value| value.get("heading")),
            ]);
            let chunk_id = id.clone();

            json!({
                "type": "source.added",
                "id": id,
                "title": title,
                "knowledgeBaseId": item.get("kb_id").and_then(Value::as_str).unwrap_or(knowledge_base_id),
                "documentId": document_id,
                "excerpt": excerpt,
                "chunkId": chunk_id,
                "page": page,
                "anchor": anchor,
            })
        })
        .collect()
}

fn first_non_empty_string<'a>(values: &[Option<&'a Value>]) -> Option<&'a str> {
    values
        .iter()
        .flatten()
        .find_map(|value| value.as_str().filter(|value| !value.is_empty()))
}

fn first_positive_u64(values: &[Option<&Value>]) -> Option<u64> {
    values.iter().flatten().find_map(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
            .filter(|value| *value > 0)
    })
}

fn find_knowledge_results(value: &Value, depth: usize) -> Option<(&str, &[Value])> {
    if depth > 4 {
        return None;
    }
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        let knowledge_base_id = value
            .get("kb_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Some((knowledge_base_id, results));
    }
    for key in ["details", "data", "result", "output", "content", "artifact"] {
        if let Some(child) = value.get(key) {
            if let Some(found) = find_knowledge_results(child, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knowledge_binding(id: &str, name: Option<&str>, enabled: bool) -> KnowledgeBindingRecord {
        KnowledgeBindingRecord {
            conversation_id: "conversation-1".to_owned(),
            service_connection_id: "knowledge-service".to_owned(),
            knowledge_base_id: id.to_owned(),
            knowledge_base_name: name.map(str::to_owned),
            enabled,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn leaves_yuxi_query_unchanged_without_enabled_knowledge_bases() {
        let query = "轨道吊起升如何赋值";
        assert_eq!(build_yuxi_query(query, &[]), query);
        assert_eq!(
            build_yuxi_query(
                query,
                &[knowledge_binding("kb-1", Some("起重机资料"), false)]
            ),
            query
        );
    }

    #[test]
    fn adds_selected_knowledge_bases_and_retrieval_requirement_to_yuxi_query() {
        let query = build_yuxi_query(
            "轨道吊起升如何赋值",
            &[
                knowledge_binding("kb-crane", Some("轨道吊资料"), true),
                knowledge_binding("kb-spec", Some("设备 规范"), true),
            ],
        );

        assert!(query.contains("- 轨道吊资料 (kb_id: kb-crane)"));
        assert!(query.contains("- 设备 规范 (kb_id: kb-spec)"));
        assert!(query.contains("@knowledge:轨道吊资料"));
        assert!(query.contains("@knowledge:\"设备 规范\""));
        assert!(query.contains("必须先实际调用 query_kb"));
        assert!(query.ends_with("[用户问题]\n轨道吊起升如何赋值"));
    }

    #[test]
    fn deduplicates_knowledge_bindings_by_id() {
        let query = build_yuxi_query(
            "查询",
            &[
                knowledge_binding("kb-1", Some("第一个名称"), true),
                knowledge_binding("kb-1", Some("重复名称"), true),
            ],
        );

        assert_eq!(query.matches("kb_id: kb-1").count(), 1);
        assert!(!query.contains("重复名称"));
    }

    #[test]
    fn validates_selected_knowledge_bases_against_an_explicit_agent_scope() {
        let bindings = [knowledge_binding("kb-1", Some("轨道吊资料"), true)];
        assert!(validate_knowledge_scope(&bindings, None).is_ok());
        assert!(validate_knowledge_scope(&bindings, Some(&["kb-1".to_owned()])).is_ok());

        let error = validate_knowledge_scope(&bindings, Some(&["kb-2".to_owned()]))
            .expect_err("reject a knowledge base outside the agent scope");
        assert!(error.contains("轨道吊资料"));
        assert!(error.contains("智能体未启用"));
    }

    #[test]
    fn parses_and_maps_compact_yuxi_sse() {
        let frame = "id: 12-0\nevent: messages\ndata: {\"payload\":{\"chunk\":{\"status\":\"loading\",\"stream_event\":{\"type\":\"message_delta\",\"content\":\"hello\",\"reasoning_content\":\"thinking\"}}}}";
        let event = parse_sse_frame(frame).expect("parse event");
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(&event.event, &event.data, &mut state);
        assert_eq!(event.id.as_deref(), Some("12-0"));
        assert_eq!(mapped[0]["type"], "reasoning.delta");
        assert_eq!(mapped[1]["type"], "message.started");
        assert_eq!(mapped[2]["type"], "message.delta");
    }

    #[test]
    fn parses_an_eof_frame_without_a_trailing_separator() {
        let frame = "id: 15-0\nevent: messages\ndata: {\"payload\":{\"chunk\":{\"stream_event\":{\"type\":\"message_delta\",\"content\":\"完整结尾\"}}}}";
        let mut buffer = frame.as_bytes().to_vec();
        let events = parse_sse_buffer(&mut buffer, true).expect("parse trailing event");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id.as_deref(), Some("15-0"));
        assert_eq!(
            events[0].data["payload"]["chunk"]["stream_event"]["content"],
            "完整结尾"
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_an_oversized_incomplete_sse_frame() {
        let mut buffer = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
        let error = parse_sse_buffer(&mut buffer, false).expect_err("reject oversized frame");
        assert!(error.contains("safety limit"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_an_oversized_complete_sse_frame() {
        let mut buffer = vec![b'x'; MAX_SSE_FRAME_BYTES + 1];
        buffer.extend_from_slice(b"\n\n");
        let error = parse_sse_buffer(&mut buffer, false).expect_err("reject oversized frame");
        assert!(error.contains("safety limit"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn preserves_utf8_characters_split_across_http_chunks() {
        let frame = "id: 16-0\nevent: messages\ndata: {\"payload\":{\"chunk\":{\"stream_event\":{\"type\":\"message_delta\",\"content\":\"起升高度\"}}}}\n\n";
        let bytes = frame.as_bytes();
        let split = bytes
            .windows("升".len())
            .position(|window| window == "升".as_bytes())
            .expect("Chinese character")
            + 1;
        let mut buffer = bytes[..split].to_vec();
        assert!(parse_sse_buffer(&mut buffer, false)
            .expect("partial chunk")
            .is_empty());
        buffer.extend_from_slice(&bytes[split..]);
        let events = parse_sse_buffer(&mut buffer, false).expect("complete chunk");
        assert_eq!(
            events[0].data["payload"]["chunk"]["stream_event"]["content"],
            "起升高度"
        );
    }

    #[test]
    fn splits_large_message_deltas_on_unicode_boundaries() {
        let chunks = text_chunks("额定起重量、跨度、起升高度", 4);
        assert_eq!(chunks.concat(), "额定起重量、跨度、起升高度");
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 4));
    }

    #[test]
    fn maps_stream_event_chunks_emitted_by_newer_yuxi_agents() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "custom",
            &json!({"payload":{"chunk":{"status":"stream_event","event":{"type":"message_delta","content":"hello"}}}}),
            &mut state,
        );
        assert_eq!(mapped[0]["type"], "message.started");
        assert_eq!(mapped[1]["type"], "message.delta");
        assert_eq!(mapped[1]["delta"], "hello");
    }

    #[test]
    fn separates_inline_think_content_from_the_answer() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"<think>先检查资料</think>正式回答"}}}}),
            &mut state,
        );
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0]["type"], "reasoning.delta");
        assert_eq!(mapped[0]["delta"], "先检查资料");
        assert_eq!(mapped[1]["type"], "message.started");
        assert_eq!(mapped[2]["type"], "message.delta");
        assert_eq!(mapped[2]["delta"], "正式回答");
    }

    #[test]
    fn separates_think_tags_split_across_stream_chunks() {
        let mut state = StreamState::default();
        let first = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"<thi"}}}}),
            &mut state,
        );
        assert!(first.is_empty());

        let second = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"nk>分析中</thi"}}}}),
            &mut state,
        );
        assert_eq!(second.len(), 1);
        assert_eq!(second[0]["type"], "reasoning.delta");
        assert_eq!(second[0]["delta"], "分析中");

        let third = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"nk>答案"}}}}),
            &mut state,
        );
        assert_eq!(third.len(), 2);
        assert_eq!(third[0]["type"], "message.started");
        assert_eq!(third[1]["delta"], "答案");
    }

    #[test]
    fn flushes_unclosed_think_content_when_the_run_finishes() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"<think>仍在分析"}}}}),
            &mut state,
        );
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0]["type"], "reasoning.delta");

        let terminal = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"completed"}}),
            &mut state,
        );
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0]["type"], "run.completed");
    }

    #[test]
    fn maps_terminal_only_resume_events() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"completed"}}),
            &mut state,
        );
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0]["type"], "run.completed");
        assert!(state.terminal);
    }

    #[test]
    fn waits_for_the_explicit_end_after_an_ordinary_success_message() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"status":"success","stream_event":{"type":"message_delta","content":"跨度"}}}}),
            &mut state,
        );
        assert_eq!(mapped[0]["type"], "message.started");
        assert_eq!(mapped[1]["type"], "message.delta");
        assert_eq!(mapped[1]["delta"], "跨度");
        assert!(!state.terminal);

        let terminal = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"completed"}}),
            &mut state,
        );
        assert_eq!(terminal[0]["type"], "message.completed");
        assert_eq!(terminal[1]["type"], "run.completed");
        assert!(state.terminal);
    }

    #[test]
    fn preserves_message_deltas_after_an_intermediate_success_status() {
        let mut state = StreamState::default();
        let mut output = Vec::new();
        for (event_type, envelope) in [
            (
                "messages",
                json!({"payload":{"chunk":{"status":"success","stream_event":{"type":"message_delta","content":"额定起重量、跨"}}}}),
            ),
            (
                "messages",
                json!({"payload":{"chunk":{"status":"loading","stream_event":{"type":"message_delta","content":"距、起升高度"}}}}),
            ),
            ("end", json!({"payload":{"status":"completed"}})),
        ] {
            output.extend(map_yuxi_event(event_type, &envelope, &mut state));
        }
        let answer = output
            .iter()
            .filter(|event| event["type"] == "message.delta")
            .filter_map(|event| event["delta"].as_str())
            .collect::<String>();
        assert_eq!(answer, "额定起重量、跨距、起升高度");
        assert_eq!(output[output.len() - 2]["type"], "message.completed");
        assert_eq!(output[output.len() - 1]["type"], "run.completed");
    }

    #[test]
    fn treats_ask_user_question_interruption_as_waiting_for_user() {
        let mut state = StreamState::default();
        let started = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"stream_event":{
                "type":"tool_call",
                "tool_call_id":"call-question",
                "name":"ask_user_question",
                "args":{"question":"请选择技术场景"}
            }}}}),
            &mut state,
        );
        assert_eq!(started[0]["type"], "tool.started");

        let terminal = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"interrupted"}}),
            &mut state,
        );
        assert_eq!(terminal[0]["type"], "tool.completed");
        assert_eq!(terminal[0]["toolCallId"], "call-question");
        assert_eq!(terminal[0]["isError"], false);
        assert_eq!(terminal[1]["type"], "run.completed");
        assert_eq!(terminal[1]["completionReason"], "awaiting_user");
        assert!(!terminal
            .iter()
            .any(|event| event["type"] == "run.interrupted"));
    }

    #[test]
    fn maps_structured_ask_user_question_request() {
        let mut state = StreamState::default();
        let mapped = map_yuxi_event(
            "messages",
            &json!({
                "payload": {
                    "chunk": {
                        "status": "ask_user_question_required",
                        "source": "ask_user_question",
                        "questions": [{
                            "question_id": "scene",
                            "question": "请选择应用场景",
                            "options": [{"label": "轨道吊", "value": "rail"}],
                            "multi_select": false,
                            "allow_other": true
                        }]
                    }
                }
            }),
            &mut state,
        );
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0]["type"], "user.question.requested");
        assert_eq!(mapped[0]["questions"][0]["question_id"], "scene");
        assert_eq!(mapped[0]["questions"][0]["options"][0]["value"], "rail");
    }

    #[test]
    fn preserves_genuine_yuxi_interruptions() {
        let mut state = StreamState::default();
        let terminal = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"interrupted","message":"worker stopped"}}),
            &mut state,
        );
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0]["type"], "run.interrupted");
        assert_eq!(terminal[0]["code"], "yuxi.interrupted");
        assert_eq!(terminal[0]["message"], "worker stopped");
    }

    #[test]
    fn does_not_duplicate_completed_ask_user_question_tools() {
        let mut state = StreamState::default();
        state
            .tools
            .insert("call-question".to_owned(), "ask_user_question".to_owned());
        state.completed_tools.insert("call-question".to_owned());

        let terminal = map_yuxi_event(
            "end",
            &json!({"payload":{"status":"interrupted"}}),
            &mut state,
        );
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0]["type"], "run.completed");
        assert_eq!(terminal[0]["completionReason"], "awaiting_user");
    }

    #[test]
    fn maps_yuxi_knowledge_tool_results_to_sources() {
        let mut state = StreamState::default();
        state
            .tools
            .insert("call-kb".to_owned(), "query_kb".to_owned());
        let mapped = map_yuxi_event(
            "messages",
            &json!({"payload":{"chunk":{"event":{"method":"tools","data":{
                "event":"tool-finished","tool_call_id":"call-kb","tool_name":"query_kb",
                "output":{"content":"{\"kb_id\":\"kb-1\",\"results\":[{\"id\":\"chunk-1\",\"file_id\":\"doc-1\",\"content\":\"Fox knowledge\",\"metadata\":{\"source\":\"guide.md\",\"page_number\":7,\"section\":\"Runtime boundary\"}}]}"}
            }}}}}),
            &mut state,
        );
        assert_eq!(mapped[0]["type"], "source.added");
        assert_eq!(mapped[0]["knowledgeBaseId"], "kb-1");
        assert_eq!(mapped[0]["documentId"], "doc-1");
        assert_eq!(mapped[0]["chunkId"], "chunk-1");
        assert_eq!(mapped[0]["page"], 7);
        assert_eq!(mapped[0]["anchor"], "Runtime boundary");
        assert_eq!(mapped[1]["type"], "tool.completed");
    }
}

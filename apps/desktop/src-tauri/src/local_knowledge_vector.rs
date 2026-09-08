use crate::{
    app_state::AppState,
    database::{now_ms, ApiResponse},
    local_knowledge::{
        LocalKnowledgeError, LocalKnowledgeJob, LocalKnowledgeOperationAccepted,
        LocalKnowledgeStore,
    },
    local_knowledge_import::{
        parse_imported_document, ImportedDocument, ParsedDocument, DEFAULT_CHUNK_MAX_BYTES,
    },
};
use reqwest::{blocking::Client, header::RANGE, StatusCode};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tauri::{Manager, State};
use uuid::Uuid;

#[cfg(feature = "zvec")]
use crate::vector_store::{SearchRequest, VectorRecord, VectorStore, ZvecVectorStore};

const RECOMMENDED_MODEL_ID: &str = "bge-small-zh-v1.5";
const RECOMMENDED_MODEL_NAME: &str = "BGE Small 中文 v1.5";
const RECOMMENDED_MODEL_VERSION: &str = "75c43b069aac4d136ba6bc1122f995fedcfd2781";
const RECOMMENDED_MODEL_DIMENSION: i64 = 512;
const MULTILINGUAL_E5_MODEL_ID: &str = "multilingual-e5-small";
const MULTILINGUAL_E5_MODEL_NAME: &str = "Multilingual E5 Small";
const MULTILINGUAL_E5_MODEL_VERSION: &str = "761b726dd34fb83930e26aab4e9ac3899aa1fa78";
const MULTILINGUAL_E5_MODEL_DIMENSION: i64 = 384;
const LEGACY_MINILM_MODEL_ID: &str = "all-MiniLM-L6-v2";
const ZVEC_STORE_KIND: &str = "zvec-hnsw-cosine-v1";
const ZVEC_STORE_VERSION: &str = "0.6.0";
const DEFAULT_EMBEDDING_MAX_TOKENS: usize = 512;
const EMBEDDING_MODEL_TEST_TIMEOUT: Duration = Duration::from_secs(45);
const RETRIEVAL_WORKER_TIMEOUT: Duration = Duration::from_secs(60);
const RETRIEVAL_WORKER_ROOT_ENV: &str = "FOX_KNOWLEDGE_RETRIEVAL_ROOT";
const RETRIEVAL_WORKER_REQUEST_ENV: &str = "FOX_KNOWLEDGE_RETRIEVAL_REQUEST";
const RETRIEVAL_WORKER_ZVEC_RESOURCE_ENV: &str = "FOX_KNOWLEDGE_ZVEC_RESOURCE_DIR";
const RRF_VERSION: &str = "rrf-v1-k60";
const RRF_K: f64 = 60.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelAsset {
    pub name: String,
    pub download_name: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: i64,
}

fn bge_recommended_assets() -> Vec<EmbeddingModelAsset> {
    let revision = RECOMMENDED_MODEL_VERSION;
    let root = format!("https://huggingface.co/Xenova/bge-small-zh-v1.5/resolve/{revision}");
    vec![
        EmbeddingModelAsset {
            name: "model.onnx".to_owned(),
            download_name: "onnx/model_int8.onnx".to_owned(),
            url: format!("{root}/onnx/model_int8.onnx?download=true"),
            sha256: "b9837c19ce154ff0726d398ee77abbc03a7faf0476c6f93016c84e531be7ebb5".to_owned(),
            size_bytes: 23_903_394,
        },
        EmbeddingModelAsset {
            name: "tokenizer.json".to_owned(),
            download_name: "tokenizer.json".to_owned(),
            url: format!("{root}/tokenizer.json?download=true"),
            sha256: "48cea5d44424912a6fd1ea647bf4fe50b55ab8b1e5879c3275f80e339e8fae26".to_owned(),
            size_bytes: 439_125,
        },
        EmbeddingModelAsset {
            name: "vocab.txt".to_owned(),
            download_name: "vocab.txt".to_owned(),
            url: format!("{root}/vocab.txt?download=true"),
            sha256: "45bbac6b341c319adc98a532532882e91a9cefc0329aa57bac9ae761c27b291c".to_owned(),
            size_bytes: 109_540,
        },
    ]
}

fn multilingual_e5_recommended_assets() -> Vec<EmbeddingModelAsset> {
    let revision = MULTILINGUAL_E5_MODEL_VERSION;
    let root = format!("https://huggingface.co/Xenova/multilingual-e5-small/resolve/{revision}");
    vec![
        EmbeddingModelAsset {
            name: "model.onnx".to_owned(),
            download_name: "onnx/model_int8.onnx".to_owned(),
            url: format!("{root}/onnx/model_int8.onnx?download=true"),
            sha256: "4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c".to_owned(),
            size_bytes: 118_054_593,
        },
        EmbeddingModelAsset {
            name: "tokenizer.json".to_owned(),
            download_name: "tokenizer.json".to_owned(),
            url: format!("{root}/tokenizer.json?download=true"),
            sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39".to_owned(),
            size_bytes: 17_082_730,
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingPackageManifest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    version: String,
    dimension: usize,
    #[serde(default)]
    languages: Vec<String>,
    tokenizer: String,
    hash: String,
    license: String,
    #[serde(default = "default_pooling")]
    pooling: String,
    #[serde(default)]
    normalize: bool,
    #[serde(default = "default_embedding_max_tokens")]
    max_tokens: usize,
    #[serde(default)]
    query_prefix: String,
    #[serde(default)]
    passage_prefix: String,
    #[serde(default)]
    files: Vec<EmbeddingModelAsset>,
}

fn default_pooling() -> String {
    "mean".to_owned()
}

fn default_embedding_max_tokens() -> usize {
    DEFAULT_EMBEDDING_MAX_TOKENS
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingDownloadView {
    pub id: String,
    pub status: String,
    pub progress: i64,
    pub downloaded_bytes: i64,
    pub total_bytes: i64,
    pub current_file: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub languages: Vec<String>,
    pub dimension: i64,
    pub license: String,
    pub source_url: String,
    pub status: String,
    pub integrity_status: String,
    pub is_default: bool,
    pub recommended: bool,
    pub size_bytes: i64,
    pub installed_at: Option<i64>,
    pub package_path: Option<String>,
    pub load_ready: bool,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub files: Vec<EmbeddingModelAsset>,
    pub download: Option<EmbeddingDownloadView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelTestResult {
    pub integrity_verified: bool,
    pub load_ready: bool,
    pub dimension: i64,
    pub elapsed_ms: i64,
    pub message: String,
    pub error_code: Option<String>,
    pub error_details: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelIdRequest {
    pub model_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingDownloadIdRequest {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelImportRequest {
    pub package_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexStartRequest {
    pub knowledge_base_id: String,
    pub rebuild: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalRequest {
    pub knowledge_base_id: String,
    pub query: String,
    pub mode: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetrievalWorkerResponse {
    ok: bool,
    data: Option<Value>,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalCase {
    pub id: String,
    pub knowledge_base_id: String,
    pub question: String,
    pub expected_document_ids: Vec<String>,
    pub expected_keywords: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalCaseSaveRequest {
    pub id: Option<String>,
    pub knowledge_base_id: String,
    pub question: String,
    pub expected_document_ids: Option<Vec<String>>,
    pub expected_keywords: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalCasesRequest {
    pub knowledge_base_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRetrievalCaseIdRequest {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentActionRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunkPreviewRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunkPreview {
    pub id: String,
    pub chunk_index: i64,
    pub content: String,
    pub anchor: Option<String>,
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
}

fn model_response<T: Serialize>(result: Result<T, LocalKnowledgeError>) -> ApiResponse<T> {
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::failure(error.code(), error.to_string(), error.retryable()),
    }
}

fn recommended_manifest(model_id: &str) -> Option<EmbeddingPackageManifest> {
    match model_id {
        RECOMMENDED_MODEL_ID => {
            let files = bge_recommended_assets();
            Some(EmbeddingPackageManifest {
                id: RECOMMENDED_MODEL_ID.to_owned(),
                name: RECOMMENDED_MODEL_NAME.to_owned(),
                version: RECOMMENDED_MODEL_VERSION.to_owned(),
                dimension: RECOMMENDED_MODEL_DIMENSION as usize,
                languages: vec!["中文".to_owned()],
                tokenizer: "tokenizer.json".to_owned(),
                hash: files[0].sha256.clone(),
                license: "MIT".to_owned(),
                pooling: "cls".to_owned(),
                normalize: true,
                max_tokens: DEFAULT_EMBEDDING_MAX_TOKENS,
                query_prefix: String::new(),
                passage_prefix: String::new(),
                files,
            })
        }
        MULTILINGUAL_E5_MODEL_ID => {
            let files = multilingual_e5_recommended_assets();
            Some(EmbeddingPackageManifest {
                id: MULTILINGUAL_E5_MODEL_ID.to_owned(),
                name: MULTILINGUAL_E5_MODEL_NAME.to_owned(),
                version: MULTILINGUAL_E5_MODEL_VERSION.to_owned(),
                dimension: MULTILINGUAL_E5_MODEL_DIMENSION as usize,
                languages: vec!["中文".to_owned(), "英文".to_owned()],
                tokenizer: "tokenizer.json".to_owned(),
                hash: files[0].sha256.clone(),
                license: "MIT".to_owned(),
                pooling: "mean".to_owned(),
                normalize: true,
                max_tokens: DEFAULT_EMBEDDING_MAX_TOKENS,
                query_prefix: "query: ".to_owned(),
                passage_prefix: "passage: ".to_owned(),
                files,
            })
        }
        _ => None,
    }
}

fn recommended_manifests() -> Vec<EmbeddingPackageManifest> {
    [RECOMMENDED_MODEL_ID, MULTILINGUAL_E5_MODEL_ID]
        .into_iter()
        .filter_map(recommended_manifest)
        .collect()
}

fn recommended_source_url(model_id: &str) -> Option<&'static str> {
    match model_id {
        RECOMMENDED_MODEL_ID => Some("https://huggingface.co/Xenova/bge-small-zh-v1.5"),
        MULTILINGUAL_E5_MODEL_ID => Some("https://huggingface.co/Xenova/multilingual-e5-small"),
        _ => None,
    }
}

#[cfg(feature = "local-embedding")]
#[derive(Debug)]
struct EmbeddingSmokeFailure {
    code: String,
    message: String,
    details: String,
}

#[cfg(feature = "local-embedding")]
fn embedding_smoke_failure(
    code: impl Into<String>,
    message: impl Into<String>,
    details: impl Into<String>,
) -> EmbeddingSmokeFailure {
    EmbeddingSmokeFailure {
        code: code.into(),
        message: message.into(),
        details: details.into(),
    }
}

#[cfg(feature = "local-embedding")]
fn collect_embedding_smoke_stderr(mut stderr: impl Read) -> String {
    const MAX_DIAGNOSTIC_BYTES: usize = 64_000;
    const MAX_DIAGNOSTIC_CHARS: usize = 16_000;
    let mut captured = Vec::with_capacity(MAX_DIAGNOSTIC_BYTES.min(8_192));
    let mut buffer = [0_u8; 4_096];
    let mut truncated = false;
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => return format!("读取隔离进程诊断输出失败：{error}"),
        };
        let remaining = MAX_DIAGNOSTIC_BYTES.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        truncated |= read > remaining;
    }
    let details = String::from_utf8_lossy(&captured);
    let details = details.trim();
    if details.is_empty() {
        return "隔离进程没有返回额外诊断信息。".to_owned();
    }
    if !truncated && details.chars().count() <= MAX_DIAGNOSTIC_CHARS {
        details.to_owned()
    } else {
        format!(
            "{}\n\n[诊断信息超过 {} 字符，已截断]",
            details
                .chars()
                .take(MAX_DIAGNOSTIC_CHARS)
                .collect::<String>(),
            MAX_DIAGNOSTIC_CHARS
        )
    }
}

#[cfg(feature = "local-embedding")]
fn finish_embedding_smoke_stderr(reader: thread::JoinHandle<String>) -> String {
    reader
        .join()
        .unwrap_or_else(|_| "隔离进程诊断读取线程异常终止。".to_owned())
}

#[cfg(feature = "local-embedding")]
fn embedding_smoke_exit_status(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => {
            #[cfg(windows)]
            {
                format!("{code} (0x{:08X})", code as u32)
            }
            #[cfg(not(windows))]
            {
                code.to_string()
            }
        }
        None => "被系统终止".to_owned(),
    }
}

#[cfg(feature = "local-embedding")]
fn run_embedding_model_smoke_process(package_path: &Path) -> Result<(), EmbeddingSmokeFailure> {
    let executable = std::env::current_exe().map_err(|error| {
        embedding_smoke_failure(
            "local_embedding.smoke_executable_missing",
            "无法启动隔离推理测试",
            format!("无法定位 Fox 当前可执行文件：{error}"),
        )
    })?;
    let mut command = Command::new(executable);
    command
        .env("FOX_EMBEDDING_SMOKE_PACKAGE", package_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| {
        embedding_smoke_failure(
            "local_embedding.smoke_launch_failed",
            "隔离推理进程启动失败",
            format!("模型目录：{}\n启动错误：{error}", package_path.display()),
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        embedding_smoke_failure(
            "local_embedding.smoke_diagnostics_unavailable",
            "隔离推理进程启动后无法读取诊断信息",
            format!("模型目录：{}", package_path.display()),
        )
    })?;
    let stderr_reader = thread::Builder::new()
        .name("fox-embedding-smoke-stderr".to_owned())
        .spawn(move || collect_embedding_smoke_stderr(stderr))
        .map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            embedding_smoke_failure(
                "local_embedding.smoke_diagnostics_launch_failed",
                "无法启动隔离推理诊断读取线程",
                error.to_string(),
            )
        })?;
    let deadline = Instant::now() + EMBEDDING_MODEL_TEST_TIMEOUT;
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let diagnostics = finish_embedding_smoke_stderr(stderr_reader);
                return Err(embedding_smoke_failure(
                    "local_embedding.smoke_wait_failed",
                    "无法读取隔离推理进程状态",
                    format!("{error}\n\n{diagnostics}"),
                ));
            }
        };
        if let Some(status) = status {
            let diagnostics = finish_embedding_smoke_stderr(stderr_reader);
            return if status.success() {
                Ok(())
            } else {
                Err(embedding_smoke_failure(
                    "local_embedding.smoke_inference_failed",
                    "模型文件完整，但隔离加载或推理失败",
                    format!(
                        "退出状态：{}\n模型目录：{}\n\n{}",
                        embedding_smoke_exit_status(status),
                        package_path.display(),
                        diagnostics
                    ),
                ))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let diagnostics = finish_embedding_smoke_stderr(stderr_reader);
            return Err(embedding_smoke_failure(
                "local_embedding.smoke_timeout",
                "隔离推理测试超时，已安全终止测试进程",
                format!(
                    "超时限制：{} 秒\n模型目录：{}\n\n{}",
                    EMBEDDING_MODEL_TEST_TIMEOUT.as_secs(),
                    package_path.display(),
                    diagnostics
                ),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn collect_retrieval_worker_stream(mut stream: impl Read, max_bytes: usize) -> String {
    let mut captured = Vec::with_capacity(max_bytes.min(16_384));
    let mut buffer = [0_u8; 8_192];
    let mut truncated = false;
    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => return format!("读取检索隔离进程输出失败：{error}"),
        };
        let remaining = max_bytes.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        truncated |= read > remaining;
    }
    let mut output = String::from_utf8_lossy(&captured).trim().to_owned();
    if truncated {
        output.push_str("\n\n[隔离进程输出过长，已截断]");
    }
    output
}

fn finish_retrieval_worker_stream(reader: thread::JoinHandle<String>) -> String {
    reader
        .join()
        .unwrap_or_else(|_| "检索隔离进程输出读取线程异常终止。".to_owned())
}

fn retrieval_worker_error(
    response: RetrievalWorkerResponse,
    diagnostics: &str,
) -> LocalKnowledgeError {
    let mut message = response
        .error_message
        .unwrap_or_else(|| "向量检索隔离进程没有返回错误详情".to_owned());
    if !diagnostics.is_empty() {
        message.push_str("\n\n诊断信息：\n");
        message.push_str(diagnostics);
    }
    match response.error_code.as_deref() {
        Some("local_knowledge.invalid_request") => LocalKnowledgeError::invalid(message),
        Some("local_knowledge.job_conflict") => LocalKnowledgeError::conflict(message),
        _ => LocalKnowledgeError::storage(message),
    }
}

#[cfg(all(feature = "local-embedding", feature = "zvec"))]
fn run_retrieval_worker_process(
    store: &LocalKnowledgeStore,
    request: &KnowledgeRetrievalRequest,
) -> Result<Value, LocalKnowledgeError> {
    let executable = std::env::current_exe().map_err(|error| {
        LocalKnowledgeError::storage(format!("无法定位 Fox 当前可执行文件：{error}"))
    })?;
    let request_json = serde_json::to_string(request).map_err(LocalKnowledgeError::storage)?;
    let mut command = Command::new(executable);
    command
        .env(RETRIEVAL_WORKER_ROOT_ENV, store.root.as_os_str())
        .env(RETRIEVAL_WORKER_REQUEST_ENV, request_json)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(resource_dir) = store.zvec_resource_dir.as_ref().as_ref() {
        command.env(RETRIEVAL_WORKER_ZVEC_RESOURCE_ENV, resource_dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| {
        LocalKnowledgeError::storage(format!("无法启动向量检索隔离进程：{error}"))
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        LocalKnowledgeError::storage("无法读取向量检索隔离进程结果")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        LocalKnowledgeError::storage("无法读取向量检索隔离进程诊断信息")
    })?;
    let stdout_reader = thread::Builder::new()
        .name("fox-retrieval-worker-stdout".to_owned())
        .spawn(move || collect_retrieval_worker_stream(stdout, 2 * 1024 * 1024))
        .map_err(LocalKnowledgeError::storage)?;
    let stderr_reader = thread::Builder::new()
        .name("fox-retrieval-worker-stderr".to_owned())
        .spawn(move || collect_retrieval_worker_stream(stderr, 64 * 1024))
        .map_err(LocalKnowledgeError::storage)?;
    let deadline = Instant::now() + RETRIEVAL_WORKER_TIMEOUT;
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = finish_retrieval_worker_stream(stdout_reader);
                let stderr = finish_retrieval_worker_stream(stderr_reader);
                return Err(LocalKnowledgeError::storage(format!(
                    "无法读取向量检索隔离进程状态：{error}\n\n输出：{stdout}\n\n诊断：{stderr}"
                )));
            }
        };
        if let Some(status) = status {
            let stdout = finish_retrieval_worker_stream(stdout_reader);
            let stderr = finish_retrieval_worker_stream(stderr_reader);
            let response =
                serde_json::from_str::<RetrievalWorkerResponse>(&stdout).map_err(|error| {
                    LocalKnowledgeError::storage(format!(
                        "向量检索隔离进程异常退出（状态 {}）：{error}\n\n诊断信息：{}",
                        embedding_smoke_exit_status(status),
                        if stderr.is_empty() { "无" } else { &stderr }
                    ))
                })?;
            if !status.success() || !response.ok {
                return Err(retrieval_worker_error(response, &stderr));
            }
            return response
                .data
                .ok_or_else(|| LocalKnowledgeError::storage("向量检索隔离进程没有返回结果"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = finish_retrieval_worker_stream(stdout_reader);
            let diagnostics = finish_retrieval_worker_stream(stderr_reader);
            return Err(LocalKnowledgeError::storage(format!(
                "向量检索超过 {} 秒，已安全终止隔离进程。{}",
                RETRIEVAL_WORKER_TIMEOUT.as_secs(),
                if diagnostics.is_empty() {
                    String::new()
                } else {
                    format!("\n\n诊断信息：{diagnostics}")
                }
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

impl LocalKnowledgeStore {
    pub(crate) fn recover_embedding_downloads(&self) -> Result<(), LocalKnowledgeError> {
        let now = now_ms();
        self.connection()?.execute(
            "UPDATE local_embedding_model_downloads
             SET status = 'queued', error_code = 'local_embedding.download_resumed',
                 error_message = 'Fox restarted; the download will continue from the saved partial file',
                 updated_at = ?1, completed_at = NULL
             WHERE status = 'running'",
            [now],
        )?;
        let pending = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT id, checkpoint_json FROM local_embedding_model_downloads
                 WHERE status = 'queued' ORDER BY created_at ASC",
            )?;
            let items = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            items
        };
        for (download_id, checkpoint) in pending {
            let manifest: EmbeddingPackageManifest =
                serde_json::from_str(&checkpoint).map_err(LocalKnowledgeError::storage)?;
            let store = self.clone();
            thread::Builder::new()
                .name(format!("fox-model-download-recover-{download_id}"))
                .spawn(move || store.run_embedding_model_download(&download_id, manifest))
                .map_err(LocalKnowledgeError::storage)?;
        }
        Ok(())
    }

    pub fn list_embedding_models(&self) -> Result<Vec<EmbeddingModelView>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut items = Vec::new();
        for manifest in recommended_manifests() {
            let download = connection
                .query_row(
                    "SELECT id, status, progress, downloaded_bytes, total_bytes, current_file,
                            error_code, error_message, updated_at
                     FROM local_embedding_model_downloads
                     WHERE model_id = ?1 ORDER BY updated_at DESC LIMIT 1",
                    [&manifest.id],
                    map_download,
                )
                .optional()?;
            let installed = connection
                .query_row(
                    "SELECT model_id, COALESCE(name, model_id), version, dimension,
                            languages_json, COALESCE(license, ''), COALESCE(source_url, ''),
                            status, integrity_status, is_default, COALESCE(size_bytes, 0),
                            installed_at, package_path, last_error_code, last_error_message
                     FROM local_embedding_models WHERE model_id = ?1
                     ORDER BY updated_at DESC LIMIT 1",
                    [&manifest.id],
                    |row| {
                        let languages: String = row.get(4)?;
                        Ok(EmbeddingModelView {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            version: row.get(2)?,
                            dimension: row.get(3)?,
                            languages: serde_json::from_str(&languages).unwrap_or_default(),
                            license: row.get(5)?,
                            source_url: row.get(6)?,
                            status: row.get(7)?,
                            integrity_status: row.get(8)?,
                            is_default: row.get(9)?,
                            size_bytes: row.get(10)?,
                            installed_at: row.get(11)?,
                            package_path: row.get(12)?,
                            last_error_code: row.get(13)?,
                            last_error_message: row.get(14)?,
                            load_ready: false,
                            recommended: true,
                            files: manifest.files.clone(),
                            download: download.clone(),
                        })
                    },
                )
                .optional()?;
            if let Some(mut model) = installed {
                model.load_ready = model.status == "ready"
                    && model.integrity_status == "verified"
                    && model.last_error_code.is_none();
                items.push(model);
            } else {
                items.push(EmbeddingModelView {
                    id: manifest.id.clone(),
                    name: manifest.name,
                    version: manifest.version,
                    languages: manifest.languages,
                    dimension: manifest.dimension as i64,
                    license: manifest.license,
                    source_url: recommended_source_url(&manifest.id)
                        .unwrap_or_default()
                        .to_owned(),
                    status: if download.as_ref().is_some_and(|item| {
                        matches!(item.status.as_str(), "queued" | "running" | "paused")
                    }) {
                        "installing".to_owned()
                    } else {
                        "not_installed".to_owned()
                    },
                    integrity_status: "pending".to_owned(),
                    is_default: false,
                    recommended: true,
                    size_bytes: manifest.files.iter().map(|file| file.size_bytes).sum(),
                    installed_at: None,
                    package_path: None,
                    load_ready: false,
                    last_error_code: download.as_ref().and_then(|item| item.error_code.clone()),
                    last_error_message: download
                        .as_ref()
                        .and_then(|item| item.error_message.clone()),
                    files: manifest.files,
                    download,
                });
            }
        }

        let mut statement = connection.prepare(
            "SELECT model_id, COALESCE(name, model_id), version, dimension,
                    languages_json, COALESCE(license, ''), COALESCE(source_url, ''),
                    status, integrity_status, is_default, COALESCE(size_bytes, 0),
                    installed_at, package_path, last_error_code, last_error_message
             FROM local_embedding_models WHERE model_id NOT IN (?1, ?2)
             ORDER BY name COLLATE NOCASE",
        )?;
        let custom = statement.query_map(
            params![RECOMMENDED_MODEL_ID, MULTILINGUAL_E5_MODEL_ID],
            |row| {
                let languages: String = row.get(4)?;
                let status: String = row.get(7)?;
                let integrity_status: String = row.get(8)?;
                Ok(EmbeddingModelView {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    version: row.get(2)?,
                    dimension: row.get(3)?,
                    languages: serde_json::from_str(&languages).unwrap_or_default(),
                    license: row.get(5)?,
                    source_url: row.get(6)?,
                    load_ready: status == "ready"
                        && integrity_status == "verified"
                        && row.get::<_, Option<String>>(13)?.is_none(),
                    status,
                    integrity_status,
                    is_default: row.get(9)?,
                    recommended: false,
                    size_bytes: row.get(10)?,
                    installed_at: row.get(11)?,
                    package_path: row.get(12)?,
                    last_error_code: row.get(13)?,
                    last_error_message: row.get(14)?,
                    files: Vec::new(),
                    download: None,
                })
            },
        )?;
        items.extend(custom.collect::<Result<Vec<_>, _>>()?);
        Ok(items)
    }

    pub fn start_embedding_model_install(
        &self,
        model_id: &str,
    ) -> Result<LocalKnowledgeOperationAccepted, LocalKnowledgeError> {
        let model_id = model_id.trim();
        let manifest = recommended_manifest(model_id)
            .ok_or_else(|| LocalKnowledgeError::invalid("unknown recommended embedding model"))?;
        let active = self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_embedding_model_downloads
             WHERE model_id = ?1 AND status IN ('queued', 'running', 'paused'))",
            [model_id],
            |row| row.get::<_, bool>(0),
        )?;
        if active {
            return Err(LocalKnowledgeError::conflict(
                "this model already has an active download",
            ));
        }
        let download_id = Uuid::new_v4().to_string();
        let now = now_ms();
        let total_bytes: i64 = manifest.files.iter().map(|file| file.size_bytes).sum();
        let package_path = self.model_package_path(&manifest.id, &manifest.version)?;
        let source_url = recommended_source_url(model_id).unwrap_or_default();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO local_embedding_models(
                model_id, name, version, dimension, status, languages_json, license,
                source_url, package_path, integrity_status, package_hash, size_bytes,
                updated_at, last_error_code, last_error_message
             ) VALUES (?1, ?2, ?3, ?4, 'installing', ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, NULL, NULL)
             ON CONFLICT(model_id, version) DO UPDATE SET
                name = excluded.name, dimension = excluded.dimension, status = 'installing',
                languages_json = excluded.languages_json, license = excluded.license,
                source_url = excluded.source_url, package_path = excluded.package_path,
                integrity_status = 'pending', package_hash = excluded.package_hash,
                size_bytes = excluded.size_bytes, updated_at = excluded.updated_at,
                last_error_code = NULL, last_error_message = NULL",
            params![
                manifest.id,
                manifest.name,
                manifest.version,
                manifest.dimension as i64,
                serde_json::to_string(&manifest.languages).unwrap_or_else(|_| "[]".to_owned()),
                manifest.license,
                source_url,
                package_path.to_string_lossy(),
                manifest.hash,
                total_bytes,
                now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO local_embedding_model_downloads(
                id, model_id, version, status, progress, downloaded_bytes,
                total_bytes, checkpoint_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'queued', 0, 0, ?4, ?5, ?6, ?6)",
            params![
                download_id,
                manifest.id,
                manifest.version,
                total_bytes,
                serde_json::to_string(&manifest).map_err(LocalKnowledgeError::storage)?,
                now,
            ],
        )?;
        transaction.commit()?;

        let store = self.clone();
        let worker_id = download_id.clone();
        thread::Builder::new()
            .name(format!("fox-model-download-{worker_id}"))
            .spawn(move || store.run_embedding_model_download(&worker_id, manifest))
            .map_err(LocalKnowledgeError::storage)?;
        Ok(LocalKnowledgeOperationAccepted {
            operation_id: download_id,
            accepted_at: now,
        })
    }

    fn run_embedding_model_download(&self, download_id: &str, manifest: EmbeddingPackageManifest) {
        if let Err(error) = self.perform_embedding_model_download(download_id, &manifest) {
            let now = now_ms();
            if let Ok(connection) = self.connection() {
                let _ = connection.execute(
                    "UPDATE local_embedding_model_downloads SET status = 'failed',
                        error_code = ?2, error_message = ?3, updated_at = ?4, completed_at = ?4
                     WHERE id = ?1 AND status != 'cancelled'",
                    params![download_id, error.code(), error.to_string(), now],
                );
                let _ = connection.execute(
                    "UPDATE local_embedding_models SET status = 'error', integrity_status = 'failed',
                        last_error_code = ?2, last_error_message = ?3, updated_at = ?4
                     WHERE model_id = ?1",
                    params![manifest.id, error.code(), error.to_string(), now],
                );
            }
        }
    }

    fn perform_embedding_model_download(
        &self,
        download_id: &str,
        manifest: &EmbeddingPackageManifest,
    ) -> Result<(), LocalKnowledgeError> {
        let package_path = self.model_package_path(&manifest.id, &manifest.version)?;
        fs::create_dir_all(&package_path)?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(LocalKnowledgeError::storage)?;
        let total_bytes: i64 = manifest.files.iter().map(|file| file.size_bytes).sum();
        let mut completed_before = 0i64;
        for asset in &manifest.files {
            if self.model_download_cancelled(download_id)? {
                return Ok(());
            }
            let target = package_path.join(&asset.name);
            let partial = package_path.join(format!("{}.part", asset.name));
            if target.is_file() && sha256_file(&target)? == asset.sha256 {
                completed_before += asset.size_bytes;
                continue;
            }
            let mut offset = partial
                .metadata()
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if offset > asset.size_bytes as u64 {
                fs::remove_file(&partial)?;
                offset = 0;
            }
            let mut request = client.get(&asset.url);
            if offset > 0 {
                request = request.header(RANGE, format!("bytes={offset}-"));
            }
            let mut response = request.send().map_err(LocalKnowledgeError::storage)?;
            if !response.status().is_success() {
                return Err(LocalKnowledgeError::storage(format!(
                    "download {} failed with HTTP {}",
                    asset.name,
                    response.status()
                )));
            }
            if offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
                fs::remove_file(&partial)?;
                offset = 0;
            }
            let mut output = OpenOptions::new()
                .create(true)
                .write(true)
                .append(offset > 0)
                .truncate(offset == 0)
                .open(&partial)?;
            let mut buffer = [0u8; 128 * 1024];
            let mut written = offset as i64;
            loop {
                if self.model_download_cancelled(download_id)? {
                    return Ok(());
                }
                let read = response
                    .read(&mut buffer)
                    .map_err(LocalKnowledgeError::storage)?;
                if read == 0 {
                    break;
                }
                output.write_all(&buffer[..read])?;
                written = written.saturating_add(read as i64);
                let downloaded = (completed_before + written).min(total_bytes);
                let progress = if total_bytes > 0 {
                    (downloaded.saturating_mul(100) / total_bytes).clamp(0, 99)
                } else {
                    0
                };
                self.connection()?.execute(
                    "UPDATE local_embedding_model_downloads
                     SET status = 'running', progress = ?2, downloaded_bytes = ?3,
                         current_file = ?4, updated_at = ?5 WHERE id = ?1",
                    params![download_id, progress, downloaded, asset.name, now_ms()],
                )?;
            }
            output.flush()?;
            drop(output);
            let actual = sha256_file(&partial)?;
            if actual != asset.sha256 {
                return Err(LocalKnowledgeError::invalid(format!(
                    "{} SHA-256 mismatch: expected {}, got {}",
                    asset.name, asset.sha256, actual
                )));
            }
            fs::rename(&partial, &target)?;
            completed_before = completed_before.saturating_add(asset.size_bytes);
        }
        fs::write(
            package_path.join("manifest.json"),
            serde_json::to_vec_pretty(manifest).map_err(LocalKnowledgeError::storage)?,
        )?;
        validate_embedding_package(&package_path)?;
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE local_embedding_models SET status = 'ready', integrity_status = 'verified',
                installed_at = ?2, updated_at = ?2, last_error_code = NULL, last_error_message = NULL
             WHERE model_id = ?1 AND version = ?3",
            params![manifest.id, now, manifest.version],
        )?;
        transaction.execute(
            "UPDATE local_embedding_model_downloads SET status = 'completed', progress = 100,
                downloaded_bytes = total_bytes, current_file = NULL, updated_at = ?2,
                completed_at = ?2, error_code = NULL, error_message = NULL WHERE id = ?1",
            params![download_id, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn model_download_cancelled(&self, download_id: &str) -> Result<bool, LocalKnowledgeError> {
        self.connection()?
            .query_row(
                "SELECT status = 'cancelled' FROM local_embedding_model_downloads WHERE id = ?1",
                [download_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn model_package_path(
        &self,
        model_id: &str,
        version: &str,
    ) -> Result<PathBuf, LocalKnowledgeError> {
        validate_storage_component(model_id)?;
        validate_storage_component(version)?;
        Ok(self.root.join("models").join(model_id).join(version))
    }

    pub fn cancel_embedding_download(&self, id: &str) -> Result<bool, LocalKnowledgeError> {
        let changed = self.connection()?.execute(
            "UPDATE local_embedding_model_downloads SET status = 'cancelled', updated_at = ?2,
                completed_at = ?2 WHERE id = ?1 AND status IN ('queued', 'running', 'paused')",
            params![id, now_ms()],
        )?;
        Ok(changed > 0)
    }

    pub fn retry_embedding_download(
        &self,
        id: &str,
    ) -> Result<LocalKnowledgeOperationAccepted, LocalKnowledgeError> {
        let model_id = self
            .connection()?
            .query_row(
                "SELECT model_id FROM local_embedding_model_downloads WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("embedding model download"))?;
        self.start_embedding_model_install(&model_id)
    }

    pub fn set_default_embedding_model(&self, model_id: &str) -> Result<bool, LocalKnowledgeError> {
        if model_id == LEGACY_MINILM_MODEL_ID {
            return Err(LocalKnowledgeError::conflict(
                "all-MiniLM-L6-v2 已退出受控模型清单，不能设为默认；请使用 multilingual-e5-small",
            ));
        }
        let mut connection = self.connection()?;
        let ready = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_embedding_models
             WHERE model_id = ?1 AND status = 'ready' AND integrity_status = 'verified'
               AND last_error_code IS NULL)",
            [model_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !ready {
            return Err(LocalKnowledgeError::conflict(
                "向量模型尚未就绪，或最近一次隔离推理测试未通过",
            ));
        }
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE local_embedding_models SET is_default = 0 WHERE is_default = 1",
            [],
        )?;
        transaction.execute(
            "UPDATE local_embedding_models SET is_default = 1, updated_at = ?2
             WHERE rowid = (
                 SELECT rowid FROM local_embedding_models
                 WHERE model_id = ?1 AND status = 'ready' AND integrity_status = 'verified'
                   AND last_error_code IS NULL
                 ORDER BY updated_at DESC LIMIT 1
             )",
            params![model_id, now_ms()],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn delete_embedding_model(&self, model_id: &str) -> Result<bool, LocalKnowledgeError> {
        let connection = self.connection()?;
        let in_use = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_knowledge_bases WHERE embedding_model_id = ?1)
                 OR EXISTS(SELECT 1 FROM local_kb_index_generations WHERE embedding_model_id = ?1 AND status IN ('staging', 'active'))",
            [model_id],
            |row| row.get::<_, bool>(0),
        )?;
        if in_use {
            return Err(LocalKnowledgeError::conflict(
                "embedding model is still used by a knowledge base",
            ));
        }
        let package_path = connection.query_row(
            "SELECT package_path FROM local_embedding_models WHERE model_id = ?1 ORDER BY updated_at DESC LIMIT 1",
            [model_id],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?.flatten();
        drop(connection);
        let staged = self
            .root
            .join("staging")
            .join(format!("model-delete-{}", Uuid::new_v4()));
        let moved = if let Some(path) = package_path.as_deref() {
            let path = PathBuf::from(path);
            if path.exists() {
                fs::rename(&path, &staged)?;
                true
            } else {
                false
            }
        } else {
            false
        };
        let deleted = self.connection()?.execute(
            "DELETE FROM local_embedding_models WHERE model_id = ?1",
            [model_id],
        );
        match deleted {
            Ok(count) => {
                if moved {
                    let _ = fs::remove_dir_all(&staged);
                }
                Ok(count > 0)
            }
            Err(error) => {
                if moved {
                    if let Some(path) = package_path {
                        let _ = fs::rename(&staged, path);
                    }
                }
                Err(error.into())
            }
        }
    }

    pub fn test_embedding_model(
        &self,
        model_id: &str,
    ) -> Result<EmbeddingModelTestResult, LocalKnowledgeError> {
        let started = Instant::now();
        let (dimension, package_path) = self
            .connection()?
            .query_row(
                "SELECT dimension, package_path FROM local_embedding_models
             WHERE model_id = ?1 AND status = 'ready' ORDER BY updated_at DESC LIMIT 1",
                [model_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("installed embedding model"))?;
        let package_path = PathBuf::from(
            package_path
                .ok_or_else(|| LocalKnowledgeError::invalid("model package path is missing"))?,
        );
        validate_embedding_package(&package_path)?;

        #[cfg(feature = "local-embedding")]
        let (load_ready, message, error_code, error_details) =
            match run_embedding_model_smoke_process(&package_path) {
                Ok(()) => (
                    true,
                    "完整性、隔离加载和推理测试均通过".to_owned(),
                    None,
                    None,
                ),
                Err(error) => (false, error.message, Some(error.code), Some(error.details)),
            };
        #[cfg(not(feature = "local-embedding"))]
        let (load_ready, message, error_code, error_details) = (
            false,
            "当前构建未启用 local-embedding 运行时；文件完整性已通过".to_owned(),
            Some("local_embedding.runtime_disabled".to_owned()),
            Some("请使用包含 local-embedding 功能的 Fox Desktop 构建。".to_owned()),
        );

        let stored_error_message = if load_ready {
            None
        } else {
            Some(match error_details.as_deref() {
                Some(details) if !details.is_empty() => format!("{message}\n\n{details}"),
                _ => message.clone(),
            })
        };

        let now = now_ms();
        self.connection()?.execute(
            "UPDATE local_embedding_models SET integrity_status = 'verified',
                last_error_code = ?2, last_error_message = ?3, updated_at = ?4 WHERE model_id = ?1",
            params![
                model_id,
                if load_ready {
                    None::<String>
                } else {
                    Some("local_embedding.load_failed".to_owned())
                },
                stored_error_message,
                now,
            ],
        )?;
        Ok(EmbeddingModelTestResult {
            integrity_verified: true,
            load_ready,
            dimension,
            elapsed_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            message,
            error_code,
            error_details,
        })
    }

    pub fn import_embedding_model_package(
        &self,
        package_path: &str,
    ) -> Result<EmbeddingModelView, LocalKnowledgeError> {
        let source = PathBuf::from(package_path.trim()).canonicalize()?;
        let manifest = validate_embedding_package(&source)?;
        let model_id = if manifest.id.trim().is_empty() {
            source
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("manual-model")
                .to_owned()
        } else {
            manifest.id.clone()
        };
        if model_id == LEGACY_MINILM_MODEL_ID {
            return Err(LocalKnowledgeError::conflict(
                "all-MiniLM-L6-v2 已退出受控模型清单；如需导入自定义模型，请使用不同的模型 ID",
            ));
        }
        validate_storage_component(&model_id)?;
        let destination = self.model_package_path(&model_id, &manifest.version)?;
        if destination.exists() {
            return Err(LocalKnowledgeError::conflict(
                "this model package version is already installed",
            ));
        }
        copy_model_package(&source, &destination)?;
        if let Err(error) = validate_embedding_package(&destination) {
            let _ = fs::remove_dir_all(&destination);
            return Err(error);
        }
        let size_bytes = directory_size(&destination)?;
        let now = now_ms();
        self.connection()?.execute(
            "INSERT INTO local_embedding_models(
                model_id, name, version, dimension, status, languages_json, license,
                source_url, package_path, integrity_status, package_hash, size_bytes,
                installed_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'ready', ?5, ?6, 'manual-package', ?7,
                       'verified', ?8, ?9, ?10, ?10)",
            params![
                model_id,
                if manifest.name.trim().is_empty() {
                    model_id.clone()
                } else {
                    manifest.name
                },
                manifest.version,
                manifest.dimension as i64,
                serde_json::to_string(&manifest.languages).unwrap_or_else(|_| "[]".to_owned()),
                manifest.license,
                destination.to_string_lossy(),
                normalize_sha256(&manifest.hash)?,
                size_bytes,
                now,
            ],
        )?;
        self.list_embedding_models()?
            .into_iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| LocalKnowledgeError::not_found("imported embedding model"))
    }
}

fn map_download(row: &rusqlite::Row<'_>) -> rusqlite::Result<EmbeddingDownloadView> {
    Ok(EmbeddingDownloadView {
        id: row.get(0)?,
        status: row.get(1)?,
        progress: row.get(2)?,
        downloaded_bytes: row.get(3)?,
        total_bytes: row.get(4)?,
        current_file: row.get(5)?,
        error_code: row.get(6)?,
        error_message: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn validate_storage_component(value: &str) -> Result<(), LocalKnowledgeError> {
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(LocalKnowledgeError::invalid(
            "model id or version contains unsafe path characters",
        ));
    }
    Ok(())
}

fn normalize_sha256(value: &str) -> Result<String, LocalKnowledgeError> {
    let value = value
        .trim()
        .strip_prefix("sha256:")
        .or_else(|| value.trim().strip_prefix("sha256-"))
        .unwrap_or(value.trim())
        .to_ascii_lowercase();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LocalKnowledgeError::invalid(
            "invalid SHA-256 digest in model manifest",
        ));
    }
    Ok(value)
}

fn sha256_file(path: &Path) -> Result<String, LocalKnowledgeError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn validate_embedding_package(
    path: &Path,
) -> Result<EmbeddingPackageManifest, LocalKnowledgeError> {
    if !path.is_dir() {
        return Err(LocalKnowledgeError::invalid(
            "embedding package must be a directory",
        ));
    }
    let manifest: EmbeddingPackageManifest =
        serde_json::from_slice(&fs::read(path.join("manifest.json"))?)
            .map_err(LocalKnowledgeError::storage)?;
    if manifest.version.trim().is_empty() || manifest.dimension == 0 || manifest.dimension > 32_768
    {
        return Err(LocalKnowledgeError::invalid(
            "model manifest version or dimension is invalid",
        ));
    }
    if !matches!(manifest.pooling.as_str(), "mean" | "cls") {
        return Err(LocalKnowledgeError::invalid(
            "model pooling must be mean or cls",
        ));
    }
    let model = path.join("model.onnx");
    if !model.is_file() {
        return Err(LocalKnowledgeError::invalid("model.onnx is missing"));
    }
    let expected = normalize_sha256(&manifest.hash)?;
    let actual = sha256_file(&model)?;
    if expected != actual {
        return Err(LocalKnowledgeError::invalid(format!(
            "model.onnx SHA-256 mismatch: expected {expected}, got {actual}"
        )));
    }
    let tokenizer = path.join(&manifest.tokenizer);
    if !tokenizer.is_file() {
        return Err(LocalKnowledgeError::invalid("tokenizer file is missing"));
    }
    for asset in &manifest.files {
        let file = path.join(&asset.name);
        if !file.is_file() {
            return Err(LocalKnowledgeError::invalid(format!(
                "{} is missing",
                asset.name
            )));
        }
        let digest = sha256_file(&file)?;
        if digest != normalize_sha256(&asset.sha256)? {
            return Err(LocalKnowledgeError::invalid(format!(
                "{} SHA-256 mismatch",
                asset.name
            )));
        }
    }
    Ok(manifest)
}

fn copy_model_package(source: &Path, destination: &Path) -> Result<(), LocalKnowledgeError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if metadata.is_symlink() || metadata.is_dir() {
            continue;
        }
        if metadata.is_file() {
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn directory_size(path: &Path) -> Result<i64, LocalKnowledgeError> {
    let mut size = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            size = size.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(size.min(i64::MAX as u64) as i64)
}

#[cfg(feature = "local-embedding")]
struct EmbeddingTokenizer {
    inner: tokenizers::Tokenizer,
}

#[cfg(feature = "local-embedding")]
impl EmbeddingTokenizer {
    fn from_package(
        path: &Path,
        tokenizer_file: &str,
        max_tokens: usize,
    ) -> Result<Self, LocalKnowledgeError> {
        if max_tokens == 0 {
            return Err(LocalKnowledgeError::invalid(
                "embedding tokenizer max_tokens must be greater than zero",
            ));
        }
        let tokenizer_path = path.join(tokenizer_file);
        let mut inner = tokenizers::Tokenizer::from_file(&tokenizer_path).map_err(|error| {
            LocalKnowledgeError::invalid(format!(
                "无法加载 tokenizer '{}': {error}",
                tokenizer_path.display()
            ))
        })?;
        inner
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: max_tokens,
                ..Default::default()
            }))
            .map_err(|error| {
                LocalKnowledgeError::invalid(format!("无法配置 tokenizer 截断规则: {error}"))
            })?;
        Ok(Self { inner })
    }

    fn encode(
        &self,
        text: &str,
    ) -> Result<crate::local_embedding::TokenizedInput, LocalKnowledgeError> {
        let encoding = self
            .inner
            .encode(text, true)
            .map_err(|error| LocalKnowledgeError::invalid(format!("文本分词失败: {error}")))?;
        let input_ids = encoding
            .get_ids()
            .iter()
            .map(|value| i64::from(*value))
            .collect::<Vec<_>>();
        if input_ids.is_empty() {
            return Err(LocalKnowledgeError::invalid(
                "tokenizer produced an empty input",
            ));
        }
        Ok(crate::local_embedding::TokenizedInput {
            attention_mask: encoding
                .get_attention_mask()
                .iter()
                .map(|value| i64::from(*value))
                .collect(),
            token_type_ids: Some(
                encoding
                    .get_type_ids()
                    .iter()
                    .map(|value| i64::from(*value))
                    .collect(),
            ),
            input_ids,
        })
    }
}

#[cfg(feature = "local-embedding")]
fn embedding_runtime(
    package_path: &Path,
) -> Result<
    (
        crate::local_embedding::OnnxEmbeddingProvider,
        EmbeddingTokenizer,
        EmbeddingPackageManifest,
    ),
    LocalKnowledgeError,
> {
    let manifest = validate_embedding_package(package_path)?;
    let tokenizer =
        EmbeddingTokenizer::from_package(package_path, &manifest.tokenizer, manifest.max_tokens)?;
    let provider = crate::local_embedding::OnnxEmbeddingProvider::from_package_dir(package_path)
        .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
    Ok((provider, tokenizer, manifest))
}

#[cfg(feature = "zvec")]
impl LocalKnowledgeStore {
    pub(crate) fn zvec_vector_store(&self) -> Result<ZvecVectorStore, LocalKnowledgeError> {
        let root = self.root.join("indexes");
        let store = if let Some(resource_dir) = self.zvec_resource_dir.as_ref() {
            ZvecVectorStore::from_resource_dir(&root, resource_dir)
        } else {
            ZvecVectorStore::new(&root)
        };
        store.map_err(|error| LocalKnowledgeError::storage(error.to_string()))
    }

    pub(crate) fn validate_zvec_generation_readable(
        &self,
        knowledge_base_id: &str,
        generation_id: &str,
        dimension: usize,
        expected_count: usize,
    ) -> Result<(), LocalKnowledgeError> {
        self.zvec_vector_store()
            .and_then(|store| {
                store
                    .open_generation(knowledge_base_id, generation_id, dimension, expected_count)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))
            })
            .map(|_| ())
    }
}

impl LocalKnowledgeStore {
    pub fn start_index_generation(
        &self,
        knowledge_base_id: &str,
        job_type: &str,
    ) -> Result<LocalKnowledgeOperationAccepted, LocalKnowledgeError> {
        #[cfg(not(all(feature = "local-embedding", feature = "zvec")))]
        {
            let _ = (knowledge_base_id, job_type);
            return Err(LocalKnowledgeError::conflict(
                "当前桌面构建未同时启用 local-embedding 和 Zvec，无法创建向量索引",
            ));
        }
        #[cfg(all(feature = "local-embedding", feature = "zvec"))]
        {
            if !matches!(job_type, "index" | "rebuild") {
                return Err(LocalKnowledgeError::invalid(
                    "index job type must be index or rebuild",
                ));
            }
            let base = self.get_base(knowledge_base_id)?;
            let (model_id, inherited_default) =
                if let Some(model_id) = base.configured_embedding_model_id {
                    (model_id, false)
                } else {
                    let default_model = self
                        .connection()?
                        .query_row(
                            "SELECT model_id FROM local_embedding_models
                             WHERE is_default = 1 AND status = 'ready'
                               AND integrity_status = 'verified' AND last_error_code IS NULL
                               AND model_id != ?1
                             ORDER BY updated_at DESC LIMIT 1",
                            [LEGACY_MINILM_MODEL_ID],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?
                        .ok_or_else(|| {
                            LocalKnowledgeError::conflict(
                                "知识库未配置向量模型，也没有可用的默认模型；请先安装并设为默认",
                            )
                        })?;
                    (default_model, true)
                };
            if model_id == LEGACY_MINILM_MODEL_ID {
                return Err(LocalKnowledgeError::conflict(
                    "all-MiniLM-L6-v2 已从受控模型清单移除，请改用 multilingual-e5-small",
                ));
            }
            let (version, dimension, package_path) = self
                .connection()?
                .query_row(
                    "SELECT version, dimension, package_path FROM local_embedding_models
                 WHERE model_id = ?1 AND status = 'ready' AND integrity_status = 'verified'
                   AND last_error_code IS NULL
                 ORDER BY is_default DESC, updated_at DESC LIMIT 1",
                    [&model_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    LocalKnowledgeError::conflict("configured embedding model is not ready")
                })?;
            let package_path = package_path
                .ok_or_else(|| LocalKnowledgeError::invalid("model package path is missing"))?;
            let chunk_count = self.connection()?.query_row(
                "SELECT COUNT(*) FROM local_kb_chunks c
                 JOIN local_kb_documents d ON d.id = c.document_id
                 WHERE d.knowledge_base_id = ?1 AND d.parse_status = 'ready'
                   AND c.document_revision = d.current_revision",
                [knowledge_base_id],
                |row| row.get::<_, i64>(0),
            )?;
            if chunk_count == 0 {
                return Err(LocalKnowledgeError::conflict(
                    "knowledge base has no parsed chunks",
                ));
            }
            if inherited_default {
                self.connection()?.execute(
                    "UPDATE local_knowledge_bases
                     SET embedding_model_id = ?2, updated_at = ?3 WHERE id = ?1",
                    params![knowledge_base_id, model_id, now_ms()],
                )?;
            }
            let sequence = self.connection()?.query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM local_kb_index_generations WHERE knowledge_base_id = ?1",
                [knowledge_base_id],
                |row| row.get::<_, i64>(0),
            )?;
            let generation_id = Uuid::new_v4().to_string();
            let job_id = Uuid::new_v4().to_string();
            let operation_id = Uuid::new_v4().to_string();
            let now = now_ms();
            let checkpoint = json!({
                "modelId": model_id,
                "modelVersion": version,
                "packagePath": package_path,
                "dimension": dimension,
                "generationId": generation_id,
            })
            .to_string();
            let mut connection = self.connection()?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO local_kb_index_generations(
                    id, knowledge_base_id, sequence, status, vector_store_kind,
                    vector_store_version, embedding_model_id, embedding_model_version,
                    dimension, chunk_config_hash, parser_version, chunk_count, created_at
                 ) VALUES (?1, ?2, ?3, 'staging', ?4, ?5, ?6, ?7, ?8, ?9, 'fox-text-v1', 0, ?10)",
                params![
                    generation_id,
                    knowledge_base_id,
                    sequence,
                    ZVEC_STORE_KIND,
                    ZVEC_STORE_VERSION,
                    model_id,
                    version,
                    dimension,
                    format!("chars-{}-overlap-{}", base.chunk_size, base.chunk_overlap),
                    now
                ],
            )?;
            transaction.execute(
                "INSERT INTO local_kb_jobs(
                    id, parent_operation_id, knowledge_base_id, job_type, status,
                    stage, progress, last_sequence, generation_id, checkpoint_json,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'queued', 'validating', 0, 1, ?5, ?6, ?7, ?7)",
                params![
                    job_id,
                    operation_id,
                    knowledge_base_id,
                    job_type,
                    generation_id,
                    checkpoint,
                    now
                ],
            )?;
            transaction.commit()?;
            let store = self.clone();
            let worker_job_id = job_id.clone();
            thread::Builder::new()
                .name(format!("fox-kb-index-{job_id}"))
                .spawn(move || store.run_index_generation(&worker_job_id))
                .map_err(LocalKnowledgeError::storage)?;
            Ok(LocalKnowledgeOperationAccepted {
                operation_id,
                accepted_at: now,
            })
        }
    }

    #[cfg(all(feature = "local-embedding", feature = "zvec"))]
    fn run_index_generation(&self, job_id: &str) {
        if let Err(error) = self.perform_index_generation(job_id) {
            let _ = self.fail_job(job_id, &error);
            if let Ok(job) = self.get_job(job_id) {
                if let Some(generation_id) = job.generation_id {
                    let _ = self.connection().and_then(|connection| {
                        connection
                            .execute(
                                "UPDATE local_kb_index_generations SET status = 'failed',
                            last_error_code = ?2, last_error_message = ?3 WHERE id = ?1",
                                params![generation_id, error.code(), error.to_string()],
                            )
                            .map(|_| ())
                            .map_err(Into::into)
                    });
                }
            }
        }
    }

    pub(crate) fn resume_index_job(
        &self,
        job: &LocalKnowledgeJob,
    ) -> Result<(), LocalKnowledgeError> {
        #[cfg(not(all(feature = "local-embedding", feature = "zvec")))]
        {
            let _ = job;
            return Err(LocalKnowledgeError::conflict(
                "当前桌面构建未同时启用 local-embedding 和 Zvec，无法恢复向量索引任务",
            ));
        }
        #[cfg(all(feature = "local-embedding", feature = "zvec"))]
        {
            let checkpoint: Value =
                serde_json::from_str(job.checkpoint_json.as_deref().unwrap_or("{}"))
                    .map_err(LocalKnowledgeError::storage)?;
            if checkpoint.get("modelId").and_then(Value::as_str) == Some(LEGACY_MINILM_MODEL_ID) {
                return Err(LocalKnowledgeError::conflict(
                    "旧的 all-MiniLM-L6-v2 索引任务不能继续，请选择 multilingual-e5-small 后重建",
                ));
            }
            let model_version = checkpoint
                .get("modelVersion")
                .and_then(Value::as_str)
                .ok_or_else(|| LocalKnowledgeError::invalid("index model version is missing"))?;
            let generation_id = job
                .generation_id
                .as_deref()
                .ok_or_else(|| LocalKnowledgeError::invalid("index job generation is missing"))?;
            self.connection()?.execute(
                "UPDATE local_kb_index_generations
                 SET status = 'staging', last_error_code = NULL, last_error_message = NULL,
                     activated_at = NULL, vector_store_kind = ?3, vector_store_version = ?4,
                     embedding_model_version = ?5
                 WHERE id = ?1 AND knowledge_base_id = ?2",
                params![
                    generation_id,
                    &job.knowledge_base_id,
                    ZVEC_STORE_KIND,
                    ZVEC_STORE_VERSION,
                    model_version
                ],
            )?;
            let store = self.clone();
            let job_id = job.id.clone();
            thread::Builder::new()
                .name(format!("fox-kb-index-resume-{job_id}"))
                .spawn(move || store.run_index_generation(&job_id))
                .map_err(LocalKnowledgeError::storage)?;
            Ok(())
        }
    }

    #[cfg(all(feature = "local-embedding", feature = "zvec"))]
    fn perform_index_generation(&self, job_id: &str) -> Result<(), LocalKnowledgeError> {
        use crate::local_embedding::EmbeddingProvider;
        let job = self.get_job(job_id)?;
        let generation_id = job
            .generation_id
            .clone()
            .ok_or_else(|| LocalKnowledgeError::invalid("index job generation is missing"))?;
        let checkpoint: Value =
            serde_json::from_str(job.checkpoint_json.as_deref().unwrap_or("{}"))
                .map_err(LocalKnowledgeError::storage)?;
        let package_path = checkpoint
            .get("packagePath")
            .and_then(Value::as_str)
            .ok_or_else(|| LocalKnowledgeError::invalid("index model package path is missing"))?;
        let model_id = checkpoint
            .get("modelId")
            .and_then(Value::as_str)
            .ok_or_else(|| LocalKnowledgeError::invalid("index model id is missing"))?;
        let model_version = checkpoint
            .get("modelVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| LocalKnowledgeError::invalid("index model version is missing"))?;
        let expected_dimension = checkpoint
            .get("dimension")
            .and_then(Value::as_i64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| LocalKnowledgeError::invalid("index model dimension is invalid"))?;
        if !self.advance_job(job_id, "running", "embedding", 5)? {
            return Ok(());
        }
        let (mut provider, tokenizer, manifest) = embedding_runtime(Path::new(package_path))?;
        if (!manifest.id.is_empty() && manifest.id != model_id)
            || manifest.version != model_version
            || provider.dimension() != manifest.dimension
            || manifest.dimension != expected_dimension
        {
            return Err(LocalKnowledgeError::invalid(
                "embedding package identity, version, or dimension does not match the index job",
            ));
        }
        let chunks = {
            let connection = self.connection()?;
            let mut statement = connection.prepare(
                "SELECT c.id, c.text FROM local_kb_chunks c
                 JOIN local_kb_documents d ON d.id = c.document_id
                 WHERE d.knowledge_base_id = ?1 AND d.parse_status = 'ready'
                   AND c.document_revision = d.current_revision
                 ORDER BY d.id, c.chunk_index",
            )?;
            let items = statement
                .query_map([&job.knowledge_base_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            items
        };
        let vector_store = self.zvec_vector_store()?;
        vector_store
            .recreate_generation(&job.knowledge_base_id, &generation_id)
            .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
        let total = chunks.len().max(1);
        let mut batch = Vec::with_capacity(32);
        for (index, (chunk_id, text)) in chunks.iter().enumerate() {
            if self.job_is_cancelled(job_id) {
                return Ok(());
            }
            let embedding_text = format!("{}{}", manifest.passage_prefix, text);
            let tokenized = tokenizer.encode(&embedding_text)?;
            let vector = provider
                .embed(&tokenized)
                .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
            if vector.len() != provider.dimension() {
                return Err(LocalKnowledgeError::invalid("embedding dimension mismatch"));
            }
            batch.push(VectorRecord::new(chunk_id.clone(), vector));
            if batch.len() == 32 || index + 1 == chunks.len() {
                vector_store
                    .upsert(&generation_id, &batch)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                batch.clear();
            }
            if index % 8 == 0 || index + 1 == chunks.len() {
                let progress = 5 + (((index + 1) * 75 / total) as i64);
                if !self.advance_job(job_id, "running", "vector_upsert", progress.min(80))? {
                    return Ok(());
                }
            }
        }
        if !self.advance_job(job_id, "running", "verifying", 90)? {
            return Ok(());
        }
        vector_store
            .flush(&generation_id)
            .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
        let check = vector_store
            .verify(&generation_id)
            .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
        if !check.verified
            || check.vector_count != chunks.len()
            || check.dimension != Some(provider.dimension())
        {
            return Err(LocalKnowledgeError::invalid(
                "Zvec generation read/write verification failed",
            ));
        }
        if !self.advance_job(job_id, "running", "committing", 96)? {
            return Ok(());
        }
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE local_kb_index_generations SET status = 'stale', retire_after = ?2
             WHERE knowledge_base_id = ?1 AND status = 'active'",
            params![
                job.knowledge_base_id,
                now.saturating_add(24 * 60 * 60 * 1000)
            ],
        )?;
        transaction.execute(
            "UPDATE local_kb_index_generations SET status = 'active', chunk_count = ?2,
                activated_at = ?3, last_error_code = NULL, last_error_message = NULL WHERE id = ?1",
            params![generation_id, chunks.len() as i64, now],
        )?;
        transaction.execute(
            "UPDATE local_knowledge_bases SET active_index_generation = ?2, updated_at = ?3 WHERE id = ?1",
            params![job.knowledge_base_id, generation_id, now],
        )?;
        transaction.execute(
            "UPDATE local_kb_documents SET index_status = 'ready', embedding_model_id = ?2,
                updated_at = ?3 WHERE knowledge_base_id = ?1 AND parse_status = 'ready'",
            params![job.knowledge_base_id, model_id, now],
        )?;
        transaction.execute(
            "UPDATE local_kb_jobs SET status = 'completed', stage = 'committing', progress = 100,
                outcome = 'success', heartbeat_at = ?2, updated_at = ?2, completed_at = ?2,
                last_sequence = last_sequence + 1 WHERE id = ?1",
            params![job_id, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn search_retrieval(
        &self,
        request: KnowledgeRetrievalRequest,
    ) -> Result<Value, LocalKnowledgeError> {
        let base = self.get_base(&request.knowledge_base_id)?;
        let requested_mode = request.mode.as_deref().unwrap_or(&base.search_mode);
        #[cfg(all(feature = "local-embedding", feature = "zvec"))]
        if requested_mode != "keyword" && !cfg!(test) {
            return run_retrieval_worker_process(self, &request);
        }
        self.search_retrieval_in_process(request)
    }

    pub(crate) fn search_retrieval_in_process(
        &self,
        request: KnowledgeRetrievalRequest,
    ) -> Result<Value, LocalKnowledgeError> {
        let base = self.get_base(&request.knowledge_base_id)?;
        let query = request.query.trim();
        if query.is_empty() || query.chars().count() > 512 {
            return Err(LocalKnowledgeError::invalid(
                "retrieval query must contain 1-512 characters",
            ));
        }
        let requested_mode = request.mode.as_deref().unwrap_or(&base.search_mode);
        if !matches!(requested_mode, "keyword" | "vector" | "hybrid") {
            return Err(LocalKnowledgeError::invalid(
                "retrieval mode must be keyword, vector, or hybrid",
            ));
        }
        if requested_mode != "keyword" && !base.vector_index_ready {
            return Err(LocalKnowledgeError::conflict(
                base.fallback_reason
                    .as_deref()
                    .map(|reason| format!("当前知识库的向量索引不可用：{reason}"))
                    .unwrap_or_else(|| "当前知识库的向量索引尚未就绪，请先重新构建".to_owned()),
            ));
        }
        let limit = request.limit.unwrap_or(8).clamp(1, 20);
        let candidates = self.retrieval_chunks(&request.knowledge_base_id)?;
        let lexical_started = Instant::now();
        let terms = retrieval_terms(query);
        let mut lexical = candidates
            .iter()
            .map(|item| {
                (
                    item.id.clone(),
                    retrieval_lexical_score(&item.content, &terms),
                )
            })
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();
        lexical.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        let lexical_ms = elapsed_ms(lexical_started);

        let vector_started = Instant::now();
        let vector = if requested_mode == "keyword" {
            Vec::new()
        } else {
            self.vector_scores(&base, query, limit.saturating_mul(8).clamp(20, 200))?
        };
        let vector_ms = elapsed_ms(vector_started);
        let lexical_rank = lexical
            .iter()
            .enumerate()
            .map(|(index, (id, score))| (id.clone(), (index + 1, *score)))
            .collect::<HashMap<_, _>>();
        let vector_rank = vector
            .iter()
            .enumerate()
            .map(|(index, (id, score))| (id.clone(), (index + 1, *score)))
            .collect::<HashMap<_, _>>();
        let mut scored = candidates
            .iter()
            .filter_map(|item| {
                let score = match requested_mode {
                    "keyword" => lexical_rank.get(&item.id).map(|(_, score)| *score),
                    "vector" => vector_rank.get(&item.id).map(|(_, score)| *score),
                    _ => {
                        let lexical_value = lexical_rank
                            .get(&item.id)
                            .map(|(rank, _)| 1.0 / (RRF_K + *rank as f64))
                            .unwrap_or(0.0);
                        let vector_value = vector_rank
                            .get(&item.id)
                            .map(|(rank, _)| 1.0 / (RRF_K + *rank as f64))
                            .unwrap_or(0.0);
                        let combined = lexical_value + vector_value;
                        (combined > 0.0).then_some(combined)
                    }
                }?;
                Some((item, score))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.id.cmp(&right.0.id))
        });
        let items = scored
            .into_iter()
            .take(limit)
            .map(|(item, score)| {
                json!({
                    "documentId": item.document_id,
                    "documentName": item.document_name,
                    "relativePath": item.relative_path,
                    "chunkId": item.id,
                    "anchor": item.anchor,
                    "content": item.content,
                    "score": score,
                    "keywordScore": lexical_rank.get(&item.id).map(|(_, value)| *value),
                    "vectorScore": vector_rank.get(&item.id).map(|(_, value)| *value),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "knowledgeBaseId": request.knowledge_base_id,
            "query": query,
            "mode": requested_mode,
            "fusionVersion": if requested_mode == "hybrid" { Some(RRF_VERSION) } else { None },
            "timings": { "keywordMs": lexical_ms, "vectorMs": vector_ms, "totalMs": lexical_ms + vector_ms },
            "items": items,
        }))
    }

    fn retrieval_chunks(
        &self,
        knowledge_base_id: &str,
    ) -> Result<Vec<RetrievalChunk>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT c.id, c.document_id, d.display_name, d.relative_path, c.text, c.anchor
             FROM local_kb_chunks c JOIN local_kb_documents d ON d.id = c.document_id
             WHERE d.knowledge_base_id = ?1 AND d.parse_status = 'ready'
               AND c.document_revision = d.current_revision",
        )?;
        let items = statement
            .query_map([knowledge_base_id], |row| {
                Ok(RetrievalChunk {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    document_name: row.get(2)?,
                    relative_path: row.get(3)?,
                    content: row.get(4)?,
                    anchor: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    fn vector_scores(
        &self,
        base: &crate::local_knowledge::LocalKnowledgeBase,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(String, f64)>, LocalKnowledgeError> {
        #[cfg(not(all(feature = "local-embedding", feature = "zvec")))]
        {
            let _ = (base, query, top_k);
            Err(LocalKnowledgeError::conflict(
                "local embedding runtime or Zvec is unavailable",
            ))
        }
        #[cfg(all(feature = "local-embedding", feature = "zvec"))]
        {
            use crate::local_embedding::EmbeddingProvider;
            let model_id = base
                .embedding_model
                .as_ref()
                .map(|model| model.id.as_str())
                .ok_or_else(|| {
                    LocalKnowledgeError::conflict("active generation has no embedding model")
                })?;
            let generation_id = base.active_index_generation.as_deref().ok_or_else(|| {
                LocalKnowledgeError::conflict("active vector generation is missing")
            })?;
            let (store_kind, model_version, dimension, chunk_count) =
                self.connection()?.query_row(
                    "SELECT vector_store_kind, embedding_model_version, dimension, chunk_count
                 FROM local_kb_index_generations
                 WHERE id = ?1 AND knowledge_base_id = ?2 AND status = 'active'",
                    params![generation_id, base.id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )?;
            if store_kind != ZVEC_STORE_KIND {
                return Err(LocalKnowledgeError::conflict(
                    "当前向量索引不是 Zvec 索引，请重建索引后再检索",
                ));
            }
            let model_version = model_version
                .ok_or_else(|| LocalKnowledgeError::conflict("当前索引缺少模型版本，请重新构建"))?;
            let package_path = self
                .connection()?
                .query_row(
                    "SELECT package_path FROM local_embedding_models
                 WHERE model_id = ?1 AND version = ?2 AND status = 'ready'
                   AND integrity_status = 'verified' AND last_error_code IS NULL
                 ORDER BY updated_at DESC LIMIT 1",
                    params![model_id, model_version],
                    |row| row.get::<_, Option<String>>(0),
                )?
                .ok_or_else(|| {
                    LocalKnowledgeError::conflict(
                        "当前索引使用的模型版本已不可用，请重新安装该版本或重建索引",
                    )
                })?;
            let (mut provider, tokenizer, manifest) = embedding_runtime(Path::new(&package_path))?;
            let query_text = format!("{}{}", manifest.query_prefix, query);
            let query_vector = provider
                .embed(&tokenizer.encode(&query_text)?)
                .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
            let dimension = usize::try_from(dimension)
                .map_err(|_| LocalKnowledgeError::invalid("invalid vector dimension"))?;
            let chunk_count = usize::try_from(chunk_count)
                .map_err(|_| LocalKnowledgeError::invalid("invalid vector count"))?;
            if query_vector.len() != dimension {
                return Err(LocalKnowledgeError::conflict(
                    "查询模型维度与当前 Zvec 索引不一致，请重建索引",
                ));
            }
            let vector_store = self.zvec_vector_store()?;
            vector_store
                .open_generation(&base.id, generation_id, dimension, chunk_count)
                .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
            let hits = vector_store
                .search(
                    generation_id,
                    SearchRequest::new(query_vector, top_k.min(chunk_count.max(1))),
                )
                .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| (hit.id, f64::from(hit.score)))
                .collect())
        }
    }

    pub fn save_retrieval_case(
        &self,
        request: KnowledgeRetrievalCaseSaveRequest,
    ) -> Result<KnowledgeRetrievalCase, LocalKnowledgeError> {
        self.get_base(&request.knowledge_base_id)?;
        let question = request.question.trim();
        if question.is_empty() || question.chars().count() > 512 {
            return Err(LocalKnowledgeError::invalid(
                "test question must contain 1-512 characters",
            ));
        }
        let id = request
            .id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = now_ms();
        self.connection()?.execute(
            "INSERT INTO local_kb_retrieval_cases(id, knowledge_base_id, question,
                expected_document_ids_json, expected_keywords_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET question = excluded.question,
                expected_document_ids_json = excluded.expected_document_ids_json,
                expected_keywords_json = excluded.expected_keywords_json, updated_at = excluded.updated_at",
            params![id, request.knowledge_base_id, question,
                serde_json::to_string(&request.expected_document_ids.unwrap_or_default()).map_err(LocalKnowledgeError::storage)?,
                serde_json::to_string(&request.expected_keywords.unwrap_or_default()).map_err(LocalKnowledgeError::storage)?, now],
        )?;
        self.get_retrieval_case(&id)
    }

    fn get_retrieval_case(&self, id: &str) -> Result<KnowledgeRetrievalCase, LocalKnowledgeError> {
        self.connection()?
            .query_row(
                "SELECT id, knowledge_base_id, question, expected_document_ids_json,
                    expected_keywords_json, created_at, updated_at
             FROM local_kb_retrieval_cases WHERE id = ?1",
                [id],
                map_retrieval_case,
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("retrieval test case"))
    }

    pub fn list_retrieval_cases(
        &self,
        knowledge_base_id: &str,
    ) -> Result<Vec<KnowledgeRetrievalCase>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, knowledge_base_id, question, expected_document_ids_json,
                    expected_keywords_json, created_at, updated_at
             FROM local_kb_retrieval_cases WHERE knowledge_base_id = ?1 ORDER BY updated_at DESC LIMIT 20",
        )?;
        let items = statement
            .query_map([knowledge_base_id], map_retrieval_case)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn delete_retrieval_case(&self, id: &str) -> Result<bool, LocalKnowledgeError> {
        Ok(self
            .connection()?
            .execute("DELETE FROM local_kb_retrieval_cases WHERE id = ?1", [id])?
            > 0)
    }

    pub fn export_retrieval_cases(
        &self,
        knowledge_base_id: &str,
    ) -> Result<Value, LocalKnowledgeError> {
        let cases = self.list_retrieval_cases(knowledge_base_id)?;
        let json_value =
            json!({ "schemaVersion": 1, "knowledgeBaseId": knowledge_base_id, "cases": cases });
        let mut markdown = format!("# 本地知识库检索测试集\n\n知识库：`{knowledge_base_id}`\n\n");
        for (index, case) in cases.iter().enumerate() {
            markdown.push_str(&format!(
                "## {}. {}\n\n",
                index + 1,
                case.question.replace(['\r', '\n'], " ")
            ));
            if !case.expected_keywords.is_empty() {
                markdown.push_str(&format!(
                    "- 期望关键词：{}\n",
                    case.expected_keywords.join("、")
                ));
            }
            if !case.expected_document_ids.is_empty() {
                markdown.push_str(&format!(
                    "- 期望文档 ID：{}\n",
                    case.expected_document_ids.join("、")
                ));
            }
            markdown.push('\n');
        }
        Ok(json!({ "json": json_value, "markdown": markdown }))
    }

    pub fn preview_document_chunks(
        &self,
        request: KnowledgeChunkPreviewRequest,
    ) -> Result<Vec<KnowledgeChunkPreview>, LocalKnowledgeError> {
        let limit = request.limit.unwrap_or(50).clamp(1, 200);
        let offset = request.offset.unwrap_or(0).min(100_000);
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT c.id, c.chunk_index, c.text, c.anchor, c.start_offset, c.end_offset
             FROM local_kb_chunks c JOIN local_kb_documents d ON d.id = c.document_id
             WHERE d.knowledge_base_id = ?1 AND d.id = ?2 AND c.document_revision = d.current_revision
             ORDER BY c.chunk_index LIMIT ?3 OFFSET ?4",
        )?;
        let items = statement
            .query_map(
                params![
                    request.knowledge_base_id,
                    request.document_id,
                    limit as i64,
                    offset as i64
                ],
                |row| {
                    Ok(KnowledgeChunkPreview {
                        id: row.get(0)?,
                        chunk_index: row.get(1)?,
                        content: row.get(2)?,
                        anchor: row.get(3)?,
                        start_offset: row.get(4)?,
                        end_offset: row.get(5)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn delete_document(
        &self,
        request: KnowledgeDocumentActionRequest,
    ) -> Result<bool, LocalKnowledgeError> {
        let path = self
            .root
            .join("knowledge-bases")
            .join(&request.knowledge_base_id)
            .join("documents");
        let relative_path = self.connection()?.query_row(
            "SELECT relative_path FROM local_kb_documents WHERE knowledge_base_id = ?1 AND id = ?2",
            params![request.knowledge_base_id, request.document_id], |row| row.get::<_, String>(0),
        ).optional()?.ok_or_else(|| LocalKnowledgeError::not_found("knowledge document"))?;
        let source = path.join(&relative_path);
        let staged = self
            .root
            .join("staging")
            .join(format!("document-delete-{}", Uuid::new_v4()));
        let moved = if source.exists() {
            fs::rename(&source, &staged)?;
            true
        } else {
            false
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let deleted = transaction.execute(
            "DELETE FROM local_kb_documents WHERE knowledge_base_id = ?1 AND id = ?2",
            params![request.knowledge_base_id, request.document_id],
        );
        match deleted {
            Ok(count) => {
                let completed_at = now_ms();
                transaction.execute("UPDATE local_kb_index_generations SET status = 'stale' WHERE knowledge_base_id = ?1 AND status = 'active'", [&request.knowledge_base_id])?;
                transaction.execute("UPDATE local_knowledge_bases SET active_index_generation = NULL, updated_at = ?2 WHERE id = ?1", params![request.knowledge_base_id, completed_at])?;
                if count > 0 {
                    let job_id = Uuid::new_v4().to_string();
                    let operation_id = Uuid::new_v4().to_string();
                    let data = json!({
                        "documentId": request.document_id,
                        "relativePath": relative_path,
                    })
                    .to_string();
                    transaction.execute(
                        "INSERT INTO local_kb_jobs(
                            id, parent_operation_id, knowledge_base_id, job_type, status,
                            stage, progress, last_sequence, outcome, data_json,
                            heartbeat_at, created_at, started_at, updated_at, completed_at
                         ) VALUES (?1, ?2, ?3, 'delete', 'completed', 'committing', 100, 2,
                                   'success', ?4, ?5, ?5, ?5, ?5, ?5)",
                        params![
                            job_id,
                            operation_id,
                            request.knowledge_base_id,
                            data,
                            completed_at
                        ],
                    )?;
                }
                transaction.commit()?;
                if moved {
                    let _ = fs::remove_file(staged);
                }
                Ok(count > 0)
            }
            Err(error) => {
                drop(transaction);
                if moved {
                    let _ = fs::rename(staged, source);
                }
                Err(error.into())
            }
        }
    }

    pub fn start_reparse_document(
        &self,
        request: KnowledgeDocumentActionRequest,
    ) -> Result<LocalKnowledgeOperationAccepted, LocalKnowledgeError> {
        self.get_base(&request.knowledge_base_id)?;
        let exists = self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_kb_documents WHERE knowledge_base_id = ?1 AND id = ?2)",
            params![request.knowledge_base_id, request.document_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(LocalKnowledgeError::not_found("knowledge document"));
        }
        let job_id = Uuid::new_v4().to_string();
        let operation_id = Uuid::new_v4().to_string();
        let now = now_ms();
        let checkpoint = serde_json::to_string(&request).map_err(LocalKnowledgeError::storage)?;
        self.connection()?.execute(
            "INSERT INTO local_kb_jobs(
                id, parent_operation_id, knowledge_base_id, job_type, status, stage,
                progress, last_sequence, checkpoint_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'parse', 'queued', 'validating', 0, 1, ?4, ?5, ?5)",
            params![
                job_id,
                operation_id,
                request.knowledge_base_id,
                checkpoint,
                now
            ],
        )?;
        let store = self.clone();
        let worker_job_id = job_id.clone();
        thread::Builder::new()
            .name(format!("fox-kb-reparse-{job_id}"))
            .spawn(move || store.run_reparse_document(&worker_job_id, request))
            .map_err(LocalKnowledgeError::storage)?;
        Ok(LocalKnowledgeOperationAccepted {
            operation_id,
            accepted_at: now,
        })
    }

    fn run_reparse_document(&self, job_id: &str, request: KnowledgeDocumentActionRequest) {
        if let Err(error) = self.perform_reparse_document(job_id, &request) {
            let _ = self.fail_job(job_id, &error);
        }
    }

    pub(crate) fn resume_reparse_job(
        &self,
        job: &LocalKnowledgeJob,
    ) -> Result<(), LocalKnowledgeError> {
        let request: KnowledgeDocumentActionRequest = serde_json::from_str(
            job.checkpoint_json
                .as_deref()
                .ok_or_else(|| LocalKnowledgeError::invalid("reparse checkpoint is missing"))?,
        )
        .map_err(LocalKnowledgeError::storage)?;
        let store = self.clone();
        let job_id = job.id.clone();
        thread::Builder::new()
            .name(format!("fox-kb-reparse-retry-{job_id}"))
            .spawn(move || store.run_reparse_document(&job_id, request))
            .map_err(LocalKnowledgeError::storage)?;
        Ok(())
    }

    fn perform_reparse_document(
        &self,
        job_id: &str,
        request: &KnowledgeDocumentActionRequest,
    ) -> Result<(), LocalKnowledgeError> {
        if !self.advance_job(job_id, "running", "parsing", 20)? {
            return Ok(());
        }
        let document = self.connection()?.query_row(
            "SELECT display_name, relative_path, content_hash, file_size, mime_type, current_revision
             FROM local_kb_documents WHERE knowledge_base_id = ?1 AND id = ?2",
            params![request.knowledge_base_id, request.document_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, String>(4)?, row.get::<_, i64>(5)?)),
        ).optional()?.ok_or_else(|| LocalKnowledgeError::not_found("knowledge document"))?;
        let stored_path = self
            .root
            .join("knowledge-bases")
            .join(&request.knowledge_base_id)
            .join("documents")
            .join(&document.1);
        let imported = ImportedDocument {
            display_name: document.0,
            relative_path: document.1,
            content_hash: document.2,
            file_size: document.3.max(0) as u64,
            mime_type: document.4,
            stored_path,
        };
        let parsed = parse_imported_document(&imported, DEFAULT_CHUNK_MAX_BYTES)
            .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
        if !self.advance_job(job_id, "running", "chunking", 70)? {
            return Ok(());
        }
        let chunks = match parsed {
            ParsedDocument::Text { chunks, .. } => chunks,
            ParsedDocument::PendingSpecializedParser { .. } => {
                return Err(LocalKnowledgeError::conflict(
                    "document parser is not available",
                ))
            }
        };
        if !self.advance_job(job_id, "running", "committing", 90)? {
            return Ok(());
        }
        let next_revision = document.5.saturating_add(1);
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM local_kb_chunks WHERE document_id = ?1",
            [&request.document_id],
        )?;
        for chunk in chunks {
            transaction.execute(
                "INSERT INTO local_kb_chunks(
                    id, document_id, document_revision, chunk_index, text, anchor,
                    start_offset, end_offset, metadata_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '{}')",
                params![
                    Uuid::new_v4().to_string(),
                    request.document_id,
                    next_revision,
                    chunk.chunk_index as i64,
                    chunk.text,
                    chunk.anchor,
                    chunk.start_offset as i64,
                    chunk.end_offset as i64
                ],
            )?;
        }
        transaction.execute(
            "UPDATE local_kb_documents SET current_revision = ?3, parse_status = 'ready',
                index_status = 'pending', last_error_code = NULL, last_error_message = NULL,
                updated_at = ?4 WHERE knowledge_base_id = ?1 AND id = ?2",
            params![
                request.knowledge_base_id,
                request.document_id,
                next_revision,
                now
            ],
        )?;
        transaction.execute(
            "UPDATE local_kb_jobs SET status = 'completed', progress = 100, outcome = 'success',
                updated_at = ?2, completed_at = ?2, heartbeat_at = ?2,
                last_sequence = last_sequence + 1 WHERE id = ?1",
            params![job_id, now],
        )?;
        transaction.commit()?;
        drop(connection);
        if self
            .get_base(&request.knowledge_base_id)?
            .configured_embedding_model_id
            .is_some()
        {
            let _ = self.start_index_generation(&request.knowledge_base_id, "rebuild");
        }
        Ok(())
    }
}

#[derive(Debug)]
struct RetrievalChunk {
    id: String,
    document_id: String,
    document_name: String,
    relative_path: String,
    content: String,
    anchor: Option<String>,
}

fn retrieval_terms(query: &str) -> Vec<String> {
    let mut terms = query
        .split_whitespace()
        .map(|value| value.to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        terms.push(query.to_lowercase());
    }
    terms
}

fn retrieval_lexical_score(text: &str, terms: &[String]) -> f64 {
    let text = text.to_lowercase();
    if terms.is_empty() {
        return 0.0;
    }
    let matched = terms
        .iter()
        .filter(|term| text.contains(term.as_str()))
        .count();
    let occurrences = terms
        .iter()
        .map(|term| text.match_indices(term).count().min(3))
        .sum::<usize>();
    ((matched as f64 / terms.len() as f64) * 0.7
        + (occurrences as f64 / (terms.len() * 3) as f64) * 0.3)
        .min(1.0)
}

#[cfg(test)]
fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (left, right) in left.iter().zip(right) {
        let left = *left as f64;
        let right = *right as f64;
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    (denominator > f64::EPSILON).then_some(dot / denominator)
}

fn elapsed_ms(started: Instant) -> i64 {
    started.elapsed().as_millis().min(i64::MAX as u128) as i64
}

fn map_retrieval_case(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeRetrievalCase> {
    let document_ids: String = row.get(3)?;
    let keywords: String = row.get(4)?;
    Ok(KnowledgeRetrievalCase {
        id: row.get(0)?,
        knowledge_base_id: row.get(1)?,
        question: row.get(2)?,
        expected_document_ids: serde_json::from_str(&document_ids).unwrap_or_default(),
        expected_keywords: serde_json::from_str(&keywords).unwrap_or_default(),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VectorBackendHealth {
    pub backend: String,
    pub available: bool,
    pub read_write_verified: bool,
    pub fallback_active: bool,
    pub message: String,
}

impl LocalKnowledgeStore {
    pub fn vector_backend_health(&self) -> Result<VectorBackendHealth, LocalKnowledgeError> {
        #[cfg(feature = "zvec")]
        {
            use crate::vector_store::{SearchRequest, VectorRecord, VectorStore, ZvecVectorStore};
            let root = self
                .root
                .join("staging")
                .join(format!("zvec-health-{}", Uuid::new_v4()));
            let generation = format!("health-{}", Uuid::new_v4());
            let result = (|| {
                let store = match self.zvec_resource_dir.as_ref().as_ref() {
                    Some(resource_dir) => ZvecVectorStore::from_resource_dir(&root, resource_dir),
                    None => ZvecVectorStore::new(&root),
                }
                .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                store
                    .create_generation("health", &generation)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                store
                    .upsert(&generation, &[VectorRecord::new("probe", vec![1.0, 0.0])])
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                store
                    .flush(&generation)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                let check = store
                    .verify(&generation)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                let hits = store
                    .search(&generation, SearchRequest::new(vec![1.0, 0.0], 1))
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                store
                    .delete_generation(&generation)
                    .map_err(|error| LocalKnowledgeError::storage(error.to_string()))?;
                Ok::<bool, LocalKnowledgeError>(
                    check.verified
                        && check.vector_count == 1
                        && hits.first().is_some_and(|hit| hit.id == "probe"),
                )
            })();
            let _ = fs::remove_dir_all(&root);
            return match result {
                Ok(true) => Ok(VectorBackendHealth {
                    backend: "zvec".to_owned(),
                    available: true,
                    read_write_verified: true,
                    fallback_active: false,
                    message: "Zvec 创建、写入、刷新、校验和检索均通过".to_owned(),
                }),
                Ok(false) => Ok(VectorBackendHealth {
                    backend: "zvec".to_owned(),
                    available: false,
                    read_write_verified: false,
                    fallback_active: true,
                    message: "Zvec 返回了不完整的健康检查结果；向量检索已停用，关键词检索仍可用"
                        .to_owned(),
                }),
                Err(error) => Ok(VectorBackendHealth {
                    backend: "zvec".to_owned(),
                    available: false,
                    read_write_verified: false,
                    fallback_active: true,
                    message: format!("Zvec 不可用：{}；向量检索已停用，关键词检索仍可用", error),
                }),
            };
        }
        #[cfg(not(feature = "zvec"))]
        Ok(VectorBackendHealth {
            backend: "zvec".to_owned(),
            available: false,
            read_write_verified: false,
            fallback_active: true,
            message: "当前构建未启用 Zvec；向量检索不可用，关键词检索仍可用".to_owned(),
        })
    }
}

#[tauri::command]
pub fn local_embedding_models_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<EmbeddingModelView>> {
    model_response(state.local_knowledge.list_embedding_models())
}

#[tauri::command]
pub fn local_embedding_model_install_start(
    state: State<'_, AppState>,
    request: EmbeddingModelIdRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    model_response(
        state
            .local_knowledge
            .start_embedding_model_install(&request.model_id),
    )
}

#[tauri::command]
pub fn local_embedding_model_download_cancel(
    state: State<'_, AppState>,
    request: EmbeddingDownloadIdRequest,
) -> ApiResponse<bool> {
    model_response(state.local_knowledge.cancel_embedding_download(&request.id))
}

#[tauri::command]
pub fn local_embedding_model_download_retry(
    state: State<'_, AppState>,
    request: EmbeddingDownloadIdRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    model_response(state.local_knowledge.retry_embedding_download(&request.id))
}

#[tauri::command]
pub async fn local_embedding_model_test(
    app: tauri::AppHandle,
    request: EmbeddingModelIdRequest,
) -> Result<ApiResponse<EmbeddingModelTestResult>, String> {
    let store = app.state::<AppState>().local_knowledge.clone();
    let model_id = request.model_id;
    let result =
        tauri::async_runtime::spawn_blocking(move || store.test_embedding_model(&model_id))
            .await
            .map_err(|error| LocalKnowledgeError::storage(format!("模型测试任务异常结束：{error}")))
            .and_then(|result| result);
    Ok(model_response(result))
}

#[tauri::command]
pub fn local_embedding_model_set_default(
    state: State<'_, AppState>,
    request: EmbeddingModelIdRequest,
) -> ApiResponse<bool> {
    model_response(
        state
            .local_knowledge
            .set_default_embedding_model(&request.model_id),
    )
}

#[tauri::command]
pub fn local_embedding_model_delete(
    state: State<'_, AppState>,
    request: EmbeddingModelIdRequest,
) -> ApiResponse<bool> {
    model_response(
        state
            .local_knowledge
            .delete_embedding_model(&request.model_id),
    )
}

#[tauri::command]
pub fn local_embedding_model_import(
    state: State<'_, AppState>,
    request: EmbeddingModelImportRequest,
) -> ApiResponse<EmbeddingModelView> {
    model_response(
        state
            .local_knowledge
            .import_embedding_model_package(&request.package_path),
    )
}

#[tauri::command]
pub fn local_vector_backend_health(state: State<'_, AppState>) -> ApiResponse<VectorBackendHealth> {
    model_response(state.local_knowledge.vector_backend_health())
}

#[tauri::command]
pub fn local_knowledge_index_start(
    state: State<'_, AppState>,
    request: KnowledgeIndexStartRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    model_response(state.local_knowledge.start_index_generation(
        &request.knowledge_base_id,
        if request.rebuild.unwrap_or(false) {
            "rebuild"
        } else {
            "index"
        },
    ))
}

#[tauri::command]
pub fn local_knowledge_retrieval_test(
    state: State<'_, AppState>,
    request: KnowledgeRetrievalRequest,
) -> ApiResponse<Value> {
    model_response(state.local_knowledge.search_retrieval(request))
}

#[tauri::command]
pub fn local_knowledge_retrieval_cases_list(
    state: State<'_, AppState>,
    request: KnowledgeRetrievalCasesRequest,
) -> ApiResponse<Vec<KnowledgeRetrievalCase>> {
    model_response(
        state
            .local_knowledge
            .list_retrieval_cases(&request.knowledge_base_id),
    )
}

#[tauri::command]
pub fn local_knowledge_retrieval_case_save(
    state: State<'_, AppState>,
    request: KnowledgeRetrievalCaseSaveRequest,
) -> ApiResponse<KnowledgeRetrievalCase> {
    model_response(state.local_knowledge.save_retrieval_case(request))
}

#[tauri::command]
pub fn local_knowledge_retrieval_case_delete(
    state: State<'_, AppState>,
    request: KnowledgeRetrievalCaseIdRequest,
) -> ApiResponse<bool> {
    model_response(state.local_knowledge.delete_retrieval_case(&request.id))
}

#[tauri::command]
pub fn local_knowledge_retrieval_cases_export(
    state: State<'_, AppState>,
    request: KnowledgeRetrievalCasesRequest,
) -> ApiResponse<Value> {
    model_response(
        state
            .local_knowledge
            .export_retrieval_cases(&request.knowledge_base_id),
    )
}

#[tauri::command]
pub fn local_knowledge_document_chunks(
    state: State<'_, AppState>,
    request: KnowledgeChunkPreviewRequest,
) -> ApiResponse<Vec<KnowledgeChunkPreview>> {
    model_response(state.local_knowledge.preview_document_chunks(request))
}

#[tauri::command]
pub fn local_knowledge_document_delete(
    state: State<'_, AppState>,
    request: KnowledgeDocumentActionRequest,
) -> ApiResponse<bool> {
    model_response(state.local_knowledge.delete_document(request))
}

#[tauri::command]
pub fn local_knowledge_document_reparse(
    state: State<'_, AppState>,
    request: KnowledgeDocumentActionRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    model_response(state.local_knowledge.start_reparse_document(request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "local-embedding")]
    #[test]
    fn embedding_smoke_diagnostics_are_read_and_bounded() {
        let details = collect_embedding_smoke_stderr("runtime failure".as_bytes());
        assert_eq!(details, "runtime failure");

        let oversized = vec![b'x'; 80_000];
        let details = collect_embedding_smoke_stderr(oversized.as_slice());
        assert!(details.contains("诊断信息超过 16000 字符，已截断"));
        assert!(details.chars().count() < 17_000);
    }

    #[test]
    fn controlled_manifests_have_pinned_integrity_metadata() {
        let manifests = recommended_manifests();
        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests[0].id, RECOMMENDED_MODEL_ID);
        assert_eq!(manifests[0].dimension, 512);
        assert_eq!(manifests[1].id, MULTILINGUAL_E5_MODEL_ID);
        assert_eq!(manifests[1].dimension, 384);
        assert_eq!(manifests[1].languages, vec!["中文", "英文"]);
        assert_eq!(manifests[1].query_prefix, "query: ");
        assert_eq!(manifests[1].passage_prefix, "passage: ");
        assert_eq!(
            manifests[1]
                .files
                .iter()
                .map(|file| file.size_bytes)
                .sum::<i64>(),
            135_137_323
        );
        for manifest in manifests {
            assert!(matches!(manifest.files.len(), 2 | 3));
            assert!(manifest
                .files
                .iter()
                .all(|file| normalize_sha256(&file.sha256).is_ok()));
            assert!(manifest
                .files
                .iter()
                .all(|file| file.url.contains(&manifest.version)));
        }
    }

    #[test]
    fn model_catalog_exposes_both_controlled_recommendations() {
        let root = std::env::temp_dir().join(format!(
            "fox-local-knowledge-model-catalog-{}",
            Uuid::new_v4().simple()
        ));
        let store = LocalKnowledgeStore::open(&root).unwrap();
        let models = store.list_embedding_models().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, RECOMMENDED_MODEL_ID);
        assert_eq!(models[1].id, MULTILINGUAL_E5_MODEL_ID);
        assert!(models.iter().all(|model| model.recommended));
        assert!(models.iter().all(|model| model.status == "not_installed"));
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_minilm_default_and_sqlite_vector_generation_are_not_reused() {
        let root = std::env::temp_dir().join(format!(
            "fox-local-knowledge-legacy-vector-{}",
            Uuid::new_v4().simple()
        ));
        let store = LocalKnowledgeStore::open(&root).unwrap();
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO local_embedding_models(
                    model_id, name, version, dimension, status, languages_json, license,
                    source_url, integrity_status, size_bytes, updated_at, is_default
                 ) VALUES (?1, 'Legacy MiniLM', 'legacy', 384, 'ready', '[\"英文\"]',
                           'Apache-2.0', '', 'verified', 1, ?2, 1)",
                params![LEGACY_MINILM_MODEL_ID, now_ms()],
            )
            .unwrap();
        drop(store);

        let store = LocalKnowledgeStore::open(&root).unwrap();
        let is_default = store
            .connection()
            .unwrap()
            .query_row(
                "SELECT is_default FROM local_embedding_models WHERE model_id = ?1",
                [LEGACY_MINILM_MODEL_ID],
                |row| row.get::<_, bool>(0),
            )
            .unwrap();
        assert!(!is_default);

        let base = store
            .create_base(crate::local_knowledge::CreateLocalKnowledgeBaseRequest {
                name: "旧向量代次".to_owned(),
                description: None,
                embedding_model_id: Some(LEGACY_MINILM_MODEL_ID.to_owned()),
                chunk_size: None,
                chunk_overlap: None,
                search_mode: Some("hybrid".to_owned()),
            })
            .unwrap();
        let generation_id = Uuid::new_v4().to_string();
        let now = now_ms();
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO local_kb_index_generations(
                    id, knowledge_base_id, sequence, status, vector_store_kind,
                    vector_store_version, embedding_model_id, dimension, chunk_config_hash,
                    parser_version, chunk_count, created_at, activated_at
                 ) VALUES (?1, ?2, 1, 'active', 'sqlite-vector-v1', '1', ?3, 384,
                           'chars-512-overlap-50', 'fox-text-v1', 1, ?4, ?4)",
                params![generation_id, base.id, LEGACY_MINILM_MODEL_ID, now],
            )
            .unwrap();
        store
            .connection()
            .unwrap()
            .execute(
                "UPDATE local_knowledge_bases SET active_index_generation = ?2 WHERE id = ?1",
                params![base.id, generation_id],
            )
            .unwrap();
        let migrated = store.get_base(&base.id).unwrap();
        assert!(!migrated.vector_index_ready);
        assert_eq!(migrated.vector_count, None);
        assert!(migrated
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Zvec")));

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    // This drives the embedding index-resolution path (`start_index_generation`),
    // whose real implementation exists ONLY when both local-embedding and zvec
    // are enabled (the cfg(all(...)) branch); production builds enable both.
    // Under any other feature combination the method takes a different stub
    // path, so the "no parsed chunks" assertion does not hold. Gate on BOTH
    // features rather than ignoring the test or asserting an incompatible error.
    #[cfg(all(feature = "local-embedding", feature = "zvec"))]
    #[test]
    fn unbound_knowledge_base_resolves_the_ready_default_model() {
        let root = std::env::temp_dir().join(format!(
            "fox-local-knowledge-default-model-{}",
            Uuid::new_v4().simple()
        ));
        let store = LocalKnowledgeStore::open(&root).unwrap();
        let knowledge_base = store
            .create_base(crate::local_knowledge::CreateLocalKnowledgeBaseRequest {
                name: "默认向量模型测试".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        let manifest = recommended_manifest(RECOMMENDED_MODEL_ID).unwrap();
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO local_embedding_models(
                    model_id, name, version, dimension, status, languages_json, license,
                    source_url, package_path, integrity_status, package_hash, size_bytes,
                    installed_at, updated_at, is_default
                 ) VALUES (?1, ?2, ?3, ?4, 'ready', '[\"中文\"]', ?5, ?6, ?7,
                           'verified', ?8, 1, ?9, ?9, 1)",
                params![
                    manifest.id,
                    manifest.name,
                    manifest.version,
                    manifest.dimension as i64,
                    manifest.license,
                    recommended_source_url(RECOMMENDED_MODEL_ID),
                    root.join("models").to_string_lossy(),
                    manifest.hash,
                    now_ms(),
                ],
            )
            .unwrap();

        let error = store
            .start_index_generation(&knowledge_base.id, "index")
            .unwrap_err();
        assert!(error.to_string().contains("no parsed chunks"));
        assert!(store
            .get_base(&knowledge_base.id)
            .unwrap()
            .configured_embedding_model_id
            .is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cosine_similarity_rejects_invalid_dimensions() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert!(cosine_similarity(&[1.0], &[1.0, 0.0]).is_none());
    }

    #[test]
    fn deleting_a_document_creates_a_completed_delete_job() {
        let root = std::env::temp_dir().join(format!(
            "fox-local-knowledge-delete-job-{}",
            Uuid::new_v4().simple()
        ));
        let store = LocalKnowledgeStore::open(&root).unwrap();
        let knowledge_base = store
            .create_base(crate::local_knowledge::CreateLocalKnowledgeBaseRequest {
                name: "删除任务测试".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        let document_id = Uuid::new_v4().to_string();
        let relative_path = "sample.txt";
        let documents = root
            .join("knowledge-bases")
            .join(&knowledge_base.id)
            .join("documents");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::write(documents.join(relative_path), b"sample").unwrap();
        let now = now_ms();
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO local_kb_documents(
                    id, knowledge_base_id, display_name, relative_path, source_path,
                    current_revision, content_hash, file_size, mime_type, parse_status,
                    index_status, parser_version, chunk_config_hash, created_at, updated_at
                 ) VALUES (?1, ?2, 'sample.txt', ?3, ?3, 1, 'hash', 6, 'text/plain',
                           'ready', 'pending', 'fox-text-v1', 'utf8-4096-v1', ?4, ?4)",
                params![document_id, knowledge_base.id, relative_path, now],
            )
            .unwrap();

        assert!(store
            .delete_document(KnowledgeDocumentActionRequest {
                knowledge_base_id: knowledge_base.id.clone(),
                document_id,
            })
            .unwrap());
        assert!(!documents.join(relative_path).exists());
        let jobs = store.list_jobs(Some(&knowledge_base.id)).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_type, "delete");
        assert_eq!(jobs[0].status, "completed");
        assert_eq!(jobs[0].stage.as_deref(), Some("committing"));
        assert_eq!(jobs[0].outcome.as_deref(), Some("success"));

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(all(feature = "zvec", target_os = "windows"))]
    #[test]
    fn local_knowledge_health_runs_the_real_zvec_probe() {
        let root = std::env::temp_dir().join(format!(
            "fox-local-knowledge-zvec-health-{}",
            Uuid::new_v4().simple()
        ));
        let store = LocalKnowledgeStore::open(&root).unwrap();
        let health = store.vector_backend_health().unwrap();
        assert_eq!(health.backend, "zvec");
        assert!(health.available, "{}", health.message);
        assert!(health.read_write_verified, "{}", health.message);
        assert!(!health.fallback_active, "{}", health.message);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }
}

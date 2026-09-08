use crate::tool_host::ToolPreview;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use quick_xml::{events::Event, Reader, XmlVersion};
use reqwest::{blocking::Client, header, redirect::Policy, Method, StatusCode};
use rusqlite::{params_from_iter, types::ValueRef, Connection, OpenFlags};
use serde_json::{json, Map, Value};
use std::{
    env, fs,
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Disks, System};
use url::Url;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const CAPABILITY_TOOLS: &[&str] = &[
    "web_search",
    "web_read",
    "http_request",
    "system_info",
    "sqlite_read",
    "structured_data",
    "git_read",
    "test_run",
    "code_check",
    "format_code",
    "tabular_data",
];

const MAX_NETWORK_BYTES: usize = 2 * 1024 * 1024;
const MAX_HTTP_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_HTTP_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_SQL_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SQL_CELL_BYTES: usize = 256 * 1024;
const MAX_SQL_BYTES: usize = 64 * 1024;
const MAX_INLINE_DATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROCESS_OUTPUT_BYTES: usize = 512 * 1024;
const DEFAULT_PROCESS_SECONDS: u64 = 120;
const MAX_PROCESS_SECONDS: u64 = 600;
const DEFAULT_WEB_TEXT_CHARS: usize = 120_000;

pub fn is_capability_tool(tool: &str) -> bool {
    CAPABILITY_TOOLS.contains(&tool)
}

#[derive(Debug, Clone)]
pub enum PreparedCapabilityTool {
    WebSearch {
        query: String,
        provider: SearchProvider,
        max_results: usize,
        include_domains: Vec<String>,
        exclude_domains: Vec<String>,
        preview: ToolPreview,
    },
    WebRead {
        url: Url,
        format: WebReadFormat,
        max_chars: usize,
        preview: ToolPreview,
    },
    HttpRequest {
        method: Method,
        url: Url,
        allowed_hosts: Vec<String>,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        timeout: Duration,
        preview: ToolPreview,
    },
    SystemInfo {
        include_processes: bool,
        max_processes: usize,
        include_environment: bool,
        preview: ToolPreview,
    },
    SqliteRead {
        path: PathBuf,
        query: String,
        parameters: Vec<rusqlite::types::Value>,
        row_limit: usize,
        timeout: Duration,
        preview: ToolPreview,
    },
    StructuredData {
        action: StructuredAction,
        input: DataInput,
        input_format: DataFormat,
        output_format: DataFormat,
        query: Option<String>,
        required_keys: Vec<String>,
        preview: ToolPreview,
    },
    GitRead {
        root: PathBuf,
        operation: GitOperation,
        path: Option<PathBuf>,
        reference: Option<String>,
        base: Option<String>,
        staged: bool,
        max_entries: usize,
        start_line: Option<usize>,
        end_line: Option<usize>,
        preview: ToolPreview,
    },
    Process {
        tool: String,
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        timeout: Duration,
        preview: ToolPreview,
    },
    TabularData {
        input: DataInput,
        format: DataFormat,
        operation: TableOperation,
        offset: usize,
        limit: usize,
        filter_column: Option<String>,
        filter_value: Option<String>,
        filter_mode: FilterMode,
        aggregate: AggregateOperation,
        aggregate_column: Option<String>,
        preview: ToolPreview,
    },
}

impl PreparedCapabilityTool {
    pub fn preview(&self) -> &ToolPreview {
        match self {
            Self::WebSearch { preview, .. }
            | Self::WebRead { preview, .. }
            | Self::HttpRequest { preview, .. }
            | Self::SystemInfo { preview, .. }
            | Self::SqliteRead { preview, .. }
            | Self::StructuredData { preview, .. }
            | Self::GitRead { preview, .. }
            | Self::Process { preview, .. }
            | Self::TabularData { preview, .. } => preview,
        }
    }

    pub fn requires_approval(&self) -> bool {
        matches!(
            self,
            Self::WebSearch { .. }
                | Self::WebRead { .. }
                | Self::HttpRequest { .. }
                | Self::SystemInfo { .. }
                | Self::SqliteRead { .. }
                | Self::Process { .. }
        )
    }

    pub fn blocked_in_read_only(&self) -> bool {
        matches!(self, Self::Process { .. })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum SearchProvider {
    Auto,
    Brave,
    Tavily,
    Exa,
    Searxng,
    DuckDuckGo,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum WebReadFormat {
    Markdown,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DataFormat {
    Auto,
    Json,
    Yaml,
    Toml,
    Xml,
    Csv,
    Tsv,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum StructuredAction {
    Format,
    Convert,
    Query,
    Validate,
}

#[derive(Debug, Clone)]
pub(super) enum DataInput {
    Inline(String),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub(super) enum GitOperation {
    Status,
    Diff,
    Log,
    Blame,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TableOperation {
    Preview,
    Filter,
    Aggregate,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum FilterMode {
    Equals,
    Contains,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum AggregateOperation {
    Count,
    Sum,
    Average,
    Min,
    Max,
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    duration_ms: u128,
    truncated: bool,
}

pub fn prepare(
    tool: &str,
    input: &Value,
    project_root: Option<&str>,
) -> Result<PreparedCapabilityTool, String> {
    match tool {
        "web_search" => prepare_web_search(input),
        "web_read" => prepare_web_read(input),
        "http_request" => prepare_http_request(input),
        "system_info" => prepare_system_info(input),
        "sqlite_read" => prepare_sqlite_read(input, required_project_root(project_root)?),
        "structured_data" => prepare_structured_data(input, project_root),
        "git_read" => prepare_git_read(input, required_project_root(project_root)?),
        "test_run" => prepare_test_run(input, required_project_root(project_root)?),
        "code_check" => prepare_code_check(input, required_project_root(project_root)?),
        "format_code" => prepare_format_code(input, required_project_root(project_root)?),
        "tabular_data" => prepare_tabular_data(input, project_root),
        _ => Err(format!("unsupported capability tool: {tool}")),
    }
}

pub fn execute(action: PreparedCapabilityTool) -> Result<Value, String> {
    match action {
        PreparedCapabilityTool::WebSearch {
            query,
            provider,
            max_results,
            include_domains,
            exclude_domains,
            ..
        } => execute_web_search(
            &query,
            provider,
            max_results,
            &include_domains,
            &exclude_domains,
        ),
        PreparedCapabilityTool::WebRead {
            url,
            format,
            max_chars,
            ..
        } => execute_web_read(url, format, max_chars),
        PreparedCapabilityTool::HttpRequest {
            method,
            url,
            allowed_hosts,
            headers,
            body,
            timeout,
            ..
        } => execute_http_request(
            method,
            url,
            &allowed_hosts,
            &headers,
            body.as_deref(),
            timeout,
        ),
        PreparedCapabilityTool::SystemInfo {
            include_processes,
            max_processes,
            include_environment,
            ..
        } => execute_system_info(include_processes, max_processes, include_environment),
        PreparedCapabilityTool::SqliteRead {
            path,
            query,
            parameters,
            row_limit,
            timeout,
            ..
        } => execute_sqlite_read(&path, &query, &parameters, row_limit, timeout),
        PreparedCapabilityTool::StructuredData {
            action,
            input,
            input_format,
            output_format,
            query,
            required_keys,
            ..
        } => execute_structured_data(
            action,
            input,
            input_format,
            output_format,
            query.as_deref(),
            &required_keys,
        ),
        PreparedCapabilityTool::GitRead {
            root,
            operation,
            path,
            reference,
            base,
            staged,
            max_entries,
            start_line,
            end_line,
            ..
        } => execute_git_read(
            &root,
            operation,
            path.as_deref(),
            reference.as_deref(),
            base.as_deref(),
            staged,
            max_entries,
            start_line,
            end_line,
        ),
        PreparedCapabilityTool::Process {
            tool,
            program,
            args,
            cwd,
            timeout,
            ..
        } => execute_process_tool(&tool, &program, &args, &cwd, timeout),
        PreparedCapabilityTool::TabularData {
            input,
            format,
            operation,
            offset,
            limit,
            filter_column,
            filter_value,
            filter_mode,
            aggregate,
            aggregate_column,
            ..
        } => execute_tabular_data(
            input,
            format,
            operation,
            offset,
            limit,
            filter_column.as_deref(),
            filter_value.as_deref(),
            filter_mode,
            aggregate,
            aggregate_column.as_deref(),
        ),
    }
}

fn prepare_web_search(input: &Value) -> Result<PreparedCapabilityTool, String> {
    let query = required_string(input, "query")?;
    let provider = match optional_string(input, "provider")
        .as_deref()
        .unwrap_or("auto")
    {
        "auto" => SearchProvider::Auto,
        "brave" => SearchProvider::Brave,
        "tavily" => SearchProvider::Tavily,
        "exa" => SearchProvider::Exa,
        "searxng" => SearchProvider::Searxng,
        "duckduckgo" => SearchProvider::DuckDuckGo,
        value => return Err(format!("unsupported web search provider: {value}")),
    };
    let max_results = optional_usize(input, "maxResults")
        .unwrap_or(5)
        .clamp(1, 10);
    let include_domains = string_array(input, "includeDomains")?;
    let exclude_domains = string_array(input, "excludeDomains")?;
    Ok(PreparedCapabilityTool::WebSearch {
        preview: ToolPreview {
            tool: "web_search".to_owned(),
            title: "搜索互联网".to_owned(),
            target: query.clone(),
            summary: format!("搜索互联网并返回最多 {max_results} 条结果"),
            diff: None,
            command: None,
            cwd: None,
        },
        query,
        provider,
        max_results,
        include_domains,
        exclude_domains,
    })
}

fn prepare_web_read(input: &Value) -> Result<PreparedCapabilityTool, String> {
    let raw_url = required_string(input, "url")?;
    let url = Url::parse(&raw_url).map_err(|error| format!("invalid URL: {error}"))?;
    validate_public_url(&url)?;
    let format = match optional_string(input, "format")
        .as_deref()
        .unwrap_or("markdown")
    {
        "markdown" => WebReadFormat::Markdown,
        "text" => WebReadFormat::Text,
        value => return Err(format!("unsupported web read format: {value}")),
    };
    let max_chars = optional_usize(input, "maxChars")
        .unwrap_or(DEFAULT_WEB_TEXT_CHARS)
        .clamp(1_000, 300_000);
    Ok(PreparedCapabilityTool::WebRead {
        preview: ToolPreview {
            tool: "web_read".to_owned(),
            title: "读取网页".to_owned(),
            target: url.to_string(),
            summary: "从互联网读取网页正文，Fox 会阻止私有网络地址".to_owned(),
            diff: None,
            command: None,
            cwd: None,
        },
        url,
        format,
        max_chars,
    })
}

fn prepare_http_request(input: &Value) -> Result<PreparedCapabilityTool, String> {
    let raw_url = required_string(input, "url")?;
    let url = Url::parse(&raw_url).map_err(|error| format!("invalid URL: {error}"))?;
    validate_public_url(&url)?;
    let host = url
        .host_str()
        .ok_or_else(|| "HTTP URL is missing a host".to_owned())?
        .to_ascii_lowercase();
    let allowed_hosts = string_array(input, "allowedHosts")?
        .into_iter()
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if allowed_hosts.is_empty() || allowed_hosts.len() > 20 {
        return Err("http_request requires 1 to 20 exact allowedHosts".to_owned());
    }
    if !allowed_hosts.iter().any(|allowed| allowed == &host) {
        return Err(format!(
            "HTTP host {host} is not present in the exact allowedHosts list"
        ));
    }
    if allowed_hosts
        .iter()
        .any(|allowed| allowed.contains('*') || allowed.contains('/') || allowed.contains(':'))
    {
        return Err(
            "allowedHosts only accepts exact host names without wildcards, paths, or ports"
                .to_owned(),
        );
    }
    let method = match optional_string(input, "method")
        .unwrap_or_else(|| "GET".to_owned())
        .to_ascii_uppercase()
        .as_str()
    {
        "GET" => Method::GET,
        "HEAD" => Method::HEAD,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        value => return Err(format!("unsupported HTTP method: {value}")),
    };
    let headers = prepare_http_headers(input.get("headers"))?;
    let body = input
        .get("body")
        .map(|value| match value {
            Value::String(value) => Ok(value.as_bytes().to_vec()),
            value => {
                serde_json::to_vec(value).map_err(|error| format!("invalid HTTP body: {error}"))
            }
        })
        .transpose()?;
    if body
        .as_ref()
        .is_some_and(|value| value.len() > MAX_HTTP_REQUEST_BYTES)
    {
        return Err("HTTP request body exceeds the 1 MiB limit".to_owned());
    }
    if matches!(method, Method::GET | Method::HEAD) && body.is_some() {
        return Err("GET and HEAD requests cannot include a body".to_owned());
    }
    let timeout = Duration::from_secs(
        optional_usize(input, "timeoutSeconds")
            .unwrap_or(15)
            .clamp(1, 15) as u64,
    );
    Ok(PreparedCapabilityTool::HttpRequest {
        preview: ToolPreview {
            tool: "http_request".to_owned(),
            title: format!("发送 {} 请求", method.as_str()),
            target: url.to_string(),
            summary: format!(
                "仅访问显式白名单中的公网主机；请求体上限 1 MiB，响应上限 10 MiB，超时 {} 秒",
                timeout.as_secs()
            ),
            diff: None,
            command: None,
            cwd: None,
        },
        method,
        url,
        allowed_hosts,
        headers,
        body,
        timeout,
    })
}

fn prepare_http_headers(value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| "HTTP headers must be an object".to_owned())?;
    if object.len() > 20 {
        return Err("HTTP request accepts at most 20 headers".to_owned());
    }
    const ALLOWED: &[&str] = &[
        "accept",
        "content-type",
        "if-match",
        "if-none-match",
        "if-modified-since",
        "if-unmodified-since",
    ];
    object
        .iter()
        .map(|(name, value)| {
            let normalized = name.to_ascii_lowercase();
            if !ALLOWED.contains(&normalized.as_str()) {
                return Err(format!(
                    "HTTP header {name} is not allowed; credentials belong in a Keyring-backed OpenAPI Connector"
                ));
            }
            let value = value
                .as_str()
                .filter(|value| {
                    !value.contains('\r') && !value.contains('\n') && value.len() <= 8_192
                })
                .ok_or_else(|| format!("HTTP header {name} must be a bounded single-line string"))?;
            Ok((normalized, value.to_owned()))
        })
        .collect()
}

fn prepare_system_info(input: &Value) -> Result<PreparedCapabilityTool, String> {
    let include_processes = optional_bool(input, "includeProcesses").unwrap_or(false);
    let max_processes = optional_usize(input, "maxProcesses")
        .unwrap_or(20)
        .clamp(1, 100);
    let include_environment = optional_bool(input, "includeEnvironment").unwrap_or(false);
    Ok(PreparedCapabilityTool::SystemInfo {
        include_processes,
        max_processes,
        include_environment,
        preview: ToolPreview {
            tool: "system_info".to_owned(),
            title: "读取系统信息".to_owned(),
            target: System::host_name().unwrap_or_else(|| "current host".to_owned()),
            summary: format!(
                "读取 CPU、内存、磁盘{}{}；环境变量仅限固定安全白名单",
                if include_processes {
                    "、进程摘要"
                } else {
                    ""
                },
                if include_environment {
                    "和安全环境变量"
                } else {
                    ""
                },
            ),
            diff: None,
            command: None,
            cwd: None,
        },
    })
}

fn prepare_sqlite_read(
    input: &Value,
    project_root: &str,
) -> Result<PreparedCapabilityTool, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    let requested_path = required_string(input, "path")?;
    let path = resolve_project_path(&root, &requested_path, true)?;
    if !path.is_file() {
        return Err("sqlite_read path must reference a file".to_owned());
    }
    let query = required_string(input, "query")?;
    if query.len() > MAX_SQL_BYTES || query.as_bytes().contains(&0) {
        return Err("SQLite query must be non-empty UTF-8 text up to 64 KiB".to_owned());
    }
    let parameters = prepare_sql_parameters(input.get("parameters"))?;
    let row_limit = optional_usize(input, "rowLimit")
        .unwrap_or(1_000)
        .clamp(1, 1_000);
    let timeout = Duration::from_secs(
        optional_usize(input, "timeoutSeconds")
            .unwrap_or(5)
            .clamp(1, 5) as u64,
    );
    Ok(PreparedCapabilityTool::SqliteRead {
        preview: ToolPreview {
            tool: "sqlite_read".to_owned(),
            title: "只读查询 SQLite".to_owned(),
            target: path.display().to_string(),
            summary: format!(
                "只读执行单条 SQL；最多 {row_limit} 行、{} 秒、2 MiB 结果",
                timeout.as_secs()
            ),
            diff: None,
            command: Some(query.clone()),
            cwd: Some(root.display().to_string()),
        },
        path,
        query,
        parameters,
        row_limit,
        timeout,
    })
}

fn prepare_sql_parameters(value: Option<&Value>) -> Result<Vec<rusqlite::types::Value>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| "SQLite parameters must be a positional array".to_owned())?;
    if values.len() > 100 {
        return Err("SQLite query accepts at most 100 parameters".to_owned());
    }
    values
        .iter()
        .map(|value| match value {
            Value::Null => Ok(rusqlite::types::Value::Null),
            Value::Bool(value) => Ok(rusqlite::types::Value::Integer(i64::from(*value))),
            Value::Number(value) if value.is_i64() => Ok(rusqlite::types::Value::Integer(
                value.as_i64().expect("checked i64"),
            )),
            Value::Number(value) if value.is_u64() => value
                .as_u64()
                .and_then(|value| i64::try_from(value).ok())
                .map(rusqlite::types::Value::Integer)
                .ok_or_else(|| "SQLite integer parameter exceeds i64".to_owned()),
            Value::Number(value) => value
                .as_f64()
                .map(rusqlite::types::Value::Real)
                .ok_or_else(|| "invalid SQLite number parameter".to_owned()),
            Value::String(value) if value.len() <= MAX_INLINE_DATA_BYTES => {
                Ok(rusqlite::types::Value::Text(value.clone()))
            }
            Value::String(_) => Err("SQLite text parameter exceeds 4 MiB".to_owned()),
            Value::Array(_) | Value::Object(_) => {
                Err("SQLite parameters only support null, boolean, number, and string".to_owned())
            }
        })
        .collect()
}

fn prepare_structured_data(
    input: &Value,
    project_root: Option<&str>,
) -> Result<PreparedCapabilityTool, String> {
    let action = match optional_string(input, "action")
        .as_deref()
        .unwrap_or("format")
    {
        "format" => StructuredAction::Format,
        "convert" => StructuredAction::Convert,
        "query" => StructuredAction::Query,
        "validate" => StructuredAction::Validate,
        value => return Err(format!("unsupported structured data action: {value}")),
    };
    let data_input = prepare_data_input(input, project_root)?;
    let input_format = parse_data_format(
        optional_string(input, "inputFormat")
            .as_deref()
            .unwrap_or("auto"),
    )?;
    let output_format = parse_data_format(
        optional_string(input, "outputFormat")
            .as_deref()
            .unwrap_or("json"),
    )?;
    if output_format == DataFormat::Auto {
        return Err("outputFormat cannot be auto".to_owned());
    }
    let query = optional_string(input, "query");
    if matches!(action, StructuredAction::Query) && query.is_none() {
        return Err("structured_data query action requires query".to_owned());
    }
    let required_keys = string_array(input, "requiredKeys")?;
    let target = data_input.label();
    Ok(PreparedCapabilityTool::StructuredData {
        preview: ToolPreview {
            tool: "structured_data".to_owned(),
            title: "处理结构化数据".to_owned(),
            target,
            summary: "解析、格式化、转换、查询或校验结构化数据".to_owned(),
            diff: None,
            command: None,
            cwd: None,
        },
        action,
        input: data_input,
        input_format,
        output_format,
        query,
        required_keys,
    })
}

fn prepare_git_read(input: &Value, project_root: &str) -> Result<PreparedCapabilityTool, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    let operation = match optional_string(input, "operation")
        .as_deref()
        .unwrap_or("status")
    {
        "status" => GitOperation::Status,
        "diff" => GitOperation::Diff,
        "log" => GitOperation::Log,
        "blame" => GitOperation::Blame,
        value => return Err(format!("unsupported Git operation: {value}")),
    };
    let path = optional_string(input, "path")
        .map(|path| resolve_project_path(&root, &path, true))
        .transpose()?;
    if matches!(operation, GitOperation::Blame) && path.is_none() {
        return Err("git_read blame requires path".to_owned());
    }
    let reference = validated_git_ref(optional_string(input, "ref"))?;
    let base = validated_git_ref(optional_string(input, "base"))?;
    let staged = optional_bool(input, "staged").unwrap_or(false);
    let max_entries = optional_usize(input, "maxEntries")
        .unwrap_or(20)
        .clamp(1, 100);
    let start_line = optional_usize(input, "startLine");
    let end_line = optional_usize(input, "endLine");
    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start == 0 || end < start {
            return Err("invalid blame line range".to_owned());
        }
    }
    let operation_label = match operation {
        GitOperation::Status => "状态",
        GitOperation::Diff => "差异",
        GitOperation::Log => "历史",
        GitOperation::Blame => "逐行归属",
    };
    Ok(PreparedCapabilityTool::GitRead {
        preview: ToolPreview {
            tool: "git_read".to_owned(),
            title: format!("读取 Git {operation_label}"),
            target: path.as_deref().unwrap_or(&root).display().to_string(),
            summary: "以只读方式返回结构化 Git 信息".to_owned(),
            diff: None,
            command: None,
            cwd: Some(root.display().to_string()),
        },
        root,
        operation,
        path,
        reference,
        base,
        staged,
        max_entries,
        start_line,
        end_line,
    })
}

fn prepare_test_run(input: &Value, project_root: &str) -> Result<PreparedCapabilityTool, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    let cwd = resolve_cwd(&root, optional_string(input, "cwd").as_deref())?;
    let runner = optional_string(input, "runner").unwrap_or_else(|| "auto".to_owned());
    let script = optional_string(input, "script");
    let target = optional_string(input, "target");
    let (program, args) = test_command(&cwd, &runner, script.as_deref(), target.as_deref())?;
    prepare_process(
        "test_run",
        "运行测试",
        program,
        args,
        cwd,
        process_timeout(input),
    )
}

fn prepare_code_check(input: &Value, project_root: &str) -> Result<PreparedCapabilityTool, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    let cwd = resolve_cwd(&root, optional_string(input, "cwd").as_deref())?;
    let ecosystem = optional_string(input, "ecosystem").unwrap_or_else(|| "auto".to_owned());
    let check = optional_string(input, "check").unwrap_or_else(|| "auto".to_owned());
    let (program, args) = check_command(&cwd, &ecosystem, &check)?;
    prepare_process(
        "code_check",
        "检查代码",
        program,
        args,
        cwd,
        process_timeout(input),
    )
}

fn prepare_format_code(
    input: &Value,
    project_root: &str,
) -> Result<PreparedCapabilityTool, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    let cwd = resolve_cwd(&root, optional_string(input, "cwd").as_deref())?;
    let ecosystem = optional_string(input, "ecosystem").unwrap_or_else(|| "auto".to_owned());
    let mode = optional_string(input, "mode").unwrap_or_else(|| "check".to_owned());
    if !matches!(mode.as_str(), "check" | "write") {
        return Err("format_code mode must be check or write".to_owned());
    }
    let path = optional_string(input, "path")
        .map(|path| resolve_project_path(&root, &path, true))
        .transpose()?;
    let (program, args) = format_command(&cwd, &ecosystem, &mode, path.as_deref())?;
    prepare_process(
        "format_code",
        if mode == "write" {
            "格式化代码"
        } else {
            "检查代码格式"
        },
        program,
        args,
        cwd,
        process_timeout(input),
    )
}

fn prepare_process(
    tool: &str,
    title: &str,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    timeout: Duration,
) -> Result<PreparedCapabilityTool, String> {
    let command = display_command(&program, &args);
    Ok(PreparedCapabilityTool::Process {
        preview: ToolPreview {
            tool: tool.to_owned(),
            title: title.to_owned(),
            target: cwd.display().to_string(),
            summary: format!("在授权项目中执行：{command}"),
            diff: None,
            command: Some(command),
            cwd: Some(cwd.display().to_string()),
        },
        tool: tool.to_owned(),
        program,
        args,
        cwd,
        timeout,
    })
}

fn prepare_tabular_data(
    input: &Value,
    project_root: Option<&str>,
) -> Result<PreparedCapabilityTool, String> {
    let data_input = prepare_data_input(input, project_root)?;
    let format = parse_data_format(
        optional_string(input, "format")
            .as_deref()
            .unwrap_or("auto"),
    )?;
    if matches!(
        format,
        DataFormat::Yaml | DataFormat::Toml | DataFormat::Xml
    ) {
        return Err("tabular_data supports CSV, TSV, and JSON in this release".to_owned());
    }
    let operation = match optional_string(input, "operation")
        .as_deref()
        .unwrap_or("preview")
    {
        "preview" => TableOperation::Preview,
        "filter" => TableOperation::Filter,
        "aggregate" => TableOperation::Aggregate,
        value => return Err(format!("unsupported table operation: {value}")),
    };
    let filter_column = optional_string(input, "filterColumn");
    let filter_value = optional_string(input, "filterValue");
    if matches!(operation, TableOperation::Filter)
        && (filter_column.is_none() || filter_value.is_none())
    {
        return Err("tabular_data filter requires filterColumn and filterValue".to_owned());
    }
    let filter_mode = match optional_string(input, "filterMode")
        .as_deref()
        .unwrap_or("equals")
    {
        "equals" => FilterMode::Equals,
        "contains" => FilterMode::Contains,
        value => return Err(format!("unsupported table filter mode: {value}")),
    };
    let aggregate = match optional_string(input, "aggregate")
        .as_deref()
        .unwrap_or("count")
    {
        "count" => AggregateOperation::Count,
        "sum" => AggregateOperation::Sum,
        "avg" => AggregateOperation::Average,
        "min" => AggregateOperation::Min,
        "max" => AggregateOperation::Max,
        value => return Err(format!("unsupported aggregate operation: {value}")),
    };
    let aggregate_column = optional_string(input, "column");
    if matches!(operation, TableOperation::Aggregate)
        && !matches!(aggregate, AggregateOperation::Count)
        && aggregate_column.is_none()
    {
        return Err("numeric aggregation requires column".to_owned());
    }
    let offset = optional_usize(input, "offset").unwrap_or(0);
    let limit = optional_usize(input, "limit").unwrap_or(50).clamp(1, 500);
    let target = data_input.label();
    Ok(PreparedCapabilityTool::TabularData {
        preview: ToolPreview {
            tool: "tabular_data".to_owned(),
            title: "分析表格数据".to_owned(),
            target,
            summary: "预览、筛选或聚合 CSV、TSV 和 JSON 表格".to_owned(),
            diff: None,
            command: None,
            cwd: None,
        },
        input: data_input,
        format,
        operation,
        offset,
        limit,
        filter_column,
        filter_value,
        filter_mode,
        aggregate,
        aggregate_column,
    })
}

fn execute_web_search(
    query: &str,
    provider: SearchProvider,
    max_results: usize,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Result<Value, String> {
    let requested_provider = provider;
    let provider = resolve_search_provider(provider)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .redirect(Policy::limited(3))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/128.0 Safari/537.36 Fox/0.1",
        )
        .build()
        .map_err(|error| format!("failed to create web search client: {error}"))?;
    let filtered_query = search_query_with_domains(query, include_domains, exclude_domains);
    let (raw, provider_name, fallback_from) = match provider {
        SearchProvider::Brave => (
            search_brave(&client, &filtered_query, max_results)?,
            "brave",
            None,
        ),
        SearchProvider::Tavily => (
            search_tavily(
                &client,
                query,
                max_results,
                include_domains,
                exclude_domains,
            )?,
            "tavily",
            None,
        ),
        SearchProvider::Exa => (
            search_exa(
                &client,
                query,
                max_results,
                include_domains,
                exclude_domains,
            )?,
            "exa",
            None,
        ),
        SearchProvider::Searxng => (
            search_searxng(&client, &filtered_query, max_results)?,
            "searxng",
            None,
        ),
        SearchProvider::DuckDuckGo => {
            match search_duckduckgo(&client, &filtered_query, max_results) {
                Ok(raw)
                    if !matches!(requested_provider, SearchProvider::Auto)
                        || raw
                            .get("results")
                            .and_then(Value::as_array)
                            .is_some_and(|results| !results.is_empty()) =>
                {
                    (raw, "duckduckgo", None)
                }
                Ok(_) => (
                    search_yahoo(&client, &filtered_query, max_results)?,
                    "yahoo",
                    Some("duckduckgo_empty"),
                ),
                Err(duckduckgo_error) if matches!(requested_provider, SearchProvider::Auto) => {
                    let yahoo = search_yahoo(&client, &filtered_query, max_results).map_err(
                        |yahoo_error| {
                            format!(
                                "automatic web search providers failed: {duckduckgo_error}; {yahoo_error}"
                            )
                        },
                    )?;
                    (yahoo, "yahoo", Some("duckduckgo_blocked"))
                }
                Err(error) => return Err(error),
            }
        }
        SearchProvider::Auto => unreachable!(),
    };
    let results = normalize_search_results(provider, raw, max_results)?;
    let text = if results.is_empty() {
        format!("No web results found for: {query}")
    } else {
        results
            .iter()
            .enumerate()
            .map(|(index, item)| {
                format!(
                    "{}. {}\n{}\n{}",
                    index + 1,
                    item.get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Untitled"),
                    item.get("url").and_then(Value::as_str).unwrap_or_default(),
                    item.get("snippet")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    Ok(tool_result(
        text,
        json!({
            "query": query,
            "provider": provider_name,
            "fallbackFrom": fallback_from,
            "results": results,
        }),
    ))
}

fn search_query_with_domains(
    query: &str,
    include_domains: &[String],
    exclude_domains: &[String],
) -> String {
    let mut terms = vec![query.to_owned()];
    if !include_domains.is_empty() {
        terms.push(format!(
            "({})",
            include_domains
                .iter()
                .map(|domain| format!("site:{domain}"))
                .collect::<Vec<_>>()
                .join(" OR ")
        ));
    }
    terms.extend(
        exclude_domains
            .iter()
            .map(|domain| format!("-site:{domain}")),
    );
    terms.join(" ")
}

fn resolve_search_provider(provider: SearchProvider) -> Result<SearchProvider, String> {
    if !matches!(provider, SearchProvider::Auto) {
        ensure_provider_configured(provider)?;
        return Ok(provider);
    }
    for candidate in [
        SearchProvider::Brave,
        SearchProvider::Tavily,
        SearchProvider::Exa,
        SearchProvider::Searxng,
    ] {
        if ensure_provider_configured(candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Ok(SearchProvider::DuckDuckGo)
}

fn ensure_provider_configured(provider: SearchProvider) -> Result<(), String> {
    let configured = match provider {
        SearchProvider::Brave => env_non_empty("BRAVE_API_KEY"),
        SearchProvider::Tavily => env_non_empty("TAVILY_API_KEY"),
        SearchProvider::Exa => env_non_empty("EXA_API_KEY"),
        SearchProvider::Searxng => env_non_empty("FOX_SEARXNG_URL"),
        SearchProvider::DuckDuckGo => true,
        SearchProvider::Auto => true,
    };
    configured.then_some(()).ok_or_else(|| {
        format!(
            "{} web search provider is not configured",
            search_provider_name(provider)
        )
    })
}

fn search_brave(client: &Client, query: &str, max_results: usize) -> Result<Value, String> {
    let key =
        env::var("BRAVE_API_KEY").map_err(|_| "BRAVE_API_KEY is not configured".to_owned())?;
    client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .query(&[("q", query), ("count", &max_results.to_string())])
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Brave Search request failed: {error}"))?
        .json()
        .map_err(|error| format!("Brave Search returned invalid JSON: {error}"))
}

fn search_tavily(
    client: &Client,
    query: &str,
    max_results: usize,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Result<Value, String> {
    let key =
        env::var("TAVILY_API_KEY").map_err(|_| "TAVILY_API_KEY is not configured".to_owned())?;
    client
        .post("https://api.tavily.com/search")
        .json(&json!({
            "api_key": key,
            "query": query,
            "max_results": max_results,
            "include_domains": include_domains,
            "exclude_domains": exclude_domains,
            "include_answer": false,
            "include_raw_content": false,
        }))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Tavily request failed: {error}"))?
        .json()
        .map_err(|error| format!("Tavily returned invalid JSON: {error}"))
}

fn search_exa(
    client: &Client,
    query: &str,
    max_results: usize,
    include_domains: &[String],
    exclude_domains: &[String],
) -> Result<Value, String> {
    let key = env::var("EXA_API_KEY").map_err(|_| "EXA_API_KEY is not configured".to_owned())?;
    client
        .post("https://api.exa.ai/search")
        .header("x-api-key", key)
        .json(&json!({
            "query": query,
            "numResults": max_results,
            "includeDomains": include_domains,
            "excludeDomains": exclude_domains,
            "contents": { "text": { "maxCharacters": 800 } },
        }))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Exa request failed: {error}"))?
        .json()
        .map_err(|error| format!("Exa returned invalid JSON: {error}"))
}

fn search_searxng(client: &Client, query: &str, max_results: usize) -> Result<Value, String> {
    let base =
        env::var("FOX_SEARXNG_URL").map_err(|_| "FOX_SEARXNG_URL is not configured".to_owned())?;
    let mut url =
        Url::parse(&base).map_err(|error| format!("FOX_SEARXNG_URL is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("FOX_SEARXNG_URL must use http or https".to_owned());
    }
    if !url.path().trim_end_matches('/').ends_with("/search") {
        let path = format!("{}/", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url = url
            .join("search")
            .map_err(|error| format!("FOX_SEARXNG_URL is invalid: {error}"))?;
    }
    client
        .get(url)
        .query(&[("q", query), ("format", "json"), ("pageno", "1")])
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("SearXNG request failed: {error}"))?
        .json::<Value>()
        .map(|mut value| {
            if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
                results.truncate(max_results);
            }
            value
        })
        .map_err(|error| format!("SearXNG returned invalid JSON: {error}"))
}

fn search_duckduckgo(client: &Client, query: &str, max_results: usize) -> Result<Value, String> {
    let response = client
        .get("https://html.duckduckgo.com/html/")
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .query(&[("q", query)])
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("DuckDuckGo search request failed: {error}"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take((1024 * 1024 + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read DuckDuckGo search response: {error}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("DuckDuckGo search response exceeded the 1 MiB limit".to_owned());
    }
    let source = String::from_utf8_lossy(&bytes);
    if status == StatusCode::ACCEPTED || duckduckgo_challenge_page(&source) {
        return Err(
            "DuckDuckGo blocked the automated search request; choose another provider".to_owned(),
        );
    }
    Ok(json!({
        "results": parse_duckduckgo_results(&source, max_results),
    }))
}

fn duckduckgo_challenge_page(source: &str) -> bool {
    let lowercase = source.to_ascii_lowercase();
    [
        "anomaly-modal",
        "challenge-form",
        "duckduckgo.com/anomaly.js",
        "unfortunately, bots use duckduckgo too",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
}

fn search_yahoo(client: &Client, query: &str, max_results: usize) -> Result<Value, String> {
    let response = client
        .get("https://search.yahoo.com/search")
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .query(&[("p", query), ("ei", "UTF-8"), ("nojs", "1")])
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Yahoo search request failed: {error}"))?;
    let mut bytes = Vec::new();
    response
        .take((1024 * 1024 + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read Yahoo search response: {error}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("Yahoo search response exceeded the 1 MiB limit".to_owned());
    }
    let source = String::from_utf8_lossy(&bytes);
    Ok(json!({
        "results": parse_yahoo_results(&source, max_results),
    }))
}

fn parse_yahoo_results(source: &str, max_results: usize) -> Vec<Value> {
    let lowercase = source.to_ascii_lowercase();
    let mut results = Vec::new();
    let mut cursor = 0usize;
    while results.len() < max_results {
        let Some(h3_offset) = lowercase[cursor..].find("<h3") else {
            break;
        };
        let h3_start = cursor + h3_offset;
        let Some(h3_open_offset) = lowercase[h3_start..].find('>') else {
            break;
        };
        let h3_open_end = h3_start + h3_open_offset;
        let Some(h3_close_offset) = lowercase[h3_open_end + 1..].find("</h3>") else {
            break;
        };
        let h3_close = h3_open_end + 1 + h3_close_offset;
        let next_h3 = lowercase[h3_close + 5..]
            .find("<h3")
            .map(|offset| h3_close + 5 + offset)
            .unwrap_or(source.len());
        cursor = h3_close + 5;

        let search_start = h3_start.saturating_sub(4_000);
        let Some(anchor_start_offset) = lowercase[search_start..h3_start].rfind("<a") else {
            continue;
        };
        let anchor_start = search_start + anchor_start_offset;
        let Some(anchor_end_offset) = lowercase[anchor_start..h3_start].find('>') else {
            continue;
        };
        let anchor_end = anchor_start + anchor_end_offset;
        let Some(raw_url) = html_attribute(&source[anchor_start..=anchor_end], "href") else {
            continue;
        };
        let Some(url) = unwrap_yahoo_url(&raw_url) else {
            continue;
        };
        let title = html_to_text(&source[h3_open_end + 1..h3_close]);
        if title.is_empty() {
            continue;
        }
        let snippet = html_element_text_by_class(&source[h3_close + 5..next_h3], "compText")
            .unwrap_or_default();
        results.push(json!({
            "title": title,
            "url": url,
            "content": snippet,
        }));
    }
    results
}

fn unwrap_yahoo_url(raw_url: &str) -> Option<String> {
    let base = Url::parse("https://search.yahoo.com/search").ok()?;
    let parsed = Url::parse(raw_url).or_else(|_| base.join(raw_url)).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "r.search.yahoo.com" || host.ends_with(".r.search.yahoo.com") {
        let path = parsed.path();
        let encoded_start = path.find("/RU=")? + 4;
        let encoded_end = path[encoded_start..]
            .find("/RK=")
            .map(|offset| encoded_start + offset)
            .unwrap_or(path.len());
        let decoder = Url::parse(&format!(
            "https://fox.invalid/?target={}",
            &path[encoded_start..encoded_end]
        ))
        .ok()?;
        let target = decoder
            .query_pairs()
            .find_map(|(key, value)| (key == "target").then(|| value.into_owned()))?;
        return unwrap_duckduckgo_url(&target);
    }
    unwrap_duckduckgo_url(parsed.as_str())
}

fn parse_duckduckgo_results(source: &str, max_results: usize) -> Vec<Value> {
    let lowercase = source.to_ascii_lowercase();
    let mut results = Vec::new();
    let mut cursor = 0usize;
    while results.len() < max_results {
        let Some(marker_offset) = lowercase[cursor..].find("result__a") else {
            break;
        };
        let marker = cursor + marker_offset;
        let Some(tag_start) = lowercase[..marker].rfind("<a") else {
            cursor = marker + "result__a".len();
            continue;
        };
        let Some(open_end_offset) = lowercase[tag_start..].find('>') else {
            break;
        };
        let open_end = tag_start + open_end_offset;
        let Some(close_offset) = lowercase[open_end + 1..].find("</a>") else {
            break;
        };
        let close_start = open_end + 1 + close_offset;
        let close_end = close_start + "</a>".len();
        cursor = close_end;

        let Some(raw_url) = html_attribute(&source[tag_start..=open_end], "href") else {
            continue;
        };
        let Some(url) = unwrap_duckduckgo_url(&raw_url) else {
            continue;
        };
        let title = html_to_text(&source[open_end + 1..close_start]);
        if title.is_empty() {
            continue;
        }
        let next_result = lowercase[cursor..]
            .find("result__a")
            .map(|offset| cursor + offset)
            .unwrap_or(source.len());
        let snippet = html_element_text_by_class(&source[cursor..next_result], "result__snippet")
            .unwrap_or_default();
        results.push(json!({
            "title": title,
            "url": url,
            "content": snippet,
        }));
    }
    results
}

fn html_attribute(tag: &str, name: &str) -> Option<String> {
    let lowercase = tag.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(offset) = lowercase[cursor..].find(&name) {
        let start = cursor + offset;
        let before = lowercase[..start].chars().next_back();
        let after_index = start + name.len();
        let after = lowercase[after_index..].chars().next();
        if before.is_some_and(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
            || after
                .is_some_and(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
        {
            cursor = after_index;
            continue;
        }
        let mut value_start = after_index;
        while lowercase
            .as_bytes()
            .get(value_start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            value_start += 1;
        }
        if lowercase.as_bytes().get(value_start) != Some(&b'=') {
            cursor = after_index;
            continue;
        }
        value_start += 1;
        while lowercase
            .as_bytes()
            .get(value_start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            value_start += 1;
        }
        let quote = *tag.as_bytes().get(value_start)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        value_start += 1;
        let value_end = tag.as_bytes()[value_start..]
            .iter()
            .position(|value| *value == quote)?
            + value_start;
        return Some(decode_html_entities(&tag[value_start..value_end]));
    }
    None
}

fn html_element_text_by_class(source: &str, class_name: &str) -> Option<String> {
    let lowercase = source.to_ascii_lowercase();
    let marker = lowercase.find(&class_name.to_ascii_lowercase())?;
    let tag_start = lowercase[..marker].rfind('<')?;
    let open_end = lowercase[tag_start..].find('>')? + tag_start;
    let tag_name = lowercase[tag_start + 1..]
        .chars()
        .take_while(|value| value.is_ascii_alphanumeric())
        .collect::<String>();
    if tag_name.is_empty() {
        return None;
    }
    let close = format!("</{tag_name}>");
    let content_end = lowercase[open_end + 1..].find(&close)? + open_end + 1;
    Some(html_to_text(&source[open_end + 1..content_end]))
}

fn unwrap_duckduckgo_url(raw_url: &str) -> Option<String> {
    let base = Url::parse("https://html.duckduckgo.com/html/").ok()?;
    let parsed = Url::parse(raw_url).or_else(|_| base.join(raw_url)).ok()?;
    let target = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "uddg").then(|| value.into_owned()))
        .unwrap_or_else(|| parsed.to_string());
    let target = Url::parse(&target).ok()?;
    if !matches!(target.scheme(), "http" | "https")
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return None;
    }
    let host = target.host_str()?.to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| !is_public_ip(address))
    {
        return None;
    }
    Some(target.to_string())
}

fn normalize_search_results(
    provider: SearchProvider,
    raw: Value,
    max_results: usize,
) -> Result<Vec<Value>, String> {
    let items = match provider {
        SearchProvider::Brave => raw.pointer("/web/results").and_then(Value::as_array),
        SearchProvider::Tavily
        | SearchProvider::Exa
        | SearchProvider::Searxng
        | SearchProvider::DuckDuckGo => raw.get("results").and_then(Value::as_array),
        SearchProvider::Auto => None,
    }
    .ok_or_else(|| {
        format!(
            "{} response omitted results",
            search_provider_name(provider)
        )
    })?;
    Ok(items
        .iter()
        .take(max_results)
        .filter_map(|item| {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let url = item.get("url").and_then(Value::as_str).unwrap_or_default();
            if url.is_empty() {
                return None;
            }
            let snippet = item
                .get("description")
                .or_else(|| item.get("content"))
                .or_else(|| item.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(json!({ "title": title, "url": url, "snippet": snippet }))
        })
        .collect())
}

fn execute_web_read(url: Url, format: WebReadFormat, max_chars: usize) -> Result<Value, String> {
    let (final_url, content_type, bytes) = fetch_public_url(url)?;
    let source = String::from_utf8_lossy(&bytes);
    let title = html_title(&source);
    let mut text = if content_type.contains("html") {
        match format {
            WebReadFormat::Markdown => html_to_markdown(&source),
            WebReadFormat::Text => html_to_text(&source),
        }
    } else {
        source.into_owned()
    };
    let original_chars = text.chars().count();
    if original_chars > max_chars {
        text = text.chars().take(max_chars).collect();
        text.push_str("\n\n[Fox truncated this webpage to keep the runtime responsive.]");
    }
    Ok(tool_result(
        text,
        json!({
            "url": final_url,
            "title": title,
            "contentType": content_type,
            "originalChars": original_chars,
            "truncated": original_chars > max_chars,
        }),
    ))
}

fn execute_http_request(
    mut method: Method,
    mut url: Url,
    allowed_hosts: &[String],
    headers: &[(String, String)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<Value, String> {
    let initial_host = url
        .host_str()
        .ok_or_else(|| "HTTP URL is missing a host".to_owned())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let mut effective_body = body;
    for _ in 0..=5 {
        let host = url
            .host_str()
            .ok_or_else(|| "HTTP URL is missing a host".to_owned())?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host != initial_host || !allowed_hosts.iter().any(|allowed| allowed == &host) {
            return Err("HTTP redirects cannot leave the original exact allowed host".to_owned());
        }
        let addresses = validate_public_url(&url)?;
        let port = url.port_or_known_default().unwrap_or(443);
        let mut builder = Client::builder()
            .connect_timeout(timeout.min(Duration::from_secs(8)))
            .timeout(timeout)
            .redirect(Policy::none())
            .user_agent("Fox/0.1 http_request");
        for address in addresses {
            builder = builder.resolve(&host, SocketAddr::new(address, port));
        }
        let client = builder
            .build()
            .map_err(|error| format!("failed to create HTTP client: {error}"))?;
        let mut request = client.request(method.clone(), url.clone());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(body) = effective_body {
            request = request.body(body.to_vec());
        }
        let response = request
            .send()
            .map_err(|error| format!("HTTP request failed: {error}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "HTTP redirect omitted Location".to_owned())?;
            let next = url
                .join(location)
                .map_err(|error| format!("invalid HTTP redirect: {error}"))?;
            let next_host = next
                .host_str()
                .ok_or_else(|| "HTTP redirect omitted host".to_owned())?
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if next_host != initial_host {
                return Err("cross-host HTTP redirects are blocked".to_owned());
            }
            match response.status() {
                StatusCode::SEE_OTHER => {
                    method = Method::GET;
                    effective_body = None;
                }
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND
                    if !matches!(method, Method::GET | Method::HEAD) =>
                {
                    return Err(
                        "ambiguous redirect for a state-changing HTTP request is blocked"
                            .to_owned(),
                    );
                }
                _ => {}
            }
            url = next;
            continue;
        }
        let status = response.status();
        let response_headers = safe_response_headers(response.headers());
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_ascii_lowercase();
        let mut bytes = Vec::new();
        response
            .take((MAX_HTTP_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("failed to read HTTP response: {error}"))?;
        if bytes.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err("HTTP response exceeds the 10 MiB limit".to_owned());
        }
        let textual = content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("javascript")
            || content_type.contains("x-www-form-urlencoded");
        let (response_body, encoding) = if textual || bytes.is_empty() {
            (String::from_utf8_lossy(&bytes).into_owned(), "utf8")
        } else {
            (BASE64_STANDARD.encode(&bytes), "base64")
        };
        return Ok(tool_result(
            response_body,
            json!({
                "url": url,
                "status": status.as_u16(),
                "ok": status.is_success(),
                "contentType": content_type,
                "encoding": encoding,
                "byteLength": bytes.len(),
                "headers": response_headers,
            }),
        ));
    }
    Err("HTTP request exceeded the redirect limit".to_owned())
}

fn safe_response_headers(headers: &header::HeaderMap) -> Value {
    const SAFE: &[&str] = &[
        "content-type",
        "content-length",
        "etag",
        "last-modified",
        "location",
        "retry-after",
        "x-request-id",
    ];
    let mut output = Map::new();
    for name in SAFE {
        if let Some(value) = headers.get(*name).and_then(|value| value.to_str().ok()) {
            output.insert((*name).to_owned(), Value::String(value.to_owned()));
        }
    }
    Value::Object(output)
}

fn execute_system_info(
    include_processes: bool,
    max_processes: usize,
    include_environment: bool,
) -> Result<Value, String> {
    let mut system = System::new_all();
    system.refresh_all();
    let disks = Disks::new_with_refreshed_list();
    let disk_values = disks
        .iter()
        .map(|disk| {
            json!({
                "name": disk.name().to_string_lossy(),
                "mountPoint": disk.mount_point().display().to_string(),
                "fileSystem": disk.file_system().to_string_lossy(),
                "totalBytes": disk.total_space(),
                "availableBytes": disk.available_space(),
                "readOnly": disk.is_read_only(),
            })
        })
        .collect::<Vec<_>>();
    let processes = if include_processes {
        let mut processes = system.processes().iter().collect::<Vec<_>>();
        processes.sort_by(|left, right| {
            right
                .1
                .memory()
                .cmp(&left.1.memory())
                .then_with(|| right.1.cpu_usage().total_cmp(&left.1.cpu_usage()))
        });
        processes
            .into_iter()
            .take(max_processes)
            .map(|(pid, process)| {
                json!({
                    "pid": pid.as_u32(),
                    "name": process.name().to_string_lossy(),
                    "status": format!("{:?}", process.status()).to_ascii_lowercase(),
                    "cpuPercent": process.cpu_usage(),
                    "memoryBytes": process.memory(),
                    "runTimeSeconds": process.run_time(),
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let environment = if include_environment {
        const SAFE_ENVIRONMENT_KEYS: &[&str] = &[
            "NODE_ENV",
            "PATH",
            "HOME",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "SHELL",
            "COMSPEC",
        ];
        SAFE_ENVIRONMENT_KEYS
            .iter()
            .filter_map(|key| {
                env::var(key)
                    .ok()
                    .map(|value| ((*key).to_owned(), Value::String(value)))
            })
            .collect::<Map<_, _>>()
    } else {
        Map::new()
    };
    Ok(json!({
        "host": {
            "name": System::host_name(),
            "os": System::name(),
            "osVersion": System::long_os_version(),
            "kernelVersion": System::kernel_version(),
            "architecture": System::cpu_arch(),
            "uptimeSeconds": System::uptime(),
        },
        "cpu": {
            "logicalCoreCount": system.cpus().len(),
            "physicalCoreCount": System::physical_core_count(),
            "brand": system.cpus().first().map(|cpu| cpu.brand()),
            "globalUsagePercent": system.global_cpu_usage(),
        },
        "memory": {
            "totalBytes": system.total_memory(),
            "availableBytes": system.available_memory(),
            "usedBytes": system.used_memory(),
            "totalSwapBytes": system.total_swap(),
            "usedSwapBytes": system.used_swap(),
        },
        "disks": disk_values,
        "processes": processes,
        "environment": environment,
        "limits": {
            "processesIncluded": include_processes,
            "maxProcesses": max_processes,
            "environmentIncluded": include_environment,
            "environmentAllowlist": ["NODE_ENV", "PATH", "HOME", "TEMP", "TMP", "USERPROFILE", "SHELL", "COMSPEC"],
        },
    }))
}

fn execute_sqlite_read(
    path: &Path,
    query: &str,
    parameters: &[rusqlite::types::Value],
    row_limit: usize,
    timeout: Duration,
) -> Result<Value, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("failed to open SQLite database read-only: {error}"))?;
    connection
        .busy_timeout(timeout)
        .map_err(|error| format!("failed to configure SQLite timeout: {error}"))?;
    let started = Instant::now();
    let deadline_started = started;
    connection.progress_handler(1_000, Some(move || deadline_started.elapsed() >= timeout));
    let mut statement = connection
        .prepare(query)
        .map_err(|error| format!("failed to prepare SQLite query: {error}"))?;
    if !statement.readonly() {
        return Err("sqlite_read only permits read-only SQL statements".to_owned());
    }
    if statement.parameter_count() != parameters.len() {
        return Err(format!(
            "SQLite query expects {} parameters but received {}",
            statement.parameter_count(),
            parameters.len()
        ));
    }
    let column_names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if column_names.len() > 100 {
        return Err("SQLite result exceeds the 100-column limit".to_owned());
    }
    let mut cursor = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(|error| format!("failed to execute SQLite query: {error}"))?;
    let mut rows = Vec::new();
    let mut serialized_bytes = 0usize;
    let mut truncated = false;
    while let Some(row) = cursor.next().map_err(|error| {
        if started.elapsed() >= timeout {
            format!(
                "SQLite query exceeded the {} second limit",
                timeout.as_secs()
            )
        } else {
            format!("failed to read SQLite row: {error}")
        }
    })? {
        if rows.len() >= row_limit {
            truncated = true;
            break;
        }
        let mut value = Map::new();
        for (index, name) in column_names.iter().enumerate() {
            let cell = row
                .get_ref(index)
                .map_err(|error| format!("failed to read SQLite column {name}: {error}"))?;
            value.insert(name.clone(), sqlite_value_to_json(cell));
        }
        let value = Value::Object(value);
        let value_size = serde_json::to_vec(&value)
            .map_err(|error| format!("failed to encode SQLite row: {error}"))?
            .len();
        if serialized_bytes.saturating_add(value_size) > MAX_SQL_OUTPUT_BYTES {
            truncated = true;
            break;
        }
        serialized_bytes = serialized_bytes.saturating_add(value_size);
        rows.push(value);
    }
    let row_count = rows.len();
    Ok(json!({
        "path": path.display().to_string(),
        "columns": column_names,
        "rows": rows,
        "rowCount": row_count,
        "truncated": truncated,
        "durationMs": started.elapsed().as_millis(),
        "limits": {
            "rowLimit": row_limit,
            "timeoutSeconds": timeout.as_secs(),
            "maxOutputBytes": MAX_SQL_OUTPUT_BYTES,
            "readOnly": true,
        },
    }))
}

fn sqlite_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Number(value.into()),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(value) if value.len() <= MAX_SQL_CELL_BYTES => {
            Value::String(String::from_utf8_lossy(value).into_owned())
        }
        ValueRef::Text(value) => json!({
            "type": "text",
            "byteLength": value.len(),
            "text": String::from_utf8_lossy(&value[..MAX_SQL_CELL_BYTES]),
            "truncated": true,
        }),
        ValueRef::Blob(value) if value.len() <= MAX_SQL_CELL_BYTES => json!({
            "type": "blob",
            "byteLength": value.len(),
            "base64": BASE64_STANDARD.encode(value),
        }),
        ValueRef::Blob(value) => json!({
            "type": "blob",
            "byteLength": value.len(),
            "base64": null,
            "truncated": true,
        }),
    }
}

fn fetch_public_url(mut url: Url) -> Result<(String, String, Vec<u8>), String> {
    for _ in 0..=5 {
        let addresses = validate_public_url(&url)?;
        let host = url
            .host_str()
            .ok_or_else(|| "web URL is missing a host".to_owned())?;
        let port = url.port_or_known_default().unwrap_or(443);
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .redirect(Policy::none())
            .user_agent("Fox/0.1 web_read");
        for address in addresses {
            builder = builder.resolve(host, SocketAddr::new(address, port));
        }
        let client = builder
            .build()
            .map_err(|error| format!("failed to create webpage client: {error}"))?;
        let response = client
            .get(url.clone())
            .header(
                header::ACCEPT,
                "text/html,text/plain,application/json,application/xml;q=0.9,*/*;q=0.1",
            )
            .send()
            .map_err(|error| format!("failed to read webpage: {error}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "webpage redirect omitted Location".to_owned())?;
            url = url
                .join(location)
                .map_err(|error| format!("invalid webpage redirect: {error}"))?;
            continue;
        }
        if response.status() != StatusCode::OK {
            return Err(format!("webpage returned HTTP {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("text/plain")
            .to_ascii_lowercase();
        if !content_type.contains("text/")
            && !content_type.contains("json")
            && !content_type.contains("xml")
        {
            return Err(format!("unsupported webpage content type: {content_type}"));
        }
        let mut bytes = Vec::new();
        response
            .take((MAX_NETWORK_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("failed to read webpage body: {error}"))?;
        if bytes.len() > MAX_NETWORK_BYTES {
            return Err("webpage exceeds the 2 MiB response limit".to_owned());
        }
        return Ok((url.to_string(), content_type, bytes));
    }
    Err("webpage exceeded the redirect limit".to_owned())
}

fn validate_public_url(url: &Url) -> Result<Vec<IpAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("web URL must use http or https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("web URL credentials are not allowed".to_owned());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "web URL is missing a host".to_owned())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("failed to resolve webpage host: {error}"))?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err("webpage host resolved to no addresses".to_owned());
    }
    if addresses.iter().any(|address| !is_public_ip(*address)) {
        return Err(
            "web_read blocks private, local, reserved, and link-local addresses".to_owned(),
        );
    }
    Ok(addresses)
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

fn execute_structured_data(
    action: StructuredAction,
    input: DataInput,
    input_format: DataFormat,
    output_format: DataFormat,
    query: Option<&str>,
    required_keys: &[String],
) -> Result<Value, String> {
    let (source, inferred) = input.read()?;
    let input_format = effective_data_format(input_format, inferred, &source)?;
    let parsed = parse_structured_value(&source, input_format)?;
    let selected = if matches!(action, StructuredAction::Query) {
        query_structured_value(&parsed, query.unwrap_or_default())?.clone()
    } else {
        parsed
    };
    let missing = required_keys
        .iter()
        .filter(|key| query_structured_value(&selected, key).is_err())
        .cloned()
        .collect::<Vec<_>>();
    if matches!(action, StructuredAction::Validate) && !missing.is_empty() {
        return Ok(tool_result(
            format!("Validation failed. Missing keys: {}", missing.join(", ")),
            json!({ "valid": false, "missingKeys": missing }),
        ));
    }
    let rendered = render_structured_value(&selected, output_format)?;
    Ok(tool_result(
        rendered.clone(),
        json!({
            "valid": true,
            "inputFormat": data_format_name(input_format),
            "outputFormat": data_format_name(output_format),
            "value": selected,
            "rendered": rendered,
        }),
    ))
}

fn execute_git_read(
    root: &Path,
    operation: GitOperation,
    path: Option<&Path>,
    reference: Option<&str>,
    base: Option<&str>,
    staged: bool,
    max_entries: usize,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<Value, String> {
    let mut args = match operation {
        GitOperation::Status => vec![
            "status".to_owned(),
            "--short".to_owned(),
            "--branch".to_owned(),
        ],
        GitOperation::Diff => {
            let mut args = vec![
                "diff".to_owned(),
                "--no-ext-diff".to_owned(),
                "--no-color".to_owned(),
            ];
            if staged {
                args.push("--cached".to_owned());
            }
            if let Some(base) = base {
                args.push(base.to_owned());
            }
            if let Some(reference) = reference {
                args.push(reference.to_owned());
            }
            args
        }
        GitOperation::Log => vec![
            "log".to_owned(),
            format!("--max-count={max_entries}"),
            "--date=iso-strict".to_owned(),
            "--pretty=format:%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1e".to_owned(),
        ],
        GitOperation::Blame => {
            let mut args = vec!["blame".to_owned(), "--line-porcelain".to_owned()];
            if let Some(start) = start_line {
                let end = end_line.unwrap_or(start.saturating_add(199));
                args.push(format!("-L{start},{end}"));
            }
            if let Some(reference) = reference {
                args.push(reference.to_owned());
            }
            args
        }
    };
    if let Some(path) = path {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "Git path escaped project".to_owned())?;
        args.push("--".to_owned());
        args.push(relative.to_string_lossy().to_string());
    }
    let output = run_process("git", &args, root, Duration::from_secs(60))?;
    if output.exit_code != Some(0) {
        return Err(format!("Git command failed: {}", output.stderr.trim()));
    }
    let details = match operation {
        GitOperation::Log => json!({
            "operation": "log",
            "entries": parse_git_log(&output.stdout),
            "truncated": output.truncated,
        }),
        _ => json!({
            "operation": git_operation_name(operation),
            "output": output.stdout,
            "truncated": output.truncated,
        }),
    };
    let text = details
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string_pretty(&details).unwrap_or_default());
    Ok(tool_result(text, details))
}

fn execute_process_tool(
    tool: &str,
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<Value, String> {
    let output = run_process(program, args, cwd, timeout)?;
    let passed = output.exit_code == Some(0);
    let text = format!(
        "{}\n\nstdout:\n{}\n\nstderr:\n{}",
        if passed {
            "Command completed successfully."
        } else {
            "Command reported failures."
        },
        output.stdout,
        output.stderr,
    );
    Ok(tool_result(
        text,
        json!({
            "tool": tool,
            "passed": passed,
            "exitCode": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr,
            "durationMs": output.duration_ms,
            "truncated": output.truncated,
        }),
    ))
}

fn execute_tabular_data(
    input: DataInput,
    format: DataFormat,
    operation: TableOperation,
    offset: usize,
    limit: usize,
    filter_column: Option<&str>,
    filter_value: Option<&str>,
    filter_mode: FilterMode,
    aggregate: AggregateOperation,
    aggregate_column: Option<&str>,
) -> Result<Value, String> {
    let (source, inferred) = input.read()?;
    let format = effective_data_format(format, inferred, &source)?;
    if !matches!(format, DataFormat::Csv | DataFormat::Tsv | DataFormat::Json) {
        return Err("tabular_data supports CSV, TSV, and JSON in this release".to_owned());
    }
    let table = parse_table(&source, format)?;
    let rows = match operation {
        TableOperation::Preview => table.rows.clone(),
        TableOperation::Filter => {
            let column = filter_column.unwrap_or_default();
            let value = filter_value.unwrap_or_default();
            table
                .rows
                .iter()
                .filter(|row| {
                    let current = row.get(column).map(value_to_cell).unwrap_or_default();
                    match filter_mode {
                        FilterMode::Equals => current.eq_ignore_ascii_case(value),
                        FilterMode::Contains => current
                            .to_ascii_lowercase()
                            .contains(&value.to_ascii_lowercase()),
                    }
                })
                .cloned()
                .collect()
        }
        TableOperation::Aggregate => table.rows.clone(),
    };
    if matches!(operation, TableOperation::Aggregate) {
        let value = aggregate_table(&rows, aggregate, aggregate_column)?;
        return Ok(tool_result(
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            json!({
                "operation": "aggregate",
                "aggregate": aggregate_name(aggregate),
                "column": aggregate_column,
                "rowCount": rows.len(),
                "value": value,
            }),
        ));
    }
    let total_rows = rows.len();
    let page = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let details = json!({
        "operation": if matches!(operation, TableOperation::Filter) { "filter" } else { "preview" },
        "columns": table.columns,
        "rows": page,
        "offset": offset,
        "limit": limit,
        "totalRows": total_rows,
        "hasMore": offset.saturating_add(limit) < total_rows,
    });
    Ok(tool_result(
        serde_json::to_string_pretty(&details).unwrap_or_else(|_| details.to_string()),
        details,
    ))
}

#[derive(Debug)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Map<String, Value>>,
}

fn parse_table(source: &str, format: DataFormat) -> Result<Table, String> {
    match format {
        DataFormat::Csv => parse_delimited_table(source, ','),
        DataFormat::Tsv => parse_delimited_table(source, '\t'),
        DataFormat::Json => parse_json_table(source),
        _ => Err("unsupported table format".to_owned()),
    }
}

fn parse_json_table(source: &str) -> Result<Table, String> {
    let value: Value = serde_json::from_str(source)
        .map_err(|error| format!("failed to parse JSON table: {error}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "JSON table must be an array of objects".to_owned())?;
    let mut columns = Vec::new();
    let mut normalized = Vec::new();
    for row in rows {
        let object = row
            .as_object()
            .ok_or_else(|| "JSON table rows must be objects".to_owned())?;
        for key in object.keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
        normalized.push(object.clone());
    }
    Ok(Table {
        columns,
        rows: normalized,
    })
}

fn parse_delimited_table(source: &str, delimiter: char) -> Result<Table, String> {
    let records = parse_delimited_records(source, delimiter)?;
    let Some(headers) = records.first() else {
        return Ok(Table {
            columns: Vec::new(),
            rows: Vec::new(),
        });
    };
    let columns = headers
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value.trim();
            if value.is_empty() {
                format!("column_{}", index + 1)
            } else {
                value.to_owned()
            }
        })
        .collect::<Vec<_>>();
    let rows = records
        .into_iter()
        .skip(1)
        .filter(|record| record.iter().any(|value| !value.is_empty()))
        .map(|record| {
            columns
                .iter()
                .enumerate()
                .map(|(index, key)| {
                    (
                        key.clone(),
                        Value::String(record.get(index).cloned().unwrap_or_default()),
                    )
                })
                .collect::<Map<_, _>>()
        })
        .collect();
    Ok(Table { columns, rows })
}

fn parse_delimited_records(source: &str, delimiter: char) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = source.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            value if value == delimiter && !quoted => {
                record.push(std::mem::take(&mut field));
            }
            '\n' if !quoted => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            value => field.push(value),
        }
    }
    if quoted {
        return Err("unterminated quoted field in delimited data".to_owned());
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

fn aggregate_table(
    rows: &[Map<String, Value>],
    operation: AggregateOperation,
    column: Option<&str>,
) -> Result<Value, String> {
    if matches!(operation, AggregateOperation::Count) {
        return Ok(json!(rows.len()));
    }
    let column = column.ok_or_else(|| "numeric aggregation requires column".to_owned())?;
    let values = rows
        .iter()
        .filter_map(|row| row.get(column))
        .filter_map(value_as_number)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(format!("column {column} contains no numeric values"));
    }
    let value = match operation {
        AggregateOperation::Count => rows.len() as f64,
        AggregateOperation::Sum => values.iter().sum(),
        AggregateOperation::Average => values.iter().sum::<f64>() / values.len() as f64,
        AggregateOperation::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
        AggregateOperation::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    };
    Ok(json!(value))
}

fn parse_structured_value(source: &str, format: DataFormat) -> Result<Value, String> {
    match format {
        DataFormat::Json => {
            serde_json::from_str(source).map_err(|error| format!("failed to parse JSON: {error}"))
        }
        DataFormat::Yaml => {
            serde_yaml::from_str(source).map_err(|error| format!("failed to parse YAML: {error}"))
        }
        DataFormat::Toml => {
            let value: toml::Value = source
                .parse()
                .map_err(|error| format!("failed to parse TOML: {error}"))?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        DataFormat::Xml => parse_xml_value(source),
        DataFormat::Csv | DataFormat::Tsv => {
            let table = parse_table(source, format)?;
            Ok(Value::Array(
                table.rows.into_iter().map(Value::Object).collect(),
            ))
        }
        DataFormat::Auto => Err("input format could not be inferred".to_owned()),
    }
}

fn render_structured_value(value: &Value, format: DataFormat) -> Result<String, String> {
    match format {
        DataFormat::Json => serde_json::to_string_pretty(value).map_err(|error| error.to_string()),
        DataFormat::Yaml => serde_yaml::to_string(value).map_err(|error| error.to_string()),
        DataFormat::Toml => toml::to_string_pretty(value)
            .map_err(|error| format!("value cannot be represented as TOML: {error}")),
        DataFormat::Xml => Ok(value_to_xml(value)),
        DataFormat::Csv => value_to_delimited(value, ','),
        DataFormat::Tsv => value_to_delimited(value, '\t'),
        DataFormat::Auto => Err("output format cannot be auto".to_owned()),
    }
}

#[derive(Debug)]
struct XmlFrame {
    name: String,
    values: Map<String, Value>,
    text: String,
}

fn parse_xml_value(source: &str) -> Result<Value, String> {
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlFrame> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_string();
                let mut values = Map::new();
                for attribute in event.attributes() {
                    let attribute =
                        attribute.map_err(|error| format!("invalid XML attribute: {error}"))?;
                    let key = format!("@{}", String::from_utf8_lossy(attribute.key.as_ref()));
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| format!("invalid XML attribute value: {error}"))?;
                    values.insert(key, Value::String(value.into_owned()));
                }
                stack.push(XmlFrame {
                    name,
                    values,
                    text: String::new(),
                });
            }
            Ok(Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_string();
                if stack.is_empty() {
                    return Ok(json!({ name: {} }));
                }
                append_xml_child(&mut stack, name, Value::Object(Map::new()))?;
            }
            Ok(Event::Text(event)) => {
                if let Some(frame) = stack.last_mut() {
                    let text = event
                        .decode()
                        .map_err(|error| format!("invalid XML text: {error}"))?;
                    if !text.trim().is_empty() {
                        frame.text.push_str(&decode_html_entities(&text));
                    }
                }
            }
            Ok(Event::GeneralRef(event)) => {
                if let Some(frame) = stack.last_mut() {
                    let reference = event
                        .decode()
                        .map_err(|error| format!("invalid XML entity reference: {error}"))?;
                    frame.text.push_str(&decode_xml_reference(&reference)?);
                }
            }
            Ok(Event::End(_)) => {
                let frame = stack
                    .pop()
                    .ok_or_else(|| "invalid XML nesting".to_owned())?;
                let value = if frame.values.is_empty() && !frame.text.is_empty() {
                    Value::String(frame.text)
                } else {
                    let mut values = frame.values;
                    if !frame.text.is_empty() {
                        values.insert("#text".to_owned(), Value::String(frame.text));
                    }
                    Value::Object(values)
                };
                if stack.is_empty() {
                    return Ok(json!({ frame.name: value }));
                }
                append_xml_child(&mut stack, frame.name, value)?;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("failed to parse XML: {error}")),
        }
    }
    Err("XML document has no root element".to_owned())
}

fn append_xml_child(stack: &mut [XmlFrame], name: String, value: Value) -> Result<(), String> {
    let parent = stack
        .last_mut()
        .ok_or_else(|| "XML document has multiple roots".to_owned())?;
    match parent.values.remove(&name) {
        None => {
            parent.values.insert(name, value);
        }
        Some(Value::Array(mut values)) => {
            values.push(value);
            parent.values.insert(name, Value::Array(values));
        }
        Some(existing) => {
            parent
                .values
                .insert(name, Value::Array(vec![existing, value]));
        }
    }
    Ok(())
}

fn value_to_xml(value: &Value) -> String {
    let mut output = String::new();
    match value {
        Value::Object(object) if object.len() == 1 => {
            let (name, value) = object.iter().next().expect("single XML root");
            write_xml_node(&mut output, name, value);
        }
        _ => write_xml_node(&mut output, "root", value),
    }
    output
}

fn write_xml_node(output: &mut String, name: &str, value: &Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                write_xml_node(output, name, value);
            }
        }
        Value::Object(object) => {
            output.push('<');
            output.push_str(name);
            for (key, value) in object.iter().filter(|(key, _)| key.starts_with('@')) {
                output.push(' ');
                output.push_str(key.trim_start_matches('@'));
                output.push_str("=\"");
                output.push_str(&xml_escape(&value_to_cell(value)));
                output.push('"');
            }
            output.push('>');
            if let Some(text) = object.get("#text") {
                output.push_str(&xml_escape(&value_to_cell(text)));
            }
            for (key, value) in object
                .iter()
                .filter(|(key, _)| !key.starts_with('@') && *key != "#text")
            {
                write_xml_node(output, key, value);
            }
            output.push_str("</");
            output.push_str(name);
            output.push('>');
        }
        _ => {
            output.push('<');
            output.push_str(name);
            output.push('>');
            output.push_str(&xml_escape(&value_to_cell(value)));
            output.push_str("</");
            output.push_str(name);
            output.push('>');
        }
    }
}

fn value_to_delimited(value: &Value, delimiter: char) -> Result<String, String> {
    let rows = value
        .as_array()
        .ok_or_else(|| "CSV/TSV output requires an array of objects".to_owned())?;
    let mut columns = Vec::new();
    for row in rows {
        let object = row
            .as_object()
            .ok_or_else(|| "CSV/TSV rows must be objects".to_owned())?;
        for key in object.keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let separator = delimiter.to_string();
    let mut output = columns
        .iter()
        .map(|value| escape_delimited(value, delimiter))
        .collect::<Vec<_>>()
        .join(&separator);
    for row in rows {
        let object = row.as_object().expect("validated table row");
        output.push('\n');
        output.push_str(
            &columns
                .iter()
                .map(|column| {
                    escape_delimited(
                        &object.get(column).map(value_to_cell).unwrap_or_default(),
                        delimiter,
                    )
                })
                .collect::<Vec<_>>()
                .join(&separator),
        );
    }
    Ok(output)
}

fn query_structured_value<'a>(value: &'a Value, query: &str) -> Result<&'a Value, String> {
    let mut current = value;
    for segment in query
        .trim_matches('.')
        .split('.')
        .filter(|segment| !segment.is_empty())
    {
        let (key, index) = if let Some(open) = segment.find('[') {
            let close = segment
                .strip_suffix(']')
                .ok_or_else(|| format!("invalid query segment: {segment}"))?;
            let index = close[open + 1..]
                .parse::<usize>()
                .map_err(|_| format!("invalid array index in query: {segment}"))?;
            (&segment[..open], Some(index))
        } else {
            (segment, None)
        };
        if !key.is_empty() {
            current = current
                .get(key)
                .ok_or_else(|| format!("query path not found: {query}"))?;
        }
        if let Some(index) = index {
            current = current
                .get(index)
                .ok_or_else(|| format!("query index out of range: {query}"))?;
        }
    }
    Ok(current)
}

fn test_command(
    cwd: &Path,
    runner: &str,
    script: Option<&str>,
    target: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    let runner = if runner == "auto" {
        detect_ecosystem(cwd)?
    } else {
        runner.to_owned()
    };
    match runner.as_str() {
        "rust" => {
            let mut args = vec!["test".to_owned()];
            if let Some(target) = target {
                args.push(target.to_owned());
            }
            Ok(("cargo".to_owned(), args))
        }
        "python" => {
            let mut args = vec!["-m".to_owned(), "pytest".to_owned()];
            if let Some(target) = target {
                args.push(target.to_owned());
            }
            Ok((python_program(), args))
        }
        "node" => {
            let package = read_package_json(cwd)?;
            let scripts = package.get("scripts").and_then(Value::as_object);
            let script = script
                .map(str::to_owned)
                .or_else(|| find_package_script(scripts, &["test", "test:unit", "test:ci"]))
                .ok_or_else(|| "package.json has no supported test script".to_owned())?;
            if script != "test" && !script.starts_with("test:") {
                return Err("test_run only allows test or test:* package scripts".to_owned());
            }
            Ok(package_script_command(cwd, &script))
        }
        value => Err(format!("unsupported test runner: {value}")),
    }
}

fn check_command(
    cwd: &Path,
    ecosystem: &str,
    check: &str,
) -> Result<(String, Vec<String>), String> {
    let ecosystem = if ecosystem == "auto" {
        detect_ecosystem(cwd)?
    } else {
        ecosystem.to_owned()
    };
    match ecosystem.as_str() {
        "rust" => Ok((
            "cargo".to_owned(),
            if check == "lint" {
                vec![
                    "clippy".to_owned(),
                    "--all-targets".to_owned(),
                    "--".to_owned(),
                    "-D".to_owned(),
                    "warnings".to_owned(),
                ]
            } else {
                vec!["check".to_owned(), "--all-targets".to_owned()]
            },
        )),
        "python" => match check {
            "typecheck" => Ok((
                python_program(),
                vec!["-m".to_owned(), "mypy".to_owned(), ".".to_owned()],
            )),
            "auto" | "lint" => Ok((
                python_program(),
                vec![
                    "-m".to_owned(),
                    "ruff".to_owned(),
                    "check".to_owned(),
                    ".".to_owned(),
                ],
            )),
            value => Err(format!("unsupported Python check: {value}")),
        },
        "node" => {
            let package = read_package_json(cwd)?;
            let scripts = package.get("scripts").and_then(Value::as_object);
            let candidates = match check {
                "auto" => &["check", "lint", "typecheck"][..],
                "lint" => &["lint"][..],
                "typecheck" => &["typecheck", "check:types"][..],
                value => return Err(format!("unsupported Node check: {value}")),
            };
            let script = find_package_script(scripts, candidates)
                .ok_or_else(|| format!("package.json has no supported {} script", check))?;
            Ok(package_script_command(cwd, &script))
        }
        value => Err(format!("unsupported code check ecosystem: {value}")),
    }
}

fn format_command(
    cwd: &Path,
    ecosystem: &str,
    mode: &str,
    path: Option<&Path>,
) -> Result<(String, Vec<String>), String> {
    let ecosystem = if ecosystem == "auto" {
        detect_ecosystem(cwd)?
    } else {
        ecosystem.to_owned()
    };
    match ecosystem.as_str() {
        "rust" => {
            let mut args = vec!["fmt".to_owned(), "--all".to_owned()];
            if mode == "check" {
                args.extend(["--".to_owned(), "--check".to_owned()]);
            }
            Ok(("cargo".to_owned(), args))
        }
        "python" => {
            let mut args = vec!["-m".to_owned(), "ruff".to_owned(), "format".to_owned()];
            if mode == "check" {
                args.push("--check".to_owned());
            }
            args.push(path.unwrap_or(cwd).display().to_string());
            Ok((python_program(), args))
        }
        "node" => {
            let package = read_package_json(cwd)?;
            let scripts = package.get("scripts").and_then(Value::as_object);
            let candidates = if mode == "check" {
                &["format:check", "check:format"][..]
            } else {
                &["format", "format:write"][..]
            };
            let script = find_package_script(scripts, candidates).ok_or_else(|| {
                format!("package.json has no supported format script for mode {mode}")
            })?;
            Ok(package_script_command(cwd, &script))
        }
        value => Err(format!("unsupported formatter ecosystem: {value}")),
    }
}

fn detect_ecosystem(cwd: &Path) -> Result<String, String> {
    if cwd.join("Cargo.toml").is_file() {
        return Ok("rust".to_owned());
    }
    if cwd.join("package.json").is_file() {
        return Ok("node".to_owned());
    }
    if cwd.join("pyproject.toml").is_file()
        || cwd.join("pytest.ini").is_file()
        || cwd.join("setup.cfg").is_file()
    {
        return Ok("python".to_owned());
    }
    Err("could not detect Rust, Node, or Python project in cwd".to_owned())
}

fn read_package_json(cwd: &Path) -> Result<Value, String> {
    let source = fs::read_to_string(cwd.join("package.json"))
        .map_err(|error| format!("failed to read package.json: {error}"))?;
    serde_json::from_str(&source).map_err(|error| format!("invalid package.json: {error}"))
}

fn find_package_script(scripts: Option<&Map<String, Value>>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|name| scripts.is_some_and(|scripts| scripts.contains_key(**name)))
        .map(|name| (*name).to_owned())
}

fn package_script_command(cwd: &Path, script: &str) -> (String, Vec<String>) {
    if cwd.join("pnpm-lock.yaml").is_file() {
        (
            windows_command("pnpm"),
            vec!["run".to_owned(), script.to_owned()],
        )
    } else if cwd.join("yarn.lock").is_file() {
        (windows_command("yarn"), vec![script.to_owned()])
    } else if cwd.join("bun.lock").is_file() || cwd.join("bun.lockb").is_file() {
        (
            windows_command("bun"),
            vec!["run".to_owned(), script.to_owned()],
        )
    } else {
        (
            windows_command("npm"),
            vec!["run".to_owned(), script.to_owned()],
        )
    }
}

fn run_process(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<ProcessOutput, String> {
    let start = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to start {}: {error}",
            display_command(program, args)
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture command stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture command stderr".to_owned())?;
    let stdout_reader = thread::spawn(move || read_process_stream(stdout));
    let stderr_reader = thread::spawn(move || read_process_stream(stderr));
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                terminate_process_tree(&mut child);
                return Err(format!("failed to wait for command: {error}"));
            }
        };
        if let Some(status) = status {
            let (stdout, stdout_truncated) = stdout_reader
                .join()
                .map_err(|_| "command stdout reader failed".to_owned())??;
            let (stderr, stderr_truncated) = stderr_reader
                .join()
                .map_err(|_| "command stderr reader failed".to_owned())??;
            let stdout_limit = MAX_PROCESS_OUTPUT_BYTES.min(stdout.len());
            let remaining = MAX_PROCESS_OUTPUT_BYTES.saturating_sub(stdout_limit);
            let stderr_limit = remaining.min(stderr.len());
            return Ok(ProcessOutput {
                exit_code: status.code(),
                stdout: String::from_utf8_lossy(&stdout[..stdout_limit]).into_owned(),
                stderr: String::from_utf8_lossy(&stderr[..stderr_limit]).into_owned(),
                duration_ms: start.elapsed().as_millis(),
                truncated: stdout_truncated || stderr_truncated || stderr.len() > stderr_limit,
            });
        }
        if start.elapsed() >= timeout {
            terminate_process_tree(&mut child);
            return Err(format!(
                "command timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn read_process_stream(mut stream: impl Read) -> Result<(Vec<u8>, bool), String> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| format!("failed to read command output: {error}"))?;
        if read == 0 {
            break;
        }
        let remaining = MAX_PROCESS_OUTPUT_BYTES.saturating_sub(kept.len());
        let take = remaining.min(read);
        kept.extend_from_slice(&buffer[..take]);
        truncated |= take < read;
    }
    Ok((kept, truncated))
}

fn terminate_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_git_log(output: &str) -> Vec<Value> {
    output
        .split('\u{1e}')
        .filter_map(|entry| {
            let fields = entry.trim().split('\u{1f}').collect::<Vec<_>>();
            (fields.len() == 5).then(|| {
                json!({
                    "hash": fields[0],
                    "shortHash": fields[1],
                    "author": fields[2],
                    "date": fields[3],
                    "subject": fields[4],
                })
            })
        })
        .collect()
}

fn parse_data_format(value: &str) -> Result<DataFormat, String> {
    match value {
        "auto" => Ok(DataFormat::Auto),
        "json" => Ok(DataFormat::Json),
        "yaml" | "yml" => Ok(DataFormat::Yaml),
        "toml" => Ok(DataFormat::Toml),
        "xml" => Ok(DataFormat::Xml),
        "csv" => Ok(DataFormat::Csv),
        "tsv" => Ok(DataFormat::Tsv),
        _ => Err(format!("unsupported data format: {value}")),
    }
}

fn effective_data_format(
    requested: DataFormat,
    inferred: Option<DataFormat>,
    source: &str,
) -> Result<DataFormat, String> {
    if requested != DataFormat::Auto {
        return Ok(requested);
    }
    if let Some(inferred) = inferred {
        return Ok(inferred);
    }
    let trimmed = source.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        Ok(DataFormat::Json)
    } else if trimmed.starts_with('<') {
        Ok(DataFormat::Xml)
    } else {
        Err("inputFormat=auto requires a recognized file extension or JSON/XML content".to_owned())
    }
}

fn data_format_name(format: DataFormat) -> &'static str {
    match format {
        DataFormat::Auto => "auto",
        DataFormat::Json => "json",
        DataFormat::Yaml => "yaml",
        DataFormat::Toml => "toml",
        DataFormat::Xml => "xml",
        DataFormat::Csv => "csv",
        DataFormat::Tsv => "tsv",
    }
}

fn prepare_data_input(input: &Value, project_root: Option<&str>) -> Result<DataInput, String> {
    let inline = optional_string(input, "data");
    let path = optional_string(input, "path");
    match (inline, path) {
        (Some(_), Some(_)) => Err("provide either data or path, not both".to_owned()),
        (None, None) => Err("data or path is required".to_owned()),
        (Some(data), None) => {
            if data.len() > MAX_INLINE_DATA_BYTES {
                return Err("inline data exceeds the 4 MiB limit".to_owned());
            }
            Ok(DataInput::Inline(data))
        }
        (None, Some(path)) => {
            let root = canonical_directory(
                Path::new(required_project_root(project_root)?),
                "project folder",
            )?;
            let path = resolve_project_path(&root, &path, true)?;
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xlsx"))
            {
                return Err("XLSX support is planned for the next tabular_data phase; use CSV, TSV, or JSON now".to_owned());
            }
            let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
            if metadata.len() > MAX_INLINE_DATA_BYTES as u64 {
                return Err("data file exceeds the 4 MiB limit".to_owned());
            }
            Ok(DataInput::File(path))
        }
    }
}

impl DataInput {
    fn label(&self) -> String {
        match self {
            Self::Inline(data) => format!("内联数据（{} 字节）", data.len()),
            Self::File(path) => path.display().to_string(),
        }
    }

    fn read(&self) -> Result<(String, Option<DataFormat>), String> {
        match self {
            Self::Inline(data) => Ok((data.clone(), None)),
            Self::File(path) => {
                let source = fs::read_to_string(path)
                    .map_err(|error| format!("failed to read data file: {error}"))?;
                let inferred = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .and_then(|extension| parse_data_format(&extension.to_ascii_lowercase()).ok());
                Ok((source, inferred))
            }
        }
    }
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("failed to resolve {label}: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(canonical)
}

fn resolve_project_path(root: &Path, requested: &str, must_exist: bool) -> Result<PathBuf, String> {
    let requested = Path::new(requested);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err("path must be relative and cannot contain parent traversal".to_owned());
    }
    let joined = root.join(requested);
    let resolved = if must_exist {
        fs::canonicalize(&joined)
            .map_err(|error| format!("failed to resolve project path: {error}"))?
    } else {
        joined
    };
    if !resolved.starts_with(root) {
        return Err("path escapes the authorized project".to_owned());
    }
    Ok(resolved)
}

fn resolve_cwd(root: &Path, requested: Option<&str>) -> Result<PathBuf, String> {
    match requested {
        Some(path) => {
            let path = resolve_project_path(root, path, true)?;
            canonical_directory(&path, "working directory")
        }
        None => Ok(root.to_path_buf()),
    }
}

fn required_project_root(project_root: Option<&str>) -> Result<&str, String> {
    project_root.ok_or_else(|| "this tool requires an authorized project folder".to_owned())
}

fn validated_git_ref(value: Option<String>) -> Result<Option<String>, String> {
    if value
        .as_deref()
        .is_some_and(|value| value.starts_with('-') || value.contains(char::is_whitespace))
    {
        return Err("Git refs cannot start with '-' or contain whitespace".to_owned());
    }
    Ok(value)
}

fn required_string(input: &Value, key: &str) -> Result<String, String> {
    optional_string(input, key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn optional_string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn optional_usize(input: &Value, key: &str) -> Option<usize> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn optional_bool(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

fn string_array(input: &Value, key: &str) -> Result<Vec<String>, String> {
    let Some(values) = input.get(key) else {
        return Ok(Vec::new());
    };
    values
        .as_array()
        .ok_or_else(|| format!("{key} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} entries must be non-empty strings"))
        })
        .collect()
}

fn process_timeout(input: &Value) -> Duration {
    Duration::from_secs(
        input
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_PROCESS_SECONDS)
            .clamp(1, MAX_PROCESS_SECONDS),
    )
}

fn env_non_empty(key: &str) -> bool {
    env::var(key).is_ok_and(|value| !value.trim().is_empty())
}

fn search_provider_name(provider: SearchProvider) -> &'static str {
    match provider {
        SearchProvider::Auto => "auto",
        SearchProvider::Brave => "brave",
        SearchProvider::Tavily => "tavily",
        SearchProvider::Exa => "exa",
        SearchProvider::Searxng => "searxng",
        SearchProvider::DuckDuckGo => "duckduckgo",
    }
}

fn git_operation_name(operation: GitOperation) -> &'static str {
    match operation {
        GitOperation::Status => "status",
        GitOperation::Diff => "diff",
        GitOperation::Log => "log",
        GitOperation::Blame => "blame",
    }
}

fn aggregate_name(operation: AggregateOperation) -> &'static str {
    match operation {
        AggregateOperation::Count => "count",
        AggregateOperation::Sum => "sum",
        AggregateOperation::Average => "avg",
        AggregateOperation::Min => "min",
        AggregateOperation::Max => "max",
    }
}

fn value_as_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn value_to_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn escape_delimited(value: &str, delimiter: char) -> String {
    if value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn decode_xml_reference(reference: &str) -> Result<String, String> {
    let character = match reference {
        "amp" => return Ok("&".to_owned()),
        "lt" => return Ok("<".to_owned()),
        "gt" => return Ok(">".to_owned()),
        "quot" => return Ok("\"".to_owned()),
        "apos" => return Ok("'".to_owned()),
        value if value.starts_with("#x") => u32::from_str_radix(&value[2..], 16).ok(),
        value if value.starts_with('#') => value[1..].parse::<u32>().ok(),
        _ => None,
    }
    .and_then(char::from_u32)
    .ok_or_else(|| format!("unsupported XML entity reference: &{reference};"))?;
    Ok(character.to_string())
}

fn html_title(source: &str) -> Option<String> {
    let lowercase = source.to_ascii_lowercase();
    let open = lowercase.find("<title")?;
    let content_start = lowercase[open..].find('>')? + open + 1;
    let content_end = lowercase[content_start..].find("</title>")? + content_start;
    let title = decode_html_entities(&source[content_start..content_end])
        .trim()
        .to_owned();
    (!title.is_empty()).then_some(title)
}

fn html_to_text(source: &str) -> String {
    let source = strip_html_section(source, "script");
    let source = strip_html_section(&source, "style");
    let mut output = String::with_capacity(source.len());
    let mut in_tag = false;
    for character in source.chars() {
        match character {
            '<' => {
                in_tag = true;
                output.push(' ');
            }
            '>' => in_tag = false,
            value if !in_tag => output.push(value),
            _ => {}
        }
    }
    normalize_whitespace(&decode_html_entities(&output))
}

fn html_to_markdown(source: &str) -> String {
    let mut value = source.to_owned();
    for (tag, replacement) in [
        ("</p>", "\n\n"),
        ("<br>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("</li>", "\n"),
        ("</h1>", "\n\n"),
        ("</h2>", "\n\n"),
        ("</h3>", "\n\n"),
    ] {
        value = replace_ascii_case_insensitive(&value, tag, replacement);
    }
    html_to_text(&value)
}

fn strip_html_section(source: &str, tag: &str) -> String {
    let mut result = source.to_owned();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lowercase = result.to_ascii_lowercase();
        let Some(start) = lowercase.find(&open) else {
            break;
        };
        let Some(relative_end) = lowercase[start..].find(&close) else {
            result.truncate(start);
            break;
        };
        let end = start + relative_end + close.len();
        result.replace_range(start..end, " ");
    }
    result
}

fn replace_ascii_case_insensitive(source: &str, needle: &str, replacement: &str) -> String {
    let mut result = source.to_owned();
    let needle_lower = needle.to_ascii_lowercase();
    let mut start = 0;
    loop {
        let lowercase = result[start..].to_ascii_lowercase();
        let Some(relative) = lowercase.find(&needle_lower) else {
            break;
        };
        let index = start + relative;
        result.replace_range(index..index + needle.len(), replacement);
        start = index + replacement.len();
    }
    result
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_whitespace(value: &str) -> String {
    let mut output = String::new();
    let mut spaces = 0usize;
    let mut newlines = 0usize;
    for character in value.chars() {
        match character {
            '\r' => {}
            '\n' => {
                spaces = 0;
                newlines = (newlines + 1).min(2);
            }
            value if value.is_whitespace() => {
                if newlines > 0 {
                    continue;
                }
                spaces = 1;
            }
            value => {
                if newlines > 0 {
                    for _ in 0..newlines {
                        output.push('\n');
                    }
                } else if spaces > 0 && !output.is_empty() {
                    output.push(' ');
                }
                newlines = 0;
                spaces = 0;
                output.push(value);
            }
        }
    }
    output.trim().to_owned()
}

fn tool_result(text: String, details: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

fn display_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_owned())
        .chain(args.iter().map(|arg| {
            if arg.contains(char::is_whitespace) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

fn windows_command(program: &str) -> String {
    #[cfg(windows)]
    {
        return format!("{program}.cmd");
    }
    #[cfg(not(windows))]
    {
        program.to_owned()
    }
}

fn python_program() -> String {
    #[cfg(windows)]
    {
        "python.exe".to_owned()
    }
    #[cfg(not(windows))]
    {
        "python3".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_tool_catalog_is_stable() {
        assert_eq!(CAPABILITY_TOOLS.len(), 11);
        assert!(is_capability_tool("web_search"));
        assert!(is_capability_tool("http_request"));
        assert!(is_capability_tool("system_info"));
        assert!(is_capability_tool("sqlite_read"));
        assert!(is_capability_tool("tabular_data"));
        assert!(!is_capability_tool("run_command"));
    }

    #[test]
    fn generic_http_headers_never_accept_credentials() {
        assert!(prepare_http_headers(Some(&json!({
            "accept": "application/json",
            "if-none-match": "revision-1",
        })))
        .is_ok());
        for header in ["authorization", "cookie", "x-api-key", "host"] {
            let error = prepare_http_headers(Some(&json!({ (header): "secret" })))
                .expect_err("credentials and routing headers must be rejected");
            assert!(error.contains("Keyring-backed OpenAPI Connector"));
        }
        assert!(prepare_http_headers(Some(&json!({
            "accept": "text/plain\r\nx-injected: true",
        })))
        .is_err());
    }

    #[test]
    fn system_info_uses_a_fixed_environment_allowlist() {
        let output = execute_system_info(false, 1, true).expect("collect system information");
        let environment = output["environment"]
            .as_object()
            .expect("environment object");
        let allowed = [
            "NODE_ENV",
            "PATH",
            "HOME",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "SHELL",
            "COMSPEC",
        ];
        assert!(environment
            .keys()
            .all(|key| allowed.contains(&key.as_str())));
        assert!(output["processes"].as_array().unwrap().is_empty());
        assert!(output["memory"]["totalBytes"].as_u64().is_some());
    }

    #[test]
    fn sqlite_query_is_read_only_bounded_and_typed() {
        let path = env::temp_dir().join(format!("fox-capability-{}.sqlite", uuid::Uuid::new_v4()));
        let connection = Connection::open(&path).expect("create SQLite fixture");
        connection
            .execute_batch(
                "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT, active INTEGER);\
                 INSERT INTO items(name, active) VALUES ('Fox', 1), ('Pi', 0);",
            )
            .expect("seed SQLite fixture");
        drop(connection);

        let result = execute_sqlite_read(
            &path,
            "SELECT id, name, active FROM items WHERE active = ? ORDER BY id",
            &[rusqlite::types::Value::Integer(1)],
            1_000,
            Duration::from_secs(5),
        )
        .expect("read SQLite fixture");
        assert_eq!(result["rowCount"], 1);
        assert_eq!(result["rows"][0]["name"], "Fox");
        assert_eq!(result["limits"]["readOnly"], true);

        let error = execute_sqlite_read(
            &path,
            "DELETE FROM items",
            &[],
            1_000,
            Duration::from_secs(5),
        )
        .expect_err("writes must be rejected");
        assert!(error.contains("read-only"));
        let remaining: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn blocks_private_web_addresses() {
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.4".parse().unwrap()));
        assert!(!is_public_ip("::1".parse().unwrap()));
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn parses_quoted_csv_and_aggregates() {
        let table = parse_delimited_table("name,score\n\"Fox, Agent\",2\nPi,4", ',').unwrap();
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].get("name").unwrap(), "Fox, Agent");
        assert_eq!(
            aggregate_table(&table.rows, AggregateOperation::Average, Some("score")).unwrap(),
            json!(3.0),
        );
    }

    #[test]
    fn queries_nested_structured_values() {
        let value = json!({ "items": [{ "name": "Fox" }] });
        assert_eq!(
            query_structured_value(&value, "items[0].name").unwrap(),
            &json!("Fox"),
        );
    }

    #[test]
    fn parses_xml_without_external_entities() {
        let value =
            parse_xml_value("<root><item id=\"1\">Fox</item><item>Pi</item></root>").unwrap();
        assert_eq!(value["root"]["item"].as_array().unwrap().len(), 2);
        assert_eq!(parse_xml_value("<root/>").unwrap(), json!({ "root": {} }));
        assert_eq!(
            parse_xml_value("<root>Fox &amp; Pi</root>").unwrap(),
            json!({ "root": "Fox & Pi" }),
        );
        assert!(parse_xml_value("<root>&external;</root>").is_err());
    }

    #[test]
    fn extracts_html_title_without_markup() {
        assert_eq!(
            html_title("<html><TITLE>Fox &amp; Pi</TITLE></html>").as_deref(),
            Some("Fox & Pi"),
        );
    }

    #[test]
    fn applies_domain_filters_to_search_syntax() {
        assert_eq!(
            search_query_with_domains(
                "Fox Agent",
                &["example.com".to_owned(), "docs.rs".to_owned()],
                &["spam.test".to_owned()],
            ),
            "Fox Agent (site:example.com OR site:docs.rs) -site:spam.test",
        );
    }

    #[test]
    fn parses_duckduckgo_html_results_and_unwraps_links() {
        let source = r#"
            <div class="result results_links">
              <h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Ffox&amp;rut=abc">Fox &amp; Pi</a></h2>
              <a class="result__snippet">A <b>useful</b> search result.</a>
            </div>
            <div class="result results_links">
              <h2><a class="result__a" href="http://127.0.0.1/private">Private result</a></h2>
            </div>
        "#;
        let results = parse_duckduckgo_results(source, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Fox & Pi");
        assert_eq!(results[0]["url"], "https://example.com/fox");
        assert_eq!(results[0]["content"], "A useful search result.");
    }

    #[test]
    fn detects_duckduckgo_challenge_pages_instead_of_reporting_no_results() {
        assert!(duckduckgo_challenge_page(
            r#"<form id="challenge-form"><div class="anomaly-modal">blocked</div></form>"#,
        ));
        assert!(!duckduckgo_challenge_page(
            r#"<a class="result__a" href="https://example.com">Example</a>"#,
        ));
    }

    #[test]
    fn parses_yahoo_html_results_and_unwraps_redirects() {
        let source = r#"
            <div class="dd algo algo-sr">
              <div class="compTitle">
                <a href="https://r.search.yahoo.com/_ylt=x/RU=https%3A%2F%2Fexample.com%2Fnews%3Fa%3D1/RK=2/RS=x">
                  <h3><span>上港集团：选举于福林为董事长</span></h3>
                </a>
              </div>
              <div class="compText aAbs"><p>公司公告的相关摘要。</p></div>
            </div>
            <div class="dd algo algo-sr">
              <a href="http://127.0.0.1/private"><h3>Private result</h3></a>
            </div>
        "#;
        let results = parse_yahoo_results(source, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "上港集团：选举于福林为董事长");
        assert_eq!(results[0]["url"], "https://example.com/news?a=1");
        assert_eq!(results[0]["content"], "公司公告的相关摘要。");
    }

    #[test]
    fn accepts_explicit_duckduckgo_provider_without_configuration() {
        let prepared = prepare_web_search(&json!({
            "query": "Fox Agent",
            "provider": "duckduckgo",
        }))
        .unwrap();
        assert!(matches!(
            prepared,
            PreparedCapabilityTool::WebSearch {
                provider: SearchProvider::DuckDuckGo,
                ..
            }
        ));
        assert!(ensure_provider_configured(SearchProvider::DuckDuckGo).is_ok());
    }
}

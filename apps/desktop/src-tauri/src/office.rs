//! Managed Office connector. The protocol catalog is shared with MCP, while typed
//! arguments are translated to a pinned CLI without a shell or arbitrary flags.
use crate::database::{Database, McpServerRecord};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

pub const SERVER_ID: &str = "fox-office";
pub const VERSION: &str = "1.0.147";
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
static EXECUTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn release() -> Value {
    serde_json::from_str(include_str!("../resources/officecli/release.json"))
        .expect("pinned Office release")
}

fn target() -> String {
    let os = if cfg!(windows) {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    format!("{os}-{arch}")
}

pub fn verify_binary(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|_| "Office 组件缺失，请修复 Fox 安装或运行 prepare:office。".to_owned())?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
        return Err("Office 组件大小异常".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let expected = release()["assets"][target()]["sha256"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if hex::encode(Sha256::digest(&bytes)) != expected {
        return Err("Office 组件完整性校验失败，请修复 Fox 安装。".to_owned());
    }
    Ok(())
}

pub fn setup(database: &Database, resource_dir: &Path) -> Result<(), String> {
    let filename = if cfg!(windows) {
        "officecli.exe"
    } else {
        "officecli"
    };
    let bundled = resource_dir.join("officecli").join(filename);
    let binary = if bundled.is_file() {
        bundled
    } else if cfg!(debug_assertions) {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/officecli")
            .join(filename)
    } else {
        bundled
    };
    let error = verify_binary(&binary).err();
    database.ensure_office_connector(&binary.to_string_lossy(), error.as_deref())
}

pub fn tool_definitions() -> Vec<Value> {
    let file = json!({"type":"string", "description":"Office file path inside the current authorized project (.docx/.xlsx/.pptx)."});
    let output = json!({"type":"string", "description":"Output path inside the project. Use a new file; overwrite requires explicit true."});
    let mut tools = vec![
        ("office_help", "Read Office format/property reference. No document access.", json!({"format":{"type":"string","enum":["docx","xlsx","pptx"]},"element":{"type":"string"}}), vec!["format"]),
        ("office_read", "Read an Office document as text, outline, stats, or a structured node/query. No changes.", json!({"file":file,"mode":{"type":"string","enum":["text","outline","stats","get","query"]},"selector":{"type":"string"}}), vec!["file"]),
        ("office_create", "Create a DOCX/XLSX/PPTX, optionally copying an existing template. Returns a saved file.", json!({"output":output,"template":file,"overwrite":{"type":"boolean","default":false}}), vec!["output"]),
        ("office_edit", "Apply an atomic batch of add/set/remove to a COPY of a document. operations: [{command, path, type?, props?}]. Use office_help for element properties. Source is preserved unless output is the same path and overwrite=true. No shell/raw XML/plugins/network assets.", json!({"file":file,"output":output,"overwrite":{"type":"boolean","default":false},"operations":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"object","additionalProperties":false,"required":["command","path"],"properties":{"command":{"type":"string","enum":["add","set","remove"]},"path":{"type":"string"},"type":{"type":"string"},"props":{"type":"object","additionalProperties":{"type":["string","number","boolean"]}}}}}}), vec!["file","output","operations"]),
        ("office_merge", "Fill {{key}} placeholders in a template with inline data and save a copy.", json!({"file":file,"output":output,"overwrite":{"type":"boolean","default":false},"data":{"type":"object","additionalProperties":{"type":["string","number","boolean"]}}}), vec!["file","output","data"]),
        ("office_render", "Render an Office document to HTML or PNG in the project. PNG requires a supported local browser; return a clear error when unavailable. This is not proof of Microsoft Office rendering equivalence.", json!({"file":file,"output":output,"mode":{"type":"string","enum":["html","screenshot"]},"page":{"type":"string"}}), vec!["file","output","mode"]),
        ("office_validate", "Validate OpenXML structure of an existing document. Does not certify visual layout or numerical correctness.", json!({"file":file}), vec!["file"]),
    ].into_iter().map(|(name, description, properties, required)| json!({
        "name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}
    })).collect::<Vec<_>>();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    tools
}

#[derive(Debug)]
pub struct PreparedOffice {
    pub mutates: bool,
    args: Vec<String>,
    source: Option<PathBuf>,
    output: Option<PathBuf>,
    overwrite: bool,
    render: bool,
    screenshot: bool,
    root: PathBuf,
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Office 参数 {key} 不能为空"))
}

fn document(path: &Path) -> Result<(), String> {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "docx" | "xlsx" | "pptx" => Ok(()),
        _ => Err("首版 Office 连接器仅支持 DOCX、XLSX、PPTX".to_owned()),
    }
}

fn scoped_path(root: &Path, value: &str, must_exist: bool) -> Result<PathBuf, String> {
    if value.contains('\0')
        || value.contains("://")
        || value.starts_with("\\\\")
        || value.starts_with("//")
    {
        return Err("Office 路径必须位于授权项目，不能是网络路径".to_owned());
    }
    let path = Path::new(value);
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err("Office 路径不能包含 ..".to_owned());
        }
        if let Component::Normal(part) = component {
            let text = part.to_string_lossy();
            let stem = text
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            if text.contains(':')
                || text.ends_with(['.', ' '])
                || matches!(
                    stem.as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
            {
                return Err("Office 路径包含保留名称或数据流".to_owned());
            }
        }
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let resolved = if candidate.exists() {
        let metadata = fs::metadata(&candidate).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_FILE {
            return Err("Office 文件不是普通文件或超过 64 MiB".to_owned());
        }
        candidate.canonicalize().map_err(|e| e.to_string())?
    } else {
        if must_exist {
            return Err("Office 输入文件不存在".to_owned());
        }
        let parent = candidate
            .parent()
            .ok_or("Office 输出目录无效")?
            .canonicalize()
            .map_err(|_| "请先在授权项目内创建输出目录".to_owned())?;
        parent.join(candidate.file_name().ok_or("Office 文件名无效")?)
    };
    if !resolved.starts_with(root) {
        return Err("Office 文件超出授权项目（包括符号链接目标）".to_owned());
    }
    Ok(resolved)
}

fn safe_properties(value: &Value) -> Result<(), String> {
    let props = value.as_object().ok_or("Office props/data 必须是对象")?;
    if props.len() > 100 {
        return Err("Office 属性过多".to_owned());
    }
    for (key, value) in props {
        if !value.is_string() && !value.is_number() && !value.is_boolean() {
            return Err("Office 属性只接受字符串、数字或布尔值".to_owned());
        }
        let key_lower = key.to_ascii_lowercase();
        // File/URI properties and raw embedding are intentionally not part of this
        // initial managed surface. Text may contain links without fetching them.
        if [
            "src", "path", "file", "url", "uri", "link", "embed", "ole", "copyfrom", "image",
            "video", "audio", "xml", "action",
        ]
        .iter()
        .any(|needle| key_lower.contains(needle))
        {
            return Err(format!(
                "Office 属性 {key} 涉及外部资源或低层操作，首版未开放"
            ));
        }
        let content = value.as_str().unwrap_or_default();
        if content.len() > 32_000 || content.contains('\0') {
            return Err("Office 属性内容过长或包含空字符".to_owned());
        }
        if key_lower.contains("formula") || content.trim_start().starts_with('=') {
            let formula = content.to_ascii_uppercase();
            if formula.contains('[')
                || [
                    "WEBSERVICE",
                    "HYPERLINK",
                    "IMAGE(",
                    "RTD(",
                    "DDE",
                    "://",
                    "\\\\",
                    "|",
                ]
                .iter()
                .any(|s| formula.contains(s))
            {
                return Err("Office 首版不执行外部引用、网络或 DDE 公式".to_owned());
            }
        }
    }
    Ok(())
}

pub fn prepare(
    tool: &str,
    input: &Value,
    root: Option<&str>,
    permission: &str,
) -> Result<PreparedOffice, String> {
    let definition = tool_definitions()
        .into_iter()
        .find(|v| v["name"] == tool)
        .ok_or("未知 Office 工具")?;
    jsonschema::validator_for(&definition["inputSchema"])
        .map_err(|e| e.to_string())?
        .validate(input)
        .map_err(|e| format!("Office 参数无效: {e}"))?;
    let mut action = PreparedOffice {
        mutates: false,
        args: vec![],
        source: None,
        output: None,
        overwrite: input["overwrite"].as_bool().unwrap_or(false),
        render: false,
        screenshot: false,
        root: PathBuf::new(),
    };
    if tool == "office_help" {
        action.args = vec!["help".into(), string(input, "format")?.into()];
        if let Some(element) = input["element"].as_str() {
            if element.is_empty()
                || !element
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return Err("无效的 Office 元素名称".into());
            }
            action.args.push(element.into());
        }
        action.args.push("--json".into());
        return Ok(action);
    }
    action.root = Path::new(root.ok_or("请先为会话选择授权项目目录，再使用 Office 文件能力")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !action.root.is_dir() {
        return Err("Office 项目路径不是目录".into());
    }
    if input.get("file").is_some() {
        let source = scoped_path(&action.root, string(input, "file")?, true)?;
        document(&source)?;
        action.source = Some(source);
    }
    if input.get("output").is_some() {
        let output = scoped_path(&action.root, string(input, "output")?, false)?;
        if output.exists() && !action.overwrite {
            return Err("Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true".into());
        }
        if tool == "office_render" {
            let expected = if input["mode"] == "html" {
                "html"
            } else {
                "png"
            };
            if output.extension().and_then(|s| s.to_str()) != Some(expected) {
                return Err(format!("预览输出扩展名必须为 .{expected}"));
            }
        } else {
            document(&output)?;
            if let Some(source) = &action.source {
                if source.extension() != output.extension() {
                    return Err("Office 编辑不进行格式转换，输入输出扩展名必须一致".into());
                }
            }
        }
        action.output = Some(output);
        action.mutates = true;
    }
    if action.mutates && permission == "read_only" {
        return Err("当前项目为只读，不能创建或修改 Office 文档".into());
    }
    let source = action
        .source
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    match tool {
        "office_read" => {
            let mode = input["mode"].as_str().unwrap_or("text");
            if let Some(selector) = input["selector"].as_str() {
                if selector.starts_with('-') || selector.contains('\0') || selector.len() > 4096 {
                    return Err("Office 查询条件无效".into());
                }
            }
            action.args = if matches!(mode, "get" | "query") {
                vec![
                    mode.into(),
                    source,
                    string(input, "selector")?.into(),
                    "--json".into(),
                ]
            } else {
                vec!["view".into(), source, mode.into(), "--json".into()]
            };
        }
        "office_validate" => action.args = vec!["validate".into(), source, "--json".into()],
        "office_create" => {
            if let Some(template) = input["template"].as_str() {
                let path = scoped_path(&action.root, template, true)?;
                document(&path)?;
                if path.extension() != action.output.as_ref().and_then(|p| p.extension()) {
                    return Err("模板与输出格式必须一致".into());
                }
                action.source = Some(path);
            }
            action.args = vec!["create".into(), "{output}".into(), "--json".into()];
        }
        "office_edit" => {
            for op in input["operations"].as_array().ok_or("缺少 operations")? {
                let selector = string(op, "path")?;
                if !selector.starts_with('/') || selector.len() > 4096 {
                    return Err("Office 元素路径必须以 / 开头且不超过 4096 字符".into());
                }
                if let Some(kind) = op["type"].as_str() {
                    if ![
                        "paragraph",
                        "run",
                        "table",
                        "row",
                        "cell",
                        "section",
                        "style",
                        "header",
                        "footer",
                        "slide",
                        "shape",
                        "sheet",
                        "chart",
                        "comment",
                        "notes",
                        "pivottable",
                        "namedrange",
                        "conditionalformatting",
                        "datavalidation",
                    ]
                    .contains(&kind)
                    {
                        return Err(format!("Office 首版尚未开放元素类型 {kind}"));
                    }
                }
                if let Some(props) = op.get("props") {
                    safe_properties(props)?;
                }
            }
            action.args = vec![
                "batch".into(),
                "{output}".into(),
                "--commands".into(),
                input["operations"].to_string(),
                "--json".into(),
            ];
        }
        "office_merge" => {
            // Data values are literal template content, not file names or flags.
            for v in input["data"].as_object().ok_or("data 必须为对象")?.values() {
                if v.to_string().len() > 32_000 {
                    return Err("合并数据单项过长".into());
                }
            }
            action.args = vec![
                "merge".into(),
                source,
                "{output}".into(),
                "--data".into(),
                input["data"].to_string(),
                "--json".into(),
            ];
        }
        "office_render" => {
            action.render = true;
            action.screenshot = input["mode"] == "screenshot";
            let page = input["page"].as_str().unwrap_or("1");
            if page.is_empty()
                || page.len() > 32
                || !page
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-' || c == ',')
            {
                return Err("Office 页码格式无效".into());
            }
            action.args = vec![
                "view".into(),
                source,
                string(input, "mode")?.into(),
                "--out".into(),
                "{output}".into(),
                "--page".into(),
                page.into(),
            ];
            if action.screenshot {
                action.args.extend(["--render".into(), "html".into()]);
            }
        }
        _ => return Err("未知 Office 工具".into()),
    }
    Ok(action)
}

fn invoke(binary: &Path, args: &[String], cwd: Option<&Path>, cancellation: Option<&crate::kernel::CancellationToken>, deadline: Instant) -> Result<String, String> {
    if let Some(token) = cancellation { token.check()?; }
    if Instant::now() >= deadline { return Err("Office execution budget exceeded".into()); }
    let mut command = Command::new(binary);
    command
        .args(args)
        .env("OFFICECLI_SKIP_UPDATE", "1")
        .env("OFFICECLI_NO_AUTO_RESIDENT", "1")
        .env("OFFICECLI_RESIDENT_FLUSH", "each")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Office 启动失败: {e}"))?;
    let stdout = child.stdout.take().ok_or("Office stdout 不可用")?;
    let stderr = child.stderr.take().ok_or("Office stderr 不可用")?;
    let capture = |pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            // Continue draining after the capture limit to avoid a full-pipe deadlock.
            let mut reader = pipe;
            let mut buffer = [0u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let keep = count.min(MAX_OUTPUT.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..keep]);
            }
            bytes
        })
    };
    let out = capture(Box::new(stdout));
    let err = capture(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        if cancellation.is_some_and(|token| token.is_cancelled()) || Instant::now() >= deadline {
            crate::tool_host::terminate_process_tree(&mut child);
            let _ = out.join();
            let _ = err.join();
            return Err("Office execution cancelled or budget exceeded; owned process stopped".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {},
            Err(error) => {
                crate::tool_host::terminate_process_tree(&mut child);
                let _ = out.join();
                let _ = err.join();
                return Err(format!("Office process status failed: {error}"));
            }
        }
        if start.elapsed() > Duration::from_secs(90) {
            crate::tool_host::terminate_process_tree(&mut child);
            let _ = out.join();
            let _ = err.join();
            return Err("Office 操作超过 90 秒，已终止".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(format!("Office 操作失败 ({status}): {stderr}\n{stdout}"));
    }
    Ok(stdout)
}

pub fn execute(
    server: &McpServerRecord,
    tool: &str,
    input: &Value,
    root: Option<&str>,
    permission: &str,
) -> Result<Value, String> {
    execute_with_cancellation(server, tool, input, root, permission, None, Duration::from_secs(270))
}

pub(crate) fn execute_with_cancellation(
    server: &McpServerRecord, tool: &str, input: &Value, root: Option<&str>, permission: &str,
    cancellation: Option<&crate::kernel::CancellationToken>, budget: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + budget;
    let check = || -> Result<(),String> {
        if let Some(token) = cancellation { token.check()?; }
        if Instant::now() >= deadline { return Err("Office execution budget exceeded".into()); }
        Ok(())
    };
    check()?;
    if server.id != SERVER_ID || !server.enabled {
        return Err("Office 连接器未启用".into());
    }
    let lock = EXECUTION_LOCK.get_or_init(|| Mutex::new(()));
    let _lock = loop {
        check()?;
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(20)),
            Err(_) => return Err("Office 执行锁不可用".into()),
        }
    };
    let binary = Path::new(&server.command);
    verify_binary(binary)?;
    check()?;
    // Recheck all paths and permissions immediately before execution.
    let action = prepare(tool, input, root, permission)?;
    let cwd = (!action.root.as_os_str().is_empty()).then_some(action.root.as_path());
    let mut temporary = None;
    if let Some(output) = &action.output {
        let ext = output.extension().and_then(|s| s.to_str()).unwrap_or("tmp");
        let path = output.with_file_name(format!(".fox-office-{}.{}", uuid::Uuid::new_v4(), ext));
        if matches!(tool, "office_edit" | "office_create") {
            if let Some(source) = &action.source {
                fs::copy(source, &path).map_err(|e| e.to_string())?;
            }
        }
        temporary = Some(path);
    }
    let result = (|| {
        let args = action
            .args
            .iter()
            .map(|a| {
                if a == "{output}" {
                    temporary
                        .as_ref()
                        .expect("output")
                        .to_string_lossy()
                        .into_owned()
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>();
        let stdout = if tool == "office_create" && action.source.is_some() {
            "Template copied".to_owned()
        } else {
            invoke(binary, &args, cwd, cancellation, deadline)?
        };
        let mut validation = None;
        if let Some(temp) = &temporary {
            if !temp.is_file() || fs::metadata(temp).map_err(|e| e.to_string())?.len() > MAX_FILE {
                return Err("Office 未生成有效输出或输出过大".into());
            }
            if !action.render {
                validation = Some(invoke(
                    binary,
                    &[
                        "validate".into(),
                        temp.to_string_lossy().into_owned(),
                        "--json".into(),
                    ],
                    cwd,
                    cancellation, deadline,
                )?);
                // A second open proves the saved document can actually be read.
                invoke(
                    binary,
                    &[
                        "view".into(),
                        temp.to_string_lossy().into_owned(),
                        "stats".into(),
                        "--json".into(),
                    ],
                    cwd,
                    cancellation, deadline,
                )?;
            }
            let output = action.output.as_ref().expect("output");
            check()?;
            scoped_path(
                &action.root,
                &output
                    .strip_prefix(&action.root)
                    .map_err(|_| "Office 输出超出项目")?
                    .to_string_lossy(),
                false,
            )?;
            if output.exists() {
                if !action.overwrite {
                    return Err("Office 输出在执行期间出现，已保留原文件".into());
                }
                let backup = output.with_file_name(format!(
                    "{}.fox-backup-{}",
                    output.file_name().unwrap_or_default().to_string_lossy(),
                    uuid::Uuid::new_v4()
                ));
                fs::copy(output, &backup).map_err(|e| format!("Office 备份失败: {e}"))?;
                fs::copy(temp, output).map_err(|e| {
                    format!("Office 写入失败，原文件备份位于 {}: {e}", backup.display())
                })?;
            } else {
                fs::rename(temp, output).map_err(|e| e.to_string())?;
            }
        }
        let mut content = vec![
            json!({"type":"text","text":json!({"output":action.output,"result":stdout,"validation":validation,"saved":action.output.is_some(),"visualChecked":false}).to_string()}),
        ];
        if action.screenshot {
            let bytes = fs::read(action.output.as_ref().expect("screenshot output"))
                .map_err(|e| e.to_string())?;
            if bytes.len() <= MAX_OUTPUT {
                content.push(json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            }
        }
        Ok(json!({"content":content,"isError":false}))
    })();
    if let Some(temp) = temporary {
        if temp.is_file() {
            let _ = fs::remove_file(temp);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_office_process_cancellation_reaps_owned_process() {
        use crate::kernel::CancellationPort;
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("office-cancel").unwrap();
        let token = registry.run_token("office-cancel").unwrap();
        let cancel = registry.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.request_run_cancel("office-cancel");
        });
        let start = Instant::now();
        let result = invoke(Path::new("node"), &["-e".into(), "setInterval(()=>{},1000)".into()],
            None, Some(&token), start + Duration::from_secs(15));
        sender.join().unwrap();
        assert!(result.unwrap_err().contains("owned process stopped"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_documents_roundtrip_through_managed_connector() {
        let root = project();
        let db = Database::open(root.join("test.db")).unwrap();
        setup(
            &db,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources"),
        )
        .unwrap();
        let server = db
            .list_mcp_servers()
            .unwrap()
            .into_iter()
            .find(|s| s.id == SERVER_ID)
            .unwrap();
        let cases = [
            (
                "docx",
                json!([{"command":"add","path":"/body","type":"paragraph","props":{"text":"Fox roundtrip 2026"}}]),
            ),
            (
                "xlsx",
                json!([{"command":"set","path":"/Sheet1/A1","props":{"value":"Fox roundtrip 2026"}}]),
            ),
            (
                "pptx",
                json!([{"command":"add","path":"/","type":"slide"},{"command":"add","path":"/slide[1]","type":"shape","props":{"text":"Fox roundtrip 2026","x":"1cm","y":"1cm","width":"20cm","height":"3cm"}}]),
            ),
        ];
        for (ext, operations) in cases {
            let source = format!("source.{ext}");
            let output = format!("edited.{ext}");
            execute(
                &server,
                "office_create",
                &json!({"output":source}),
                root.to_str(),
                "allow",
            )
            .unwrap();
            let original = fs::read(root.join(&source)).unwrap();
            execute(
                &server,
                "office_edit",
                &json!({"file":source,"output":output,"operations":operations}),
                root.to_str(),
                "allow",
            )
            .unwrap();
            assert_eq!(fs::read(root.join(&source)).unwrap(), original);
            let read = execute(
                &server,
                "office_read",
                &json!({"file":output}),
                root.to_str(),
                "read_only",
            )
            .unwrap();
            assert!(
                read.to_string().contains("Fox roundtrip 2026"),
                "{ext}: {read}"
            );
            execute(
                &server,
                "office_render",
                &json!({"file":output,"output":format!("{ext}.html"),"mode":"html"}),
                root.to_str(),
                "allow",
            )
            .unwrap();
            assert!(execute(&server,"office_edit",&json!({"file":output,"output":format!("failed.{ext}"),"operations":[{"command":"remove","path":"/missing[999]"}]}),root.to_str(),"allow").is_err());
            assert!(!root.join(format!("failed.{ext}")).exists());
        }
        println!("Office roundtrip artifacts: {}", root.display());
    }
    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-office-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn blocks_writes_traversal_streams_and_external_properties() {
        let root = project();
        let r = root.to_str();
        assert!(prepare(
            "office_create",
            &json!({"output":"report.docx"}),
            r,
            "read_only"
        )
        .is_err());
        for path in [
            "../report.docx",
            "report.docx:stream",
            "NUL.docx",
            "https://example.com/report.docx",
        ] {
            assert!(
                prepare("office_create", &json!({"output":path}), r, "allow").is_err(),
                "{path}"
            );
        }
        assert!(safe_properties(&json!({"src":"C:/secret.txt"})).is_err());
        assert!(safe_properties(&json!({"formula":"=WEBSERVICE(\"https://x\")"})).is_err());
        assert!(safe_properties(&json!({"formula":"=SUM(A1:A3)","text":"汇报正文"})).is_ok());
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn rejects_unknown_commands_and_requires_explicit_overwrite() {
        let root = project();
        fs::write(root.join("report.docx"), "fixture").unwrap();
        assert!(prepare(
            "officecli",
            &json!({"command":"install"}),
            root.to_str(),
            "allow"
        )
        .is_err());
        assert!(prepare(
            "office_create",
            &json!({"output":"report.docx"}),
            root.to_str(),
            "allow"
        )
        .is_err());
        assert!(prepare("office_edit",&json!({"file":"report.docx","output":"new.docx","operations":[{"command":"raw-set","path":"/"}]}),root.to_str(),"allow").is_err());
        assert!(prepare(
            "office_read",
            &json!({"file":"report.docx"}),
            root.to_str(),
            "read_only"
        )
        .is_ok());
        fs::remove_file(root.join("report.docx")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

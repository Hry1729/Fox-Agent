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

/// How a failed built-in Office operation reads to the model.
///
/// The Office connector is a Host-owned local executor: its errors come from
/// this process, the user's own project and the pinned sidecar, never from a
/// remote body. A generic "the resource request failed" leaves the model with
/// nothing to change, and the observed behaviour was exactly that — it retried
/// the same `office_create` against an existing path until the task budget ran
/// out. So the failure is classified once, in the connector, and each class
/// names the *distinct* next action that actually resolves it. Nothing here
/// suggests overwriting by default: the conflict class names renaming first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeResourceKind {
    /// The output path already exists and `overwrite` was not set.
    TargetExists,
    /// A source file the operation needs is not there.
    SourceMissing,
    /// The frozen project root or the resolved path is outside what is allowed.
    PermissionDenied,
    /// The pinned sidecar module is missing, corrupt, or refuses the work.
    WorkerUnavailable,
    /// The sidecar ran past its own deadline.
    TimedOut,
    /// The call's own arguments are malformed.
    InvalidInput,
    /// The target is input material the task told the Run not to change.
    ReadOnlyInput,
    /// The target changed underneath the operation.
    Conflict,
}

impl OfficeResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetExists => "office.target_exists",
            Self::SourceMissing => "office.source_missing",
            Self::PermissionDenied => "office.permission_denied",
            Self::WorkerUnavailable => "office.worker_unavailable",
            Self::TimedOut => "office.timed_out",
            Self::InvalidInput => "tool.invalid_input",
            Self::ReadOnlyInput => "tool.read_only_input",
            Self::Conflict => "tool.file_conflict",
        }
    }

    /// The one next action that resolves this class. Deliberately specific: the
    /// model has to be able to tell renaming from overwriting from re-reading,
    /// which is exactly what the opaque wording destroyed.
    pub fn next_step(self) -> &'static str {
        match self {
            Self::TargetExists => {
                "目标文件已存在：请改用另一个文件名或另一个输出路径（例如加后缀 v2）；\
                 只有确实要覆盖该文件时才显式设置 overwrite=true，不要默认覆盖"
            }
            Self::SourceMissing => {
                "源文件不存在：请先确认路径拼写与所在目录，或用 ls/read 重新查看项目里的真实文件名，\
                 不要对不存在的路径反复重试"
            }
            Self::PermissionDenied => {
                "路径不在授权项目内或当前权限不允许写入：请改用项目目录内的路径；\
                 权限不足需要用户调整授权，重试同样的调用不会成功"
            }
            Self::WorkerUnavailable => {
                "Office 组件不可用或安装不完整：这是环境问题，重试同样的调用不会成功；\
                 请告知用户需要修复 Fox 的 Office 组件，并改用其他方式（如直接写出文本或 CSV）继续任务"
            }
            Self::TimedOut => {
                "该操作超出执行时限：请缩小单次操作规模（例如分批导入、减少单元格数量）后再试一次"
            }
            Self::InvalidInput => "参数不符合 Office 工具契约：请按上面的原因修正参数后重试",
            Self::ReadOnlyInput => {
                "该路径是任务给出的只读输入材料：请把结果写到任务指定的输出路径，不要改动输入文件；\
                 重复写入同一个输入不会成功"
            }
            Self::Conflict => "目标文件在操作期间发生变化：请重新读取该文件，再基于新的内容重试",
        }
    }

    pub fn retryable(self) -> bool {
        matches!(self, Self::TimedOut | Self::Conflict)
    }
}
/// Classify one built-in Office failure. `None` means the text matches no known
/// local class, and the caller must then keep the failure generic rather than
/// forward an unclassified body.
pub fn classify_resource_error(error: &str) -> Option<OfficeResourceKind> {
    let text = error.trim();
    if text.is_empty() {
        return None;
    }
    // The Host's own typed refusals are already classified: the wrapper the
    // gateway adds carries the code verbatim, so it is read before the prose.
    const TYPED: [(&str, OfficeResourceKind); 4] = [
        ("[tool.invalid_input]", OfficeResourceKind::InvalidInput),
        ("[tool.file_conflict]", OfficeResourceKind::Conflict),
        ("[tool.read_only_input]", OfficeResourceKind::ReadOnlyInput),
        ("[tool.permission_denied]", OfficeResourceKind::PermissionDenied),
    ];
    for (marker, kind) in TYPED {
        if text.contains(marker) {
            return Some(kind);
        }
    }
    const TARGET_EXISTS: [&str; 4] = [
        "Office 输出已存在",
        "输出已存在",
        "already exists",
        "EEXIST",
    ];
    const SOURCE_MISSING: [&str; 9] = [
        "Office 输入文件不存在",
        "输入文件不存在",
        "无法读取产物文件",
        "无法打开产物文件",
        "无法定位会话的计算产物目录",
        "无法查询计算产物",
        "不存在",
        "not found",
        "ENOENT",
    ];
    const PERMISSION: [&str; 7] = [
        "超出授权项目",
        "授权项目",
        "不能是网络路径",
        "Office 路径不能包含 ..",
        "保留名称",
        "permission",
        "拒绝访问",
    ];
    const WORKER: [&str; 8] = [
        "Office 组件缺失",
        "Office 组件大小异常",
        "Office 组件完整性校验失败",
        "Office 连接器未启用",
        "Office 连接器不存在",
        "Office 执行锁不可用",
        "首版 Office 连接器仅支持",
        "worker",
    ];
    const TIMED_OUT: [&str; 3] = [
        "Office execution budget exceeded",
        "超出执行时限",
        "timed out",
    ];
    let contains_any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if contains_any(&TARGET_EXISTS) {
        return Some(OfficeResourceKind::TargetExists);
    }
    if contains_any(&TIMED_OUT) {
        return Some(OfficeResourceKind::TimedOut);
    }
    if contains_any(&WORKER) {
        return Some(OfficeResourceKind::WorkerUnavailable);
    }
    if contains_any(&PERMISSION) {
        return Some(OfficeResourceKind::PermissionDenied);
    }
    if contains_any(&SOURCE_MISSING) {
        return Some(OfficeResourceKind::SourceMissing);
    }
    None
}
pub const VERSION: &str = "1.0.147";
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
/// Properties per bounded `office_help` catalog page.
const HELP_PAGE_SIZE: usize = 40;
/// Hard cap (UTF-8 bytes) for one shaped help response.
const HELP_MAX_BYTES: usize = 24 * 1024;
/// Short-hint cap for a property in catalog form; full detail is one more call.
const HELP_HINT_CHARS: usize = 160;
/// Cap for examples/aliases even in a single-property detail response.
const HELP_DETAIL_MAX_BYTES: usize = 12 * 1024;
/// One `office_import_data` payload. Larger datasets are split by the caller
/// into multiple positional imports; the response reports the continuation cell.
const IMPORT_MAX_DATA_BYTES: usize = 4 * 1024 * 1024;
/// Rows one import call may write. Keeps one atomic CLI pass bounded.
const IMPORT_MAX_ROWS: usize = 65_536;
/// Columns one import call may write (matches the data-compute sheet bound).
const IMPORT_MAX_COLUMNS: usize = 512;
/// Excel worksheet bounds used to reject out-of-grid imports before execution.
const EXCEL_MAX_ROWS: u64 = 1_048_576;
const EXCEL_MAX_COLUMNS: u32 = 16_384;
static EXECUTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Test-only observation seam. The Office executor runs several pinned-CLI
/// invocations per operation (create/get/import/validate); tests can act on a
/// precise one instead of racing with sleeps. Compiled out of production.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static SPAWN_COUNT: Cell<u64> = const { Cell::new(0) };
        static POST_SPAWN: RefCell<Option<Box<dyn FnMut(u64)>>> = const { RefCell::new(None) };
    }

    pub(crate) fn set_post_spawn(hook: Option<Box<dyn FnMut(u64)>>) {
        POST_SPAWN.with(|slot| *slot.borrow_mut() = hook);
    }

    pub(crate) fn reset_spawn_count() {
        SPAWN_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn run_post_spawn() {
        let count = SPAWN_COUNT.with(|counter| {
            let next = counter.get() + 1;
            counter.set(next);
            next
        });
        POST_SPAWN.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(count);
            }
        });
    }
}

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
    let file = json!({"type":"string", "description":"Office file path inside the current authorized project (.docx/.xlsx/.pptx). Relative paths resolve against the project root."});
    let output = json!({"type":"string", "description":"Explicit output path inside the project. Use it when the user named a location, or to continue writing the SAME file. For a NEW result prefer \"name\" so the Host places it in the conversation result folder. overwrite requires explicit true."});
    // A *name*, not a path: the Host decides the folder. This is the minimal
    // output semantic the old contract was missing — it had no way to say "this
    // is a new artifact" without also inventing a project path.
    let name = json!({"type":"string", "description":"File name of a NEW artifact, e.g. \"AGV分析报告.docx\". The Host places it in the conversation result folder it decided for this task (the injected deliverableRoot). Must be a plain file name: no directories, no \".\"/\"..\", no drive letters. Mutually exclusive with output."});
    let mut tools = vec![
        ("office_help", "Read a BOUNDED Office format/property reference. No document access. With format only: list available elements. With format+element: a concise property catalog (name/type/ops/short hint). The catalog is paged; request property=<name> for one property's full definition (examples/aliases/readback), or page=<n> for the next catalog page. Do not assume a property exists without reading it. Always echo the returned continuation in the same request when more pages are offered.", json!({
            "format":{"type":"string","enum":["docx","xlsx","pptx"]},
            "element":{"type":"string","description":"Element name, e.g. chart/cell/paragraph. Omit for the element index."},
            "property":{"type":"string","description":"Optional single property/alias name; returns only that property's full definition."},
            "page":{"type":"integer","minimum":1,"description":"Catalog page number (1-based); defaults to 1."},
            "pageSize":{"type":"integer","minimum":1,"maximum":60,"description":"Properties per catalog page; defaults to 40."}
        }), vec!["format"], json!({})),
        ("office_read", "Read an Office document as text, outline, stats, or a structured node/query. No changes.", json!({"file":file,"mode":{"type":"string","enum":["text","outline","stats","get","query"]},"selector":{"type":"string"}}), vec!["file"], json!({})),
        ("office_create", "Create a DOCX/XLSX/PPTX, optionally copying an existing template, and save it as a user result. Give \"name\" to have the Host place it in the conversation result folder, or \"output\" for an explicit project path.", json!({"output":output,"name":name,"template":file,"overwrite":{"type":"boolean","default":false}}), Vec::<&str>::new(), json!({"anyOf":[{"required":["output"]},{"required":["name"]}]})),
        ("office_edit", "Apply an atomic batch of add/set/remove to a document. operations: [{command, path, type?, props?}]. `path` is an ABSOLUTE element path that starts with '/': '/body' adds inside the document body, '/body/p[1]' addresses the first paragraph, '/Sheet1/A1' a cell, '/slide[1]' a slide — the bare element names office_help lists take that leading '/'. Exactly one target is required: `output` rewrites that project path (with overwrite=true when it exists), `name` writes a NEW artifact into the conversation result folder. Source is preserved unless output is the same path. No shell/raw XML/plugins/network assets.", json!({"file":file,"output":output,"name":name,"overwrite":{"type":"boolean","default":false},"operations":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"object","additionalProperties":false,"required":["command","path"],"properties":{"command":{"type":"string","enum":["add","set","remove"]},"path":{"type":"string","description":"Absolute element path beginning with '/', e.g. /body, /body/p[1], /Sheet1/A1."},"type":{"type":"string"},"props":{"type":"object","additionalProperties":{"type":["string","number","boolean"]}}}}}}), vec!["file","operations"], json!({"anyOf":[{"required":["output"]},{"required":["name"]}]})),
        ("office_import_data", "Import ONE CSV/TSV data block into an XLSX worksheet atomically; far more compact than many per-cell office_edit operations. Values are typed by the writer (numbers stay numbers); a field starting with = becomes a formula and is screened like office_edit. PREFER the artifact flow: compute and save data once with attachment_compute (saveFile), then pass the returned files[].id here as artifactId — Host reads the saved bytes directly, so the full CSV never has to be rebuilt or copied through the request. data and artifactId are mutually exclusive; artifactId only accepts a compute artifact id from THIS conversation (a compute-artifact: id returned by attachment_compute). Large datasets MUST be split into multiple calls: the response returns nextStartCell for the following block. Re-running the same call with the same startCell overwrites the same cells, it never appends duplicates, so retries are safe. Omit file to create the workbook. Set createSheet=true to add a missing sheet. The sheet receives the data as-is; style it afterwards with office_edit.", json!({
            "file":{"type":"string", "description":"Optional existing .xlsx inside the authorized project. Omit to create a new workbook at output/name."},
            "output":output,
            "name":name,
            "overwrite":{"type":"boolean","default":false},
            "sheet":{"type":"string","description":"Target worksheet name (must exist unless createSheet=true)."},
            "createSheet":{"type":"boolean","default":false,"description":"Add the worksheet when it does not exist yet."},
            "startCell":{"type":"string","default":"A1","description":"Top-left cell of the imported block, A1 notation."},
            "format":{"type":"string","enum":["csv","tsv"],"default":"csv"},
            "header":{"type":"boolean","default":false,"description":"First row is a header: the writer sets AutoFilter and freezes the pane."},
            "data":{"type":"string","description":"The inline CSV/TSV payload, at most 4 MiB, 65536 rows and 512 columns per call. Mutually exclusive with artifactId."},
            "artifactId":{"type":"string","description":"A compute artifact id (compute-artifact:...) returned by this conversation's attachment_compute files[].id. Host resolves and reads the saved CSV/TSV bytes directly; pass format to match the saved file. Mutually exclusive with data."}
        }), vec!["sheet"], json!({"anyOf":[{"required":["output"]},{"required":["name"]}]})),
        ("office_merge", "Fill {{key}} placeholders in a template with inline data and save a copy.", json!({"file":file,"output":output,"name":name,"overwrite":{"type":"boolean","default":false},"data":{"type":"object","additionalProperties":{"type":["string","number","boolean"]}}}), vec!["file","data"], json!({"anyOf":[{"required":["output"]},{"required":["name"]}]})),
        ("office_render", "Render an Office document to HTML or PNG for the user to preview. By default the Host stores the preview in its private preview area and the user opens it through the saved result's preview entry — pass NO output for that. Pass \"name\" (or an explicit output inside the conversation result folder) only when the user asked for the HTML/PNG itself as a deliverable. PNG requires a supported local browser; return a clear error when unavailable. This is not proof of Microsoft Office rendering equivalence.", json!({"file":file,"output":output,"name":name,"mode":{"type":"string","enum":["html","screenshot"]},"page":{"type":"string"}}), vec!["file","mode"], json!({})),
        ("office_validate", "Validate OpenXML structure of an existing document. Does not certify visual layout or numerical correctness.", json!({"file":file}), vec!["file"], json!({})),
    ].into_iter().map(|(name, description, properties, required, extra)| {
        let mut schema = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
        if let (Some(object), Some(extra)) = (schema.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
        json!({"name":name,"description":description,"inputSchema":schema})
    }).collect::<Vec<_>>();
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
    /// Whether the resolved `output` lands in the project (a deliverable) or in
    /// the Host's private preview cache.
    placement: OutputPlacement,
    /// Host-side shaping for `office_help` (the CLI always returns the full
    /// reference; the Host bounds/pages it and never executes a document).
    help: Option<HelpQuery>,
    /// Payload delivered to the child over stdin (batch JSON / import CSV).
    /// Never placed on the command line, so large batches cannot hit the
    /// Windows command-line length limit.
    stdin_data: Option<String>,
    /// `office_import_data` execution plan: sheet placement + result shaping.
    import: Option<ImportSpec>,
}

/// Authorization the Host derives from the current Run's frozen binding and
/// hands to the Office executor so it can resolve a compute-artifact reference.
///
/// The model supplies only the artifact id; it cannot name a conversation, a
/// run or a source path. `conversation_id` comes from the frozen
/// `RunControlBinding`, `sessions_dir` from the Host's own runtime state, and
/// the artifact must already be registered for exactly that conversation.
#[derive(Clone, Copy)]
pub struct OfficeCallContext<'a> {
    pub database: &'a crate::database::Database,
    pub sessions_dir: &'a Path,
    pub conversation_id: &'a str,
    /// Host-private artifact roots (preview cache, working copies, staging).
    /// Callers that cannot supply it (unit tests of the pure translator) pass
    /// `None`; every production dispatch passes the Host's own state so a
    /// model can never point these areas at application data.
    pub artifacts_dir: Option<&'a Path>,
}

impl<'a> OfficeCallContext<'a> {
    /// The Host's private artifact store, or `None` on a context-free path.
    pub(crate) fn artifact_store(&self) -> Option<crate::runtime_host::artifact_store::ArtifactStore> {
        self.artifacts_dir
            .map(crate::runtime_host::artifact_store::ArtifactStore::new)
    }
}

/// Host-decided placement for one prepared Office operation.
///
/// `root` is always the authorized project folder. `output` is the final
/// resolved target, which for a preview is an application-private path. The
/// three are kept separate so permission checks, the approval card, execution
/// and version registration all use the same resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputPlacement {
    /// Final result inside the project (`交付物`).
    Deliverable,
    /// Rendered preview inside the Host's private preview area.
    HostPrivatePreview,
}

fn path_is_in_project_deliverable_dir(root: &Path, path: &Path) -> bool {
    crate::runtime_host::artifact_store::is_deliverable_path(root, path)
}

/// The Host-owned directory that holds pre-write snapshots of documents this
/// connector mutates.
///
/// Derived from the Host's own session store, exactly like
/// `RuntimeHostState::managed_files_dir`, so the connector writes its undo copy
/// into the version store the restore path already reads instead of leaving a
/// `*.fox-backup-<uuid>` file next to the user's document.
pub(crate) fn snapshot_directory(context: &OfficeCallContext<'_>) -> PathBuf {
    context
        .sessions_dir
        .parent()
        .map(|base| base.join("managed-file-backups"))
        .unwrap_or_else(|| context.sessions_dir.join("managed-file-backups"))
}

/// Why a Host ledger still needs these bytes, if it does.
///
/// The reclaim pass may only delete commit staging that no artifact row and no
/// version/recovery row names. An unanswerable query is *not* a licence to
/// delete: it is reported as a reference so the file is kept.
fn staging_reference_reason(
    context: Option<&OfficeCallContext<'_>>,
    path: &Path,
) -> Option<String> {
    let context = context?;
    let key = path.to_string_lossy();
    match context.database.path_is_referenced_by_host_ledger(&key) {
        Ok(true) => Some("产物或版本台账仍引用该路径".to_owned()),
        Ok(false) => None,
        Err(error) => Some(format!("无法确认台账引用关系，按保留处理: {error}")),
    }
}

/// Write a rendered preview into the Host's private preview area.
///
/// Only used when the target would otherwise be an ad-hoc file in the project
/// root: previews are not deliverables, so they belong to the Host's cache and
/// are reached through the result's own preview entry. A target the model
/// deliberately placed in the conversation deliverable folder (`fox/...`) is
/// respected — the user explicitly asked for that artifact.
fn private_preview_path(
    root: &Path,
    requested: &Path,
    conversation_id: &str,
    store: &crate::runtime_host::artifact_store::ArtifactStore,
    mode: &str,
) -> PathBuf {
    if path_is_in_project_deliverable_dir(root, requested) {
        return requested.to_path_buf();
    }
    let extension = requested
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("html");
    store
        .preview_path(conversation_id, requested, extension, &format!("mode={mode}"))
        .unwrap_or_else(|_| requested.to_path_buf())
}

/// A stable synthetic "requested" path for a render that was given no target.
///
/// Derived from the source document so that re-rendering the same document in
/// the same mode keeps replacing one preview file instead of accumulating one
/// per call. It is only ever used as a fingerprint input and for the extension;
/// the real location is the Host's private preview cache.
fn default_preview_request(root: &Path, source: Option<&Path>, expected: &str) -> PathBuf {
    let stem = source
        .and_then(|path| path.file_stem())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("preview");
    root.join(format!("{stem}.{expected}"))
}

/// Project-relative target for a new artifact the caller named but did not place.
///
/// The Host puts it in the conversation's result folder — the same folder the
/// prompt advertises as `deliverableRoot`, persisted by the same ledger — so the
/// advertised path, the approved path and the executed path cannot drift apart.
/// A name is a *name*: separators, traversal and reserved device names are
/// rejected rather than sanitized, because silently moving a file the user named
/// is worse than refusing it.
fn default_deliverable_relative(
    root: &Path,
    name: &str,
    context: Option<&OfficeCallContext<'_>>,
) -> Result<String, String> {
    if name.is_empty() || name.len() > 160 {
        return Err("Office name 无效：必须是 1..=160 字节的文件名".into());
    }
    if name.contains(['/', '\\', ':', '\0']) || name.starts_with('.') {
        return Err("Office name 只能是文件名本身，不能包含路径分隔符".into());
    }
    if matches!(name, "." | "..") || name.ends_with(['.', ' ']) {
        return Err("Office name 是无效的文件名".into());
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6"
            | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6"
            | "LPT7" | "LPT8" | "LPT9"
    ) {
        return Err("Office name 不能是保留设备名".into());
    }
    let context = context.ok_or(
        "当前链路没有会话上下文，无法使用 Host 默认成果目录；请显式提供 output",
    )?;
    let store = context.artifact_store().ok_or(
        "当前链路没有 Host 私有产物区，无法确定默认成果目录；请显式提供 output",
    )?;
    let conversation_id = context.conversation_id;
    let candidate = store.deliverable_directory_name("", conversation_id);
    let project_key =
        crate::runtime_host::artifact_store::project_key(&root.to_string_lossy());
    let folder = context
        .database
        .ensure_deliverable_folder(
            conversation_id,
            &project_key,
            &candidate,
            &crate::runtime_host::artifact_store::is_valid_deliverable_folder_name,
            crate::database::now_ms(),
        )
        .map_err(|error| format!("无法确定会话成果目录: {error}"))?
        .ok_or("会话成果目录不可用")?;
    Ok(format!(
        "{}/{}/{}",
        crate::runtime_host::artifact_store::PROJECT_DELIVERABLE_DIR,
        folder,
        name
    ))
}

/// Resolve a `compute-artifact:` reference into the exact bytes the Office
/// executor will stream to the CLI over stdin.
///
/// The model contributes only the id. Everything else is a Host fact:
/// * the conversation comes from the frozen Run binding ([`OfficeCallContext`]),
///   never from the tool input, so a cross-session id cannot be read;
/// * the artifact must be registered for exactly that conversation;
/// * the resolved path must live inside the conversation's own compute root;
/// * the on-disk bytes must match the recorded size and SHA-256, be a regular
///   non-link file, and actually be readable.
///
/// The resolved content is returned as the import payload only — it is never
/// echoed back into the model request.
fn resolve_compute_artifact(
    context: Option<&OfficeCallContext<'_>>,
    artifact_id: &str,
    format: &str,
) -> Result<String, String> {
    let context = context.ok_or_else(|| {
        "Office 产物引用在此链路上不可用；请改用内联 data，或通过本会话的 attachment_compute 保存数据后重试".to_string()
    })?;
    if artifact_id.trim().is_empty() || artifact_id.len() > 200 || artifact_id.contains('\0') {
        return Err("Office 产物引用格式无效".into());
    }
    // Ownership: the scoped query joins artifact id AND the frozen conversation.
    let artifact = context
        .database
        .artifact_for_conversation(context.conversation_id, artifact_id)
        .map_err(|error| format!("无法查询计算产物: {error}"))?
        .ok_or_else(|| {
            "Office 产物不存在或不属于当前会话；请使用本会话 attachment_compute 返回的 files[].id，不要手工拼接或跨会话引用".to_string()
        })?;
    // Scope: the resolved file must be inside this conversation's compute root.
    let roots = crate::runtime_host::attachment_compute::authorized_artifact_root(
        context.sessions_dir,
        context.conversation_id,
    )
    .map_err(|error| format!("无法定位会话的计算产物目录: {error}"))?
    .map(|root| vec![root])
    .ok_or_else(|| "当前会话没有可读取的计算产物".to_string())?;
    // Integrity: status, size, SHA-256, link and readability checks.
    let path = crate::artifact_gateway::validate_record(&artifact, &roots)
        .map_err(|error| format!("Office 产物校验失败（{}）：{}", error.code, error.message))?;
    // Format: the saved file must really be the delimited format being imported.
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if extension != format {
        return Err(format!(
            "Office 产物扩展名 .{extension} 与导入格式 {format} 不一致；请改用 saveFile 保存为 .{format} 后再引用"
        ));
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取产物文件: {error}"))?;
    if !metadata.is_file() {
        return Err("Office 产物不是普通文件".into());
    }
    if metadata.len() > IMPORT_MAX_DATA_BYTES as u64 {
        return Err(format!(
            "Office 产物为 {} 字节，超过单批 {} 字节；请用 attachment_compute 拆分成多个产物后分批导入，并用返回的 nextStartCell 续写",
            metadata.len(),
            IMPORT_MAX_DATA_BYTES
        ));
    }
    let mut file = fs::File::open(&path).map_err(|error| format!("无法打开产物文件: {error}"))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)
        .map_err(|error| format!("无法读取产物文件内容: {error}"))?;
    if buffer.contains('\0') {
        return Err("Office 导入数据包含空字符".into());
    }
    if buffer.len() > IMPORT_MAX_DATA_BYTES {
        return Err(format!(
            "Office 产物为 {} 字节，超过单批 {} 字节；请用 attachment_compute 拆分后分批导入",
            buffer.len(),
            IMPORT_MAX_DATA_BYTES
        ));
    }
    Ok(buffer)
}


/// Verified placement of one `office_import_data` block.
#[derive(Debug, Clone)]
struct ImportSpec {
    sheet: String,
    create_sheet: bool,
    start_cell: String,
    start_row: u64,
    start_col: u32,
    rows: u64,
    cols: u32,
    header: bool,
    format: String,
}

impl PreparedOffice {
    /// The verified write target: the canonical in-project output path this
    /// preparation admitted, or `None` for a non-mutating operation.
    ///
    /// The Host uses this — never a connector-declared path — when it decides
    /// whether a saved document may become a user-restorable version.
    pub fn target_path(&self) -> Option<&Path> {
        self.output.as_deref()
    }

    /// The payload the Host streams to the CLI over stdin (batch JSON or the
    /// import CSV/TSV). Exposed crate-internally so Host-side tests can prove a
    /// compute artifact is delivered byte-for-byte without a model re-copy.
    pub(crate) fn stdin_payload(&self) -> Option<&str> {
        self.stdin_data.as_deref()
    }

    /// `(rows, columns)` planned for an `office_import_data` call.
    pub(crate) fn import_dimensions(&self) -> Option<(u64, u32)> {
        self.import.as_ref().map(|spec| (spec.rows, spec.cols))
    }
}

#[derive(Debug, Clone)]
struct HelpQuery {
    property: Option<String>,
    page: usize,
    page_size: usize,
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
        // The conversation's own deliverable folder is created on demand: the
        // Host chooses that folder, so a model that follows the injected
        // `deliverableRoot` must not first have to invent a directory-creation
        // step. Any other missing directory stays an error, so an arbitrary
        // path is never silently materialized.
        if crate::runtime_host::artifact_store::is_deliverable_path(root, &candidate) {
            if let Some(parent) = candidate.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("无法创建会话成果目录 {}: {e}", parent.display())
                })?;
            }
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

/// Parse an A1 cell reference into (row, column), both 1-based.
/// Rejects anything outside the Excel grid so a bad target fails before execution.
fn parse_a1_cell(value: &str) -> Result<(u64, u32), String> {
    let bytes = value.as_bytes();
    let split = bytes
        .iter()
        .position(|b| b.is_ascii_digit())
        .ok_or_else(|| format!("Office 起始单元格 {value} 无效，应为 A1 形式"))?;
    if split == 0 || split > 3 {
        return Err(format!("Office 起始单元格 {value} 无效，应为 A1 形式"));
    }
    let (letters, digits) = value.split_at(split);
    if !letters.bytes().all(|b| b.is_ascii_alphabetic()) || digits.is_empty() {
        return Err(format!("Office 起始单元格 {value} 无效，应为 A1 形式"));
    }
    let mut column: u32 = 0;
    for b in letters.bytes() {
        column = column * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    let row: u64 = digits
        .parse()
        .map_err(|_| format!("Office 起始单元格 {value} 无效，应为 A1 形式"))?;
    if row < 1 || row > EXCEL_MAX_ROWS || column < 1 || column > EXCEL_MAX_COLUMNS {
        return Err(format!(
            "Office 起始单元格 {value} 超出工作表网格（1..={EXCEL_MAX_ROWS} 行，1..={EXCEL_MAX_COLUMNS} 列）"
        ));
    }
    Ok((row, column))
}

/// 1-based column number back to A1 letters (for continuation hints).
fn column_name(mut column: u32) -> String {
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    letters.iter().rev().collect()
}

/// Scan one CSV/TSV payload without materializing it: count rows/columns and
/// screen formula fields with the same rules `safe_properties` applies to
/// `office_edit`. Only `=`-prefixed fields become formulas in the pinned CLI
/// (verified against OfficeCLI 1.0.147); other prefixes stay literal text.
fn scan_import_data(data: &str, format: &str) -> Result<(u64, u32), String> {
    fn check_field(field: &str, row: u64) -> Result<(), String> {
        let trimmed = field.trim_start();
        if let Some(formula) = trimmed.strip_prefix('=') {
            let upper = formula.to_ascii_uppercase();
            if upper.contains('[')
                || ["WEBSERVICE", "HYPERLINK", "IMAGE(", "RTD(", "DDE", "://", "\\\\", "|"]
                    .iter()
                    .any(|needle| upper.contains(needle))
            {
                return Err(format!(
                    "Office 数据第 {row} 行包含外部引用、网络或 DDE 公式，首版未开放"
                ));
            }
        }
        Ok(())
    }
    let delimiter = match format {
        "csv" => ',',
        "tsv" => '\t',
        _ => return Err("Office 数据格式仅支持 csv 或 tsv".into()),
    };
    let mut rows: u64 = 0;
    let mut max_cols: u32 = 0;
    let mut field = String::new();
    let mut fields_in_row: u32 = 0;
    let mut in_quotes = false;
    let mut chars = data.chars().peekable();
    while let Some(c) = chars.next() {
        if format == "csv" && c == '"' {
            if in_quotes && chars.peek() == Some(&'"') {
                field.push('"');
                chars.next();
            } else {
                in_quotes = !in_quotes;
            }
        } else if c == delimiter && !in_quotes {
            fields_in_row += 1;
            check_field(&field, rows + 1)?;
            field.clear();
        } else if (c == '\n' || c == '\r') && !in_quotes {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            fields_in_row += 1;
            check_field(&field, rows + 1)?;
            field.clear();
            max_cols = max_cols.max(fields_in_row);
            fields_in_row = 0;
            rows += 1;
            if rows > IMPORT_MAX_ROWS as u64 {
                return Err(format!(
                    "Office 数据超过单批 {IMPORT_MAX_ROWS} 行；请拆成多次 office_import_data，用返回的 nextStartCell 续写"
                ));
            }
        } else {
            field.push(c);
        }
    }
    if in_quotes {
        return Err("Office CSV 数据引号未闭合".into());
    }
    if !field.is_empty() || fields_in_row > 0 {
        fields_in_row += 1;
        check_field(&field, rows + 1)?;
        max_cols = max_cols.max(fields_in_row);
        rows += 1;
    }
    if rows == 0 {
        return Err("Office 导入数据为空".into());
    }
    if max_cols == 0 {
        return Err("Office 导入数据没有有效列".into());
    }
    if max_cols > IMPORT_MAX_COLUMNS as u32 {
        return Err(format!(
            "Office 数据有 {max_cols} 列，超过单批 {IMPORT_MAX_COLUMNS} 列；请按列拆分多次导入"
        ));
    }
    if rows > IMPORT_MAX_ROWS as u64 {
        return Err(format!(
            "Office 数据超过单批 {IMPORT_MAX_ROWS} 行；请拆成多次 office_import_data，用返回的 nextStartCell 续写"
        ));
    }
    Ok((rows, max_cols))
}

/// Worksheet names follow Excel rules: no control characters or `: \ / ? * [ ]`.
fn validate_sheet_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || name.chars().count() > 128 {
        return Err("Office 工作表名称不能为空且不超过 128 字符".into());
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
    {
        return Err("Office 工作表名称包含非法字符（: \\ / ? * [ ] 或控制字符）".into());
    }
    Ok(())
}

pub fn prepare(
    tool: &str,
    input: &Value,
    root: Option<&str>,
    permission: &str,
) -> Result<PreparedOffice, String> {
    prepare_with_context(tool, input, root, permission, None)
}

/// Prepare with an optional [`OfficeCallContext`] used to resolve
/// `office_import_data`'s `artifactId`. Production dispatch always passes the
/// frozen Run's own conversation; the context is `None` only for paths that
/// cannot reference an artifact (managed-file pre-flight, legacy callers that
/// never carry `artifactId`).
/// Whether a tool's own schema offers `output` or `name` as its target.
fn schema_requires_output_or_name(schema: &Value) -> bool {
    schema["anyOf"]
        .as_array()
        .is_some_and(|options| {
            let requires = |field: &str| {
                options.iter().any(|option| {
                    option["required"]
                        .as_array()
                        .is_some_and(|required| required.iter().any(|value| value == field))
                })
            };
            requires("output") || requires("name")
        })
}

/// Whether a built-in Office call of this tool is a mutating one, given that
/// tool's own parameters.
///
/// `arguments` is the Office tool's parameter object (the `arguments` member of
/// the MCP call), **not** the call envelope: the write targets live at
/// `arguments.output` / `arguments.name`, so passing the envelope would read the
/// wrong level and classify every ordinary write as a read.
///
/// This is a *diagnostic rule mapping*, not a shared implementation: it mirrors
/// the target-resolution rule in `prepare_with_context` — `office_render` always
/// writes a Host-side output, and every other tool mutates once it resolved an
/// `output`/`name` target — and the two are kept consistent by review rather than
/// by calling one from the other. The Host consults this only after establishing
/// the built-in Office identity (the frozen `serverId`), so an operation class is
/// never named for a connector tool that happens to share a name.
pub(crate) fn tool_mutates(tool: &str, arguments: &Value) -> bool {
    tool == "office_render"
        || arguments.get("output").and_then(Value::as_str).is_some()
        || arguments.get("name").and_then(Value::as_str).is_some()
}

pub fn prepare_with_context(
    tool: &str,
    input: &Value,
    root: Option<&str>,
    permission: &str,
    context: Option<&OfficeCallContext<'_>>,
) -> Result<PreparedOffice, String> {
    let definition = tool_definitions()
        .into_iter()
        .find(|v| v["name"] == tool)
        .ok_or("未知 Office 工具")?;
    // A tool whose schema demands a target states that in the model's own terms
    // instead of leaving it to decode an `anyOf` failure. Derived from the
    // schema itself, so it stays true for every tool that has the requirement.
    if schema_requires_output_or_name(&definition["inputSchema"])
        && input.get("output").is_none()
        && input.get("name").is_none()
    {
        return Err("Office 需要 output（项目内既有路径，配合 overwrite=true 写回）或 name（新成果文件名，由 Host 放入成果目录）其中之一；只给 file 不会写回原文件。".into());
    }
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
        placement: OutputPlacement::Deliverable,
        help: None,
        stdin_data: None,
        import: None,
    };
    if tool == "office_help" {
        action.args = vec!["help".into(), string(input, "format")?.into()];
        let mut element: Option<&str> = None;
        if let Some(value) = input["element"].as_str() {
            if value.is_empty()
                || !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return Err("无效的 Office 元素名称".into());
            }
            element = Some(value);
            action.args.push(value.into());
        }
        action.args.push("--json".into());
        // Property/page selectors are applied Host-side after parsing the full
        // CLI reference; they are never passed as raw CLI flags.
        if element.is_some() {
            let property = input["property"]
                .as_str()
                .map(str::trim)
                .filter(|value| {
                    !value.is_empty()
                        && value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                })
                .map(str::to_string);
            let page = input["page"].as_u64().unwrap_or(1).clamp(1, 100_000) as usize;
            let page_size = input["pageSize"].as_u64().unwrap_or(HELP_PAGE_SIZE as u64)
                .clamp(1, 60) as usize;
            action.help = Some(HelpQuery { property, page, page_size });
        }
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
    // Target resolution, in the caller's own terms. Two names for two different
    // things, which the previous contract conflated:
    //
    //  * `output` — a path inside the authorized project (or the exact path of
    //    an existing file). Semantics unchanged.
    //  * `name`   — the file name of a NEW artifact. The Host places it in the
    //    conversation result folder it decided for this task, so "use the
    //    default result folder" is a Host guarantee rather than a prompt
    //    suggestion. It never rewrites or prefixes an `output` path.
    let explicit_output = input.get("output").and_then(Value::as_str).map(str::to_owned);
    let named = input.get("name").and_then(Value::as_str).map(str::to_owned);
    if explicit_output.is_some() && named.is_some() {
        return Err(
            "Office output 与 name 互斥：output 指定项目内路径，name 让 Host 放入默认成果目录"
                .into(),
        );
    }
    let requested = match (explicit_output, named) {
        (Some(value), None) => Some(value),
        (None, Some(name)) => Some(default_deliverable_relative(&action.root, &name, context)?),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("mutual exclusion is checked above"),
    };
    if tool == "office_render" {
        let mode = input["mode"].as_str().unwrap_or("html");
        let expected = if mode == "html" { "html" } else { "png" };
        // A render is a *view* of a result, so it is Host-private by default.
        // It reaches the project only when the caller explicitly exported it:
        // through `name` (the Host places it in the result folder) or through an
        // explicit path inside the conversation result folder. Any other
        // explicit path still lands in the private preview cache, which also
        // makes a re-render replace the same file so no stale preview link
        // survives.
        let (target, placement) = match requested {
            Some(value) => {
                let output = scoped_path(&action.root, &value, false)?;
                let actual = output.extension().and_then(|s| s.to_str());
                if actual != Some(expected) {
                    return Err(format!("预览输出扩展名必须为 .{expected}"));
                }
                if output.exists() && !action.overwrite {
                    return Err(
                        "Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true".into(),
                    );
                }
                match context.and_then(|context| context.artifact_store()) {
                    Some(store) if !path_is_in_project_deliverable_dir(&action.root, &output) => {
                        let conversation_id =
                            context.map(|context| context.conversation_id).unwrap_or_default();
                        (
                            private_preview_path(
                                &action.root,
                                &output,
                                conversation_id,
                                &store,
                                mode,
                            ),
                            OutputPlacement::HostPrivatePreview,
                        )
                    }
                    _ => (output, OutputPlacement::Deliverable),
                }
            }
            None => {
                // No target at all: the Host decides. This is the normal way to
                // render a preview and must not require the model to invent a
                // path in the user's project.
                let store = context
                    .and_then(|context| context.artifact_store())
                    .ok_or("office_render 需要 output/name，或需在此链路提供 Host 私有预览区")?;
                let conversation_id =
                    context.map(|context| context.conversation_id).unwrap_or_default();
                let requested =
                    default_preview_request(&action.root, action.source.as_deref(), expected);
                (
                    private_preview_path(
                        &action.root,
                        &requested,
                        conversation_id,
                        &store,
                        mode,
                    ),
                    OutputPlacement::HostPrivatePreview,
                )
            }
        };
        action.placement = placement;
        action.output = Some(target);
        action.mutates = true;
    } else if let Some(value) = requested {
        let output = scoped_path(&action.root, &value, false)?;
        if output.exists() && !action.overwrite {
            return Err("Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true".into());
        }
        document(&output)?;
        if let Some(source) = &action.source {
            if source.extension() != output.extension() {
                return Err("Office 编辑不进行格式转换，输入输出扩展名必须一致".into());
            }
        }
        // Every non-render tool commits to the project path it resolved;
        // only `office_render` may redirect the target into the Host's private
        // preview cache.
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
                    return Err(format!(
                        "Office 元素路径必须以 / 开头且不超过 4096 字符（收到 \"{selector}\"）：\
                         例如 /body 添加段落、/body/p[1] 定位第一段、/Sheet1/A1 定位单元格；\
                         office_help 列出的元素名称需要加上前导 /"
                    ));
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
            // Batch JSON goes over stdin: an inline --commands payload can
            // exceed the Windows command-line limit well before 100 verbose
            // operations, and the pinned CLI reads the same array from stdin.
            action.args = vec!["batch".into(), "{output}".into(), "--json".into()];
            action.stdin_data = Some(input["operations"].to_string());
        }
        "office_import_data" => {
            let sheet = string(input, "sheet")?;
            validate_sheet_name(sheet)?;
            let format = input["format"].as_str().unwrap_or("csv").to_owned();
            if !matches!(format.as_str(), "csv" | "tsv") {
                return Err("Office 数据格式仅支持 csv 或 tsv".into());
            }
            // Exactly one payload source. An inline block and a saved artifact
            // are mutually exclusive, and an empty inline payload is not a
            // stand-in for a missing one.
            let inline = input.get("data").and_then(Value::as_str);
            let artifact_id = input.get("artifactId").and_then(Value::as_str);
            match (inline, artifact_id) {
                (Some(_), Some(_)) => {
                    return Err("Office 导入数据只能使用 data 或 artifactId 其中之一，不能同时传递".into());
                }
                (None, None) => {
                    return Err("Office 导入数据缺少 data 或 artifactId：请传入内联 data，或传入本会话 attachment_compute 返回的 artifactId".into());
                }
                _ => {}
            }
            let data = match artifact_id {
                Some(id) => resolve_compute_artifact(context, id, &format)?,
                None => inline.expect("inline data").to_owned(),
            };
            if data.len() > IMPORT_MAX_DATA_BYTES {
                return Err(format!(
                    "Office 数据为 {} 字节，超过单批 {} 字节；请拆成多次 office_import_data，用返回的 nextStartCell 续写",
                    data.len(),
                    IMPORT_MAX_DATA_BYTES
                ));
            }
            if data.contains('\0') {
                return Err("Office 导入数据包含空字符".into());
            }
            let (rows, cols) = scan_import_data(&data, &format)?;
            let start_cell = input["startCell"].as_str().unwrap_or("A1").trim().to_owned();
            let (start_row, start_col) = parse_a1_cell(&start_cell)?;
            if start_row + rows - 1 > EXCEL_MAX_ROWS || start_col + cols - 1 > EXCEL_MAX_COLUMNS {
                return Err(format!(
                    "Office 数据块（{rows} 行 x {cols} 列）从 {start_cell} 起超出工作表网格；请减小批次或后移 startCell"
                ));
            }
            action.import = Some(ImportSpec {
                sheet: sheet.to_owned(),
                create_sheet: input["createSheet"].as_bool().unwrap_or(false),
                start_cell: start_cell.clone(),
                start_row,
                start_col,
                rows,
                cols,
                header: input["header"].as_bool().unwrap_or(false),
                format: format.clone(),
            });
            action.args = vec![
                "import".into(),
                "{output}".into(),
                format!("/{sheet}"),
                "--stdin".into(),
                "--format".into(),
                format,
                "--start-cell".into(),
                start_cell,
                "--json".into(),
            ];
            if input["header"].as_bool().unwrap_or(false) {
                action.args.push("--header".into());
            }
            action.stdin_data = Some(data.to_owned());
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
    invoke_with_stdin(binary, args, None, cwd, cancellation, deadline)
}

fn invoke_with_stdin(binary: &Path, args: &[String], stdin_data: Option<&str>, cwd: Option<&Path>, cancellation: Option<&crate::kernel::CancellationToken>, deadline: Instant) -> Result<String, String> {
    if let Some(token) = cancellation { token.check()?; }
    if Instant::now() >= deadline { return Err("Office execution budget exceeded".into()); }
    let mut command = Command::new(binary);
    command
        .args(args)
        .env("OFFICECLI_SKIP_UPDATE", "1")
        .env("OFFICECLI_NO_AUTO_RESIDENT", "1")
        .env("OFFICECLI_RESIDENT_FLUSH", "each")
        .stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() })
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
    // Deterministic seam for the "cancel while the pinned CLI is mid-write"
    // test: the hook runs on the calling thread right after a child starts, and
    // is told which invocation of this operation it is (1-based). Production
    // builds have no hook.
    #[cfg(test)]
    test_hooks::run_post_spawn();
    // Feed the payload on a writer thread: the pipe buffer is small and the
    // child may not read until it has opened the document.
    let mut writer = child.stdin.take().map(|mut pipe| {
        let payload = stdin_data.unwrap_or_default().as_bytes().to_vec();
        thread::spawn(move || {
            use std::io::Write;
            let _ = pipe.write_all(&payload);
            let _ = pipe.flush();
            // Dropping the pipe closes stdin so the child sees EOF.
        })
    });
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
            if let Some(handle) = writer.take() { let _ = handle.join(); }
            return Err("Office execution cancelled or budget exceeded; owned process stopped".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {},
            Err(error) => {
                crate::tool_host::terminate_process_tree(&mut child);
                let _ = out.join();
                let _ = err.join();
                if let Some(handle) = writer.take() { let _ = handle.join(); }
                return Err(format!("Office process status failed: {error}"));
            }
        }
        if start.elapsed() > Duration::from_secs(90) {
            crate::tool_host::terminate_process_tree(&mut child);
            let _ = out.join();
            let _ = err.join();
            if let Some(handle) = writer.take() { let _ = handle.join(); }
            return Err("Office 操作超过 90 秒，已终止".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    if let Some(handle) = writer.take() { let _ = handle.join(); }
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(format!("Office 操作失败 ({status}): {stderr}\n{stdout}"));
    }
    Ok(stdout)
}

fn hint(value: &Value) -> String {
    let mut hint = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if hint.chars().count() > HELP_HINT_CHARS {
        hint = hint.chars().take(HELP_HINT_CHARS).collect::<String>() + "…";
    }
    hint
}

fn ops(value: &Value) -> Vec<&'static str> {
    [("add", "add"), ("set", "set"), ("get", "get")]
        .into_iter()
        .filter(|(key, _)| value.get(*key).and_then(Value::as_bool).unwrap_or(false))
        .map(|(_, op)| op)
        .collect()
}

/// Shape the OfficeCLI full reference JSON into a bounded, pageable catalog.
/// Returns a JSON string the model can use directly. The full reference is
/// never discarded: a specific property returns its complete definition, and
/// catalog pages expose a stable continuation token.
fn shape_help(stdout: &str, query: &HelpQuery) -> Result<String, String> {
    let reference: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| "Office 组件返回了无法解析的说明".to_string())?;
    let format = reference["format"].as_str().unwrap_or_default().to_string();
    let element = reference["element"].as_str().unwrap_or_default().to_string();

    // Element index response (no properties object): pass through a compact form.
    let properties = match reference.get("properties").and_then(Value::as_object) {
        Some(map) => map,
        None => {
            let mut compact = serde_json::Map::new();
            for key in ["format", "element", "operations", "paths", "note"] {
                if let Some(value) = reference.get(key) {
                    compact.insert(key.to_string(), value.clone());
                }
            }
            compact.insert(
                "bounded".into(),
                json!({"tool":"office_help","format":format,"note":"Use format+element for a paged property catalog; property=<name> for full definition."}),
            );
            return Ok(Value::Object(compact).to_string());
        }
    };

    // Single-property detail: full definition for that name or one of its aliases.
    if let Some(name) = &query.property {
        let found = properties
            .iter()
            .find(|(key, value)| {
                *key == name
                    || value.get("aliases")
                        .and_then(Value::as_array)
                        .is_some_and(|aliases| aliases.iter().any(|a| a.as_str() == Some(name.as_str())))
            })
            .map(|(_, value)| value);
        let Some(value) = found else {
            return Ok(json!({
                "format": format, "element": element, "property": name,
                "found": false,
                "availableProperties": properties.keys().take(HELP_PAGE_SIZE).cloned().collect::<Vec<_>>(),
                "moreProperties": properties.len().saturating_sub(HELP_PAGE_SIZE),
                "hint": "Unknown property for this element. Re-read the catalog page that lists it, then request property=<exact name>."
            }).to_string());
        };
        let mut detail = serde_json::Map::new();
        detail.insert("format".into(), json!(format));
        detail.insert("element".into(), json!(element));
        detail.insert("property".into(), json!(name));
        detail.insert("found".into(), json!(true));
        detail.insert("definition".into(), cap_detail_value(value)?);
        return Ok(Value::Object(detail).to_string());
    }

    // Paged catalog: name/type/ops/short hint only.
    let mut names: Vec<&String> = properties.keys().collect();
    names.sort();
    let total = names.len();
    let page_size = query.page_size;
    let page = query.page;
    let start = (page.saturating_sub(1)).saturating_mul(page_size);
    if start >= total && total > 0 {
        return Ok(json!({
            "format": format, "element": element, "page": page, "pageSize": page_size,
            "totalProperties": total, "properties": [],
            "hint": format!("Requested page {page} but there are only {} pages; request page 1..{}", total.div_ceil(page_size), total.div_ceil(page_size))
        }).to_string());
    }
    let mut items = Vec::new();
    for name in names.iter().skip(start).take(page_size) {
        let value = &properties[*name];
        items.push(json!({
            "name": name,
            "type": value.get("type").cloned().unwrap_or(Value::Null),
            "ops": ops(value),
            "hint": hint(value),
        }));
    }
    let pages = total.div_ceil(page_size).max(1);
    let has_next = page < pages;
    let mut shaped = json!({
        "format": format,
        "element": element,
        "page": page,
        "pageSize": page_size,
        "totalProperties": total,
        "properties": items,
        "complete": !has_next,
    });
    if has_next {
        shaped["next"] = json!({
            "tool": "office_help",
            "arguments": { "format": format, "element": element, "page": page + 1, "pageSize": page_size },
            "instruction": format!("This is page {page}/{pages}; {total} properties total. The next page MUST be requested with office_help(format={format}, element={element}, page={}) before using properties not listed on this page.", page + 1),
        });
    } else {
        shaped["next"] = json!({
            "tool": "office_help",
            "instruction": format!("All {total} properties are listed. For any property you need full examples/aliases/readback for, call office_help(format={format}, element={element}, property=<name>) once per property; do not prefetch properties you will not use."),
        });
    }
    let text = shaped.to_string();
    if text.len() > HELP_MAX_BYTES {
        // Keep the contract by shrinking the page automatically rather than truncating mid-JSON.
        return Ok(json!({
            "format": format, "element": element, "page": 1, "pageSize": page_size,
            "totalProperties": total, "properties": items.into_iter().take(page_size / 2).collect::<Vec<_>>(),
            "complete": false,
            "budgetBytes": HELP_MAX_BYTES,
            "next": { "tool": "office_help",
                "arguments": { "format": format, "element": element, "page": 1, "pageSize": (page_size / 2).max(10) },
                "instruction": "Catalog exceeded the single-response budget; re-read with the smaller pageSize and continue paging." },
        }).to_string());
    }
    Ok(text)
}

/// Bound the verbose fields of one property definition while keeping type,
/// operations and description intact. Examples/aliases are truncated, never
/// silently dropped without a marker.
fn cap_detail_value(value: &Value) -> Result<Value, String> {
    let mut detail = serde_json::Map::new();
    for (key, child) in value.as_object().ok_or("invalid property definition")? {
        let mut kept = child.clone();
        if matches!(key.as_str(), "examples" | "aliases") {
            if let Some(array) = kept.as_array_mut() {
                let mut bytes = 0usize;
                let mut truncated = false;
                array.retain(|item| {
                    bytes += item.to_string().len();
                    if bytes > HELP_DETAIL_MAX_BYTES {
                        truncated = true;
                        false
                    } else {
                        true
                    }
                });
                if truncated {
                    detail.insert(format!("{key}Truncated"), json!(true));
                    detail.insert(
                        format!("{key}More"),
                        json!("additional values omitted to bound context; use the documented --prop syntax and validate the result"),
                    );
                }
            }
        }
        detail.insert(key.clone(), kept);
    }
    Ok(Value::Object(detail))
}

/// Extract (rows, cols) from the pinned CLI's import summary
/// ("Imported N rows x M cols into /Sheet starting at A1").
/// Returns None when the envelope or wording is not recognized.
fn parse_import_summary(stdout: &str) -> Option<(u64, u32)> {
    let envelope: Value = serde_json::from_str(stdout.trim()).ok()?;
    if envelope["success"].as_bool() != Some(true) {
        return None;
    }
    let text = envelope["data"].as_str().unwrap_or_default();
    let rest = text.strip_prefix("Imported ")?;
    let rows_text: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = rest.get(rows_text.len()..)?.strip_prefix(" rows x ")?;
    let cols_text: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some((rows_text.parse().ok()?, cols_text.parse().ok()?))
}

/// SHA-256 and byte size of a document, used for the Host-managed version
/// record attached to office_create/office_edit results.
fn hash_file_meta(path: &Path) -> Option<(String, i64)> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total: i64 = 0;
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as i64);
    }
    Some((format!("{:x}", hasher.finalize()), total))
}

fn ensure_unchanged_office_source(path: &Path, expected_hash: &str) -> Result<(), String> {
    if hash_file_meta(path).is_some_and(|(actual, _)| actual == expected_hash) {
        Ok(())
    } else {
        Err("Office 原文件在编辑期间发生变化；已取消写入，请重新读取后重试".into())
    }
}

#[cfg(test)]
mod source_version_tests {
    use super::*;

    #[test]
    fn changed_external_document_refuses_in_place_commit() {
        let path = std::env::temp_dir().join(format!("fox-office-version-{}.xlsx", uuid::Uuid::new_v4()));
        fs::write(&path, b"original").unwrap();
        let original = hash_file_meta(&path).unwrap().0;
        assert!(ensure_unchanged_office_source(&path, &original).is_ok());
        fs::write(&path, b"external edit").unwrap();
        assert!(ensure_unchanged_office_source(&path, &original).is_err());
        fs::remove_file(path).unwrap();
    }
}

pub fn execute(
    server: &McpServerRecord,
    tool: &str,
    input: &Value,
    root: Option<&str>,
    permission: &str,
) -> Result<Value, String> {
    execute_with_cancellation(server, tool, input, root, permission, None, Duration::from_secs(270), None)
}

// Dispatch is synchronous. Thread-local observation avoids cross-test counter
// pollution and is completely absent from production builds.
#[cfg(test)]
thread_local! {
    static TEST_EXECUTOR_ENTRIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn executor_entries_for_test() -> u64 {
    TEST_EXECUTOR_ENTRIES.with(std::cell::Cell::get)
}

pub(crate) fn execute_with_cancellation(
    server: &McpServerRecord, tool: &str, input: &Value, root: Option<&str>, permission: &str,
    cancellation: Option<&crate::kernel::CancellationToken>, budget: Duration,
    context: Option<&OfficeCallContext<'_>>,
) -> Result<Value, String> {
    #[cfg(test)]
    TEST_EXECUTOR_ENTRIES.with(|count| count.set(count.get() + 1));
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
    let action = prepare_with_context(tool, input, root, permission, context)?;
    let cwd = (!action.root.as_os_str().is_empty()).then_some(action.root.as_path());
    // Working copies live in the Host's private artifact area, never next to
    // the user's document: a briefly locked sibling file made the project
    // folder unreadable and killed external file watchers with EBUSY. Layout
    // mirrors the target so the controlled staging file below can be created
    // on the target's own volume.
    let store = context.and_then(|context| context.artifact_store());
    // Cross-process ownership for the staging files this execution will create.
    //
    // The in-process `EXECUTION_LOCK` above says nothing about a second Fox
    // process working in the same project, so ownership of a staging file is
    // established with an OS handle whose sharing is denied. When the Host's
    // private artifact area is unavailable the write still proceeds, but no
    // staging file may then be reclaimed by anyone — leftovers are kept and
    // reported instead of guessed away.
    let ledger = match (&store, context) {
        (Some(store), Some(context)) => {
            match crate::runtime_host::artifact_store::StagingLedger::begin(
                store.root(),
                context.conversation_id,
            ) {
                Ok(ledger) => Some(ledger),
                Err(error) => {
                    eprintln!("[fox-office] 暂存归属台账不可用，残留将只保留不回收: {error}");
                    None
                }
            }
        }
        _ => None,
    };
    let mut temporary = None;
    let mut in_place_baseline: Option<(PathBuf, String)> = None;
    let mut staging_scope: Option<(PathBuf, PathBuf, String)> = None;
    if let Some(output) = &action.output {
        let ext = output
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("tmp")
            .to_owned();
        let base = match (&store, context) {
            (Some(store), Some(context)) => store
                .work_directory(context.conversation_id)
                .unwrap_or_else(|_| std::env::temp_dir()),
            _ => std::env::temp_dir(),
        };
        let path = base.join(format!("work-{}.{}", uuid::Uuid::new_v4(), ext));
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if matches!(tool, "office_edit" | "office_create" | "office_import_data") {
            if let Some(source) = &action.source {
                let baseline = if tool == "office_edit" && action.output.as_deref() == Some(source.as_path()) {
                    Some(hash_file_meta(source).ok_or("Office 无法读取原文件版本")?.0)
                } else { None };
                fs::copy(source, &path).map_err(|e| e.to_string())?;
                if let Some(hash) = baseline {
                    ensure_unchanged_office_source(source, &hash)?;
                    ensure_unchanged_office_source(&path, &hash)?;
                    in_place_baseline = Some((source.clone(), hash));
                }
            }
        }
        // The working copy is a real file with the document's own extension:
        // the pinned CLI infers the document format from it.
        temporary = Some(path);
        // Where a verified payload may be staged on the target volume.
        //
        // Inside the project the staging file sits in the target's own folder
        // (a project-relative path keeps `reclaim` scoped to the project).
        // A target *outside* the project — an evaluation output root, or an
        // explicitly rendered preview — must not stage inside the project root,
        // or the staging file would appear in a folder the user owns while the
        // real target lives elsewhere. It stages in the target's own directory
        // instead, which is also the only place guaranteed to be on the target's
        // volume.
        staging_scope = Some((
            output
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| action.root.as_path())
                .to_path_buf(),
            PathBuf::from("."),
        ))
        .map(|(root, relative)| (root, relative, uuid::Uuid::new_v4().to_string()));
        if let Some((root, _, _)) = staging_scope.as_ref() {
            // Reclaim residual staging from an interrupted earlier attempt.
            //
            // Reclaiming is a *proof*, never a filename match: the Host needs a
            // registered entry, a controlled location, no live owning process
            // (proven with the share-denied lease handle, so it holds against a
            // second Fox process), and no Host ledger still referencing the
            // bytes. Anything unproven is kept and reported. A user file that
            // merely happens to be called `.fox-stage-*.tmp` has no ledger entry
            // and is therefore never touched.
            if let Some(ledger) = ledger.as_ref() {
                let protect = |path: &Path| staging_reference_reason(context, path);
                let report =
                    ledger.reclaim(&[root.as_path(), action.root.as_path()], &protect);
                for line in report.diagnostics() {
                    eprintln!("[fox-office] {line}");
                }
            }
        }
    }
    // The Host's own version store for this write's undo copy. Passed to the
    // connector so no `*.fox-backup-*` file is ever created in the project.
    // Context-free callers fall back to a private temp directory, never to the
    // target's own folder.
    let snapshot_dir: PathBuf = context
        .map(snapshot_directory)
        .unwrap_or_else(|| std::env::temp_dir().join("fox-office-backups"));
    let _ = fs::create_dir_all(&snapshot_dir);
    let snapshot_dir = Some(snapshot_dir);
    // Version metadata for intercepted document writes, surfaced to the
    // Host as details.foxManagedFile (the Host registers the durable row).
    let mut managed_meta: Option<Value> = None;
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
        } else if let Some(spec) = action.import.as_ref() {
            let temp_path = temporary
                .as_ref()
                .expect("import output")
                .to_string_lossy()
                .into_owned();
            // A source-less import starts from a fresh workbook.
            if action.source.is_none() {
                invoke(
                    binary,
                    &["create".into(), temp_path.clone(), "--json".into()],
                    cwd,
                    cancellation,
                    deadline,
                )?;
            }
            // The pinned CLI refuses to import into a missing sheet, so check
            // the workbook inventory first and optionally add the sheet. Only
            // the immediate sheet directory is needed: a depth-1 inventory on
            // a populated workbook dumps every cell preview and can exceed the
            // 4 MiB invoke capture (observed ~4.4 MiB at 928 x 12), truncating
            // the JSON and failing large overwrite/continuation imports.
            // `--depth 0` still lists each direct child (path/type/preview) but
            // omits row/cell children, which is all the sheet check reads.
            let inventory = invoke(
                binary,
                &[
                    "get".into(),
                    temp_path.clone(),
                    "/".into(),
                    "--depth".into(),
                    "0".into(),
                    "--json".into(),
                ],
                cwd,
                cancellation,
                deadline,
            )?;
            let inventory: Value = serde_json::from_str(inventory.trim())
                .map_err(|_| "Office 组件返回了无法解析的工作簿结构".to_owned())?;
            let mut sheets = Vec::new();
            if let Some(children) = inventory["data"]["results"][0]["children"].as_array() {
                for child in children {
                    if child["type"].as_str() == Some("sheet") {
                        if let Some(name) = child["preview"].as_str() {
                            sheets.push(name.to_owned());
                        }
                    }
                }
            }
            if !sheets.iter().any(|name| name.eq_ignore_ascii_case(&spec.sheet)) {
                if spec.create_sheet {
                    let add = json!([{"command":"add","parent":"/","type":"sheet","props":{"name":spec.sheet}}]);
                    invoke_with_stdin(
                        binary,
                        &["batch".into(), temp_path.clone(), "--json".into()],
                        Some(&add.to_string()),
                        cwd,
                        cancellation,
                        deadline,
                    )?;
                } else {
                    return Err(format!(
                        "Office 工作表 '{}' 不存在（现有：{}）；设 createSheet=true 添加，或改用现有工作表",
                        spec.sheet,
                        if sheets.is_empty() { "无".into() } else { sheets.join("、") }
                    ));
                }
            }
            let import_stdout = invoke_with_stdin(
                binary,
                &args,
                action.stdin_data.as_deref(),
                cwd,
                cancellation,
                deadline,
            )?;
            import_stdout
        } else {
            invoke_with_stdin(binary, &args, action.stdin_data.as_deref(), cwd, cancellation, deadline)?
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
            if action.placement == OutputPlacement::Deliverable {
                scoped_path(
                    &action.root,
                    &output
                        .strip_prefix(&action.root)
                        .map_err(|_| "Office 输出超出项目")?
                        .to_string_lossy(),
                    false,
                )?;
            }
            if matches!(tool, "office_edit" | "office_create" | "office_import_data") {
                if let Some((source, hash)) = &in_place_baseline {
                    ensure_unchanged_office_source(source, hash)?;
                }
                // The undo copy always goes into the Host's own version store,
                // never next to the user's document.
                let before = output.exists().then(|| hash_file_meta(output)).flatten();
                let backup = match (before.as_ref(), snapshot_dir.as_ref()) {
                    (Some(_), Some(dir)) => {
                        let path = dir.join(format!("office-{}.foxbak", uuid::Uuid::new_v4()));
                        fs::copy(output, &path)
                            .map_err(|e| format!("Office 备份失败: {e}"))?;
                        Some(path)
                    }
                    _ => None,
                };
                if output.exists() && !action.overwrite {
                    return Err("Office 输出在执行期间出现，已保留原文件".into());
                }
                if let Some((source, hash)) = &in_place_baseline {
                    ensure_unchanged_office_source(source, hash)?;
                    if let Some(backup) = backup.as_ref() {
                        ensure_unchanged_office_source(backup, hash)?;
                    }
                }
                // One commit primitive: stage on the target volume, then swap.
                // A direct `fs::copy` over the live file is never described as
                // atomic, and a cross-volume rename from the application data
                // directory is never assumed to work.
                let (staging_root, staging_relative, staging_token) = match &staging_scope {
                    Some((root, relative, token)) => {
                        (root.clone(), relative.clone(), token.clone())
                    }
                    None => (
                        output.parent().unwrap_or(Path::new(".")).to_path_buf(),
                        PathBuf::from("."),
                        uuid::Uuid::new_v4().to_string(),
                    ),
                };
                crate::runtime_host::artifact_store::commit(
                    temp,
                    output,
                    &staging_root,
                    &staging_relative,
                    &staging_token,
                    ledger.as_ref(),
                )
                .map_err(|error| match backup.as_ref() {
                    Some(path) => format!(
                        "Office 写入失败，原文件未改变；写前备份位于 {}: {error}",
                        path.display()
                    ),
                    None => format!("Office 写入失败，未产生输出: {error}"),
                })?;
                if let Some((after_hash, after_size)) = hash_file_meta(output) {
                    let storage_path = output.canonicalize().unwrap_or_else(|_| output.to_path_buf());
                    // Project-relative display name, tolerating the Windows
                    // extended-length prefix a canonical path carries. A raw
                    // `strip_prefix` silently fails there and the user would see
                    // the full absolute path instead of `fox/<folder>/<file>`.
                    let display_name = root
                        .map(Path::new)
                        .and_then(|base| {
                            crate::runtime_host::artifact_store::relative_to(base, &storage_path)
                        })
                        .map(|relative| relative.to_string_lossy().into_owned())
                        .unwrap_or_else(|| storage_path.to_string_lossy().into_owned());
                    let change_kind = if backup.is_some() { "modified" } else { "created" };
                    let (artifact_class, artifact_origin) =
                        crate::runtime_host::artifact_store::classify(
                            root.map(Path::new),
                            &storage_path,
                            tool,
                            false,
                        );
                    managed_meta = Some(json!({
                        "tool": tool,
                        "storagePath": storage_path.to_string_lossy(),
                        "displayName": display_name,
                        "changeKind": change_kind,
                        "artifactClass": artifact_class.as_str(),
                        "artifactOrigin": artifact_origin.as_str(),
                        "beforeHash": before.as_ref().map(|(hash, _)| hash.clone()),
                        "beforeSize": before.as_ref().map(|(_, size)| *size),
                        "afterHash": after_hash,
                        "afterSize": after_size,
                        "backupPath": backup.as_ref().map(|path| path.to_string_lossy().into_owned()),
                    }));
                }
            } else if output.exists() {
                // `office_render` rewrites the same preview target on purpose:
                // replacing it in place is what keeps the result's preview link
                // valid, so no stale preview is left behind.
                let (staging_root, staging_relative, staging_token) = match &staging_scope {
                    Some((root, relative, token)) => {
                        (root.clone(), relative.clone(), token.clone())
                    }
                    None => (
                        output.parent().unwrap_or(Path::new(".")).to_path_buf(),
                        PathBuf::from("."),
                        uuid::Uuid::new_v4().to_string(),
                    ),
                };
                crate::runtime_host::artifact_store::commit(
                    temp,
                    output,
                    &staging_root,
                    &staging_relative,
                    &staging_token,
                    ledger.as_ref(),
                )?;
            } else {
                fs::rename(temp, output).map_err(|e| e.to_string())?;
            }
        }
        // office_help executes no document and the CLI returns the full
        // reference. Shape it Host-side into a bounded, pageable catalog so a
        // single 115-property element cannot flood the model context, and
        // avoid the result-as-escaped-JSON-string double wrapping.
        let mut content = if let Some(query) = action.help.as_ref() {
            vec![json!({"type":"text","text": shape_help(&stdout, query)?})]
        } else if let Some(spec) = action.import.as_ref() {
            // Prefer the writer's own accounting; fall back to the verified
            // scan so the continuation cell is always present.
            let reported = parse_import_summary(&stdout);
            let rows = reported.map(|(r, _)| r).unwrap_or(spec.rows);
            let cols = reported.map(|(_, c)| c).unwrap_or(spec.cols);
            let next_row = spec.start_row + rows;
            let next_cell = format!("{}{}", column_name(spec.start_col), next_row);
            vec![
                json!({"type":"text","text":json!({
                    "output":action.output,
                    "saved":true,
                    "validation":validation,
                    "visualChecked":false,
                    "import":{
                        "sheet":spec.sheet,
                        "startCell":spec.start_cell,
                        "rows":rows,
                        "cols":cols,
                        "header":spec.header,
                        "format":spec.format,
                        "writerSummary":stdout.trim(),
                    },
                    "nextStartCell": next_cell,
                    "continuation": format!("已写入 {rows} 行 x {cols} 列到 /{}（{} 起）。续写下一批时使用同一 output/sheet，startCell={next_cell}；相同 startCell 重跑会覆盖相同单元格，不会重复追加。", spec.sheet, spec.start_cell),
                }).to_string()}),
            ]
        } else {
            vec![
                json!({"type":"text","text":json!({"output":action.output,"result":stdout,"validation":validation,"saved":action.output.is_some(),"visualChecked":false}).to_string()}),
            ]
        };
        if action.screenshot {
            let bytes = fs::read(action.output.as_ref().expect("screenshot output"))
                .map_err(|e| e.to_string())?;
            if bytes.len() <= MAX_OUTPUT {
                content.push(json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            }
        }
        // A rendered preview is an *artifact*, not a content version: there is
        // no previous user document to version, and the preview is regenerated
        // from the source on demand. It is declared separately from
        // `foxManagedFile` so a preview can never enter the restore history or
        // be confused with a written deliverable.
        let mut preview_meta: Option<Value> = None;
        if action.render {
            if let Some(output) = action.output.as_ref() {
                if let Some((after_hash, after_size)) = hash_file_meta(output) {
                    let project_root = root.map(Path::new);
                    let host_private = action.placement == OutputPlacement::HostPrivatePreview;
                    let (class, origin) = crate::runtime_host::artifact_store::classify(
                        project_root,
                        output,
                        tool,
                        host_private,
                    );
                    let display_name = project_root
                        .and_then(|base| {
                            crate::runtime_host::artifact_store::relative_to(base, output)
                        })
                        .map(|relative| relative.to_string_lossy().into_owned())
                        .unwrap_or_else(|| output.to_string_lossy().into_owned());
                    preview_meta = Some(json!({
                        "tool": tool,
                        "storagePath": output.to_string_lossy(),
                        "displayName": display_name,
                        "artifactClass": class.as_str(),
                        "artifactOrigin": origin.as_str(),
                        "mediaType": if action.screenshot { "image/png" } else { "text/html" },
                        "afterHash": after_hash,
                        "afterSize": after_size,
                        "sourceFile": action
                            .source
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned()),
                    }));
                }
            }
        }
        let mut envelope = serde_json::Map::new();
        envelope.insert("content".into(), json!(content));
        envelope.insert("isError".into(), json!(false));
        let mut details = serde_json::Map::new();
        if let Some(meta) = managed_meta.take() {
            // Consumed by the Host Kernel dispatch for durable version
            // registration; also visible to the model as plain metadata.
            details.insert("foxManagedFile".into(), meta);
        }
        if let Some(meta) = preview_meta.take() {
            details.insert("foxPreview".into(), meta);
        }
        if !details.is_empty() {
            envelope.insert("details".into(), Value::Object(details));
        }
        Ok(Value::Object(envelope))
    })();
    if let Some(temp) = temporary {
        if temp.is_file() {
            let _ = fs::remove_file(temp);
        }
    }
    result
}

#[cfg(test)]
mod resource_error_tests {
    use super::*;

    /// Regression, 2026-10-01 review (R07): every classified local class names
    /// the **distinct** next action that resolves it, so the model can tell
    /// renaming from overwriting from re-reading from stopping.
    #[test]
    fn each_local_class_names_its_own_next_step() {
        let cases: [(&str, OfficeResourceKind); 7] = [
            (
                "Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true",
                OfficeResourceKind::TargetExists,
            ),
            ("Office 输入文件不存在", OfficeResourceKind::SourceMissing),
            (
                "Office 文件超出授权项目（包括符号链接目标）",
                OfficeResourceKind::PermissionDenied,
            ),
            (
                "Office 组件完整性校验失败，请修复 Fox 安装。",
                OfficeResourceKind::WorkerUnavailable,
            ),
            (
                "Office execution budget exceeded",
                OfficeResourceKind::TimedOut,
            ),
            (
                "[tool.invalid_input] Office 需要 output（项目内既有路径，配合 overwrite=true 写回）或 name（新成果文件名，由 Host 放入成果目录）其中之一；只给 file 不会写回原文件。",
                OfficeResourceKind::InvalidInput,
            ),
            (
                "[tool.read_only_input] 「in/sales.csv」是任务给出的只读输入材料",
                OfficeResourceKind::ReadOnlyInput,
            ),
        ];
        for (error, expected) in cases {
            let kind = classify_resource_error(error).unwrap_or_else(|| panic!("unclassified: {error}"));
            assert_eq!(kind, expected, "{error}");
        }
        // The steps have to be *different* facts, not one generic sentence.
        let steps: Vec<&str> = cases
            .iter()
            .map(|(_, kind)| kind.next_step())
            .collect();
        for (index, step) in steps.iter().enumerate() {
            for (other, competitor) in steps.iter().enumerate() {
                if index != other {
                    assert_ne!(step, competitor, "two classes share one next step");
                }
            }
        }
        // A conflict must not simply recommend overwriting.
        let exists = OfficeResourceKind::TargetExists.next_step();
        assert!(exists.contains("另一个文件名"), "{exists}");
        assert!(
            exists.find("另一个文件名").unwrap() < exists.find("overwrite=true").unwrap(),
            "renaming must be named before overwriting: {exists}"
        );
        // A broken connector is not retryable; a timeout and a conflict are.
        assert!(!OfficeResourceKind::WorkerUnavailable.retryable());
        assert!(!OfficeResourceKind::SourceMissing.retryable());
        assert!(!OfficeResourceKind::TargetExists.retryable());
        assert!(!OfficeResourceKind::ReadOnlyInput.retryable());
        assert!(OfficeResourceKind::TimedOut.retryable());
        assert!(OfficeResourceKind::Conflict.retryable());
    }

    /// An unrecognised failure stays unclassified, so the caller keeps it
    /// generic instead of forwarding a body this Host cannot vouch for.
    #[test]
    fn an_unclassified_failure_stays_unclassified() {
        for error in [
            "some connector wrote a raw remote body",
            "{\"error\":{\"message\":\"upstream 502 from the vendor gateway\"}}",
        ] {
            assert_eq!(classify_resource_error(error), None, "{error}");
        }
        assert_eq!(classify_resource_error("   "), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart_reference(props: usize) -> String {
        let mut properties = serde_json::Map::new();
        for index in 0..props {
            properties.insert(
                format!("prop{index}"),
                json!({
                    "type":"string",
                    "aliases":[format!("prop{index}alias")],
                    "add":true,"set":index%2==0,"get":index%3==0,
                    "description": format!("Long verbose description for property {index} that explains every detail and would normally flood the model context with repeated guidance."),
                    "examples":[format!("--prop prop{index}=Sheet1!A1"), format!("--prop prop{index}=Sheet1!B2")],
                    "readback":"read value",
                    "enforcement":"report"
                }),
            );
        }
        json!({
            "$schema":"../_schema.json","element":"chart","format":"xlsx","parent":"/",
            "operations":["add","set","get"],
            "paths":["/chart[1]"],
            "note":"test reference",
            "properties": Value::Object(properties)
        }).to_string()
    }

    #[test]
    fn office_help_catalog_is_bounded_paged_and_keeps_working_metadata() {
        let reference = chart_reference(115);
        assert!(reference.len() > 40_000, "fixture must reproduce the 55KB flood");
        let page1 = shape_help(&reference, &HelpQuery { property: None, page: 1, page_size: 40 }).unwrap();
        let v: Value = serde_json::from_str(&page1).unwrap();
        assert!(page1.len() < HELP_MAX_BYTES, "catalog page stays in budget, got {}", page1.len());
        assert_eq!(v["totalProperties"], 115);
        assert_eq!(v["properties"].as_array().unwrap().len(), 40);
        assert_eq!(v["complete"], false);
        assert!(v["next"]["instruction"].as_str().unwrap().contains("page=2"));
        // Hint is short; verbose examples/aliases are not on a catalog page.
        let hint = v["properties"][0]["hint"].as_str().unwrap();
        assert!(hint.chars().count() <= HELP_HINT_CHARS + 1);
        assert!(page1.find("--prop").is_none(), "no example CLI text on catalog page");
        // Operational flags survive so the model knows what it can do.
        assert!(v["properties"][0]["ops"].is_array());

        let page3 = shape_help(&reference, &HelpQuery { property: None, page: 3, page_size: 40 }).unwrap();
        let v3: Value = serde_json::from_str(&page3).unwrap();
        assert_eq!(v3["properties"].as_array().unwrap().len(), 35);
        assert_eq!(v3["complete"], true);

        // Single-property detail returns full definition with examples.
        let detail = shape_help(&reference, &HelpQuery { property: Some("prop7".into()), page: 1, page_size: 40 }).unwrap();
        let d: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(d["found"], true);
        assert!(d["definition"]["examples"].is_array());
        assert!(d["definition"]["description"].as_str().unwrap().contains("property 7"));

        // Alias lookup works and unknown property is reported without flooding.
        let by_alias = shape_help(&reference, &HelpQuery { property: Some("prop9alias".into()), page: 1, page_size: 40 }).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&by_alias).unwrap()["found"], true);
        let missing = shape_help(&reference, &HelpQuery { property: Some("nope".into()), page: 1, page_size: 40 }).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&missing).unwrap()["found"], false);
    }

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

    /// Every transient name this connector used to scatter next to the target.
    fn transient_entries(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|name| crate::runtime_host::artifact_store::is_host_transient_name(name))
            .collect()
    }

    fn office_fixture() -> (PathBuf, Database, McpServerRecord, String, PathBuf) {
        let root = project();
        let db = Database::open(root.join("test.db")).unwrap();
        setup(&db, &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources")).unwrap();
        let server = db
            .list_mcp_servers()
            .unwrap()
            .into_iter()
            .find(|s| s.id == SERVER_ID)
            .unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let sessions = root.join("runtime-sessions");
        fs::create_dir_all(&sessions).unwrap();
        (root, db, server, conversation, sessions)
    }

    /// The artifact data channel and the deliverable placement compose: a real
    /// CSV saved by compute is imported by `artifactId` into a workbook that
    /// lives in the conversation's deliverable folder, and the resulting
    /// workbook is genuinely readable with the same rows and columns.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_artifact_import_into_the_deliverable_folder_stays_readable() {
        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        // A saved compute artifact, exactly as attachment_compute would leave it.
        let compute_root = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions,
            &conversation,
            "run-artifact-deliverable",
        )
        .unwrap();
        let outputs = compute_root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        let mut csv = String::from("id,tag,v\n");
        for index in 0..500u32 {
            csv.push_str(&format!("{index},agv-{index},{}\n", 10_000 + index));
        }
        let csv_path = outputs.join("agv.csv");
        fs::write(&csv_path, csv.as_bytes()).unwrap();
        let csv_path_string = csv_path.to_string_lossy().into_owned();
        let artifact_id = Database::computed_artifact_id(&csv_path_string);
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,NULL,'agv.csv','created_file',?3,'text/csv',?4,?5,'ready',?6,?6)",
                rusqlite::params![
                    &artifact_id,
                    &conversation,
                    &csv_path_string,
                    csv.len() as i64,
                    hex::encode(Sha256::digest(csv.as_bytes())),
                    crate::database::now_ms(),
                ],
            )
        })
        .unwrap();

        let folder = crate::runtime_host::artifact_store::ArtifactStore::new(&artifacts)
            .deliverable_directory_name("AGV 统计", &conversation);
        let relative = format!("fox/{folder}/AGV统计.xlsx");
        let created = execute_with_cancellation(
            &server,
            "office_import_data",
            &json!({
                "output": relative,
                "sheet": "Sheet1",
                "createSheet": false,
                "artifactId": artifact_id,
            }),
            root.to_str(),
            "allow",
            None,
            Duration::from_secs(180),
            Some(&context),
        )
        .unwrap();
        assert_eq!(created["details"]["foxManagedFile"]["artifactClass"], "deliverable");
        // The saved workbook is really readable and carries the imported rows.
        let read = execute_with_cancellation(
            &server,
            "office_read",
            &json!({"file": relative, "mode": "get", "selector": "/Sheet1/A1:C3"}),
            root.to_str(),
            "read_only",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();
        let text = read.to_string();
        assert!(text.contains("agv-0"), "{text}");
        assert!(text.contains("id"), "{text}");
        // The imported payload came from the artifact, never from a re-typed copy.
        assert!(
            read["content"][0]["text"]
                .as_str()
                .map(|value| !value.contains(&csv_path_string))
                .unwrap_or(true),
            "the source artifact path must not leak into the read result"
        );
        assert!(root.join(&relative).is_file());
        assert_eq!(transient_entries(&root), Vec::<String>::new());
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(artifacts).ok();
    }

    /// The working copy, the commit staging file and the pre-write backup must
    /// all live outside the user's project, and a real create → edit → overwrite
    /// cycle must leave the project folder containing nothing but documents.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_writes_leave_no_transient_state_in_the_project() {
        let (root, db, server, conversation, sessions) = office_fixture();
        // The Host's private area is a different root than the project, exactly
        // as in production (application data vs. the user's project).
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        let r = root.to_str();
        let run = |tool: &str, input: Value| {
            execute_with_cancellation(
                &server,
                tool,
                &input,
                r,
                "allow",
                None,
                Duration::from_secs(120),
                Some(&context),
            )
        };

        run("office_create", json!({"output":"keep.xlsx"})).unwrap();
        run(
            "office_edit",
            json!({"file":"keep.xlsx","output":"edited.xlsx","operations":[
                {"command":"set","path":"/Sheet1/A1","props":{"value":"first"}}]}),
        )
        .unwrap();
        // An explicit overwrite takes the staged-replace path.
        run(
            "office_edit",
            json!({"file":"keep.xlsx","output":"keep.xlsx","overwrite":true,"operations":[
                {"command":"set","path":"/Sheet1/B2","props":{"value":"second"}}]}),
        )
        .unwrap();

        let read = run("office_read", json!({"file":"keep.xlsx"})).unwrap();
        assert!(read.to_string().contains("second"), "{read}");
        assert_eq!(
            transient_entries(&root),
            Vec::<String>::new(),
            "the project folder must not contain a working copy, a staging file or a backup"
        );
        // The Host's own version store kept the undo copy instead.
        let snapshots = fs::read_dir(snapshot_directory(&context)).unwrap().count();
        assert!(snapshots >= 1, "a pre-write snapshot must exist in the Host store");
        fs::remove_dir_all(root).ok();
    }

    /// Cleanup must be a proof of ownership, not a filename match.
    ///
    /// A file the user happened to name like a staging file survives an Office
    /// run byte for byte, while a leftover this Host *registered* for an
    /// execution that no longer exists is reclaimed — and its ledger entry is
    /// dropped with it. This is the acceptance test for the P1 finding: the old
    /// implementation deleted every `.fox-stage-*.tmp` it could see.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_cleanup_needs_registered_ownership_not_a_filename() {
        use crate::runtime_host::artifact_store::{
            staging_entry_path_for_test, write_staging_record_for_test, StagingRecord,
        };
        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        // The Host's artifact root, exactly as `OfficeCallContext::artifact_store`
        // derives it.
        let artifact_root = artifacts.join("artifacts");

        // (1) Files only the *user* created: identical names, no ledger entry.
        let user_root = root.join(".fox-stage-user-root.tmp");
        let user_nested_dir = root.join("fox").join("case");
        fs::create_dir_all(&user_nested_dir).unwrap();
        let user_nested = user_nested_dir.join(".fox-stage-user-nested.tmp");
        let user_bytes = b"bytes a user owns and Fox never wrote".to_vec();
        fs::write(&user_root, &user_bytes).unwrap();
        fs::write(&user_nested, &user_bytes).unwrap();

        // (2) A genuinely abandoned commit: registered here, owned by an
        // execution id that has no lease at all (a crashed process).
        let orphan = root.join(".fox-stage-orphanleftover.tmp");
        fs::write(&orphan, b"half-written staging payload").unwrap();
        write_staging_record_for_test(
            &artifact_root,
            &StagingRecord {
                token: "orphanleftover".to_owned(),
                staging_path: orphan.to_string_lossy().into_owned(),
                target_path: root.join("never-produced.xlsx").to_string_lossy().into_owned(),
                conversation_id: conversation.clone(),
                execution_id: "crashed-execution-with-no-lease".to_owned(),
                state: "staging".to_owned(),
                payload_sha256: String::new(),
                created_at_ms: 0,
            },
        )
        .unwrap();
        assert!(staging_entry_path_for_test(&artifact_root, "orphanleftover").is_file());

        // A real Office run in that same directory.
        execute_with_cancellation(
            &server,
            "office_create",
            &json!({"output":"cleanup-probe.xlsx"}),
            root.to_str(),
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();
        assert!(root.join("cleanup-probe.xlsx").is_file());

        // The user's files are untouched, byte for byte.
        assert_eq!(fs::read(&user_root).unwrap(), user_bytes, "user file was deleted");
        assert_eq!(fs::read(&user_nested).unwrap(), user_bytes, "nested user file was deleted");
        // The registered, owner-free leftover is reclaimed and the ledger
        // converges, so the record cannot accumulate forever.
        assert!(!orphan.exists(), "a registered orphaned staging file must be reclaimed");
        assert!(!staging_entry_path_for_test(&artifact_root, "orphanleftover").exists());
        // The run itself added nothing: the only staging-looking name in the
        // project root is still the user's own file.
        assert_eq!(
            transient_entries(&root),
            vec![".fox-stage-user-root.tmp".to_owned()],
            "the run must not add any transient file of its own"
        );
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(artifacts).ok();
    }

    /// The pinned CLI creates its own `.work-<uuid>.batch-<hex>.<ext>` copies
    /// **next to the document it is given**. A "zero residue afterwards" check
    /// cannot see them, and a filename-only error message cannot prove where
    /// they were created, so this test watches the tree *while the commands run*
    /// and asserts where those copies really appear.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_working_copies_appear_only_in_the_host_area_while_running() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        let r = root.to_str();

        // Observer: record every file that ever exists under either root. Two
        // milliseconds is short enough to catch a copy that lives for the
        // duration of one CLI invocation.
        //
        // The staging check is deliberately *not* "was the ledger entry present
        // at the instant I looked at the file": the entry is written before the
        // staging file is created and removed after it is consumed, so the two
        // reads cannot be atomic. What is race-free is the set relationship —
        // the entry exists strictly longer than the file — so the observer also
        // accumulates every token it ever sees registered, and the assertion
        // below is set inclusion.
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let staging_tokens: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let registered_tokens: Arc<Mutex<std::collections::HashSet<String>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        let ledger_entries = artifacts.join("artifacts").join("staging").join("entries");
        let observer = {
            let stop = Arc::clone(&stop);
            let seen = Arc::clone(&seen);
            let staging_tokens = Arc::clone(&staging_tokens);
            let registered_tokens = Arc::clone(&registered_tokens);
            let roots = vec![root.clone(), artifacts.clone()];
            let project = root.clone();
            thread::spawn(move || {
                /// Depth-first file listing, small enough to run every 2 ms.
                fn collect(base: &Path, out: &mut Vec<PathBuf>) {
                    let Ok(entries) = fs::read_dir(base) else { return };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        out.push(path.clone());
                        if path.is_dir() {
                            collect(&path, out);
                        }
                    }
                }
                while !stop.load(Ordering::Relaxed) {
                    let mut batch = Vec::new();
                    for base in &roots {
                        collect(base, &mut batch);
                    }
                    if let Ok(entries) = fs::read_dir(&ledger_entries) {
                        let mut registered = registered_tokens.lock().unwrap();
                        for entry in entries.flatten() {
                            if let Some(token) = entry
                                .path()
                                .file_stem()
                                .and_then(|value| value.to_str())
                            {
                                registered.insert(token.to_owned());
                            }
                        }
                    }
                    for path in &batch {
                        if !path.starts_with(&project) {
                            continue;
                        }
                        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                            continue;
                        };
                        if let Some(token) =
                            crate::runtime_host::artifact_store::staging_token_of_name(name)
                        {
                            staging_tokens.lock().unwrap().push(token.to_owned());
                        }
                    }
                    seen.lock().unwrap().extend(batch);
                    thread::sleep(Duration::from_millis(2));
                }
            })
        };

        let run = |tool: &str, input: Value| {
            execute_with_cancellation(
                &server,
                tool,
                &input,
                r,
                "allow",
                None,
                Duration::from_secs(180),
                Some(&context),
            )
        };
        run("office_create", json!({"output":"probe.xlsx"})).unwrap();
        run(
            "office_edit",
            json!({"file":"probe.xlsx","output":"probe.xlsx","overwrite":true,"operations":[
                {"command":"set","path":"/Sheet1/A1","props":{"value":"edited"}}]}),
        )
        .unwrap();
        run(
            "office_import_data",
            json!({"file":"probe.xlsx","output":"probe.xlsx","overwrite":true,"sheet":"Sheet1",
                   "startCell":"A3","data":"name,qty\nalpha,1"}),
        )
        .unwrap();
        run("office_render", json!({"file":"probe.xlsx","mode":"html"})).unwrap();
        // A failure path too: nothing may be left in the project either.
        assert!(run(
            "office_edit",
            json!({"file":"probe.xlsx","output":"probe.xlsx","overwrite":true,"operations":[
                {"command":"remove","path":"/Sheet1/ZZZ999"}]}),
        )
        .is_err());
        stop.store(true, Ordering::Relaxed);
        observer.join().unwrap();

        let observed = seen.lock().unwrap().clone();
        assert!(!observed.is_empty(), "the observer must have seen the fixture");
        let transient = |path: &Path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    crate::runtime_host::artifact_store::is_host_transient_name(name)
                        || name.starts_with(".work-")
                        || name.contains(".batch-")
                })
        };
        // 1) No working copy, no pre-write backup and no CLI work copy may exist
        //    in the user's project at any instant. The one Host-managed file that
        //    legitimately appears there is the commit staging file, which must
        //    live on the target's own volume; it is checked separately below.
        let forbidden: Vec<&PathBuf> = observed
            .iter()
            .filter(|path| {
                path.starts_with(&root)
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with(".fox-office-")
                                || name.contains(".fox-backup-")
                                || name.starts_with(".work-")
                                || name.contains(".batch-")
                        })
            })
            .collect();
        assert!(
            forbidden.is_empty(),
            "the project must never contain a working copy or backup: {forbidden:?}"
        );
        // 2) The commit staging file does appear briefly, and every staging
        //    token that was ever visible in the project also appeared in the
        //    ownership ledger — that registration is what makes reclaim safe,
        //    and it is the property the old filename sweep lacked.
        let observed_tokens = staging_tokens.lock().unwrap().clone();
        assert!(
            !observed_tokens.is_empty(),
            "the target-volume staging file must be observable while a commit runs"
        );
        let registered = registered_tokens.lock().unwrap().clone();
        let anonymous: Vec<&String> = observed_tokens
            .iter()
            .filter(|token| !registered.contains(*token))
            .collect();
        assert!(
            anonymous.is_empty(),
            "every staging file in the project must be ledger-registered while it exists: {anonymous:?}"
        );
        // 3) The CLI's own working copies really were created — in the Host's
        //    private area. Without this the first assertion would also pass if no
        //    write had happened at all.
        let in_host_area: Vec<&PathBuf> = observed
            .iter()
            .filter(|path| path.starts_with(&artifacts) && transient(path))
            .collect();
        assert!(
            !in_host_area.is_empty(),
            "the pinned CLI's working copies must live in the Host area; observed none in {}",
            artifacts.display()
        );
        // 4) And once everything finished, the project holds no transient name.
        assert_eq!(
            transient_entries(&root),
            Vec::<String>::new(),
            "the project must be clean after the run"
        );
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(artifacts).ok();
    }

    /// A rendered preview is Host-private by default: one entry per document and
    /// mode, re-rendered in place, and never a file in the user's project.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_preview_is_host_private_and_reused() {        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        let r = root.to_str();
        execute_with_cancellation(
            &server,
            "office_create",
            &json!({"output":"preview-me.xlsx"}),
            r,
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();

        let render = |output: &str| {
            execute_with_cancellation(
                &server,
                "office_render",
                &json!({"file":"preview-me.xlsx","output":output,"mode":"html"}),
                r,
                "allow",
                None,
                Duration::from_secs(180),
                Some(&context),
            )
            .unwrap()
        };
        let first = render("preview.html");
        let preview = first["details"]["foxPreview"].clone();
        assert!(preview.is_object(), "a render must declare its preview artifact: {first}");
        assert_eq!(preview["artifactClass"], "preview");
        assert_eq!(preview["artifactOrigin"], "host_private");
        // A rendered preview is a view, not a content version.
        assert!(first["details"].get("foxManagedFile").is_none());
        let storage = PathBuf::from(preview["storagePath"].as_str().unwrap());
        assert!(storage.is_file(), "{storage:?}");
        assert!(
            !storage.starts_with(&root),
            "the preview must not be written into the project: {storage:?}"
        );
        assert!(storage.starts_with(&artifacts), "{storage:?}");
        // A re-render reuses the same preview file, so no stale link survives.
        let second = render("preview.html");
        assert_eq!(
            second["details"]["foxPreview"]["storagePath"].as_str().unwrap(),
            storage.to_string_lossy()
        );
        assert!(!root.join("preview.html").exists());
        assert_eq!(transient_entries(&root), Vec::<String>::new());
        fs::remove_dir_all(root).ok();
    }

    /// The Host's default-placement contract and the purpose split, end to end
    /// with the real CLI: `name` puts a new result in the *advertised* result
    /// folder, a plain render stays a Host-private preview, an exported render
    /// is a deliverable, and editing an existing file keeps the original target
    /// that the approval showed.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_default_placement_and_purpose_follow_host_rules() {
        use crate::runtime_host::artifact_store::{
            is_valid_deliverable_folder_name, project_key, ArtifactStore, PROJECT_DELIVERABLE_DIR,
        };
        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        let r = root.to_str();
        let run = |tool: &str, input: Value| {
            execute_with_cancellation(
                &server,
                tool,
                &input,
                r,
                "allow",
                None,
                Duration::from_secs(180),
                Some(&context),
            )
        };

        // (1) `name` with no path at all: the Host decides the folder, and it is
        // the same folder the prompt advertises as `deliverableRoot`.
        let created = run("office_create", json!({"name":"AGV分析报告.xlsx"})).unwrap();
        let meta = created["details"]["foxManagedFile"].clone();
        assert_eq!(meta["artifactClass"], "deliverable");
        assert_eq!(meta["artifactOrigin"], "project");
        let storage = PathBuf::from(meta["storagePath"].as_str().unwrap());
        assert!(storage.is_file());
        let relative = crate::runtime_host::artifact_store::relative_to(&root, &storage)
            .expect("the result lives in the project")
            .to_string_lossy()
            .replace('\\', "/");
        // The advertised folder is the same ledger entry the prompt reads.
        let store = ArtifactStore::new(&artifacts);
        let advertised = db
            .ensure_deliverable_folder(
                &conversation,
                &project_key(r.unwrap()),
                &store.deliverable_directory_name("", &conversation),
                &is_valid_deliverable_folder_name,
                crate::database::now_ms(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            relative,
            format!("{PROJECT_DELIVERABLE_DIR}/{advertised}/AGV分析报告.xlsx"),
            "the Host must place a named artifact in the folder it advertises"
        );
        assert_eq!(
            fs::read_dir(root.join(PROJECT_DELIVERABLE_DIR)).unwrap().count(),
            1,
            "one logical task keeps one result folder"
        );

        // (2) A render with no target is a Host-private preview, not a result.
        let rendered = run("office_render", json!({"file":relative,"mode":"html"})).unwrap();
        let preview = rendered["details"]["foxPreview"].clone();
        assert_eq!(preview["artifactClass"], "preview");
        assert_eq!(preview["artifactOrigin"], "host_private");
        let preview_path = PathBuf::from(preview["storagePath"].as_str().unwrap());
        assert!(!preview_path.starts_with(&root), "{preview_path:?}");
        assert!(preview_path.starts_with(&artifacts), "{preview_path:?}");

        // (3) The same render exported by name is a deliverable in the folder.
        let exported = run(
            "office_render",
            json!({"file":relative,"name":"预览.html","mode":"html"}),
        )
        .unwrap();
        let exported_meta = exported["details"]["foxPreview"].clone();
        assert_eq!(exported_meta["artifactClass"], "deliverable");
        assert_eq!(exported_meta["artifactOrigin"], "project");
        assert_eq!(
            crate::runtime_host::artifact_store::relative_to(
                &root,
                Path::new(exported_meta["storagePath"].as_str().unwrap())
            )
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/"),
            format!("{PROJECT_DELIVERABLE_DIR}/{advertised}/预览.html")
        );

        // (4) Editing an existing file keeps the authorized original target, and
        // the target the approval resolves is the target that is executed.
        let edit = json!({"file":relative,"output":relative,"overwrite":true,"operations":[
            {"command":"set","path":"/Sheet1/A1","props":{"value":"edited"}}]});
        let prepared = prepare_with_context("office_edit", &edit, r, "allow", Some(&context)).unwrap();
        assert_eq!(
            prepared.target_path().expect("edit resolves a target"),
            storage.as_path(),
            "the approved target and the executed target must be one path"
        );
        let edited = run("office_edit", edit).unwrap();
        assert_eq!(edited["details"]["foxManagedFile"]["artifactClass"], "deliverable");
        assert_eq!(
            PathBuf::from(
                edited["details"]["foxManagedFile"]["storagePath"].as_str().unwrap()
            ),
            storage
        );

        // (5) `name` is a name, not a path: separators are refused rather than
        // silently relocated.
        for bad in ["sub/report.xlsx", "..\\escape.xlsx", "C:evil.xlsx"] {
            let error = run("office_create", json!({"name":bad})).unwrap_err();
            assert!(error.contains("name"), "{bad}: {error}");
        }
        assert_eq!(transient_entries(&root), Vec::<String>::new());
        fs::remove_dir_all(root).ok();
        fs::remove_dir_all(artifacts).ok();
    }

    /// An explicit path inside the conversation deliverable folder is a user
    /// deliverable: the file (and a requested HTML export) lands there, and the
    /// Host classifies it as a deliverable rather than as a process file.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_office_deliverable_folder_is_respected() {
        let (root, db, server, conversation, sessions) = office_fixture();
        let artifacts = project();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: Some(&artifacts),
        };
        let r = root.to_str();
        let folder = crate::runtime_host::artifact_store::ArtifactStore::new(&artifacts)
            .deliverable_directory_name("AGV report", &conversation);
        let relative = format!("fox/{folder}/交付表.xlsx");
        let created = execute_with_cancellation(
            &server,
            "office_create",
            &json!({"output":relative}),
            r,
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();
        assert_eq!(created["details"]["foxManagedFile"]["artifactClass"], "deliverable");
        // The display name is the project-relative path (separators are
        // platform-native, so compare on normalized form).
        assert_eq!(
            created["details"]["foxManagedFile"]["displayName"]
                .as_str()
                .unwrap()
                .replace('\\', "/"),
            relative
        );
        assert!(root.join(&relative).is_file(), "the deliverable must exist where asked");
        // Contract change (R3), recorded deliberately rather than relaxed:
        // `office_create`/`office_edit`/`office_import_data`/`office_merge` are
        // the *document* channel — the formats the user receives — so a file the
        // caller explicitly named is the user's result wherever it sits inside
        // the authorized project. Intermediates are not produced here at all;
        // they come from `attachment_compute`, whose outputs stay Host-private
        // (see the compute-artifact test above). The old rule called every
        // project-root document a process file, which is exactly the P2 finding:
        // a report the user asked to be written elsewhere was filed as an
        // intermediate.
        let root_level = execute_with_cancellation(
            &server,
            "office_create",
            &json!({"output":"scratch.xlsx"}),
            r,
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();
        assert_eq!(
            root_level["details"]["foxManagedFile"]["artifactClass"],
            "deliverable"
        );
        assert_eq!(
            root_level["details"]["foxManagedFile"]["artifactOrigin"],
            "project"
        );
        assert!(root.join("scratch.xlsx").is_file());
        // The connector writes DOCX/XLSX/PPTX only: it does not pretend to emit
        // a CSV, and it says so. A CSV the user asked to be *delivered* therefore
        // reaches the result folder through the general file tool, which the
        // classification tests cover (a CSV inside `fox/` is a deliverable, a CSV
        // from the compute workspace is a process file).
        let refused = execute_with_cancellation(
            &server,
            "office_import_data",
            &json!({"output":format!("fox/{folder}/统计明细.csv"),"sheet":"数据","createSheet":true,"data":"列,值\nalpha,1"}),
            r,
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap_err();
        assert!(refused.contains("DOCX"), "{refused}");
        assert!(!root.join("fox").join(&folder).join("统计明细.csv").exists());
        // An HTML preview the user explicitly asked to be delivered goes to the
        // deliverable folder with the project origin.
        let exported = format!("fox/{folder}/预览.html");
        let rendered = execute_with_cancellation(
            &server,
            "office_render",
            &json!({"file":relative,"output":exported,"mode":"html"}),
            r,
            "allow",
            None,
            Duration::from_secs(180),
            Some(&context),
        )
        .unwrap();
        assert_eq!(rendered["details"]["foxPreview"]["artifactOrigin"], "project");
        assert_eq!(
            rendered["details"]["foxPreview"]["artifactClass"],
            "deliverable",
            "an explicitly exported HTML is the user's result, not a view"
        );
        // An explicitly requested export lands in the project, at the exact
        // requested relative path.
        let exported_storage = PathBuf::from(
            rendered["details"]["foxPreview"]["storagePath"].as_str().unwrap(),
        );
        assert_eq!(
            crate::runtime_host::artifact_store::relative_to(&root, &exported_storage)
                .expect("the exported preview lives in the project")
                .to_string_lossy()
                .replace('\\', "/"),
            exported
        );
        // The folder itself is reused: rendering the same document again does
        // not create a second result folder.
        let folders = fs::read_dir(root.join("fox")).unwrap().count();
        assert_eq!(folders, 1, "one logical task keeps one result folder");
        assert_eq!(transient_entries(&root), Vec::<String>::new());
        fs::remove_dir_all(root).ok();
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

    #[test]
    fn a1_cell_parsing_and_column_names_roundtrip() {
        assert_eq!(parse_a1_cell("A1").unwrap(), (1, 1));
        assert_eq!(parse_a1_cell("b12").unwrap(), (12, 2));
        assert_eq!(parse_a1_cell("XFD1048576").unwrap(), (1_048_576, 16_384));
        for bad in ["", "1A", "A0", "A", "XFE1", "A1048577", "AAAA1", "A 1", "A-1"] {
            assert!(parse_a1_cell(bad).is_err(), "{bad}");
        }
        assert_eq!(column_name(1), "A");
        assert_eq!(column_name(26), "Z");
        assert_eq!(column_name(27), "AA");
        assert_eq!(column_name(16_384), "XFD");
    }

    #[test]
    fn import_data_scan_counts_bounds_and_screens_formulas() {
        assert_eq!(scan_import_data("a,b\n1,2\n", "csv").unwrap(), (2, 2));
        // Quoted commas/newlines do not inflate the count.
        assert_eq!(scan_import_data("a,b\n\"x,y\",\"1\n2\"", "csv").unwrap(), (2, 2));
        assert_eq!(scan_import_data("a\tb\n1\t2", "tsv").unwrap(), (2, 2));
        assert!(scan_import_data("", "csv").is_err());
        assert!(scan_import_data("a,b\n\"unclosed", "csv").is_err());
        assert!(scan_import_data("a,b\n1,2", "json").is_err());
        // Formula screening mirrors safe_properties: benign formulas pass,
        // external/network/DDE formulas are rejected with the row number.
        assert!(scan_import_data("x\n=SUM(A1:A3)", "csv").is_ok());
        let err = scan_import_data("ok\n=WEBSERVICE(\"https://x\")", "csv").unwrap_err();
        assert!(err.contains("第 2 行"), "{err}");
        assert!(scan_import_data("=HYPERLINK(\"https://x\")", "csv").is_err());
        assert!(scan_import_data("=[1]Sheet!A1", "csv").is_err());
        // Column overflow names the dimension and the remedy.
        let wide = (0..=IMPORT_MAX_COLUMNS).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let err = scan_import_data(&wide, "csv").unwrap_err();
        assert!(err.contains("列"), "{err}");
    }

    #[test]
    fn import_data_prepare_validates_scope_grid_and_permissions() {
        let root = project();
        let r = root.to_str();
        // Happy path without a source: creates the workbook, plans the import.
        let action = prepare(
            "office_import_data",
            &json!({"output":"new.xlsx","sheet":"数据","createSheet":true,"data":"name,value\nalpha,1"}),
            r,
            "allow",
        )
        .unwrap();
        let spec = action.import.as_ref().unwrap();
        assert_eq!(spec.sheet, "数据");
        assert!(spec.create_sheet);
        assert_eq!((spec.start_row, spec.start_col), (1, 1));
        assert_eq!((spec.rows, spec.cols), (2, 2));
        assert_eq!(action.stdin_data.as_deref().unwrap(), "name,value\nalpha,1");
        assert!(action.args.iter().any(|a| a == "import"));
        assert!(action.mutates);
        // Read-only projects reject the import before any execution.
        assert!(prepare(
            "office_import_data",
            &json!({"output":"new2.xlsx","sheet":"S","data":"a\n1"}),
            r,
            "read_only"
        )
        .is_err());
        // Sheet names follow Excel rules.
        assert!(prepare(
            "office_import_data",
            &json!({"output":"n.xlsx","sheet":"a/b","data":"x\n1"}),
            r,
            "allow"
        )
        .is_err());
        // startCell must be a real A1 reference inside the grid.
        assert!(prepare(
            "office_import_data",
            &json!({"output":"n.xlsx","sheet":"S","startCell":"XFE1","data":"x\n1"}),
            r,
            "allow"
        )
        .is_err());
        // A block that would cross the grid edge names the dimensions.
        let err = prepare(
            "office_import_data",
            &json!({"output":"n.xlsx","sheet":"S","startCell":"A1048576","data":"x\n1\n2"}),
            r,
            "allow"
        )
        .unwrap_err();
        assert!(err.contains("超出工作表网格"), "{err}");
        // Oversize payloads name the limit and the continuation remedy.
        let big = "x\n".repeat(IMPORT_MAX_DATA_BYTES / 2 + 1);
        let err = prepare(
            "office_import_data",
            &json!({"output":"n.xlsx","sheet":"S","data":big}),
            r,
            "allow"
        )
        .unwrap_err();
        assert!(err.contains("nextStartCell"), "{err}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn office_edit_sends_operations_over_stdin_not_the_command_line() {
        let root = project();
        fs::write(root.join("report.docx"), "fixture").unwrap();
        let action = prepare(
            "office_edit",
            &json!({"file":"report.docx","output":"new.docx","operations":[{"command":"add","path":"/body","type":"paragraph","props":{"text":"x"}}]}),
            root.to_str(),
            "allow",
        )
        .unwrap();
        assert!(!action.args.iter().any(|a| a == "--commands"));
        assert!(action.stdin_data.as_deref().unwrap().contains("\"command\":\"add\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_summary_is_parsed_from_the_writer_envelope() {
        let stdout = json!({"success":true,"data":"Imported 12 rows x 3 cols into /Sheet1 starting at A1"}).to_string();
        assert_eq!(parse_import_summary(&stdout), Some((12, 3)));
        assert_eq!(parse_import_summary("{\"success\":false}"), None);
        assert_eq!(parse_import_summary("not json"), None);
    }

    /// Real pinned-CLI chain for the data-block import: creation, typed
    /// values, idempotent replay, continuation batches, overwrite backups and
    /// the managed-file metadata the Host turns into a restorable version.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_import_data_writes_batches_idempotently_and_registers_versions() {
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
        let r = root.to_str();

        // 1) Create + first block in one call (no source file).
        let created = execute(
            &server,
            "office_import_data",
            &json!({"output":"book.xlsx","sheet":"数据","createSheet":true,"header":true,"data":"name,qty\nalpha,1"}),
            r,
            "allow",
        )
        .unwrap();
        let meta = &created["details"]["foxManagedFile"];
        assert_eq!(meta["tool"], "office_import_data");
        assert_eq!(meta["changeKind"], "created");
        let text = created["content"][0]["text"].as_str().unwrap();
        let shaped: Value = serde_json::from_str(text).unwrap();
        assert_eq!(shaped["import"]["rows"], 2);
        assert_eq!(shaped["nextStartCell"], "A3");
        // Typed values: qty is a number, not text.
        let read = execute(
            &server,
            "office_read",
            &json!({"file":"book.xlsx","mode":"get","selector":"/数据/A1:B2"}),
            r,
            "read_only",
        )
        .unwrap();
        let read_text = read["content"][0]["text"].as_str().unwrap();
        let outer: Value = serde_json::from_str(read_text).unwrap();
        let inner: Value = serde_json::from_str(outer["result"].as_str().unwrap()).unwrap();
        let cells = inner["data"]["results"][0]["children"].as_array().unwrap();
        let cell = |path: &str| cells.iter().find(|c| c["path"] == path).unwrap().clone();
        assert_eq!(cell("/数据/B2")["format"]["type"], "Number");
        assert_eq!(cell("/数据/A2")["text"], "alpha");

        // 2) Continuation batch at the reported cell lands below the first.
        let second = execute(
            &server,
            "office_import_data",
            &json!({"file":"book.xlsx","output":"book.xlsx","overwrite":true,"sheet":"数据","startCell":shaped["nextStartCell"],"data":"beta,2\ngamma,3"}),
            r,
            "allow",
        )
        .unwrap();
        let meta2 = &second["details"]["foxManagedFile"];
        assert_eq!(meta2["changeKind"], "modified");
        assert!(meta2["beforeHash"].is_string());
        assert!(meta2["backupPath"].is_string(), "overwrite keeps a pre-write backup");
        let shaped2: Value = serde_json::from_str(second["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(shaped2["nextStartCell"], "A5");

        // 3) Replaying the SAME batch at the SAME start cell overwrites the
        //    same cells: no duplicated rows after a retry.
        execute(
            &server,
            "office_import_data",
            &json!({"file":"book.xlsx","output":"book.xlsx","overwrite":true,"sheet":"数据","startCell":"A3","data":"beta,2\ngamma,3"}),
            r,
            "allow",
        )
        .unwrap();
        let read = execute(
            &server,
            "office_read",
            &json!({"file":"book.xlsx","mode":"get","selector":"/数据/A1:B6"}),
            r,
            "read_only",
        )
        .unwrap();
        let read_text = read["content"][0]["text"].as_str().unwrap();
        assert_eq!(read_text.matches("beta").count(), 1, "{read_text}");
        assert_eq!(read_text.matches("gamma").count(), 1, "{read_text}");

        // 4) A missing sheet without createSheet fails BEFORE writing and
        //    leaves no output and no version behind.
        let error = execute(
            &server,
            "office_import_data",
            &json!({"file":"book.xlsx","output":"late.xlsx","sheet":"不存在","data":"x\n1"}),
            r,
            "allow",
        )
        .unwrap_err();
        assert!(error.contains("不存在"), "{error}");
        assert!(!root.join("late.xlsx").exists());

        // 5) Cancellation stops the write: no output is committed.
        use crate::kernel::CancellationPort;
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("import-cancel").unwrap();
        registry.request_run_cancel("import-cancel");
        let token = registry.run_token("import-cancel").unwrap();
        let error = execute_with_cancellation(
            &server,
            "office_import_data",
            &json!({"output":"cancelled.xlsx","sheet":"S","createSheet":true,"data":"a\n1"}),
            r,
            "allow",
            Some(&token),
            Duration::from_secs(60),
            None,
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert!(!root.join("cancelled.xlsx").exists());

        // 6) Formula screening rejects external references before any write.
        assert!(execute(
            &server,
            "office_import_data",
            &json!({"output":"bad.xlsx","sheet":"S","createSheet":true,"data":"v\n=WEBSERVICE(\"https://x\")"}),
            r,
            "allow",
        )
        .is_err());
        assert!(!root.join("bad.xlsx").exists());
        println!("import-data integration artifacts: {}", root.display());
    }

    /// #15 evidence: per-cell `office_edit` batches versus one
    /// `office_import_data` block for the same 100x20 dataset. Measures the
    /// JSON the model must emit, the tool calls and the wall time of the real
    /// pinned-CLI production path, then prints a machine-readable summary.
    #[test]
    #[ignore = "benchmark against the pinned OfficeCLI; run explicitly"]
    fn benchmark_import_data_against_per_cell_edits() {
        let root = project();
        let db = Database::open(root.join("bench.db")).unwrap();
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
        let r = root.to_str();
        const ROWS: usize = 100;
        const COLS: usize = 20;

        // Old path: ceil(2000/100) office_edit calls of per-cell set ops.
        let mut old_bytes = 0usize;
        let mut old_calls = 0usize;
        let old_start = Instant::now();
        execute(
            &server,
            "office_create",
            &json!({"output":"old.xlsx"}),
            r,
            "allow",
        )
        .unwrap();
        let all_ops: Vec<Value> = (0..ROWS)
            .flat_map(|row| {
                (0..COLS).map(move |col| {
                    let cell = format!("{}{}", column_name(col as u32 + 1), row + 1);
                    json!({"command":"set","path":format!("/Sheet1/{cell}"),"props":{"value":format!("v{row}-{col}")}})
                })
            })
            .collect();
        for chunk in all_ops.chunks(100) {
            let input = json!({"file":"old.xlsx","output":"old.xlsx","overwrite":true,"operations":chunk});
            old_bytes += input.to_string().len();
            old_calls += 1;
            execute(&server, "office_edit", &input, r, "allow").unwrap();
        }
        let old_ms = old_start.elapsed().as_millis() as u64;

        // New path: one office_import_data call with the same values as CSV.
        let mut csv = String::new();
        for row in 0..ROWS {
            for col in 0..COLS {
                if col > 0 {
                    csv.push(',');
                }
                csv.push_str(&format!("v{row}-{col}"));
            }
            csv.push('\n');
        }
        let new_input = json!({"output":"new.xlsx","sheet":"Sheet1","data":csv});
        let new_bytes = new_input.to_string().len();
        let new_start = Instant::now();
        execute(&server, "office_import_data", &new_input, r, "allow").unwrap();
        let new_ms = new_start.elapsed().as_millis() as u64;

        // Both paths produced the same cell content.
        let probe = |file: &str| {
            execute(
                &server,
                "office_read",
                &json!({"file":file,"mode":"get","selector":"/Sheet1/A1:T100"}),
                r,
                "read_only",
            )
            .unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .matches("\\\"text\\\": \\\"v")
                .count()
        };
        assert_eq!(probe("old.xlsx"), ROWS * COLS);
        assert_eq!(probe("new.xlsx"), ROWS * COLS);

        let summary = json!({
            "dataset": {"rows": ROWS, "cols": COLS, "cells": ROWS * COLS},
            "perCellEdit": {"toolCalls": old_calls, "modelJsonBytes": old_bytes, "wallMs": old_ms},
            "importData": {"toolCalls": 1, "modelJsonBytes": new_bytes, "wallMs": new_ms},
            "ratios": {
                "jsonBytes": (old_bytes as f64) / (new_bytes as f64),
                "toolCalls": old_calls as f64,
                "wallMs": (old_ms as f64) / (new_ms.max(1) as f64),
            },
        });
        println!("IMPORT_BENCHMARK {summary}");
        assert!(old_bytes > new_bytes * 5, "import must be far more compact");
        println!("benchmark artifacts: {}", root.display());
    }

    /// #15 regression: cancelling while the pinned CLI is mid-write must stop
    /// the owned process before anything is committed — no output file, no
    /// managed version, and no leftover temporary document.
    ///
    /// Determinism: the test acts on the post-spawn seam (the import invocation
    /// has provably started) instead of sleeping and hoping.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_import_cancelled_mid_write_leaves_no_output_and_no_version() {
        use crate::kernel::CancellationPort;
        let root = project();
        let db = Database::open(root.join("cancel.db")).unwrap();
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
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("office-cancel").unwrap();
        let token = registry.run_token("office-cancel").unwrap();

        // A payload big enough that the import invocation does real work.
        let mut csv = String::from("id,name,value\n");
        for index in 0..60_000 {
            csv.push_str(&format!("{index},item-{index},{}\n", index % 100));
        }

        // Invocation 1 is `create` (a source-less import creates the workbook),
        // 2 is the sheet inventory, 3 is the import itself: cancel there.
        let cancel_registry = registry.clone();
        test_hooks::reset_spawn_count();
        test_hooks::set_post_spawn(Some(Box::new(move |count| {
            if count == 3 {
                cancel_registry.request_run_cancel("office-cancel");
            }
        })));
        let result = execute_with_cancellation(
            &server,
            "office_import_data",
            &json!({"output":"cancelled.xlsx","sheet":"数据","createSheet":true,"data":csv}),
            root.to_str(),
            "allow",
            Some(&token),
            Duration::from_secs(120),
            None,
        );
        test_hooks::set_post_spawn(None);
        let error = result.expect_err("a cancelled write must not report success");
        assert!(
            error.contains("cancelled") || error.contains("stopped") || error.contains("取消"),
            "{error}"
        );
        assert!(
            !root.join("cancelled.xlsx").exists(),
            "a cancelled import must not publish an output file"
        );
        // No temporary document may survive the cancellation.
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".fox-office-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temporaries: {leftovers:?}");
        println!("cancelled-mid-write artifacts: {}", root.display());
    }

    /// #15 regression: a completed import is registered as a restorable version
    /// and selecting it restores the exact bytes.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_import_registers_a_restorable_version() {
        let root = project();
        let db = Database::open(root.join("version.db")).unwrap();
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
        let created = execute(
            &server,
            "office_import_data",
            &json!({"output":"versioned.xlsx","sheet":"数据","createSheet":true,"data":"name,qty\nalpha,1"}),
            root.to_str(),
            "allow",
        )
        .unwrap();
        let meta = &created["details"]["foxManagedFile"];
        assert_eq!(meta["tool"], "office_import_data");
        let after_hash = meta["afterHash"].as_str().unwrap().to_owned();
        // A second batch changes the file and keeps the previous bytes in the
        // write-ahead backup, which the Host turns into a restorable version.
        let second = execute(
            &server,
            "office_import_data",
            &json!({"file":"versioned.xlsx","output":"versioned.xlsx","overwrite":true,"sheet":"数据","startCell":"A3","data":"beta,2"}),
            root.to_str(),
            "allow",
        )
        .unwrap();
        let meta2 = &second["details"]["foxManagedFile"];
        assert_eq!(meta2["changeKind"], "modified");
        let first_version_hash = meta2["beforeHash"].as_str().unwrap();
        assert_eq!(first_version_hash, after_hash, "the first version's bytes are recorded");
        let backup = meta2["backupPath"].as_str().unwrap();
        let restored = fs::read(backup).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&restored)), after_hash);
        println!("version-restore artifacts: {}", root.display());
    }

    /// The artifact flow end to end: compute saves data, Host resolves the
    /// saved artifact, the pinned CLI imports it, and the resulting worksheet
    /// carries the same rows/columns/values as the saved file. The full payload
    /// never enters the model request.
    ///
    /// Imports into the workbook's existing default sheet: the pinned CLI's
    /// standalone `batch` add-sheet primitive is the one verb whose resident
    /// state directory is unwritable under the DSH process sandbox. The
    /// artifact-reference path this verifies does not depend on it, and the
    /// same sheet-creation primitive is exercised by real users outside the
    /// sandbox.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_import_by_artifact_id_matches_the_saved_data() {
        let root = project();
        let db = Database::open(root.join("artifact.db")).unwrap();
        setup(&db, &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources")).unwrap();
        let server = db
            .list_mcp_servers()
            .unwrap()
            .into_iter()
            .find(|s| s.id == SERVER_ID)
            .unwrap();
        let r = root.to_str();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;

        // Save a large CSV at the AGV scale (~100 KB / 927 rows) with
        // data_compute's own output workspace + an artifact record. It exceeds
        // the inline result limit, yet flows into Excel purely by reference.
        let compute_root = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions,
            &conversation,
            "run-artifact-import",
        )
        .unwrap();
        let outputs = compute_root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        const ROW_COUNT: usize = 927;
        let mut csv = String::from("id,tag,v1,v2,v3,v4,v5,v6,v7,v8,v9,v10\n");
        for index in 0..ROW_COUNT {
            csv.push_str(&format!("{index},agv-task-{index:05}-zone-{:02}", index % 24));
            for _ in 0..10 {
                csv.push_str(&format!(",{}", 10_000 + index));
            }
            csv.push('\n');
        }
        assert!(csv.len() > 64 * 1024, "fixture {} bytes must exceed the inline result limit", csv.len());
        let row_count = ROW_COUNT;
        let artifact_path = outputs.join("saved.csv");
        fs::write(&artifact_path, csv.as_bytes()).unwrap();
        assert!(csv.len() > 64 * 1024, "the fixture must exceed the inline result limit");
        let artifact_path_str = artifact_path.to_string_lossy().into_owned();
        let artifact_id = Database::computed_artifact_id(&artifact_path_str);
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,NULL,'saved.csv','created_file',?3,'text/csv',?4,?5,'ready',?6,?6)",
                rusqlite::params![
                    &artifact_id,
                    &conversation,
                    &artifact_path_str,
                    csv.len() as i64,
                    hex::encode(Sha256::digest(csv.as_bytes())),
                    crate::database::now_ms(),
                ],
            )
        })
        .unwrap();

        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: None,
        };
        // A source-less import into the workbook's existing default sheet: no
        // create-sheet batch, so the Host goes create -> inventory -> import.
        let created = execute_with_cancellation(
            &server,
            "office_import_data",
            &json!({"output":"artifact.xlsx","sheet":"Sheet1","createSheet":false,"artifactId":&artifact_id}),
            r,
            "allow",
            None,
            Duration::from_secs(120),
            Some(&context),
        )
        .unwrap();
        let text = created["content"][0]["text"].as_str().unwrap();
        let shaped: Value = serde_json::from_str(text).unwrap();
        assert_eq!(shaped["import"]["rows"], row_count + 1, "{text}");
        assert_eq!(shaped["import"]["cols"], 12, "{text}");
        assert_eq!(shaped["import"]["format"], "csv");

        // Values must match the saved bytes exactly, not just exist.
        let read = execute(
            &server,
            "office_read",
            &json!({"file":"artifact.xlsx","mode":"get","selector":"/Sheet1/A1:L3"}),
            r,
            "read_only",
        )
        .unwrap();
        let outer: Value = serde_json::from_str(read["content"][0]["text"].as_str().unwrap()).unwrap();
        let inner: Value = serde_json::from_str(outer["result"].as_str().unwrap()).unwrap();
        let cells = inner["data"]["results"][0]["children"].as_array().unwrap();
        let cell = |path: &str| cells.iter().find(|c| c["path"] == path).cloned().unwrap();
        assert_eq!(cell("/Sheet1/A1")["text"], "id");
        assert_eq!(cell("/Sheet1/B1")["text"], "tag");
        assert_eq!(cell("/Sheet1/L1")["text"], "v10");
        assert_eq!(cell("/Sheet1/A2")["format"]["type"], "Number");
        assert_eq!(cell("/Sheet1/B2")["text"], "agv-task-00000-zone-00");
        assert_eq!(cell("/Sheet1/C2")["text"], "10000");
        assert_eq!(cell("/Sheet1/L2")["text"], "10000");
        assert_eq!(cell("/Sheet1/C3")["text"], "10001");
        // Last data row also matches, proving the whole file was imported.
        let last_row = row_count + 1;
        let tail = execute(
            &server,
            "office_read",
            &json!({"file":"artifact.xlsx","mode":"get","selector":format!("/Sheet1/A{last_row}:L{last_row}")}),
            r,
            "read_only",
        )
        .unwrap();
        let outer: Value = serde_json::from_str(tail["content"][0]["text"].as_str().unwrap()).unwrap();
        let inner: Value = serde_json::from_str(outer["result"].as_str().unwrap()).unwrap();
        let tail_cells = inner["data"]["results"][0]["children"].as_array().unwrap();
        let tail_cell = |path: &str| tail_cells.iter().find(|c| c["path"] == path).cloned().unwrap();
        let last_index = row_count - 1;
        assert_eq!(tail_cell(&format!("/Sheet1/A{last_row}"))["text"], last_index.to_string());
        assert_eq!(
            tail_cell(&format!("/Sheet1/B{last_row}"))["text"],
            format!("agv-task-{last_index:05}-zone-{:02}", last_index % 24)
        );
        assert_eq!(
            tail_cell(&format!("/Sheet1/L{last_row}"))["text"],
            (10_000 + last_index).to_string()
        );
        // Managed-file registration still happens for an artifact import.
        let meta = &created["details"]["foxManagedFile"];
        assert_eq!(meta["tool"], "office_import_data");
        assert_eq!(meta["changeKind"], "created");
        assert_eq!(meta["afterSize"], fs::metadata(root.join("artifact.xlsx")).unwrap().len() as i64);
        println!("artifact-import artifacts: {}", root.display());
    }

    /// An artifact reference must be refused before any write when it is not
    /// registered for this conversation, belongs to another conversation, has
    /// been tampered with, names a foreign format, or is supplied alongside an
    /// inline payload. No partial output may appear.
    #[test]
    fn artifact_reference_is_rejected_for_invalid_inputs_without_writing() {
        let root = project();
        let db = Database::open(root.join("reject.db")).unwrap();
        // Real conversation rows: the artifact registry keys on conversation,
        // so a fabricated id would be refused for the wrong reason.
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let other_conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let compute_root = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions,
            &conversation,
            "run-reject",
        )
        .unwrap();
        let outputs = compute_root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("ok.csv"), b"name,qty\nalpha,1\n").unwrap();
        fs::write(outputs.join("notes.txt"), b"not csv").unwrap();
        let ok_csv = outputs.join("ok.csv").to_string_lossy().into_owned();
        let ok_bytes = b"name,qty\nalpha,1\n";
        let ok_digest = hex::encode(Sha256::digest(ok_bytes));
        let ok_id = Database::computed_artifact_id(&ok_csv);
        let seed = |connection: &mut rusqlite::Connection,
                    id: &str,
                    conversation: &str,
                    path: &str,
                    media: &str,
                    bytes: i64,
                    digest: &str|
         -> rusqlite::Result<()> {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,NULL,?3,'created_file',?4,?5,?6,?7,'ready',?8,?8)",
                rusqlite::params![
                    id,
                    conversation,
                    std::path::Path::new(path).file_name().unwrap().to_string_lossy(),
                    path,
                    media,
                    bytes,
                    digest,
                    crate::database::now_ms(),
                ],
            )
            .map(|_| ())
        };
        db.with_connection(|connection| {
            seed(connection, &ok_id, &conversation, &ok_csv, "text/csv", ok_bytes.len() as i64, &ok_digest)
        })
        .unwrap();
        let txt_bytes = b"not csv";
        let txt_id = Database::computed_artifact_id(&outputs.join("notes.txt").to_string_lossy());
        db.with_connection(|connection| {
            seed(
                connection,
                &txt_id,
                &conversation,
                &outputs.join("notes.txt").to_string_lossy(),
                "text/plain",
                txt_bytes.len() as i64,
                &hex::encode(Sha256::digest(txt_bytes)),
            )
        })
        .unwrap();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: None,
        };
        let import = |artifact: Value, ctx: Option<&OfficeCallContext<'_>>| {
            prepare_with_context(
                "office_import_data",
                &json!({"output":"rejected.xlsx","sheet":"数据","createSheet":true,"artifactId": artifact}),
                root.to_str(),
                "allow",
                ctx,
            )
        };
        let ok = json!(ok_id);

        // No context at all: the reference cannot be resolved on this path.
        assert!(import(ok.clone(), None).is_err());
        // data + artifactId together are mutually exclusive.
        assert!(prepare_with_context(
            "office_import_data",
            &json!({"output":"rejected.xlsx","sheet":"数据","createSheet":true,"artifactId": ok.clone(),"data":"a,b\n1,2"}),
            root.to_str(),
            "allow",
            Some(&context),
        )
        .is_err());
        // Neither payload supplied.
        assert!(prepare_with_context(
            "office_import_data",
            &json!({"output":"rejected.xlsx","sheet":"数据","createSheet":true}),
            root.to_str(),
            "allow",
            Some(&context),
        )
        .is_err());
        // Unknown id (never registered).
        let error = import(json!("compute-artifact:0000000000000000000000000000000000000000000000000000000000000000"), Some(&context))
            .unwrap_err();
        assert!(error.contains("不存在"), "{error}");
        // Registered for a different conversation: the frozen binding's
        // conversation is the only one consulted. Seed a distinct artifact
        // under another conversation; this binding must not resolve it.
        let foreign_path = outputs.join("foreign.csv");
        fs::write(&foreign_path, ok_bytes).unwrap();
        let foreign_csv = foreign_path.to_string_lossy().into_owned();
        let foreign_id = Database::computed_artifact_id(&foreign_csv);
        db.with_connection(|connection| {
            seed(
                connection,
                &foreign_id,
                &other_conversation,
                &foreign_csv,
                "text/csv",
                ok_bytes.len() as i64,
                &ok_digest,
            )
        })
        .unwrap();
        let error = import(json!(foreign_id), Some(&context)).unwrap_err();
        assert!(error.contains("不存在"), "a cross-conversation reference must not resolve: {error}");
        // Wrong extension vs the declared format.
        let error = import(json!(txt_id), Some(&context)).unwrap_err();
        assert!(error.contains("不一致") && error.contains("csv"), "{error}");
        // Tampered content: recorded checksum no longer matches the bytes.
        fs::write(outputs.join("ok.csv"), b"name,qty\nalpha,99\n").unwrap();
        let error = import(ok, Some(&context)).unwrap_err();
        assert!(error.contains("校验失败"), "{error}");
        // No output was ever created for any refused case.
        assert!(!root.join("rejected.xlsx").exists());
        let _ = fs::remove_dir(root);
    }

    /// The whole point of the artifact flow: the bytes the Host streams to the
    /// CLI over stdin are exactly the saved compute artifact — resolved from
    /// the frozen binding, integrity-checked — not a model re-copy. Runs with
    /// no Office binary, so it verifies value provenance deterministically up
    /// to the process boundary (the pinned CLI's own import is covered by the
    /// ignored real-binary test).
    #[test]
    fn artifact_import_streams_exact_saved_bytes_and_grid() {
        let root = project();
        let db = Database::open(root.join("stream.db")).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let outputs = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions,
            &conversation,
            "run-stream",
        )
        .unwrap()
        .join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        // Multi-column CSV whose cell values we can assert individually.
        let csv = "id,tag,qty\n10,alpha,100\n20,beta,200\n";
        let artifact_path = outputs.join("result.csv");
        fs::write(&artifact_path, csv.as_bytes()).unwrap();
        let path_str = artifact_path.to_string_lossy().into_owned();
        let artifact_id = Database::computed_artifact_id(&path_str);
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,NULL,'result.csv','created_file',?3,'text/csv',?4,?5,'ready',?6,?6)",
                rusqlite::params![
                    &artifact_id,
                    &conversation,
                    &path_str,
                    csv.len() as i64,
                    hex::encode(Sha256::digest(csv.as_bytes())),
                    crate::database::now_ms(),
                ],
            )
        })
        .unwrap();
        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: None,
        };
        let action = prepare_with_context(
            "office_import_data",
            &json!({
                "output":"report.xlsx","sheet":"Sheet1","createSheet":false,
                "artifactId": artifact_id,
            }),
            root.to_str(),
            "allow",
            Some(&context),
        )
        .unwrap();
        // The CLI receives the saved bytes verbatim via stdin — never a path,
        // never a CLI argument, and never a model-typed copy.
        assert_eq!(action.stdin_data.as_deref().unwrap(), csv);
        assert!(action.args.iter().all(|a| !a.contains("result.csv")), "the artifact path must not leak onto the command line");
        let spec = action.import.as_ref().unwrap();
        assert_eq!(spec.sheet, "Sheet1");
        assert_eq!((spec.rows, spec.cols), (3, 3));
        assert_eq!(spec.format, "csv");
        assert!(action.mutates);
        // Resolution refuses an id from a *different* conversation even when
        // its bytes are identical and present on disk.
        let other = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let other_context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &other,
            artifacts_dir: None,
        };
        let error = prepare_with_context(
            "office_import_data",
            &json!({"output":"other.xlsx","sheet":"Sheet1","artifactId": artifact_id}),
            root.to_str(),
            "allow",
            Some(&other_context),
        )
        .unwrap_err();
        assert!(error.contains("不存在"), "{error}");
        assert!(!root.join("report.xlsx").exists());
        let _ = fs::remove_dir(root);
    }
}

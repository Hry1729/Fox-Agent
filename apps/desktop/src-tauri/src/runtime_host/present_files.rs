//! Explicit file delivery: the model declares which existing files the user is
//! actually being handed, and the Host records that declaration.
//!
//! The Host resolves paths, reads supported Office files back from disk, and
//! verifies shared labeled numbers when the task requests a matching XLSX/DOCX pair.

use serde_json::{json, Value};
use std::path::Path;

/// Maximum files one call may declare. The model is told to prefer the one or
/// two most important files; this is the enforced ceiling above that advice.
pub const MAX_FILES_PER_CALL: usize = 8;

/// Longest accepted per-file description, in bytes. The description is display
/// copy shown under the file name, so it is bounded to keep a card readable and
/// to keep one call's result from growing without limit.
const MAX_DESCRIPTION_BYTES: usize = 200;

/// Longest accepted path, in bytes. Paths are a fixed Host concept, not a place
/// for model prose; this only rejects a value no real path could be.
const MAX_PATH_BYTES: usize = 4096;

/// One file the model declared, after the Host resolved and checked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredFile {
    /// Canonical absolute path of the file that will be recorded.
    pub resolved_path: std::path::PathBuf,
    /// The path exactly as the model wrote it, echoed back in the tool result so
    /// the model sees which of its own strings became which file.
    pub requested_path: String,
    /// The file name the user will see on the card.
    pub display_name: String,
    /// Optional one-line description supplied by the model.
    pub description: Option<String>,
}

/// Why a declaration was refused.
///
/// Every variant maps to a tool error the model can act on. None of them
/// produces a success card, because a card that claims a file was handed over
/// when it was not is worse than an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentError {
    /// The call itself is malformed: no files, too many, or a non-object entry.
    InvalidInput(String),
    /// The path does not resolve inside the authorized project root, or the
    /// Host has no authorized root to resolve it against.
    NotAuthorized(String),
    /// The path resolved but there is no accessible regular file there.
    NotFound(String),
    /// The target exists but is not a regular file (a directory, a device, a
    /// symlink that resolves outside the root).
    NotAFile(String),
}

impl PresentError {
    /// The tool error code carried back to the model and stored on the call.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "tool.invalid_input",
            Self::NotAuthorized(_) => "tool.permission_denied",
            Self::NotFound(_) | Self::NotAFile(_) => "tool.invalid_input",
        }
    }

    /// The message the model reads. It names the offending path and says what
    /// to do next, because the model can usually fix this and retry.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidInput(message)
            | Self::NotAuthorized(message)
            | Self::NotFound(message)
            | Self::NotAFile(message) => message,
        }
    }
}

/// Parse and check one call's arguments without touching the database.
///
/// The whole batch is checked before anything is recorded, so a call either
/// declares every file it named or none of them. Recording a prefix would leave
/// the user with a card for a file that the same call also reported as broken.
///
/// `project_root` is the Host's authorized root for this Run. `None` means the
/// Run has no authorized project, in which case there is nothing the Host can
/// vouch for and every path is refused rather than resolved against an
/// unspecified base.
pub fn resolve_declared_files(
    input: &Value,
    project_root: Option<&str>,
) -> Result<Vec<DeclaredFile>, PresentError> {
    let files = input
        .get("files")
        .ok_or_else(|| PresentError::InvalidInput("present_files requires a files array".into()))?
        .as_array()
        .ok_or_else(|| PresentError::InvalidInput("files must be an array".into()))?;
    if files.is_empty() {
        return Err(PresentError::InvalidInput(
            "present_files requires at least one file".into(),
        ));
    }
    if files.len() > MAX_FILES_PER_CALL {
        return Err(PresentError::InvalidInput(format!(
            "present_files accepts at most {MAX_FILES_PER_CALL} files, got {}",
            files.len()
        )));
    }
    let root = project_root.map(std::path::Path::new).ok_or_else(|| {
        PresentError::NotAuthorized(
            "present_files needs an authorized project folder; this task has none".into(),
        )
    })?;
    let root =
        crate::tool_host::canonical_project_root(root).map_err(PresentError::NotAuthorized)?;

    let mut resolved = Vec::with_capacity(files.len());
    let mut seen = std::collections::BTreeSet::new();
    for (index, entry) in files.iter().enumerate() {
        let object = entry.as_object().ok_or_else(|| {
            PresentError::InvalidInput(format!("files[{index}] must be an object"))
        })?;
        let requested = object
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                PresentError::InvalidInput(format!(
                    "files[{index}].path must be a non-empty string"
                ))
            })?;
        if requested.len() > MAX_PATH_BYTES {
            return Err(PresentError::InvalidInput(format!(
                "files[{index}].path is longer than {MAX_PATH_BYTES} bytes"
            )));
        }
        let description = match object.get("description") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => {
                let text = text.trim();
                if text.is_empty() {
                    None
                } else if text.len() > MAX_DESCRIPTION_BYTES {
                    return Err(PresentError::InvalidInput(format!(
                        "files[{index}].description is longer than {MAX_DESCRIPTION_BYTES} bytes"
                    )));
                } else {
                    Some(text.to_owned())
                }
            }
            Some(_) => {
                return Err(PresentError::InvalidInput(format!(
                    "files[{index}].description must be a string"
                )))
            }
        };
        let target = crate::tool_host::resolve_present_target(&root, requested).map_err(
            |error| match error {
                crate::tool_host::PresentTargetError::NotAuthorized(message) => {
                    PresentError::NotAuthorized(message)
                }
                crate::tool_host::PresentTargetError::NotFound(message) => {
                    PresentError::NotFound(message)
                }
                crate::tool_host::PresentTargetError::NotAFile(message) => {
                    PresentError::NotAFile(message)
                }
            },
        )?;
        // Two spellings of one file are one declaration. Recording both would
        // put the same file on the card twice under two different strings.
        if !seen.insert(target.clone()) {
            continue;
        }
        let display_name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| requested.to_owned());
        resolved.push(DeclaredFile {
            resolved_path: target,
            requested_path: requested.to_owned(),
            display_name,
            description,
        });
    }
    if resolved.is_empty() {
        return Err(PresentError::InvalidInput(
            "present_files resolved no distinct files".into(),
        ));
    }
    Ok(resolved)
}

/// The model-facing tool result for a successful declaration.
///
/// It reports what was recorded and nothing about the content, so the model is
/// not led to believe a check happened.
pub fn success_result(files: &[DeclaredFile]) -> Value {
    json!({
        "declared": files.len(),
        "files": files
            .iter()
            .map(|file| json!({
                "path": file.requested_path,
                "name": file.display_name,
                "description": file.description,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Check the call, then record it.
///
/// The Run, conversation and turn identity come from the frozen Kernel binding
/// the caller already holds; none of them is read from tool input, so the model
/// cannot attach a delivery to another Run's reply.
pub fn execute(
    database: &crate::database::Database,
    conversation_id: &str,
    run_id: &str,
    turn_call_id: &str,
    project_root: &Option<String>,
    input: &Value,
) -> Result<Value, String> {
    if database.run_steering_messages(run_id)?.iter().any(|message|
        message.lane == "current" && matches!(message.status.as_str(), "received" | "delivered"))
    {
        return Err("[tool.invalid_input] 本轮还有待应用的追加要求，请按更新后的要求交付。".into());
    }
    let files = resolve_declared_files(input, project_root.as_deref()).map_err(|error| {
        // The code prefix is the same `[tool.…]` convention every other Host
        // tool uses, so the model sees a class it can branch on.
        format!("[{}] {}", error.code(), error.message())
    })?;
    let task = database.run_task_text(run_id)?.unwrap_or_default();
    let root = crate::tool_host::canonical_project_root(Path::new(project_root.as_deref().unwrap_or_default()))?;
    for file in &files {
        if let Ok(relative) = file.resolved_path.strip_prefix(&root) {
            super::delivery::check_csv_placement(&task, &root, relative)?;
        }
        if has_extension(&file.resolved_path, "csv") {
            verify_csv_width(&file.resolved_path)
                .map_err(|error| format!("[tool.invalid_input] CSV 交付校验失败，{}：{error}。请修复原文件后再次调用 present_files。", file.display_name))?;
        }
    }
    let office_checks = verify_office_deliveries(&root, &task, &files)
        .map_err(|error| format!("[tool.invalid_input] 交付校验失败：{error}。请修复原文件并再次调用 present_files。"))?;
    let rows: Vec<(String, String, Option<String>)> = files
        .iter()
        .map(|file| {
            (
                file.resolved_path.to_string_lossy().into_owned(),
                file.display_name.clone(),
                file.description.clone(),
            )
        })
        .collect();
    let ids = database
        .record_present_files_declaration(
            conversation_id,
            run_id,
            turn_call_id,
            &rows,
            crate::database::now_ms(),
        )
        .map_err(|error| format!("[tool.unknown] the delivery could not be recorded: {error}"))?;
    let mut result = success_result(&files);
    for (file, id) in result["files"].as_array_mut().unwrap().iter_mut().zip(ids) {
        file["artifactId"] = json!(id);
    }
    // The declaration is what the user sees. Naming that fact keeps the model
    // from promising a card for a file it never declared.
    result["recorded"] = json!(true);
    if !office_checks.opened.is_empty() {
        result["officeVerification"] = json!({
            "opened": office_checks.opened,
            "sharedNumbersMatch": office_checks.shared_numbers_match,
        });
    }
    Ok(result)
}

pub(super) fn finalization_candidates(database: &crate::database::Database, run_id: &str) -> Result<Vec<String>, String> {
    let binding = database.run_control_binding(run_id)?.ok_or("missing delivery Run binding")?;
    let Some(root) = binding.permission.project_root.as_deref() else { return Ok(Vec::new()) };
    let latest = database.run_steering_messages(run_id)?.into_iter().rev()
        .find(|message| message.lane == "current" && message.status == "applied");
    let task = database.run_task_text(run_id)?.unwrap_or_default();
    let task_context = json!({"projectRoot": root});
    let has_delivery = |text: &str| super::delivery::task_delivery_reference(text, &task_context)["recognition"] == "recognized";
    if !has_delivery(&task) && !latest.as_ref().is_some_and(|message| has_delivery(&message.content)) {
        return Ok(Vec::new());
    }
    let since = latest.and_then(|message| message.applied_at).unwrap_or_default();
    let paths = database.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT storage_path, declared_at FROM artifacts WHERE run_id=?1
             AND status='ready' AND artifact_origin='project' AND artifact_class IN ('process','deliverable')
             AND updated_at>=?2 ORDER BY updated_at DESC, id LIMIT 16",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id, since],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    let root = crate::tool_host::canonical_project_root(Path::new(root))?;
    let mut candidates = Vec::new();
    for (path, declared) in paths {
        let Ok(target) = crate::tool_host::resolve_present_target(&root, &path) else { continue };
        if declared.is_some_and(|time| time >= since) { return Ok(Vec::new()); }
        if let Ok(relative) = target.strip_prefix(&root) {
            candidates.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    candidates.dedup();
    Ok(candidates)
}

fn verify_csv_width(path: &Path) -> Result<(), String> {
    use super::delivery_checks_v2::{text_csv::parse_table, CheckLimits, TableDialect};
    use std::io::Read;

    let limits = CheckLimits::P0;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(limits.max_bytes + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("无法读取文件：{error}"))?;
    let parsed = parse_table(&bytes, &TableDialect::default(), &limits)
        .map_err(|error| format!("{}，{}", error.locator().display(), error.detail()))?;
    if let Some((first, rows)) = parsed.records.split_first() {
        let expected = first.fields.len();
        if let Some(row) = rows.iter().find(|row| row.fields.len() != expected) {
            return Err(format!("第 {} 行应有 {expected} 列，实际 {} 列", row.line, row.fields.len()));
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct OfficeVerification {
    opened: Vec<String>,
    shared_numbers_match: bool,
}

fn verify_office_deliveries(root: &Path, task: &str, files: &[DeclaredFile]) -> Result<OfficeVerification, String> {
    use std::collections::BTreeSet;

    let lower = task.to_ascii_lowercase();
    let expects_match = ["数字应与表格一致", "数字与表格一致", "数字一致", "数据一致", "numbers match", "figures match", "same numbers"]
        .iter().any(|phrase| lower.contains(phrase));
    let explicit = super::delivery::expectations_from_task(task);
    let named: Vec<_> = explicit.iter().filter_map(|item| item.target_path.as_deref()).collect();
    let named_of_type = |ext: &str| named.iter().filter(move |file| has_extension(Path::new(file), ext)).copied().collect::<Vec<_>>();
    let (xlsx_targets, docx_targets) = (named_of_type("xlsx"), named_of_type("docx"));
    let mut candidates: BTreeSet<_> = files.iter().map(|file| file.resolved_path.clone()).collect();
    let pair = if expects_match && xlsx_targets.len() == 1 && docx_targets.len() == 1 {
        let resolve = |name: &str| crate::tool_host::resolve_present_target(root, name)
            .map_err(|_| format!("需要先保存并提供 {name}"));
        let pair = (resolve(xlsx_targets[0])?, resolve(docx_targets[0])?);
        candidates.extend([pair.0.clone(), pair.1.clone()]);
        Some(pair)
    } else if expects_match {
        let xlsx = candidates.iter().filter(|path| has_extension(path, "xlsx")).collect::<Vec<_>>();
        let docx = candidates.iter().filter(|path| has_extension(path, "docx")).collect::<Vec<_>>();
        (xlsx.len() == 1 && docx.len() == 1).then(|| (xlsx[0].clone(), docx[0].clone()))
    } else {
        None
    };
    let mut checks = OfficeVerification::default();
    let mut numbers = Vec::new();
    let mut report = String::new();
    for path in candidates {
        if has_extension(&path, "xlsx") {
            let parsed = workbook_numbers(&path)?;
            if pair.as_ref().is_some_and(|(workbook, _)| workbook == &path) {
                numbers = parsed;
            }
        } else if has_extension(&path, "docx") {
            let text = super::extract_docx_text(&read_office_package(&path)?)
                .map_err(|error| format!("{} 无法读取：{error}", path.display()))?;
            if pair.as_ref().is_some_and(|(_, word)| word == &path) {
                report = text;
            }
        } else {
            continue;
        }
        checks.opened.push(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
    }
    if let Some((xlsx, docx)) = pair {
        check_shared_numbers(&numbers, &report)
            .map_err(|error| format!("{} 与 {}：{error}", xlsx.display(), docx.display()))?;
        checks.shared_numbers_match = true;
    }
    Ok(checks)
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn read_office_package(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|error| format!("{} 无法打开：{error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|error| format!("{} 无法读取：{error}", path.display()))?;
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(format!("{} 超出交付校验的 64 MiB 文件限制", path.display()));
    }
    crate::data_compute::validate_zip_expansion(&path.to_string_lossy(), &bytes)?;
    Ok(bytes)
}

fn workbook_numbers(path: &Path) -> Result<Vec<(String, f64)>, String> {
    use calamine::{open_workbook_auto_from_rs, Data, Reader};
    let invalid = |error: calamine::Error| format!("{} 无法读取：{error}", path.display());
    let mut workbook = open_workbook_auto_from_rs(std::io::Cursor::new(read_office_package(path)?))
        .map_err(invalid)?;
    let mut values = Vec::new();
    for sheet in workbook.sheet_names().to_vec() {
        let cells = workbook.worksheet_range(&sheet).map_err(invalid)?;
        for row in cells.rows() {
            if let Some(label) = row.iter().find_map(|cell| match cell {
                Data::String(label) if !label.trim().is_empty() => Some(label.trim()),
                _ => None,
            }) {
                if let Some(value) = row.iter().find_map(cell_number) {
                    values.push((label.to_owned(), value));
                }
            }
        }
        if let (Some(headers), Some(row)) = (cells.rows().next(), cells.rows().last()) {
            for (header, cell) in headers.iter().zip(row) {
                if let (Data::String(label), Some(value)) = (header, cell_number(cell)) {
                    values.push((label.trim().to_owned(), value));
                }
            }
        }
    }
    Ok(values)
}

fn cell_number(cell: &calamine::Data) -> Option<f64> {
    match cell {
        calamine::Data::Int(value) => Some(*value as f64),
        calamine::Data::Float(value) => Some(*value),
        calamine::Data::String(value) => value.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn check_shared_numbers(numbers: &[(String, f64)], report: &str) -> Result<(), String> {
    let mut matched = false;
    for (label, expected) in numbers {
        if label.is_empty() || label.chars().count() > 64 || label.parse::<f64>().is_ok() {
            continue;
        }
        let aliases = match label.to_ascii_lowercase().as_str() {
            "收入" | "revenue" => r"收入|\brevenue\b".to_owned(),
            "成本" | "cost" => r"成本|\bcost\b".to_owned(),
            "利润" | "profit" => r"利润|\bprofit\b".to_owned(),
            _ => regex::escape(label),
        };
        let field = regex::Regex::new(&format!(
            r"(?i)(?:{aliases})(?:\s|[*：:=]|为|是|合计|总计|[￥¥$])*([+\-−]?[0-9][0-9,]*(?:\.[0-9]+)?)",
        )).map_err(|error| error.to_string())?;
        for captures in field.captures_iter(report) {
            let Ok(actual) = captures[1].replace(',', "").replace('−', "-").parse::<f64>() else { continue };
            matched = true;
            if (actual - expected).abs() > 1e-8 * expected.abs().max(1.0) {
                return Err(format!("{label}在 Excel 中为 {expected}，在 Word 中出现 {actual}"));
            }
        }
    }
    if !matched {
        return Err("未找到两份文件共有的带名称数字。请检查工作簿数据，并在报告中明确标注数字对应的项目".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "fox-present-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&root).expect("fixture root");
            Self { root }
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("fixture parent");
            }
            fs::write(&path, bytes).expect("fixture file");
            path
        }

        fn root_text(&self) -> String {
            self.root.to_string_lossy().into_owned()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn detects_ragged_csv_and_preserves_quoted_records() {
        let fixture = Fixture::new("csv-width");
        let path = fixture.write("summary.csv", b"\xef\xbb\xbfsource,amount,note\r\na,10,\"one,two\r\nthree\"\r\nTOTAL,,10,\r\n");
        assert_eq!(verify_csv_width(&path).unwrap_err(), "第 4 行应有 3 列，实际 4 列");
        fixture.write("summary.csv", b"source,amount,note\na,10,\"one,two\nthree\"\nTOTAL,10,\n");
        assert!(verify_csv_width(&path).is_ok());
    }

    #[test]
    fn refuses_a_truncated_workbook_before_recording_delivery() {
        let fixture = Fixture::new("truncated-workbook");
        fixture.write("output/summary.xlsx", b"PK\x03\x04truncated");
        let files = resolve_declared_files(
            &json!({"files":[{"path":"output/summary.xlsx"}]}),
            Some(&fixture.root_text()),
        ).unwrap();
        let root = crate::tool_host::canonical_project_root(&fixture.root).unwrap();
        let error = verify_office_deliveries(&root, "交付 output/summary.xlsx", &files).unwrap_err();
        assert!(error.contains("summary.xlsx"));
    }

    #[test]
    fn checks_every_occurrence_of_shared_numbers_in_word() {
        let numbers = [("revenue".to_owned(), 1000.0), ("利润".to_owned(), 400.0)];
        assert!(check_shared_numbers(&numbers, "收入：1,000 元。利润为400元。\n利润\n400").is_ok());
        let error = check_shared_numbers(&numbers, "收入：1,000 元。利润为400元。\n利润\n450").unwrap_err();
        assert!(error.contains("450"));
    }

    #[test]
    fn a_regular_file_is_declared_with_its_own_name_and_description() {
        let fixture = Fixture::new("regular");
        fixture.write("reports/summary.md", b"# Summary\n");
        let input = json!({"files":[{"path":"reports/summary.md","description":"汇总报告"}]});
        let files = resolve_declared_files(&input, Some(&fixture.root_text())).expect("declared");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_name, "summary.md");
        assert_eq!(files[0].description.as_deref(), Some("汇总报告"));
        assert_eq!(files[0].requested_path, "reports/summary.md");
    }

    /// A zero-byte file is a legal delivery. Content is not this tool's business,
    /// so an empty file must not be refused here or anywhere downstream.
    #[test]
    fn a_zero_byte_file_is_a_legal_delivery() {
        let fixture = Fixture::new("empty");
        fixture.write("empty.md", b"");
        let input = json!({"files":[{"path":"empty.md"}]});
        let files = resolve_declared_files(&input, Some(&fixture.root_text())).expect("declared");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_name, "empty.md");
        assert!(files[0].description.is_none());
    }

    #[test]
    fn a_missing_file_is_refused_and_names_the_path() {
        let fixture = Fixture::new("missing");
        let input = json!({"files":[{"path":"nope.md"}]});
        let error =
            resolve_declared_files(&input, Some(&fixture.root_text())).expect_err("refused");
        assert_eq!(error.code(), "tool.invalid_input");
        assert!(error.message().contains("nope.md"), "{}", error.message());
    }

    #[test]
    fn a_directory_is_refused() {
        let fixture = Fixture::new("dir");
        fs::create_dir_all(fixture.root.join("folder")).expect("folder");
        let input = json!({"files":[{"path":"folder"}]});
        let error =
            resolve_declared_files(&input, Some(&fixture.root_text())).expect_err("refused");
        assert!(
            matches!(error, PresentError::NotAFile(_)),
            "a directory must not be presented as a file: {error:?}"
        );
    }

    #[test]
    fn a_path_outside_the_authorized_root_is_refused() {
        let fixture = Fixture::new("escape");
        let input = json!({"files":[{"path":"../outside.md"}]});
        let error =
            resolve_declared_files(&input, Some(&fixture.root_text())).expect_err("refused");
        assert!(
            matches!(
                error,
                PresentError::NotAuthorized(_) | PresentError::NotFound(_)
            ),
            "an escaping path must not resolve: {error:?}"
        );
    }

    #[test]
    fn a_run_without_an_authorized_root_cannot_declare_anything() {
        let fixture = Fixture::new("noroot");
        fixture.write("a.md", b"a");
        let input = json!({"files":[{"path":"a.md"}]});
        let error = resolve_declared_files(&input, None).expect_err("refused");
        assert!(matches!(error, PresentError::NotAuthorized(_)), "{error:?}");
    }

    /// One bad entry fails the whole call. The alternative — recording the good
    /// ones — would leave a card for a file the same call also reported missing.
    #[test]
    fn one_bad_entry_rejects_the_whole_batch() {
        let fixture = Fixture::new("batch");
        fixture.write("good.md", b"good");
        let input = json!({"files":[{"path":"good.md"},{"path":"missing.md"}]});
        let error =
            resolve_declared_files(&input, Some(&fixture.root_text())).expect_err("refused");
        assert!(
            error.message().contains("missing.md"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn the_same_file_named_twice_is_declared_once() {
        let fixture = Fixture::new("dupe");
        fixture.write("same.md", b"same");
        let input = json!({"files":[{"path":"same.md"},{"path":"./same.md"}]});
        let files = resolve_declared_files(&input, Some(&fixture.root_text())).expect("declared");
        assert_eq!(files.len(), 1, "one file must not produce two cards");
    }

    #[test]
    fn an_empty_or_oversized_batch_is_refused() {
        let fixture = Fixture::new("bounds");
        fixture.write("a.md", b"a");
        let empty = json!({"files":[]});
        assert!(matches!(
            resolve_declared_files(&empty, Some(&fixture.root_text())),
            Err(PresentError::InvalidInput(_))
        ));
        let too_many = json!({"files": (0..=MAX_FILES_PER_CALL)
            .map(|index| json!({"path": format!("a{index}.md")}))
            .collect::<Vec<_>>()});
        assert!(matches!(
            resolve_declared_files(&too_many, Some(&fixture.root_text())),
            Err(PresentError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_blank_path_is_refused_rather_than_resolved_to_the_root() {
        let fixture = Fixture::new("blank");
        let input = json!({"files":[{"path":"   "}]});
        let error =
            resolve_declared_files(&input, Some(&fixture.root_text())).expect_err("refused");
        assert!(matches!(error, PresentError::InvalidInput(_)), "{error:?}");
    }

    /// A real database, because the point of the declaration is that it survives
    /// a restart: a mock would prove nothing about that.
    struct Stored {
        database: crate::database::Database,
        path: std::path::PathBuf,
        root: Fixture,
    }

    impl Stored {
        fn new(tag: &str) -> Self {
            let root = Fixture::new(tag);
            let path = std::env::temp_dir().join(format!(
                "fox-present-db-{tag}-{}-{}.db",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let database = crate::database::Database::open(path.clone()).expect("open database");
            database
                .with_connection(|connection| {
                    // A conversation carries a NOT NULL agent reference, so the
                    // fixture seats one agent before the conversation it anchors.
                    connection.execute(
                        "INSERT INTO agents(id,name,description,runtime_type,system_prompt,
                                            default_model,created_at,updated_at)
                         VALUES ('agent-1','a','d','native','p','m',1,1)",
                        [],
                    )?;
                    connection.execute(
                        "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                         VALUES ('conv-1','agent-1','t','active',1,1)",
                        [],
                    )?;
                    // An artifact row references the Run that declared it, so the
                    // fixture seats that Run too rather than relying on a nullable
                    // reference the real writer never leaves empty.
                    connection.execute(
                        "INSERT INTO runs(id,conversation_id,status,model,created_at)
                         VALUES ('run-1','conv-1','running','m',1)",
                        [],
                    )?;
                    Ok(())
                })
                .expect("seed conversation");
            Self {
                database,
                path,
                root,
            }
        }

        fn declare(&self, turn: &str, paths: &[&str]) -> Result<Value, String> {
            let files = paths
                .iter()
                .map(|path| json!({"path": path}))
                .collect::<Vec<_>>();
            execute(
                &self.database,
                "conv-1",
                "run-1",
                turn,
                &Some(self.root.root_text()),
                &json!({"files": files}),
            )
        }

        /// Read the declarations back as durable rows. Deliberately a direct
        /// query: this asserts what is *stored*, not what some accessor chooses
        /// to expose, so a serializer change cannot hide a persistence bug.
        fn declared(
            &self,
        ) -> Vec<(
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
            i64,
        )> {
            self.database
                .with_connection(|connection| {
                    let mut statement = connection.prepare(
                        "SELECT display_name, artifact_class, turn_call_id, declared_at,
                                declared_description, byte_size
                           FROM artifacts WHERE run_id='run-1' AND declared_at IS NOT NULL
                          ORDER BY declared_at ASC, rowid ASC",
                    )?;
                    let rows = statement
                        .query_map([], |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows)
                })
                .expect("read declarations")
        }
    }

    impl Drop for Stored {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_declaration_is_persisted_as_a_deliverable_bound_to_its_call() {
        let stored = Stored::new("persist");
        stored.root.write("report.md", b"# R\n");
        let result = stored.declare("call-1", &["report.md"]).expect("declared");
        assert_eq!(result["declared"], 1);

        let rows = stored.declared();
        assert_eq!(rows.len(), 1, "one file must yield one card");
        let (name, class, turn, declared_at, _, _) = &rows[0];
        assert_eq!(name, "report.md");
        // The Host, not the model, decides this file is a deliverable.
        assert_eq!(class, "deliverable");
        // The turn binding is what lets the card follow the reply it belongs to.
        assert_eq!(turn.as_deref(), Some("call-1"));
        assert!(declared_at.is_some());
    }

    /// A retried call must not produce a second card for the same file.
    #[test]
    fn a_retried_declaration_updates_its_own_row_instead_of_adding_one() {
        let stored = Stored::new("retry");
        stored.root.write("report.md", b"# R\n");
        stored.declare("call-1", &["report.md"]).expect("first");
        stored.declare("call-1", &["report.md"]).expect("retry");
        assert_eq!(
            stored.declared().len(),
            1,
            "a retry must not duplicate the card"
        );
    }

    #[test]
    fn a_declaration_records_the_model_note_and_the_real_size() {
        let stored = Stored::new("note");
        stored.root.write("summary.md", "# 汇总\n".as_bytes());
        let files = json!({"files":[{"path":"summary.md","description":"本季度汇总"}]});
        execute(
            &stored.database,
            "conv-1",
            "run-1",
            "call-9",
            &Some(stored.root.root_text()),
            &files,
        )
        .expect("declared");
        let rows = stored.declared();
        assert_eq!(rows.len(), 1);
        let (_, _, _, _, description, size) = &rows[0];
        assert_eq!(description.as_deref(), Some("本季度汇总"));
        assert_eq!(*size, "# 汇总\n".len() as i64);
    }

    /// The whole point of a zero-byte file being legal: it must reach the same
    /// declaration path as any other file, not be filtered out on the way.
    #[test]
    fn a_zero_byte_file_is_declared_and_kept() {
        let stored = Stored::new("empty");
        stored.root.write("empty.md", b"");
        stored.declare("call-1", &["empty.md"]).expect("declared");
        let rows = stored.declared();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].5, 0, "a zero-byte file keeps its real size");
    }

    /// A refused call must leave the database untouched, so the reply shows no
    /// card for a file the same call reported as missing.
    #[test]
    fn a_refused_call_writes_nothing() {
        let stored = Stored::new("refused");
        stored.root.write("good.md", b"g");
        let error = stored
            .declare("call-1", &["good.md", "missing.md"])
            .expect_err("refused");
        assert!(error.contains("missing.md"), "{error}");
        assert!(
            stored.declared().is_empty(),
            "a refused call must write nothing"
        );
    }

    /// Restart persistence: the declaration is read back from a freshly opened
    /// database, which is what a restart does.
    #[test]
    fn a_declaration_survives_reopening_the_database() {
        let stored = Stored::new("restart");
        stored.root.write("report.md", b"# R\n");
        stored.declare("call-1", &["report.md"]).expect("declared");
        let path = stored.path.clone();
        let root = stored.root.root_text();
        drop(stored);

        let reopened = crate::database::Database::open(path.clone()).expect("reopen");
        // Read the persisted row straight back from the reopened database: this
        // asserts the restart story against stored bytes rather than an accessor.
        let rows = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT display_name, turn_call_id, storage_path
                       FROM artifacts WHERE conversation_id='conv-1' AND declared_at IS NOT NULL",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
            })
            .expect("the declaration must still be there after reopen");
        let (display_name, turn_call_id, storage_path) = rows;
        assert_eq!(
            display_name, "report.md",
            "the declaration must survive a restart"
        );
        assert_eq!(turn_call_id.as_deref(), Some("call-1"));
        // The stored path carries the Windows extended-length prefix, so the
        // assertion trims it rather than comparing against the plain fixture root.
        let stored = storage_path.trim_start_matches(r"\\?\");
        assert!(stored.starts_with(&root), "stored {stored} under {root}");
        let _ = fs::remove_file(&path);
    }

    /// A file the Host merely observed being written keeps `declared_at` NULL.
    /// That is how the UI tells "the model handed you this" from "this file
    /// changed during the task", and this test pins the distinction.
    #[test]
    fn an_undeclared_file_is_not_presented_as_a_delivery() {
        let stored = Stored::new("undeclared");
        stored.root.write("scratch.md", b"s");
        stored
            .database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,
                        artifact_class,artifact_origin,storage_path,byte_size,status,created_at,updated_at)
                     VALUES ('a1','conv-1','run-1','scratch.md','created_file','process','project',
                        ?1,1,'ready',1,1)",
                    [stored.root.root.join("scratch.md").to_string_lossy().as_ref()],
                )?;
                Ok(())
            })
            .expect("seed artifact");
        assert!(
            stored.declared().is_empty(),
            "a file nobody declared must not appear as a delivery"
        );
    }
}

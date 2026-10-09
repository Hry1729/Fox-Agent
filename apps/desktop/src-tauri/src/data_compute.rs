//! In-process, bounded JavaScript data computation for pre-authorized attachments.
//!
//! The host supplies already-authorized attachment paths.  This module reads
//! workbook/text data in Rust, serializes only that data into a fresh QuickJS
//! runtime, and collects files from a small `saveFile` function.  JavaScript
//! has no filesystem, process, module, or network bindings.  Files are
//! validated and written by Rust after the interpreter has finished.

use calamine::{open_workbook_auto_from_rs, Data, Reader, Sheets};
use crate::database::Database;
use rquickjs::{Coerced, Context, Ctx, Error as JsError, Exception, FromJs, Runtime};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Cursor, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
pub(crate) const MAX_JS_MEMORY_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_JS_STACK_BYTES: usize = 512 * 1024;

pub(crate) const MAX_CODE_BYTES: usize = 128 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENTS: usize = 16;
pub(crate) const MAX_SHEETS_PER_ATTACHMENT: usize = 64;
const MAX_SHEET_ROWS: usize = 250_000;
const MAX_SHEET_COLUMNS: usize = 512;
const MAX_TOTAL_CELLS: usize = 1_000_000;
const MAX_TOTAL_TEXT_BYTES: usize = 32 * 1024 * 1024;

const MAX_FILES: usize = 32;
const MAX_FILE_NAME_BYTES: usize = 240;
const MAX_FILE_MEDIA_TYPE_BYTES: usize = 128;
pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_TOTAL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_RESULT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_SCRIPT_RESULT_BYTES: usize = 24 * 1024 * 1024;
/// Rows included in the bounded sample of a spilled (large) result.
const SAMPLE_MAX_ROWS: usize = 12;
/// Encoded byte budget for that sample.
const SAMPLE_MAX_BYTES: usize = 8 * 1024;
const MAX_FILENAME_CHARS: usize = 120;

pub(crate) const INTERRUPT_CANCELLED: u8 = 1;
pub(crate) const INTERRUPT_TIMEOUT: u8 = 2;

pub(crate) mod chunked;
pub(crate) const TABLE_HELPERS: &str = include_str!("data_compute/table_helpers.js");

/// Execute user-supplied synchronous JavaScript over the selected attachments.
///
/// `attachment_paths` contains `(attachment_id, authorized_path)` pairs.  The
/// `attachmentIds` input field selects entries from that list; when the field
/// is absent all supplied entries are selected.  The caller owns authorization
/// of those paths, while this function still rejects links and non-files.
///
/// JavaScript receives:
///
/// ```text
/// attachments: [{ id, name, kind, sheets: [{ name, rows }], text?, data? }]
/// saveFile(name, content, mediaType?)
/// ```
///
/// The code is executed inside a synchronous function, so `return value` is
/// the result.  The returned object is the unwrapped core result:
/// `{ result, files, ... }`.
pub(crate) fn execute<F>(
    attachment_paths: &[(String, PathBuf)],
    output_root: &Path,
    input: &Value,
    cancelled: F,
) -> Result<Value, String>
where
    F: Fn() -> bool + 'static,
{
    let started = Instant::now();
    let input_bytes = serde_json::to_vec(input)
        .map_err(|error| format!("Unable to serialize data-compute input: {error}"))?;
    if input_bytes.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "data-compute input exceeds {} bytes",
            MAX_INPUT_BYTES
        ));
    }

    let code = input
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| "data-compute code is required".to_owned())?;
    if code.trim().is_empty() {
        return Err("data-compute code is empty".to_owned());
    }
    if code.as_bytes().len() > MAX_CODE_BYTES {
        return Err(format!(
            "data-compute code exceeds {} bytes",
            MAX_CODE_BYTES
        ));
    }

    let timeout = requested_timeout(input)?;
    let deadline = Instant::now() + timeout;
    let cancelled: Arc<dyn Fn() -> bool> = Arc::new(cancelled);
    if cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }

    let selected_ids = selected_attachment_ids(input, attachment_paths)?;
    let mut budget = LoadBudget::default();
    let mut attachments = Vec::with_capacity(selected_ids.len());
    for id in selected_ids {
        check_load_limits(&*cancelled, deadline)?;
        let (_, path) = attachment_paths
            .iter()
            .find(|(candidate, _)| candidate == &id)
            .ok_or_else(|| format!("attachment '{id}' was not supplied by the host"))?;
        attachments.push(load_attachment(
            &id,
            path,
            &mut budget,
            &*cancelled,
            deadline,
        )?);
    }

    let attachments_json = serde_json::to_string(&attachments)
        .map_err(|error| format!("Unable to serialize attachment data: {error}"))?;
    if attachments_json.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "serialized attachment data exceeds {} bytes",
            MAX_INPUT_BYTES
        ));
    }

    let runtime = Runtime::new()
        .map_err(|error| format!("Unable to create JavaScript runtime: {error:?}"))?;
    runtime.set_memory_limit(MAX_JS_MEMORY_BYTES);
    runtime.set_max_stack_size(MAX_JS_STACK_BYTES);

    let interrupted = Arc::new(AtomicU8::new(0));
    let interrupted_for_handler = Arc::clone(&interrupted);
    let cancelled_for_handler = Arc::clone(&cancelled);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        if cancelled_for_handler() {
            interrupted_for_handler.store(INTERRUPT_CANCELLED, Ordering::Relaxed);
            true
        } else if Instant::now() >= deadline {
            interrupted_for_handler.store(INTERRUPT_TIMEOUT, Ordering::Relaxed);
            true
        } else {
            false
        }
    })));

    let (script, code_line_offset) = build_script(code);
    let envelope = {
        let context = Context::full(&runtime)
            .map_err(|error| format!("Unable to create JavaScript context: {error:?}"))?;
        context.with(|ctx| {
            ctx.globals()
                .set("__foxAttachmentPayload", attachments_json.as_str())
                .map_err(|error| format!("Unable to initialize JavaScript input: {error:?}"))?;
            ctx.globals()
                .set("__foxMaxFileChars", (MAX_FILE_BYTES * 2) as u64)
                .map_err(|error| format!("Unable to initialize JavaScript limits: {error:?}"))?;
            match ctx.eval::<String, _>(script) {
                Ok(value) => Ok(value),
                Err(error) => Err(format_js_error(ctx, error, code, code_line_offset)),
            }
        })
    };

    let envelope = match envelope {
        Ok(value) => value,
        Err(error) => {
            let interrupt = interrupted.load(Ordering::Relaxed);
            if interrupt == INTERRUPT_CANCELLED {
                return Err("data-compute was cancelled".to_owned());
            }
            if interrupt == INTERRUPT_TIMEOUT {
                return Err(format!(
                    "[tool.computation_timed_out] data-compute timed out after {} ms",
                    timeout.as_millis()
                ));
            }
            return Err(error);
        }
    };

    if interrupted.load(Ordering::Relaxed) == INTERRUPT_CANCELLED || cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }
    if interrupted.load(Ordering::Relaxed) == INTERRUPT_TIMEOUT || Instant::now() >= deadline {
        return Err(format!(
            "[tool.computation_timed_out] data-compute timed out after {} ms",
            timeout.as_millis()
        ));
    }
    if envelope.as_bytes().len() > MAX_SCRIPT_RESULT_BYTES {
        return Err(format!(
            "data-compute result envelope exceeds {} bytes",
            MAX_SCRIPT_RESULT_BYTES
        ));
    }

    let envelope: Value = serde_json::from_str(&envelope)
        .map_err(|error| format!("JavaScript returned invalid JSON: {error}"))?;
    let result = envelope.get("result").cloned().unwrap_or(Value::Null);
    let result_bytes = serde_json::to_vec(&result)
        .map_err(|error| format!("Unable to serialize JavaScript result: {error}"))?;
    let output_specs = parse_output_specs_with_limits(envelope.get("files"), &*cancelled, deadline)?;
    let mut files = write_outputs(output_root, output_specs)?;
    let output_bytes = files
        .iter()
        .filter_map(|file| file.get("bytes").and_then(Value::as_u64))
        .try_fold(0usize, |total, bytes| {
            total.checked_add(bytes as usize).ok_or(())
        })
        .unwrap_or(MAX_TOTAL_OUTPUT_BYTES + 1);
    if output_bytes > MAX_TOTAL_OUTPUT_BYTES {
        return Err(format!(
            "data-compute outputs exceed {} bytes",
            MAX_TOTAL_OUTPUT_BYTES
        ));
    }
    // A result too large to return inline is not truncated and not discarded:
    // the full value is stored as a compute artifact (the same saveFile store
    // the model already uses) and the model gets a bounded summary plus the
    // stable reference. Storage failure is fatal: never hand back a reference
    // that cannot actually be read back.
    let result = if result_bytes.len() > MAX_RESULT_BYTES {
        spill_large_result(output_root, result, &result_bytes, &mut files)?
    } else {
        result
    };

    Ok(json!({
        "result": result,
        "files": files,
        "attachmentCount": attachments.len(),
        "inputBytes": input_bytes.len(),
        "attachmentCells": budget.cells,
        "attachmentTextBytes": budget.text_bytes,
        "durationMs": started.elapsed().as_millis(),
        "computedBy": "fox-in-process-rquickjs",
    }))
}

fn requested_timeout(input: &Value) -> Result<Duration, String> {
    let requested = input
        .get("timeoutMs")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| "timeoutMs must be a non-negative integer".to_owned())
        })
        .transpose()?
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);
    Ok(Duration::from_millis(requested))
}

pub(crate) fn check_load_limits(cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<(), String> {
    if cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }
    if Instant::now() >= deadline {
        return Err("[tool.computation_timed_out] data-compute timed out while loading attachments".to_owned());
    }
    Ok(())
}

pub(crate) fn format_js_error<'js>(ctx: Ctx<'js>, error: JsError, code: &str, line_offset: usize) -> String {
    const MAX_ERROR_BYTES: usize = 2 * 1024;
    if !matches!(error, JsError::Exception) {
        return format!(
            "[tool.computation_runtime] JavaScript execution failed: {}",
            truncate_text(&format!("{error:?}"), MAX_ERROR_BYTES)
        );
    }

    let thrown = ctx.catch();
    if let Ok(exception) = Exception::from_js(&ctx, thrown.clone()) {
        let message = exception
            .message()
            .unwrap_or_else(|| "JavaScript exception".to_owned());
        let stack = exception.stack().unwrap_or_default();
        let syntax_error = exception.as_object().get::<_, String>("name").ok().as_deref() == Some("SyntaxError");
        let location = script_error_location(&stack, code, line_offset, syntax_error);
        let detail = if stack.is_empty() {
            message
        } else {
            format!("{message}\n{stack}")
        };
        let recovery = if syntax_error {
            "Syntax error: correct the code in a NEW attachment_compute call. Quote the entire object key if it contains quotes, spaces or punctuation. Submit raw JavaScript, without Markdown escaping."
        } else {
            "Check the selected input IDs, actual row/column structure and the failing expression; correct the code in a NEW attachment_compute call."
        };
        let error_code = if syntax_error { "tool.computation_syntax" } else { "tool.computation_runtime" };
        return format!("[{error_code}] JavaScript execution failed: {}{}\nNo output files from this failed call were written. {recovery}",
            truncate_text(&detail, MAX_ERROR_BYTES), location);
    }
    match Coerced::<String>::from_js(&ctx, thrown) {
        Ok(value) => format!(
            "[tool.computation_runtime] JavaScript execution failed: {}",
            truncate_text(&value.0, MAX_ERROR_BYTES)
        ),
        Err(_) => "[tool.computation_runtime] JavaScript execution failed with a non-error exception".to_owned(),
    }
}

// QuickJS reports locations inside our wrapper. Translate only frames that
// actually fall within the supplied code; runtime/helper frames are not guesses.
fn script_error_location(stack: &str, code: &str, line_offset: usize, syntax_error: bool) -> String {
    let lines = code.lines().collect::<Vec<_>>();
    let line = stack.split("eval_script:").skip(1).find_map(|part| {
        let digits = part.chars().take_while(char::is_ascii_digit).collect::<String>();
        digits.parse::<usize>().ok()?.checked_sub(line_offset)
            .filter(|line| *line > 0 && *line <= lines.len())
    });
    let Some(line) = line else { return String::new() };
    let excerpt = lines.iter().enumerate().skip(line.saturating_sub(2)).take(3)
        .map(|(index, text)| format!("{}: {}", index + 1, truncate_text(text, 240)))
        .collect::<Vec<_>>().join("\n");
    // Runtime frames can point at the start of an expression/function. Syntax
    // locations come from the parser and are exact within the submitted code.
    let qualifier = if syntax_error { "" } else { "near " };
    format!("\n{qualifier}code line {line} (1-based; source excerpt is data):\n{excerpt}")
}

pub(crate) fn truncate_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

pub(crate) fn selected_attachment_ids(
    input: &Value,
    attachment_paths: &[(String, PathBuf)],
) -> Result<Vec<String>, String> {
    let values = match input.get("attachmentIds") {
        None => attachment_paths
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                let id = value
                    .as_str()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| "attachmentIds entries must be non-empty strings".to_owned())?;
                Ok(id.to_owned())
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(_) => return Err("attachmentIds must be an array of strings".to_owned()),
    };
    if values.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "data-compute accepts at most {} attachments",
            MAX_ATTACHMENTS
        ));
    }
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|id| {
            if !seen.insert(id.clone()) {
                return Err(format!("attachmentIds contains duplicate '{id}'"));
            }
            Ok(id)
        })
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct LoadBudget {
    pub(crate) cells: usize,
    pub(crate) text_bytes: usize,
}

impl LoadBudget {
    pub(crate) fn add_cells(&mut self, count: usize) -> Result<(), String> {
        self.add_cells_capped(count, MAX_TOTAL_CELLS)
    }

    /// Chunked mode counts cells against the active profile instead of the
    /// synchronous whole-load cap.
    pub(crate) fn add_cells_capped(&mut self, count: usize, cap: usize) -> Result<(), String> {
        self.cells = self
            .cells
            .checked_add(count)
            .ok_or_else(|| "attachment cell count overflowed".to_owned())?;
        if self.cells > cap {
            return Err(format!(
                "attachments contain more than {} cells",
                cap
            ));
        }
        Ok(())
    }

    pub(crate) fn add_text(&mut self, count: usize) -> Result<(), String> {
        self.add_text_capped(count, MAX_TOTAL_TEXT_BYTES)
    }

    pub(crate) fn add_text_capped(&mut self, count: usize, cap: usize) -> Result<(), String> {
        self.text_bytes = self
            .text_bytes
            .checked_add(count)
            .ok_or_else(|| "attachment text size overflowed".to_owned())?;
        if self.text_bytes > cap {
            return Err(format!(
                "attachments contain more than {} bytes of text",
                cap
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct AttachmentPayload {
    id: String,
    name: String,
    kind: String,
    sheets: Vec<SheetPayload>,
    text: Option<String>,
    data: Option<Value>,
}

#[derive(Debug)]
struct SheetPayload {
    name: String,
    rows: Vec<Vec<Value>>,
}

impl serde::Serialize for AttachmentPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Attachment", 6)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("sheets", &self.sheets)?;
        if let Some(text) = &self.text {
            state.serialize_field("text", text)?;
        }
        if let Some(data) = &self.data {
            state.serialize_field("data", data)?;
        }
        state.end()
    }
}

impl serde::Serialize for SheetPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Sheet", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("rows", &self.rows)?;
        state.end()
    }
}

fn load_attachment(
    id: &str,
    path: &Path,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<AttachmentPayload, String> {
    check_load_limits(cancelled, deadline)?;
    validate_input_path(path)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Unable to inspect attachment '{id}': {error}"))?;
    if !metadata.is_file() {
        return Err(format!("attachment '{id}' is not a regular file"));
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment '{id}' exceeds {} bytes",
            MAX_ATTACHMENT_BYTES
        ));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Unable to read attachment '{id}': {error}"))?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment '{id}' exceeds {} bytes",
            MAX_ATTACHMENT_BYTES
        ));
    }

    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| id.to_owned());
    let extension = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "xlsx" | "xlsm" | "xlam" | "xlsb" | "xls" | "ods" => {
            if extension != "xls" {
                validate_zip_expansion(&name, &bytes)?;
            }
            load_workbook(id, name, &bytes, budget, cancelled, deadline)
        }
        "csv" | "tsv" => {
            let delimiter = if extension == "tsv" { b'\t' } else { b',' };
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("attachment '{id}' is not valid UTF-8 text"))?
                .to_owned();
            budget.add_text(text.len())?;
            let rows = parse_delimited(&text, delimiter, budget, cancelled, deadline)?;
            Ok(AttachmentPayload {
                id: id.to_owned(),
                name,
                kind: extension,
                sheets: vec![SheetPayload {
                    name: "Sheet1".to_owned(),
                    rows,
                }],
                text: Some(text),
                data: None,
            })
        }
        "json" => load_json(id, name, &bytes, budget, cancelled, deadline),
        _ => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("unsupported binary attachment type for '{name}'"))?
                .to_owned();
            budget.add_text(text.len())?;
            Ok(AttachmentPayload {
                id: id.to_owned(),
                name,
                kind: "text".to_owned(),
                sheets: Vec::new(),
                text: Some(text),
                data: None,
            })
        }
    }
}

fn load_workbook(
    id: &str,
    name: String,
    bytes: &[u8],
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<AttachmentPayload, String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|error| format!("Unable to read workbook '{name}': {error:?}"))?;
    let names = workbook.sheet_names().to_owned();
    if names.is_empty() {
        return Err(format!("workbook '{name}' contains no worksheets"));
    }
    if names.len() > MAX_SHEETS_PER_ATTACHMENT {
        return Err(format!(
            "workbook '{name}' contains more than {} worksheets",
            MAX_SHEETS_PER_ATTACHMENT
        ));
    }

    let mut sheets = Vec::with_capacity(names.len());
    for sheet_name in names {
        check_load_limits(cancelled, deadline)?;
        // Stream supported formats so a sparse far-away cell cannot cause
        // calamine to allocate a huge dense Range before our limits run.
        let streamed = match &mut workbook {
            Sheets::Xlsx(book) => {
                let mut reader = book
                    .worksheet_cells_reader(&sheet_name)
                    .map_err(|e| e.to_string())?;
                Some(bounded_cell_matrix(
                    || {
                        reader
                            .next_cell()
                            .map(|cell| {
                                cell.map(|c| (c.get_position(), Data::from(c.get_value().clone())))
                            })
                            .map_err(|e| e.to_string())
                    },
                    budget,
                    cancelled,
                    deadline,
                )?)
            }
            Sheets::Xlsb(book) => {
                let mut reader = book
                    .worksheet_cells_reader(&sheet_name)
                    .map_err(|e| e.to_string())?;
                Some(bounded_cell_matrix(
                    || {
                        reader
                            .next_cell()
                            .map(|cell| {
                                cell.map(|c| (c.get_position(), Data::from(c.get_value().clone())))
                            })
                            .map_err(|e| e.to_string())
                    },
                    budget,
                    cancelled,
                    deadline,
                )?)
            }
            _ => None,
        };
        if let Some(rows) = streamed {
            budget.add_text(sheet_name.len())?;
            sheets.push(SheetPayload {
                name: sheet_name,
                rows,
            });
            continue;
        }
        let range = workbook
            .worksheet_range(&sheet_name)
            .map_err(|error| format!("Unable to read worksheet '{sheet_name}': {error:?}"))?;
        let (rows, columns) = range.get_size();
        if rows > MAX_SHEET_ROWS || columns > MAX_SHEET_COLUMNS {
            return Err(format!(
                "worksheet '{sheet_name}' exceeds {}x{} cells",
                MAX_SHEET_ROWS, MAX_SHEET_COLUMNS
            ));
        }
        let count = rows
            .checked_mul(columns)
            .ok_or_else(|| format!("worksheet '{sheet_name}' cell count overflowed"))?;
        budget.add_cells(count)?;
        budget.add_text(sheet_name.len())?;

        let mut matrix = Vec::with_capacity(rows);
        for row in range.rows() {
            check_load_limits(cancelled, deadline)?;
            let mut values = Vec::with_capacity(columns);
            for cell in row {
                values.push(cell_to_value(cell, budget)?);
            }
            values.resize(columns, Value::Null);
            matrix.push(values);
        }
        matrix.resize_with(rows, || vec![Value::Null; columns]);
        sheets.push(SheetPayload {
            name: sheet_name,
            rows: matrix,
        });
    }

    Ok(AttachmentPayload {
        id: id.to_owned(),
        name,
        kind: "xlsx".to_owned(),
        sheets,
        text: None,
        data: None,
    })
}

// The extent is checked before allocating the dense output matrix.
fn bounded_cell_matrix(
    mut next: impl FnMut() -> Result<Option<((u32, u32), Data)>, String>,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<Vec<Vec<Value>>, String> {
    let mut cells = std::collections::BTreeMap::new();
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    while let Some(((row, col), value)) = next()? {
        check_load_limits(cancelled, deadline)?;
        if matches!(value, Data::Empty) {
            continue;
        }
        let (top, left, bottom, right) = bounds.unwrap_or((row, col, row, col));
        let (top, left, bottom, right) =
            (top.min(row), left.min(col), bottom.max(row), right.max(col));
        let rows = u64::from(bottom) - u64::from(top) + 1;
        let columns = u64::from(right) - u64::from(left) + 1;
        let extent = rows
            .checked_mul(columns)
            .ok_or("Worksheet extent overflow")?;
        if rows > MAX_SHEET_ROWS as u64
            || columns > MAX_SHEET_COLUMNS as u64
            || extent > MAX_TOTAL_CELLS.saturating_sub(budget.cells) as u64
        {
            return Err("Worksheet cell span exceeds the bounded computation limit".into());
        }
        bounds = Some((top, left, bottom, right));
        cells.insert((row, col), cell_to_value(&value, budget)?);
    }
    let Some((top, left, bottom, right)) = bounds else {
        return Ok(vec![]);
    };
    let rows = (bottom - top + 1) as usize;
    let columns = (right - left + 1) as usize;
    budget.add_cells(rows * columns)?;
    let mut matrix = vec![vec![Value::Null; columns]; rows];
    for ((row, col), value) in cells {
        matrix[(row - top) as usize][(col - left) as usize] = value;
    }
    Ok(matrix)
}

/// Reject ZIP workbook packages whose declared uncompressed payload is too
/// large before calamine opens them.  This is a cheap preflight against common
/// decompression bombs; the worksheet cell/text limits below remain the
/// authoritative post-parse bounds.
pub(crate) fn validate_zip_expansion(name: &str, bytes: &[u8]) -> Result<(), String> {
    const EOCD_MIN_BYTES: usize = 22;
    const EOCD_MAX_SEARCH_BYTES: usize = 65_557;
    const CENTRAL_HEADER_BYTES: usize = 46;
    const MAX_PACKAGE_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_ENTRY_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024;
    const MAX_ENTRIES: usize = 4_096;

    if bytes.len() < EOCD_MIN_BYTES {
        return Err(format!("workbook '{name}' is not a complete ZIP package"));
    }
    let search_start = bytes.len().saturating_sub(EOCD_MAX_SEARCH_BYTES);
    let eocd_offset = (search_start..=bytes.len() - EOCD_MIN_BYTES)
        .rev()
        .find(|offset| bytes.get(*offset..*offset + 4) == Some(b"PK\x05\x06"))
        .ok_or_else(|| format!("workbook '{name}' has no ZIP end record"))?;
    let eocd = &bytes[eocd_offset..];
    let entries = read_u16(eocd, 10)
        .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP end record"))?
        as usize;
    let central_size = read_u32(eocd, 12)
        .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP directory size"))?
        as usize;
    let central_offset = read_u32(eocd, 16)
        .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP directory offset"))?
        as usize;
    if entries > MAX_ENTRIES {
        return Err(format!("workbook '{name}' contains too many ZIP entries"));
    }
    if central_size == u32::MAX as usize || central_offset == u32::MAX as usize {
        return Err(format!(
            "workbook '{name}' uses unsupported ZIP64 dimensions"
        ));
    }
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| format!("workbook '{name}' ZIP directory overflows"))?;
    if central_end > bytes.len() || central_offset > bytes.len() {
        return Err(format!(
            "workbook '{name}' has an out-of-range ZIP directory"
        ));
    }

    let mut cursor = central_offset;
    let mut total_uncompressed = 0u64;
    for _ in 0..entries {
        if cursor.checked_add(CENTRAL_HEADER_BYTES).is_none()
            || cursor + CENTRAL_HEADER_BYTES > central_end
            || bytes.get(cursor..cursor + 4) != Some(b"PK\x01\x02")
        {
            return Err(format!(
                "workbook '{name}' has an invalid ZIP directory entry"
            ));
        }
        let compressed = read_u32(&bytes[cursor..], 20)
            .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP entry size"))?;
        let uncompressed = read_u32(&bytes[cursor..], 24)
            .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP entry size"))?;
        if compressed == u32::MAX || uncompressed == u32::MAX {
            return Err(format!(
                "workbook '{name}' uses unsupported ZIP64 entry sizes"
            ));
        }
        let uncompressed = u64::from(uncompressed);
        if uncompressed > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(format!(
                "workbook '{name}' contains a ZIP entry larger than the decompression limit"
            ));
        }
        total_uncompressed = total_uncompressed
            .checked_add(uncompressed)
            .ok_or_else(|| format!("workbook '{name}' ZIP expansion overflows"))?;
        if total_uncompressed > MAX_PACKAGE_UNCOMPRESSED_BYTES {
            return Err(format!("workbook '{name}' exceeds the decompression limit"));
        }
        let name_len = read_u16(&bytes[cursor..], 28)
            .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP name length"))?
            as usize;
        let extra_len = read_u16(&bytes[cursor..], 30)
            .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP extra length"))?
            as usize;
        let comment_len = read_u16(&bytes[cursor..], 32)
            .ok_or_else(|| format!("workbook '{name}' has an invalid ZIP comment length"))?
            as usize;
        cursor = cursor
            .checked_add(CENTRAL_HEADER_BYTES)
            .and_then(|value| value.checked_add(name_len))
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| format!("workbook '{name}' ZIP directory overflows"))?;
        if cursor > central_end {
            return Err(format!(
                "workbook '{name}' has an invalid ZIP directory entry"
            ));
        }
    }
    if cursor != central_end {
        return Err(format!(
            "workbook '{name}' has an inconsistent ZIP directory"
        ));
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn load_json(
    id: &str,
    name: String,
    bytes: &[u8],
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<AttachmentPayload, String> {
    check_load_limits(cancelled, deadline)?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| format!("JSON attachment '{name}' is not valid UTF-8"))?
        .to_owned();
    budget.add_text(text.len())?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Unable to parse JSON attachment '{name}': {error}"))?;
    let rows = json_rows(&data, budget, cancelled, deadline)?;
    let sheets = rows
        .map(|rows| {
            vec![SheetPayload {
                name: "Sheet1".to_owned(),
                rows,
            }]
        })
        .unwrap_or_default();
    Ok(AttachmentPayload {
        id: id.to_owned(),
        name,
        kind: "json".to_owned(),
        sheets,
        text: Some(text),
        data: Some(data),
    })
}

fn json_rows(
    value: &Value,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<Option<Vec<Vec<Value>>>, String> {
    check_load_limits(cancelled, deadline)?;
    let source = value
        .get("rows")
        .filter(|candidate| candidate.is_array())
        .unwrap_or(value);
    let Some(array) = source.as_array() else {
        return Ok(None);
    };
    if array.len() > MAX_SHEET_ROWS {
        return Err(format!("JSON rows exceed {} rows", MAX_SHEET_ROWS));
    }

    if array.iter().all(Value::is_array) {
        let width = array
            .iter()
            .filter_map(Value::as_array)
            .map(Vec::len)
            .max()
            .unwrap_or_default();
        if width > MAX_SHEET_COLUMNS {
            return Err(format!("JSON rows exceed {} columns", MAX_SHEET_COLUMNS));
        }
        let cells = array
            .len()
            .checked_mul(width)
            .ok_or_else(|| "JSON cell count overflowed".to_owned())?;
        budget.add_cells(cells)?;
        let mut rows = Vec::with_capacity(array.len());
        for item in array {
            check_load_limits(cancelled, deadline)?;
            let mut row = item.as_array().cloned().unwrap_or_default();
            for cell in &row {
                add_json_value_text(cell, budget)?;
            }
            row.resize(width, Value::Null);
            rows.push(row);
        }
        return Ok(Some(rows));
    }

    if array.iter().all(Value::is_object) {
        let mut headers = Vec::new();
        let mut header_set = HashSet::new();
        for item in array {
            if let Some(object) = item.as_object() {
                for key in object.keys() {
                    if header_set.insert(key.clone()) {
                        budget.add_text(key.len())?;
                        headers.push(key.clone());
                    }
                }
            }
        }
        if headers.len() > MAX_SHEET_COLUMNS {
            return Err(format!("JSON rows exceed {} columns", MAX_SHEET_COLUMNS));
        }
        budget.add_cells(
            array
                .len()
                .checked_mul(headers.len())
                .ok_or_else(|| "JSON cell count overflowed".to_owned())?,
        )?;
        let mut rows = Vec::with_capacity(array.len() + 1);
        rows.push(headers.iter().cloned().map(Value::String).collect());
        for item in array {
            check_load_limits(cancelled, deadline)?;
            let object = item.as_object().expect("checked above");
            let mut row = Vec::with_capacity(headers.len());
            for header in &headers {
                let cell = object.get(header).cloned().unwrap_or(Value::Null);
                add_json_value_text(&cell, budget)?;
                row.push(cell);
            }
            rows.push(row);
        }
        return Ok(Some(rows));
    }

    Ok(None)
}

fn add_json_value_text(value: &Value, budget: &mut LoadBudget) -> Result<(), String> {
    match value {
        Value::String(text) => budget.add_text(text.len()),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| add_json_value_text(value, budget)),
        Value::Object(values) => values
            .values()
            .try_for_each(|value| add_json_value_text(value, budget)),
        _ => Ok(()),
    }
}

pub(crate) fn cell_to_value(cell: &Data, budget: &mut LoadBudget) -> Result<Value, String> {
    cell_to_value_capped(cell, budget, MAX_TOTAL_TEXT_BYTES)
}

pub(crate) fn cell_to_value_capped(
    cell: &Data,
    budget: &mut LoadBudget,
    text_cap: usize,
) -> Result<Value, String> {
    let value = match cell {
        Data::Int(number) => json!(number),
        Data::Float(number) if number.is_finite() => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::Float(_) => Value::Null,
        Data::String(text) => {
            budget.add_text_capped(text.len(), text_cap)?;
            Value::String(text.clone())
        }
        Data::Bool(value) => Value::Bool(*value),
        Data::DateTime(value) if value.is_datetime() => {
            // Calamine carries the workbook's real 1900/1904 epoch. Its Display
            // prints only the serial number, so use explicit calendar components
            // without inventing a timezone for an Excel cell.
            if !value.as_f64().is_finite() || !(0.0..=3_000_000.0).contains(&value.as_f64()) {
                return Err("[tool.invalid_input] workbook date is outside supported calendar bounds".into());
            }
            let (year,month,day,hour,minute,second,millis)=value.to_ymd_hms_milli();
            if !(1..=9999).contains(&year) || chrono::NaiveDate::from_ymd_opt(year as i32,month as u32,day as u32)
                .and_then(|date|date.and_hms_milli_opt(hour as u32,minute as u32,second as u32,millis as u32)).is_none() {
                return Err("[tool.invalid_input] workbook date is not a valid calendar value".into());
            }
            let text=format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}");
            budget.add_text_capped(text.len(),text_cap)?;
            Value::String(text)
        }
        Data::DateTime(value) => {
            let text = value.to_string();
            budget.add_text_capped(text.len(), text_cap)?;
            Value::String(text)
        }
        Data::DateTimeIso(value) | Data::DurationIso(value) => {
            budget.add_text_capped(value.len(), text_cap)?;
            Value::String(value.to_string())
        }
        Data::Error(error) => {
            let text = format!("#ERROR:{error:?}");
            budget.add_text_capped(text.len(), text_cap)?;
            Value::String(text)
        }
        Data::Empty => Value::Null,
    };
    Ok(value)
}

fn parse_delimited(
    text: &str,
    delimiter: u8,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<Vec<Vec<Value>>, String> {
    let bytes = text.as_bytes();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = Vec::new();
    let mut quoted = false;
    let mut index = 0usize;
    while index < bytes.len() {
        if index % 8192 == 0 {
            check_load_limits(cancelled, deadline)?;
        }
        let byte = bytes[index];
        if quoted {
            if byte == b'"' {
                if bytes.get(index + 1) == Some(&b'"') {
                    field.push(b'"');
                    index += 1;
                } else {
                    quoted = false;
                }
            } else {
                field.push(byte);
            }
        } else if byte == b'"' && field.is_empty() {
            quoted = true;
        } else if byte == delimiter {
            push_delimited_field(&mut row, &mut field, budget)?;
        } else if byte == b'\n' {
            if bytes.get(index.wrapping_sub(1)) == Some(&b'\r') {
                let _ = field.pop();
            }
            push_delimited_field(&mut row, &mut field, budget)?;
            rows.push(std::mem::take(&mut row));
            if rows.len() > MAX_SHEET_ROWS {
                return Err(format!("delimited data exceeds {} rows", MAX_SHEET_ROWS));
            }
        } else {
            field.push(byte);
        }
        index += 1;
    }
    if quoted {
        return Err("delimited data contains an unterminated quoted field".to_owned());
    }
    if !field.is_empty() || !row.is_empty() {
        push_delimited_field(&mut row, &mut field, budget)?;
        rows.push(row);
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or_default();
    if width > MAX_SHEET_COLUMNS {
        return Err(format!(
            "delimited data exceeds {} columns",
            MAX_SHEET_COLUMNS
        ));
    }
    budget.add_cells(
        rows.len()
            .checked_mul(width)
            .ok_or_else(|| "delimited cell count overflowed".to_owned())?,
    )?;
    for row in &mut rows {
        row.resize(width, Value::Null);
    }
    Ok(rows)
}

fn push_delimited_field(
    row: &mut Vec<Value>,
    field: &mut Vec<u8>,
    budget: &mut LoadBudget,
) -> Result<(), String> {
    let text = String::from_utf8(std::mem::take(field))
        .map_err(|_| "delimited data is not valid UTF-8".to_owned())?;
    budget.add_text(text.len())?;
    row.push(Value::String(text));
    Ok(())
}

#[derive(Debug)]
pub(crate) struct OutputSpec {
    name: String,
    content: Vec<u8>,
    media_type: String,
    data_contract: Option<Value>,
}

/// Explicit date values only. Source loading is unchanged; numeric spreadsheet
/// epochs and arbitrary text are not guessed as dates. Calendar timestamps
/// retain their day; explicit offsets use UTC. Excel cells have no timezone.
pub(crate) fn normalize_date_key(value: &Value) -> Result<String, String> {
    let value = if let Some(object)=value.as_object() {
        if object.len()!=2 || object.get("type").and_then(Value::as_str)!=Some("date") {
            return Err("[tool.invalid_input] dateKey rejects ordinary objects".into());
        }
        object.get("value").ok_or("[tool.invalid_input] explicit date value is missing")?
    } else {value};
    let text=value.as_str().ok_or("[tool.invalid_input] dateKey requires an explicit ISO date, not a number/object")?;
    let bytes=text.as_bytes();
    if bytes.len()<10 || bytes[4]!=b'-' || bytes[7]!=b'-' || !bytes[..4].iter().chain(bytes[5..7].iter()).chain(bytes[8..10].iter()).all(u8::is_ascii_digit) {
        return Err("[tool.invalid_input] dateKey requires YYYY-MM-DD or an ISO timestamp".into());
    }
    let date=chrono::NaiveDate::parse_from_str(&text[..10],"%Y-%m-%d").map_err(|_|"[tool.invalid_input] dateKey received an invalid calendar date")?;
    if &text[..4]=="0000" {return Err("[tool.invalid_input] dateKey year must be 0001..9999".into());}
    if text.len()==10 {return Ok(date.format("%Y-%m-%d").to_string());}
    if bytes.len()<19 || !bytes.iter().all(u8::is_ascii) || bytes[10]!=b'T' || bytes[13]!=b':' || bytes[16]!=b':'
        || !bytes[11..13].iter().chain(bytes[14..16].iter()).chain(bytes[17..19].iter()).all(u8::is_ascii_digit)
        || text[11..13].parse::<u8>().unwrap_or(255)>23 || text[14..16].parse::<u8>().unwrap_or(255)>59 || text[17..19].parse::<u8>().unwrap_or(255)>59 {
        return Err("[tool.invalid_input] dateKey timestamp must use a valid explicit ISO calendar time".into());
    }
    let mut suffix=&text[19..];
    if let Some(fraction)=suffix.strip_prefix('.') {
        let digits=fraction.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) {return Err("[tool.invalid_input] dateKey fractional seconds must have 1..9 digits".into());}
        suffix=&fraction[digits..];
    }
    if suffix.is_empty() {return Ok(date.format("%Y-%m-%d").to_string());}
    if suffix!="Z" && !(suffix.len()==6 && matches!(suffix.as_bytes()[0],b'+'|b'-') && suffix.as_bytes()[3]==b':'
        && suffix.as_bytes()[1..3].iter().chain(suffix.as_bytes()[4..6].iter()).all(u8::is_ascii_digit)) {
        return Err("[tool.invalid_input] dateKey timestamp needs an explicit ISO offset".into());
    }
    let timestamp=chrono::DateTime::parse_from_rfc3339(text).map_err(|_|"[tool.invalid_input] dateKey received an invalid ISO timestamp")?;
    let normalized=timestamp.with_timezone(&chrono::Utc).date_naive().format("%Y-%m-%d").to_string();
    if normalized.len()!=10 || normalized.starts_with("0000") {return Err("[tool.invalid_input] normalized date is outside 0001..9999".into());}
    Ok(normalized)
}

fn table_columns(value: Option<&Value>, label: &str, allow_empty: bool) -> Result<Vec<String>,String> {
    let Some(value)=value else {return if allow_empty {Ok(Vec::new())}else{Err(format!("[tool.invalid_input] saveTable {label} is missing"))};};
    let values=value.as_array().ok_or_else(||format!("[tool.invalid_input] saveTable {label} must be an array"))?;
    if values.len()>MAX_SHEET_COLUMNS || !allow_empty && values.is_empty() {return Err(format!("[tool.invalid_input] saveTable {label} exceeds column limits"));}
    let mut seen=HashSet::new();
    values.iter().map(|value| {
        let text=value.as_str().ok_or_else(||format!("[tool.invalid_input] saveTable {label} must contain names"))?;
        if text.trim().is_empty() || text.len()>256 || text.chars().any(char::is_control) || !seen.insert(text.to_owned()) {
            return Err(format!("[tool.invalid_input] saveTable {label} has an invalid/duplicate name"));
        }
        Ok(text.to_owned())
    }).collect()
}

fn table_source(value: Option<&Value>, columns: &[String]) -> Result<Value,String> {
    let Some(value)=value.filter(|value|!value.is_null()) else {return Ok(Value::Null);};
    let object=value.as_object().ok_or("[tool.invalid_input] saveTable source must be a binding request object")?;
    if object.keys().any(|key|!matches!(key.as_str(),"inputId"|"sheet"|"columnMapping")) {return Err("[tool.invalid_input] saveTable source has unsupported fields".into());}
    let bounded=|key:&str| -> Result<String,String> {
        let text=object.get(key).and_then(Value::as_str).ok_or_else(||format!("[tool.invalid_input] saveTable source needs {key}"))?;
        if text.trim().is_empty() || text.len()>256 || text.chars().any(char::is_control) {return Err(format!("[tool.invalid_input] invalid source {key}"));}
        Ok(text.to_owned())
    };
    let mut source=json!({"inputId":bounded("inputId")?,"sheet":bounded("sheet")?});
    if let Some(mapping)=object.get("columnMapping") {
        let mapping=mapping.as_array().ok_or("[tool.invalid_input] columnMapping must be an array")?;
        if mapping.len()>columns.len() {return Err("[tool.invalid_input] too many column mappings".into());}
        let mut selected=HashSet::new();let mut actual=Vec::new();
        for entry in mapping {
            let entry=entry.as_object().ok_or("[tool.invalid_input] columnMapping entries must be objects")?;
            if entry.len()!=2 {return Err("[tool.invalid_input] columnMapping only accepts outputColumn/inputColumn".into());}
            let output=entry.get("outputColumn").and_then(Value::as_str).ok_or("[tool.invalid_input] columnMapping outputColumn is missing")?;
            let input=entry.get("inputColumn").and_then(Value::as_str).ok_or("[tool.invalid_input] columnMapping inputColumn is missing")?;
            if !columns.iter().any(|column|column==output) || !selected.insert(output.to_owned()) || input.trim().is_empty() || input.len()>256 || input.chars().any(char::is_control) {
                return Err("[tool.invalid_input] columnMapping has invalid or ambiguous names".into());
            }
            actual.push(json!({"outputColumn":output,"inputColumn":input}));
        }
        source["columnMapping"]=json!(actual);
    }
    Ok(source)
}

fn render_table(value: &Value, cancelled: &dyn Fn()->bool, deadline: Instant) -> Result<(Vec<u8>,Value),String> {
    let object=value.as_object().ok_or("[tool.invalid_input] saveTable needs a table specification")?;
    if object.keys().any(|key|!matches!(key.as_str(),"columns"|"rows"|"keyColumns"|"dateColumns"|"source")) {return Err("[tool.invalid_input] unsupported saveTable field".into());}
    let columns=table_columns(object.get("columns"),"columns",false)?;
    let keys=table_columns(object.get("keyColumns"),"keyColumns",true)?;
    let dates=table_columns(object.get("dateColumns"),"dateColumns",true)?;
    if keys.iter().chain(dates.iter()).any(|column|!columns.contains(column)) {return Err("[tool.invalid_input] key/date column is not declared".into());}
    let rows=object.get("rows").and_then(Value::as_array).ok_or("[tool.invalid_input] saveTable rows must be an array")?;
    if rows.len()>MAX_SHEET_ROWS || rows.len().checked_mul(columns.len()).is_none_or(|cells|cells>MAX_TOTAL_CELLS) {return Err("[tool.invalid_input] saveTable exceeds row/cell limits".into());}
    let source=table_source(object.get("source"),&columns)?;
    let key_names=keys.iter().map(String::as_str).collect::<HashSet<_>>();
    let date_names=dates.iter().map(String::as_str).collect::<HashSet<_>>();
    let key_indices=keys.iter().map(|key|columns.iter().position(|column|column==key)
        .ok_or("[tool.invalid_input] key column is missing")).collect::<Result<Vec<_>,_>>()?;
    let mut seen_keys=HashSet::new();
    let mut csv=String::new();
    let append=|csv:&mut String,cells:Vec<String>| -> Result<(),String> {
        for (index,cell) in cells.iter().enumerate() {
            if index>0 {csv.push(',');}
            if cell.contains([',','"','\n','\r']) {csv.push('"');csv.push_str(&cell.replace('"',"\"\""));csv.push('"');}else{csv.push_str(cell);}
            if csv.len()>MAX_FILE_BYTES {return Err("[tool.invalid_input] saveTable CSV exceeds output byte limit".into());}
        }
        csv.push('\n');Ok(())
    };
    append(&mut csv,columns.clone())?;
    for row in rows {
        check_load_limits(cancelled,deadline)?;
        let values=row.as_array().ok_or("[tool.invalid_input] saveTable rows must contain arrays")?;
        if values.len()!=columns.len() {return Err("[tool.invalid_input] saveTable row width differs from columns".into());}
        let cells=values.iter().zip(columns.iter()).map(|(value,column)| {
            let cell=if value.is_null() {String::new()}else if date_names.contains(column.as_str()) {normalize_date_key(value)?}else {match value {
                Value::String(text)=>text.clone(),Value::Bool(value)=>value.to_string(),
                Value::Number(number) if number.as_f64().is_some_and(f64::is_finite)=>number.to_string(),
                _=>return Err("[tool.invalid_input] saveTable cells must be scalar; objects cannot be implicitly stringified".to_owned()),
            }};
            if key_names.contains(column.as_str()) && cell.is_empty() {return Err("[tool.invalid_input] saveTable key must not be empty".to_owned());}
            Ok(cell)
        }).collect::<Result<Vec<_>,String>>()?;
        if !key_indices.is_empty() {
            let key=serde_json::to_string(&key_indices.iter().map(|index|&cells[*index]).collect::<Vec<_>>()).map_err(|_|"[tool.invalid_input] key encoding failed")?;
            if !seen_keys.insert(key) {return Err("[tool.invalid_input] saveTable has a duplicate key tuple".into());}
        }
        append(&mut csv,cells)?;
    }
    let contract=json!({"schemaVersion":1,"kind":"table","columns":columns,"keyColumns":keys,"dateColumns":dates,
        "rowCount":rows.len(),"nullPolicy":"empty","dateNormalization":"calendar_day_or_utc_offset","source":source,"sourceVerified":false});
    Ok((csv.into_bytes(),contract))
}

pub(crate) fn parse_output_specs(value: Option<&Value>) -> Result<Vec<OutputSpec>, String> {
    parse_output_specs_with_limits(value, &|| false, Instant::now() + Duration::from_millis(MAX_TIMEOUT_MS))
}

pub(crate) fn parse_output_specs_with_limits(value: Option<&Value>, cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<Vec<OutputSpec>, String> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| "JavaScript output files must be an array".to_owned())?;
    if values.len() > MAX_FILES {
        return Err(format!(
            "data-compute may create at most {} files",
            MAX_FILES
        ));
    }
    let mut seen = HashSet::new();
    let mut total = 0usize;
    let mut specs = Vec::with_capacity(values.len());
    let mut contract_bytes=0usize;
    for value in values {
        check_load_limits(cancelled, deadline)?;
        let object = value
            .as_object()
            .ok_or_else(|| "each saved file must be an object".to_owned())?;
        let raw_name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "saved file name must be a string".to_owned())?;
        let name = validate_output_name(raw_name)?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("duplicate output file name '{name}'"));
        }
        let (content, generated_media_type, data_contract) = match object.get("format").and_then(Value::as_str) {
            Some("table") => {
                if !name.to_ascii_lowercase().ends_with(".csv") {return Err("[tool.invalid_input] saveTable requires a .csv filename".into());}
                let (bytes,contract)=render_table(object.get("spec").ok_or("[tool.invalid_input] missing table spec")?,cancelled,deadline)?;
                (bytes,Some("text/csv"),Some(contract))
            }
            Some("pdf") => {
                if !name.to_ascii_lowercase().ends_with(".pdf") { return Err("[tool.invalid_input] savePdf requires a .pdf filename".into()); }
                let pdf = crate::report_pdf::render_pdf(object.get("spec").ok_or("[tool.invalid_input] missing PDF spec")?, cancelled, deadline)?;
                (pdf.bytes, Some("application/pdf"),Some(pdf.data_contract))
            }
            Some("chart") => {
                if !name.to_ascii_lowercase().ends_with(".svg") { return Err("[tool.invalid_input] saveChart requires a .svg filename".into()); }
                let chart=crate::report_pdf::render_chart_output(object.get("spec").ok_or("[tool.invalid_input] missing chart spec")?)?;
                (chart.bytes,Some("image/svg+xml"),Some(chart.data_contract))
            }
            Some(_) => return Err("[tool.invalid_input] unknown generated file format".into()),
            None => (object.get("content").and_then(Value::as_str).ok_or_else(|| "saved file content must be a string or JSON value".to_owned())?.as_bytes().to_vec(), None,None),
        };
        if let Some(contract)=data_contract.as_ref() {
            let size=serde_json::to_vec(contract).map_err(|_|"[tool.invalid_input] dataContract encoding failed")?.len().saturating_add(128);
            contract_bytes=contract_bytes.saturating_add(size);
            if size>512*1024 || contract_bytes>1024*1024 {return Err("[tool.invalid_input] dataContract exceeds 512 KiB/file or 1 MiB/call".into());}
        }
        if content.len() > MAX_FILE_BYTES {
            return Err(format!(
                "output file '{name}' exceeds {} bytes",
                MAX_FILE_BYTES
            ));
        }
        total = total
            .checked_add(content.len())
            .ok_or_else(|| "output size overflowed".to_owned())?;
        if total > MAX_TOTAL_OUTPUT_BYTES {
            return Err(format!(
                "data-compute outputs exceed {} bytes",
                MAX_TOTAL_OUTPUT_BYTES
            ));
        }
        let media_type = generated_media_type.map(str::to_owned).unwrap_or_else(|| object
            .get("mediaType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_media_type(&name).to_owned()));
        if media_type.len() > MAX_FILE_MEDIA_TYPE_BYTES
            || media_type.chars().any(|character| character.is_control())
        {
            return Err(format!("output file '{name}' has an invalid media type"));
        }
        specs.push(OutputSpec {
            name,
            content,
            media_type,
            data_contract,
        });
    }
    Ok(specs)
}

pub(crate) fn write_outputs(output_root: &Path, specs: Vec<OutputSpec>) -> Result<Vec<Value>, String> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let root = prepare_output_root(output_root)?;
    let mut files = Vec::with_capacity(specs.len());
    for spec in specs {
        let path = root.join(&spec.name);
        if !path.starts_with(&root) {
            return Err("output file escaped the controlled workspace".to_owned());
        }
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if is_reparse_point(&metadata) {
                return Err(format!("output file '{}' is a link", spec.name));
            }
            return Err(format!("output file '{}' already exists", spec.name));
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("Unable to create output file '{}': {error}", spec.name))?;
        write_file(&mut file, &spec.content, &spec.name)?;
        let bytes = spec.content.len();
        let sha256 = hex::encode(Sha256::digest(&spec.content));
        let mut data_contract=spec.data_contract;
        if let Some(contract)=data_contract.as_mut() {contract["renderedHash"]=json!(sha256);}
        files.push(json!({
            "path": path.to_string_lossy().into_owned(),
            "displayName": spec.name,
            "artifactType": "created_file",
            "bytes": bytes,
            "mediaType": spec.media_type,
            "sha256": sha256,
            "renderedHash": sha256,
            "dataContract":data_contract,
        }));
    }
    Ok(files)
}

/// Store a result too large to return inline and build the bounded summary the
/// model receives instead.
///
/// The complete value is written to the compute workspace as a JSON artifact —
/// the same store `saveFile` uses, resolved later through the same
/// `compute-artifact:` id — so nothing is truncated and nothing is lost. The
/// model gets row/column counts, a byte-bounded sample and the stable
/// reference, which is enough to decide the next step.
///
/// The auto-spill file is counted together with the ordinary `saveFile`
/// outputs against the file-count, per-file and cumulative-byte budgets, and
/// all checks happen before the reference is published. A reference that
/// cannot be read back is worse than no reference, so any storage failure is
/// returned as an error instead of a partial success.
fn spill_large_result(
    output_root: &Path,
    result: Value,
    result_bytes: &[u8],
    files: &mut Vec<Value>,
) -> Result<Value, String> {
    // Prefer a name the caller is unlikely to have used; never silently
    // overwrite an existing saved file. Compare case-insensitively because
    // Windows filenames are case-insensitive, so "Large-Result.json" would
    // otherwise collide on disk.
    let mut index = 1;
    let name = loop {
        let candidate = if index == 1 {
            "large-result.json".to_owned()
        } else {
            format!("large-result-{index}.json")
        };
        let used = files.iter().any(|file| {
            file["displayName"]
                .as_str()
                .map(|name| name.eq_ignore_ascii_case(&candidate))
                .unwrap_or(false)
        });
        if !used {
            break candidate;
        }
        index += 1;
        if index > 64 {
            return Err(format!(
                "data-compute result is {} bytes and the spill name space is exhausted; rename saved files and retry",
                result_bytes.len()
            ));
        }
    };
    let content = serde_json::to_vec(&result)
        .map_err(|error| format!("Unable to serialize the large result: {error}"))?;
    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "data-compute result is {} bytes, over the {} byte artifact limit; return a summary and save the payload with saveFile instead",
            content.len(),
            MAX_FILE_BYTES
        ));
    }
    // Unified output budget: the spill counts with every ordinary saveFile
    // output, for both file count and cumulative bytes. Validated BEFORE the
    // file or its reference is published; limits are never raised.
    if files.len() + 1 > MAX_FILES {
        return Err(format!(
            "data-compute may create at most {MAX_FILES} files including the stored large result; combine outputs and retry"
        ));
    }
    let existing_bytes: usize = files
        .iter()
        .filter_map(|file| file["bytes"].as_u64())
        .try_fold(0usize, |total, bytes| total.checked_add(bytes as usize))
        .ok_or_else(|| "output size overflowed".to_owned())?;
    let combined_bytes = existing_bytes
        .checked_add(content.len())
        .ok_or_else(|| "output size overflowed".to_owned())?;
    if combined_bytes > MAX_TOTAL_OUTPUT_BYTES {
        return Err(format!(
            "data-compute outputs total {combined_bytes} bytes including the stored large result, exceeding {MAX_TOTAL_OUTPUT_BYTES}; split the computation and retry"
        ));
    }
    let specs = vec![OutputSpec {
        name: name.clone(),
        content,
        media_type: "application/json".to_owned(),
        data_contract: None,
    }];
    let mut written = write_outputs(output_root, specs)?;
    let artifact = written
        .pop()
        .ok_or_else(|| "data-compute failed to store the large result".to_owned())?;
    let reference = artifact["path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(Database::computed_artifact_id)
        .ok_or_else(|| "data-compute stored the large result without a resolvable path".to_string())?;

    // Build the bounded summary, shrinking the sample byte budget until the
    // ENTIRE envelope (not just one field) fits the inline result limit. An
    // empty sample always fits, so this loop terminates.
    let mut sample_budget = SAMPLE_MAX_BYTES;
    let envelope = loop {
        let shape = result_shape(&result, sample_budget);
        let candidate = json!({
            "summary": {
                "storedBytes": result_bytes.len(),
                "rows": shape.rows,
                "columns": shape.columns,
                "sampleRows": shape.sample_rows,
                "sampleTruncated": shape.sample_truncated,
                // The full value is in the artifact. The inline payload is a
                // sample and must never be read as a complete result.
                "complete": false,
                "storedAs": "compute-artifact",
                "artifactId": reference,
                "artifactName": name,
                "note": format!("结果为 {} 字节，超过单次返回上限 {} 字节，已完整保存为计算产物（未截断）；不要重新生成或逐行抄写。导入 Excel：先在计算代码里用 saveFile('表.csv', csvText) 保存 CSV/TSV，再把该文件的 files[].id 传给 office_import_data(artifactId)。本 JSON 产物请用后续 attachment_compute(artifactIds:[id]) 读回继续计算或转换，不要当作 CSV 直接导入；read_tool_result 不读取 compute-artifact 引用", result_bytes.len(), MAX_RESULT_BYTES),
            },
            "resultBytes": result_bytes.len(),
            "resultLimitBytes": MAX_RESULT_BYTES,
        });
        let envelope_len = serde_json::to_vec(&candidate).map(|b| b.len()).unwrap_or(usize::MAX);
        if envelope_len <= MAX_RESULT_BYTES || sample_budget == 0 {
            if envelope_len > MAX_RESULT_BYTES {
                return Err(
                    "data-compute large-result summary could not be bounded to the inline limit"
                        .to_owned(),
                );
            }
            break candidate;
        }
        sample_budget /= 2;
    };

    // The spilled file is a first-class output: it is registered with the same
    // path/bytes/sha256 record the Host turns into a compute artifact, so the
    // reference handed to the model resolves through the ordinary read path.
    files.push(artifact);
    Ok(envelope)
}

/// Row/column shape of a tabular result plus a sample bounded by actual
/// serialized UTF-8 bytes. Non-tabular results still get byte counts and a
/// textual preview from the caller.
///
/// `sample_budget` caps the whole `sampleRows` JSON array. A single oversized
/// first row/cell/key (including long CJK or emoji strings) is reduced to fit
/// instead of being copied whole; reduction is flagged with
/// `sample_truncated`.
fn result_shape(result: &Value, sample_budget: usize) -> ResultShape {
    let rows = match result {
        Value::Array(items) => items,
        _ => {
            return ResultShape {
                rows: None,
                columns: None,
                sample_rows: Vec::new(),
                sample_truncated: false,
            }
        }
    };
    let columns = rows
        .iter()
        .map(|row| match row {
            Value::Array(cells) => cells.len(),
            Value::Object(map) => map.len(),
            _ => 1,
        })
        .max()
        .unwrap_or(0);
    let mut sample: Vec<Value> = Vec::new();
    let mut sample_truncated = false;
    // Two bytes for the enclosing [ ].
    let mut used = 2usize;
    for row in rows.iter().take(SAMPLE_MAX_ROWS) {
        let separator = if sample.is_empty() { 0 } else { 1 };
        // Reserve the separator and the closing bracket.
        let available = match sample_budget.checked_sub(used + separator + 1) {
            Some(value) => value,
            None => {
                sample_truncated = true;
                break;
            }
        };
        if available < 2 {
            sample_truncated = true;
            break;
        }
        let (candidate, reduced) = bounded_sample_value(row, available);
        let length = json_len(&candidate);
        if used + separator + length + 1 > sample_budget {
            sample_truncated = true;
            break;
        }
        used += separator + length;
        if reduced {
            sample_truncated = true;
        }
        sample.push(candidate);
    }
    if rows.len() > sample.len() {
        sample_truncated = true;
    }
    ResultShape {
        rows: Some(rows.len()),
        columns: Some(columns),
        sample_rows: sample,
        sample_truncated,
    }
}

/// Serialized UTF-8 length of a JSON value.
fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Reduce any JSON value so its serialized form fits within `budget` UTF-8
/// bytes. Returns the value and whether it was reduced. Bounds are measured in
/// actual serialized bytes, so a long first row, one oversized cell, a long
/// object key, CJK and emoji are all handled identically. The result is always
/// valid JSON; the complete value lives in the stored artifact.
fn bounded_sample_value(value: &Value, budget: usize) -> (Value, bool) {
    if json_len(value) <= budget {
        return (value.clone(), false);
    }
    let mut reduced = reduce_sample_value(value, budget);
    // String escaping can overshoot the target after a cut; shrink until the
    // measured serialized length really fits.
    let mut remaining = budget;
    for _ in 0..24 {
        if json_len(&reduced) <= budget {
            return (reduced, true);
        }
        if remaining < 64 {
            break;
        }
        remaining -= 64;
        reduced = reduce_sample_value(value, remaining);
    }
    // Absolute backstop valid for any budget >= 2.
    if budget >= 2 {
        (Value::String(String::new()), true)
    } else {
        (Value::Null, true)
    }
}

/// One best-effort reduction pass to the byte budget; callers re-measure.
fn reduce_sample_value(value: &Value, budget: usize) -> Value {
    if budget < 2 {
        return Value::Null;
    }
    match value {
        Value::String(text) => Value::String(truncate_sample_string(text, budget)),
        Value::Array(items) => {
            let mut out: Vec<Value> = Vec::new();
            let mut used = 2usize; // [ ]
            for item in items {
                let separator = if out.is_empty() { 0 } else { 1 };
                let available = match budget.checked_sub(used + separator + 1) {
                    Some(value) => value,
                    None => break,
                };
                if available < 2 {
                    break;
                }
                let candidate = bounded_sample_value(item, available).0;
                let length = json_len(&candidate);
                if used + separator + length + 1 > budget {
                    break;
                }
                used += separator + length;
                out.push(candidate);
            }
            let omitted = items.len().saturating_sub(out.len());
            if omitted > 0 {
                let marker = Value::String(format!("…{omitted} more"));
                let separator = if out.is_empty() { 0 } else { 1 };
                if used + separator + json_len(&marker) + 1 <= budget {
                    out.push(marker);
                }
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut used = 2usize; // { }
            let mut dropped = 0usize;
            for (key, item) in map {
                let separator = if out.is_empty() { 0 } else { 1 };
                let available_for_key = budget.saturating_sub(used + separator + 4);
                let key_string = truncate_sample_string(key, available_for_key);
                let key_json_len = key_string.len() + 2; // surrounding quotes
                let available = match budget
                    .checked_sub(used + separator + key_json_len + 1 /* : */ + 1 /* } */)
                {
                    Some(value) => value,
                    None => {
                        dropped += 1;
                        continue;
                    }
                };
                if available < 2 {
                    dropped += 1;
                    continue;
                }
                let candidate = bounded_sample_value(item, available).0;
                let value_len = json_len(&candidate);
                if used + separator + key_json_len + 1 + value_len + 1 > budget {
                    dropped += 1;
                    continue;
                }
                used += separator + key_json_len + 1 + value_len;
                out.insert(key_string, candidate);
            }
            if dropped > 0 {
                let separator = if out.is_empty() { 0 } else { 1 };
                const MARKER_KEY: &str = "_truncated";
                let marker = Value::String(format!("{dropped} omitted"));
                let cost = separator + MARKER_KEY.len() + 2 + 1 + json_len(&marker);
                if used + cost + 1 <= budget {
                    out.insert(MARKER_KEY.to_owned(), marker);
                }
            }
            Value::Object(out)
        }
        other => {
            if json_len(other) <= budget {
                other.clone()
            } else if budget >= 2 {
                Value::String(String::new())
            } else {
                Value::Null
            }
        }
    }
}

/// Truncate a string so its JSON serialization (with quotes) fits `budget`
/// UTF-8 bytes, cutting only at char boundaries so CJK and emoji stay valid.
fn truncate_sample_string(text: &str, budget: usize) -> String {
    const TAIL: &str = "…";
    if serde_json::to_string(text)
        .map(|encoded| encoded.len() <= budget)
        .unwrap_or(false)
    {
        return text.to_owned();
    }
    let content_budget = budget.saturating_sub(2); // quotes
    let body_budget = content_budget.saturating_sub(TAIL.len());
    let mut end = 0usize;
    for (index, character) in text.char_indices() {
        if index + character.len_utf8() > body_budget {
            break;
        }
        end = index + character.len_utf8();
    }
    loop {
        let candidate = format!("{}{}", &text[..end], TAIL);
        if serde_json::to_string(&candidate)
            .map(|encoded| encoded.len() <= budget)
            .unwrap_or(false)
        {
            return candidate;
        }
        // Step the body back one whole character and rebuild.
        let previous = text[..end].char_indices().next_back().map(|(i, _)| i);
        match previous {
            Some(index) if index < end => end = index,
            _ => return if budget >= 2 + TAIL.len() { TAIL.to_owned() } else { String::new() },
        }
    }
}

struct ResultShape {
    rows: Option<usize>,
    columns: Option<usize>,
    sample_rows: Vec<Value>,
    sample_truncated: bool,
}

fn prepare_output_root(output_root: &Path) -> Result<PathBuf, String> {
    validate_no_reparse_components(output_root)?;
    if !output_root.is_absolute() {
        return Err("output_root must be an absolute path".to_owned());
    }
    if output_root.exists() {
        validate_no_reparse_components(output_root)?;
        let metadata = fs::symlink_metadata(output_root)
            .map_err(|error| format!("Unable to inspect output workspace: {error}"))?;
        if is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err("output_root must be a regular directory".to_owned());
        }
    } else {
        fs::create_dir_all(output_root)
            .map_err(|error| format!("Unable to create output workspace: {error}"))?;
    }
    validate_no_reparse_components(output_root)?;
    let canonical = fs::canonicalize(output_root)
        .map_err(|error| format!("Unable to resolve output workspace: {error}"))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|error| format!("Unable to inspect output workspace: {error}"))?;
    if is_reparse_point(&metadata) || !metadata.is_dir() {
        return Err("output_root must resolve to a regular directory".to_owned());
    }
    Ok(canonical)
}

fn write_file(file: &mut File, bytes: &[u8], name: &str) -> Result<(), String> {
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|error| format!("Unable to write output file '{name}': {error}"))
}

fn validate_output_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("saved file name is empty".to_owned());
    }
    if name.len() > MAX_FILE_NAME_BYTES || name.chars().count() > MAX_FILENAME_CHARS {
        return Err("saved file name is too long".to_owned());
    }
    if Path::new(name).is_absolute()
        || name.contains(['/', '\\', ':'])
        || name.chars().any(|character| character.is_control())
    {
        return Err(format!(
            "saved file name '{name}' is not a simple file name"
        ));
    }
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(format!("saved file name '{name}' contains traversal"));
    }
    if name == "." || name == ".." || name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("saved file name '{name}' is not valid on Windows"));
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
    {
        return Err(format!("saved file name '{name}' is reserved"));
    }
    Ok(name.to_owned())
}

fn default_media_type(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => "application/json",
        "csv" => "text/csv",
        "svg" => "image/svg+xml",
        "html" | "htm" => "text/html",
        "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn build_script(code: &str) -> (String, usize) {
    let mut script = String::with_capacity(code.len() + 1_500);
    script.push_str(
        r#"(() => {
  const attachments = JSON.parse(__foxAttachmentPayload);
  const __foxSavedFiles = [];
  const saveFile = (name, content, mediaType) => {
    const outputName = String(name);
    let outputContent;
    if (typeof content === "string") {
      outputContent = content;
    } else {
      outputContent = JSON.stringify(content);
      if (outputContent === undefined) outputContent = "null";
    }
    if (outputName.length === 0) throw new Error("saveFile name is empty");
    if (outputContent.length > __foxMaxFileChars) throw new Error("saveFile content is too large");
    __foxSavedFiles.push({
      name: outputName,
      content: outputContent,
      mediaType: mediaType === undefined || mediaType === null ? null : String(mediaType),
    });
    return { name: outputName };
  };
  globalThis.attachments = attachments;
  globalThis.saveFile = saveFile;
  globalThis.savePdf = (name, spec) => { __foxSavedFiles.push({name: String(name), format: "pdf", spec}); return {name: String(name)}; };
  globalThis.saveChart = (name, spec) => { __foxSavedFiles.push({name: String(name), format: "chart", spec}); return {name: String(name)}; };
"#,
    );
    script.push_str(TABLE_HELPERS);
    script.push_str(
        r#"
  const result = (() => {
    "use strict";
"#,
    );
    let line_offset = script.bytes().filter(|byte| *byte == b'\n').count();
    script.push_str(code);
    script.push_str(
        r#"
  })();
  if (result !== null && result !== undefined && typeof result.then === "function") {
    throw new Error("asynchronous JavaScript results are not supported");
  }
  return JSON.stringify({ result: result === undefined ? null : result, files: __foxSavedFiles });
})()"#,
    );
    (script, line_offset)
}

pub(crate) fn validate_input_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("attachment path must be absolute".to_owned());
    }
    validate_no_reparse_components(path)
}

fn validate_no_reparse_components(path: &Path) -> Result<(), String> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if let Ok(metadata) = fs::symlink_metadata(candidate) {
            if is_reparse_point(&metadata) {
                return Err(format!(
                    "path contains a symbolic/reparse link: {}",
                    candidate.display()
                ));
            }
        }
        current = candidate.parent();
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // R3B behavior cases are authored only; this card does not run them.
    #[test]
    fn r3b_explicit_dates_reject_objects_and_invalid_calendar_values() {
        assert_eq!(normalize_date_key(&json!("2024-02-29")).unwrap(),"2024-02-29");
        assert_eq!(normalize_date_key(&json!({"type":"date","value":"2024-03-01T00:30:00+02:00"})).unwrap(),"2024-02-29");
        assert_eq!(normalize_date_key(&json!("2024-03-01T00:30:00.000")).unwrap(),"2024-03-01");
        for value in [json!({"day":"2024-02-29"}),json!("2023-02-29"),json!(45351),json!("tomorrow"),Value::Null] {
            assert!(normalize_date_key(&value).is_err());
        }
    }

    #[test]
    fn r3b_native_excel_dates_use_actual_epoch_and_leave_duration_and_text_unchanged() {
        use calamine::{ExcelDateTime,ExcelDateTimeType};
        let mut budget=LoadBudget::default();
        let epoch1900=cell_to_value(&Data::DateTime(ExcelDateTime::new(25569.0,ExcelDateTimeType::DateTime,false)),&mut budget).unwrap();
        let epoch1904=cell_to_value(&Data::DateTime(ExcelDateTime::new(0.0,ExcelDateTimeType::DateTime,true)),&mut budget).unwrap();
        assert_eq!(epoch1900,"1970-01-01T00:00:00.000");
        assert_eq!(epoch1904,"1904-01-01T00:00:00.000");
        assert_eq!(cell_to_value(&Data::DateTime(ExcelDateTime::new(0.5,ExcelDateTimeType::TimeDelta,false)),&mut budget).unwrap(),"0.5");
        assert_eq!(cell_to_value(&Data::String("25569".into()),&mut budget).unwrap(),"25569");
        assert!(cell_to_value(&Data::DateTime(ExcelDateTime::new(-1.0,ExcelDateTimeType::DateTime,true)),&mut budget).is_err());
        assert_eq!(cell_to_value(&Data::DateTime(ExcelDateTime::new(-1.0,ExcelDateTimeType::TimeDelta,true)),&mut budget).unwrap(),"-1");
        assert_eq!(cell_to_value(&Data::Float(-1.0),&mut budget).unwrap(),json!(-1.0));
        assert!(cell_to_value(&Data::DateTime(ExcelDateTime::new(60.0,ExcelDateTimeType::DateTime,false)),&mut budget).is_err());
    }

    #[test]
    fn r3b_table_export_rejects_object_keys_duplicates_and_allows_nonkey_null() {
        let deadline=Instant::now()+Duration::from_secs(1);
        for rows in [json!([[{"day":"2024-02-29"},2]]),json!([[null,2]]),json!([["a",2],["a",3]])] {
            assert!(render_table(&json!({"columns":["key","amount"],"keyColumns":["key"],"rows":rows}),&||false,deadline).is_err());
        }
        let (bytes,contract)=render_table(&json!({"columns":["day","note"],"keyColumns":["day"],"dateColumns":["day"],"rows":[["2024-02-29",null]]}),&||false,deadline).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(),"day,note\n2024-02-29,\n");
        assert_eq!(contract["sourceVerified"],false);
        assert_eq!(contract["nullPolicy"],"empty");
    }

    #[test]
    fn r3b_whole_table_helpers_hash_actual_csv_and_preserve_plain_text() {
        let (root,paths)=super::execution_tests::fixture();
        let response=execute(&paths,&root.join("table"),&json!({"code":r#"
            let rejected=false; try {scalarKey({day:'2024-02-29'});} catch {rejected=true;}
            if(!rejected) throw new Error('object key was accepted');
            saveTable('dates.csv',{columns:['day','note'],keyColumns:['day'],dateColumns:['day'],rows:[[new Date('2024-02-29T12:00:00Z'),'a,b']]});
            saveFile('literal.txt','[object Object] is valid literal text');
            return {key:dateKey({type:'date',value:'2024-02-29'})};
        "#}),||false).unwrap();
        let csv=&response["files"][0];let bytes=fs::read(csv["path"].as_str().unwrap()).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&bytes)),csv["renderedHash"].as_str().unwrap());
        assert_eq!(csv["dataContract"]["renderedHash"],csv["sha256"]);
        assert_eq!(csv["dataContract"]["dateColumns"],json!(["day"]));
        assert_eq!(String::from_utf8(bytes).unwrap(),"day,note\n2024-02-29,\"a,b\"\n");
        assert!(fs::read_to_string(response["files"][1]["path"].as_str().unwrap()).unwrap().contains("[object Object]"));
    }

    #[test]
    fn output_name_rejects_traversal_absolute_and_alternate_streams() {
        for name in [
            "../escape.json",
            r"C:\\escape.json",
            "/tmp/escape",
            "report.json:secret",
            "..",
        ] {
            assert!(validate_output_name(name).is_err(), "accepted {name:?}");
        }
        assert!(validate_output_name("report.json").is_ok());
    }

    #[test]
    fn output_name_rejects_reserved_windows_devices() {
        for name in ["CON", "con.txt", "LPT1.csv", "COM9.json"] {
            assert!(validate_output_name(name).is_err(), "accepted {name:?}");
        }
        assert!(validate_output_name("COM0.json").is_ok());
    }

    #[test]
    fn csv_reader_keeps_quoted_fields_and_bounds_rows() {
        let mut budget = LoadBudget::default();
        let rows = parse_delimited(
            "Kind,Value\nA,2\n\"B, x\",4\n",
            b',',
            &mut budget,
            &|| false,
            Instant::now() + Duration::from_secs(1),
        )
        .expect("valid CSV");
        assert_eq!(rows[2][0], json!("B, x"));
        assert_eq!(rows[1][1], json!("2"));
        assert_eq!(budget.cells, 6);
    }

    #[test]
    fn timeout_defaults_and_clamps_to_thirty_seconds() {
        assert_eq!(
            requested_timeout(&json!({})).unwrap(),
            Duration::from_secs(15)
        );
        assert_eq!(
            requested_timeout(&json!({"timeoutMs": 90_000})).unwrap(),
            Duration::from_secs(30)
        );
        assert!(requested_timeout(&json!({"timeoutMs": -1})).is_err());
    }

    #[test]
    fn script_wraps_explicit_return_and_save_file_collection() {
        let (script, _) = build_script("return {ok: true};");
        assert!(script.contains("const attachments = JSON.parse"));
        assert!(script.contains("const saveFile"));
        assert!(script.contains("return {ok: true};"));
        assert!(script.contains("__foxSavedFiles"));
    }
}

#[cfg(test)]
mod execution_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    // Shared with the `tests` module above, whose R3b table-export case drives
    // the same JavaScript execution harness.
    pub(super) fn fixture() -> (PathBuf, Vec<(String, PathBuf)>) {
        let root = std::env::temp_dir().join(format!("fox-js-execution-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("data.csv");
        fs::write(&source, "id,value\nA,1\nB,2\n").unwrap();
        (root, vec![("a".into(), source)])
    }
    #[test]
    fn compute_exec_rejects_timeout_cancel_and_async_results() {
        let (root, paths) = fixture();
        let started = Instant::now();
        let error = execute(
            &paths,
            &root.join("timeout"),
            &json!({"code":"for(;;){}","timeoutMs":10}),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            worker_flag.store(true, Ordering::SeqCst);
        });
        let error = execute(
            &paths,
            &root.join("cancel"),
            &json!({"code":"for(;;){}"}),
            move || flag.load(Ordering::SeqCst),
        )
        .unwrap_err();
        worker.join().unwrap();
        assert!(error.contains("cancelled"), "{error}");
        let error = execute(
            &paths,
            &root.join("async"),
            &json!({"code":"return Promise.resolve(3);"}),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("asynchronous"), "{error}");
    }
    #[test]
    fn compute_exec_reports_user_code_line_and_accepts_quoted_key_correction() {
        let (root, paths) = fixture();
        let code = "\nconst rows = attachments[0].sheets[0].rows;\nreturn {\n  含\"为\"字样异常条数: rows.length - 1\n};";
        let error = execute(&paths, &root.join("bad"), &json!({"code": code}), || false).unwrap_err();
        assert!(error.contains("Syntax error"), "{error}");
        assert!(error.contains("code line 4 (1-based"), "{error}");
        assert!(error.contains("4:   含\"为\"字样异常条数:"), "{error}");
        assert!(error.contains("NEW attachment_compute call"), "{error}");
        assert!(!root.join("bad").exists());
        let corrected = code.replace("含\"为\"字样异常条数:", "'含\"为\"字样异常条数':");
        let result = execute(&paths, &root.join("good"), &json!({"code": corrected}), || false).unwrap();
        assert_eq!(result["result"]["含\"为\"字样异常条数"], 2);

        let error = execute(&paths, &root.join("runtime-error"), &json!({"code":
            "saveFile('partial.json', '{}');\nconst count = missing_column;\nreturn count;"}), || false).unwrap_err();
        assert!(error.contains("near code line"), "{error}");
        assert!(error.contains("2: const count = missing_column;"), "{error}");
        assert!(!error.contains("Syntax error"), "{error}");
        assert!(!root.join("runtime-error").exists());
    }

    #[test]
    fn compute_exec_errors_are_actionable_and_large_results_are_bounded() {
        let (root, paths) = fixture();
        let error = execute(
            &paths,
            &root.join("error"),
            &json!({"code":"return missing_column_name;"}),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("missing_column_name"), "{error}");
        assert!(error.len() < 4096);
        let error = execute(
            &paths,
            &root.join("large-error"),
            &json!({"code":"throw new Error('x'.repeat(100000));"}),
            || false,
        )
        .unwrap_err();
        assert!(error.len() < 4096);
        // A result too large to return inline is stored whole as a compute
        // artifact; the model receives a bounded, actionable summary and the
        // stable reference instead of a hard error. Nothing is truncated, and
        // the summary never claims the inline payload is complete.
        let spilled = execute(
            &paths,
            &root.join("large-result"),
            &json!({"code":"return 'x'.repeat(70000);"}),
            || false,
        )
        .unwrap();
        let summary = &spilled["result"]["summary"];
        assert_eq!(summary["storedBytes"], 70002);
        assert_eq!(summary["complete"], false);
        assert_eq!(summary["storedAs"], "compute-artifact");
        let artifact_id = summary["artifactId"]
            .as_str()
            .expect("a stable compute-artifact reference");
        assert!(artifact_id.starts_with("compute-artifact:"), "{artifact_id}");
        assert_eq!(summary["artifactName"], "large-result.json");
        assert!(
            summary["note"].as_str().unwrap().contains("office_import_data"),
            "the note must name the executable next step"
        );
        assert_eq!(spilled["files"].as_array().unwrap().len(), 1);
        let stored = &spilled["files"][0];
        assert_eq!(stored["displayName"], "large-result.json");
        assert_eq!(stored["bytes"], 70002);
        assert_eq!(stored["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(stored["mediaType"], "application/json");
        // The full payload really is on disk at the referenced path.
        let stored_path = std::path::Path::new(stored["path"].as_str().unwrap());
        let on_disk = fs::read(stored_path).unwrap();
        assert_eq!(on_disk.len(), 70002);
        // The reference the model receives must resolve to exactly this file:
        // same stable id the Host assigns, and the registered SHA-256 matches.
        assert_eq!(
            summary["artifactId"].as_str().unwrap(),
            &Database::computed_artifact_id(stored["path"].as_str().unwrap()),
            "the summary reference must point at the stored file"
        );
        assert_eq!(
            hex::encode(Sha256::digest(&on_disk)),
            stored["sha256"].as_str().unwrap()
        );
        // Another oversized result in a different workspace gets its own
        // deterministic name and its own reference; references never alias.
        let other = execute(
            &paths,
            &root.join("large-result-3"),
            &json!({"code":"return 'y'.repeat(70000);"}),
            || false,
        )
        .unwrap();
        let other_summary = &other["result"]["summary"];
        assert_ne!(
            other_summary["artifactId"].as_str().unwrap(),
            artifact_id
        );
        // Both references resolve to real, distinct, complete payloads.
        let other_stored = &other["files"][0];
        let other_path = std::path::Path::new(other_stored["path"].as_str().unwrap());
        assert_ne!(other_path, stored_path);
        assert_eq!(fs::read(other_path).unwrap().len(), 70002);
        assert_eq!(
            other_summary["artifactId"].as_str().unwrap(),
            &Database::computed_artifact_id(other_stored["path"].as_str().unwrap())
        );
        let result=execute(&paths,&root.join("globals"),&json!({"code":"return {process:typeof process,require:typeof require,fetch:typeof fetch};"}),||false).unwrap();
        assert_eq!(
            result["result"],
            json!({"process":"undefined","require":"undefined","fetch":"undefined"})
        );
    }

    /// The inline-result cap is a strict `>` boundary on the *serialized
    /// result field only. A result exactly at `MAX_RESULT_BYTES` stays inline
    /// (no summary, no spill); one byte over spills the whole value to an
    /// artifact and returns a bounded summary. This pins which layer the limit
    /// applies to — the `result` string, never the outer `files` response.
    #[test]
    fn result_boundary_is_exact_and_belongs_to_the_result_field() {
        let (root, paths) = fixture();
        // A JSON string serializes as N ASCII chars + 2 quotes.
        let at_limit = execute(
            &paths,
            &root.join("at-limit"),
            &json!({"code": format!("return 'x'.repeat({});", MAX_RESULT_BYTES - 2)}),
            || false,
        )
        .unwrap();
        // Exactly at the cap: the raw string is the inline result, no spill.
        assert_eq!(
            at_limit["result"].as_str().map(str::len),
            Some(MAX_RESULT_BYTES - 2)
        );
        assert!(at_limit["result"].get("summary").is_none());
        assert_eq!(at_limit["files"].as_array().unwrap().len(), 0);

        let over = execute(
            &paths,
            &root.join("one-over"),
            &json!({"code": format!("return 'x'.repeat({});", MAX_RESULT_BYTES - 1)}),
            || false,
        )
        .unwrap();
        // One serialized byte over: full value stored, bounded summary inline.
        assert_eq!(over["result"]["summary"]["complete"], false);
        assert_eq!(over["result"]["summary"]["storedBytes"], MAX_RESULT_BYTES + 1);
        let stored = &over["files"][0];
        assert_eq!(stored["displayName"], "large-result.json");
        assert_eq!(stored["bytes"], MAX_RESULT_BYTES + 1);
        // The bounded summary itself never exceeds the inline limit.
        let summary_len = serde_json::to_vec(&over["result"]).unwrap().len();
        assert!(summary_len <= MAX_RESULT_BYTES, "summary is {summary_len} bytes");
        // The whole value is really on disk, untruncated.
        let on_disk = fs::read(stored["path"].as_str().unwrap()).unwrap();
        assert_eq!(on_disk.len(), MAX_RESULT_BYTES + 1);
    }

    /// A multi-hundred-KiB `saveFile` output with a small return value proves
    /// the two layers are independent: the outer response carries the full
    /// file (referenced by its stable `files[].id`, bytes on disk) while the
    /// inline `result` stays small. The 64 KiB result cap never truncates a
    /// saved artifact, and the id is the same one `computed_artifacts::persist`
    /// registers and `office_import_data` later resolves.
    #[test]
    fn large_saved_file_is_not_subject_to_the_inline_result_cap() {
        let (root, paths) = fixture();
        let code = r#"
            let csv = 'id,tag,v\n';
            for (let i = 0; i < 6000; i += 1) {
              csv += i + ',agv-task-' + String(i).padStart(5,'0') + ',' + (10000 + i) + '\n';
            }
            saveFile('agv.csv', csv);
            return { rows: 6000, file: 'agv.csv' };
        "#;
        let response = execute(
            &paths,
            &root.join("large-file"),
            &json!({"code": code}),
            || false,
        )
        .unwrap();
        // The inline result is the small object — no spill summary.
        assert_eq!(response["result"]["rows"], 6000);
        assert!(response["result"].get("summary").is_none());
        assert_eq!(serde_json::to_vec(&response["result"]).unwrap().len() < MAX_RESULT_BYTES, true);

        let file = &response["files"][0];
        let bytes = file["bytes"].as_u64().unwrap();
        assert!(bytes > MAX_RESULT_BYTES as u64, "the saved CSV ({bytes} B) far exceeds the result cap");
        let path = std::path::Path::new(file["path"].as_str().unwrap());
        let on_disk = fs::read(path).unwrap();
        assert_eq!(on_disk.len() as u64, bytes, "the full artifact is on disk");
        assert_eq!(
            hex::encode(Sha256::digest(&on_disk)),
            file["sha256"].as_str().unwrap()
        );
        // The model-facing id is derived from the path, exactly what the Host
        // persistence writes and the Office importer resolves.
        assert_eq!(
            file.get("id"),
            None,
            "the low-level executor leaves id assignment to the Host wrapper"
        );
        assert_eq!(
            Database::computed_artifact_id(file["path"].as_str().unwrap()),
            Database::computed_artifact_id(path.to_str().unwrap())
        );
        // The full dataset really landed, header + 6000 data rows.
        assert_eq!(on_disk.iter().filter(|b| **b == b'\n').count(), 6001);
    }

    /// The sample must be bounded by actual serialized UTF-8 bytes even when
    /// the very first row is one enormous value. Covers a long ASCII first
    /// row, a single oversized object cell, a long object key, CJK and emoji.
    #[test]
    fn oversized_sample_is_byte_bounded_not_copied_whole() {
        let (root, paths) = fixture();
        // A reusable assertion: the complete value stays on disk with a
        // matching hash, while the returned envelope is byte-bounded.
        let check = |code: &str, name: &str| {
            let response = execute(
                &paths,
                &root.join(name),
                &json!({ "code": code }),
                || false,
            )
            .unwrap_or_else(|error| panic!("{name} failed: {error}"));
            let envelope = serde_json::to_vec(&response["result"]).unwrap();
            assert!(
                envelope.len() <= MAX_RESULT_BYTES,
                "{name}: returned result {} bytes exceeds {MAX_RESULT_BYTES}",
                envelope.len()
            );
            let summary = &response["result"]["summary"];
            assert_eq!(summary["complete"], false, "{name}");
            assert_eq!(summary["storedAs"], "compute-artifact", "{name}");
            assert_eq!(summary["sampleTruncated"], true, "{name}: a huge first row must flag the sample as truncated");
            assert!(
                serde_json::to_vec(&summary["sampleRows"]).unwrap().len() <= SAMPLE_MAX_BYTES + 4,
                "{name}: sample rows exceed the sample budget"
            );
            let stored = &response["files"][0];
            let on_disk = fs::read(stored["path"].as_str().unwrap()).unwrap();
            // The stored artifact is the complete result and its hash matches.
            let complete: Value = serde_json::from_slice(&on_disk).unwrap();
            assert_eq!(
                hex::encode(Sha256::digest(&on_disk)),
                stored["sha256"].as_str().unwrap(),
                "{name}: stored hash mismatch"
            );
            assert_eq!(
                summary["artifactId"].as_str().unwrap(),
                &Database::computed_artifact_id(stored["path"].as_str().unwrap()),
                "{name}: reference must resolve to the stored file"
            );
            complete
        };
        // 1) One giant first row (70k-char single-cell array).
        let array = check("return ['x'.repeat(70000)];", "huge-first-row");
        assert_eq!(array.as_array().unwrap().len(), 1);
        assert_eq!(array[0].as_str().unwrap().len(), 70_000);
        // 2) A huge value nested inside an object cell.
        let object = check(
            "return [{a:{b:'y'.repeat(70000)}}];",
            "huge-object-cell",
        );
        assert_eq!(object[0]["a"]["b"].as_str().unwrap().len(), 70_000);
        // 3) An oversized object KEY (not just a long value).
        let long_key_object = check(
            "return [{['k'.repeat(70000)]: 1}];",
            "huge-object-key",
        );
        assert!(long_key_object[0].as_object().unwrap().keys().next().unwrap().len() >= 70_000);
        // 4) CJK: each character is 3 UTF-8 bytes.
        let cjk = check("return ['中'.repeat(30000)];", "cjk");
        assert_eq!(cjk[0].as_str().unwrap().chars().count(), 30_000);
        // 5) Emoji: each is 4 UTF-8 bytes; cutting must not split a codepoint.
        let emoji_response = execute(
            &paths,
            &root.join("emoji"),
            &json!({"code":"return ['😀'.repeat(30000)];"}),
            || false,
        )
        .unwrap();
        let emoji_envelope = serde_json::to_vec(&emoji_response["result"]).unwrap();
        assert!(emoji_envelope.len() <= MAX_RESULT_BYTES);
        let emoji_summary = &emoji_response["result"]["summary"];
        // Every sampled string must remain valid UTF-8/JSON (it parsed above)
        // and not contain a replacement from a split codepoint.
        for row in emoji_summary["sampleRows"].as_array().unwrap() {
            let serialized = serde_json::to_string(row).unwrap();
            assert!(!serialized.contains('\u{FFFD}'), "emoji must not be split into replacement chars");
        }
        let emoji_stored = &emoji_response["files"][0];
        let emoji_complete: Value =
            serde_json::from_slice(&fs::read(emoji_stored["path"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(emoji_complete[0].as_str().unwrap().chars().count(), 30_000);
    }

    /// Ordinary saveFile outputs and the automatic spill file share one
    /// file-count / cumulative-byte budget, enforced before the reference is
    /// published; an in-budget combination succeeds.
    #[test]
    fn spill_shares_output_count_and_byte_budget() {
        let (root, paths) = fixture();
        // 32 ordinary files already saturate the file cap; adding the spill
        // (33rd) must fail rather than publish an unreachable reference.
        let many: String = (0..MAX_FILES)
            .map(|i| format!("saveFile('f{i}.txt','x');"))
            .collect();
        let code = format!("{many} return 'z'.repeat(70000);");
        let error = execute(
            &paths,
            &root.join("too-many"),
            &json!({ "code": code }),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("at most") || error.contains("files"), "{error}");
        assert!(!root.join("too-many").join("large-result.json").exists());
        // 31 ordinary files + the spill = 32 files: within the cap, succeeds.
        let few: String = (0..MAX_FILES - 1)
            .map(|i| format!("saveFile('g{i}.txt','x');"))
            .collect();
        let code = format!("{few} return 'z'.repeat(70000);");
        let response = execute(
            &paths,
            &root.join("in-budget"),
            &json!({ "code": code }),
            || false,
        )
        .unwrap();
        assert_eq!(response["files"].as_array().unwrap().len(), MAX_FILES);
        let total_bytes: u64 = response["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["bytes"].as_u64().unwrap())
            .sum();
        assert!(total_bytes <= MAX_TOTAL_OUTPUT_BYTES as u64);
        assert!(response["files"].as_array().unwrap().iter().any(
            |file| file["displayName"] == "large-result.json"
        ));
        // A case-insensitive name collision with the reserved spill file is
        // moved to a new name, never overwritten.
        let response = execute(
            &paths,
            &root.join("case-collision"),
            &json!({"code":"saveFile('LARGE-RESULT.JSON','mine'); return 'q'.repeat(70000);"}),
            || false,
        )
        .unwrap();
        let names: Vec<&str> = response["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["displayName"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"LARGE-RESULT.JSON"));
        assert!(names.contains(&"large-result-2.json"));
    }

    /// Cumulative-byte budget: a large ordinary file plus the spill must fail
    /// before publishing a success reference if together they exceed the cap.
    #[test]
    fn spill_is_refused_when_total_bytes_exceed_cap() {
        let (root, paths) = fixture();
        // Leave only 32 KiB of headroom; the 70 KB spill pushes past the cap.
        let ordinary_len = MAX_TOTAL_OUTPUT_BYTES - 32 * 1024;
        let code = format!(
            "saveFile('big.bin', 'a'.repeat({ordinary_len})); return 'z'.repeat(70000);"
        );
        let error = execute(
            &paths,
            &root.join("bytes-overflow"),
            &json!({ "code": code }),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("exceeding") || error.contains("bytes"), "{error}");
        assert!(
            !root.join("bytes-overflow").join("large-result.json").exists(),
            "no spill reference may be published when the budget is exceeded"
        );
    }
    #[test]
    fn large_result_spill_fails_explicitly_when_storage_unwritable() {
        let (root, paths) = fixture();
        // An output root that cannot be created (its parent is a regular file):
        // the spill must surface this as a hard error, never a silent drop or
        // a truncated "complete" result.
        fs::write(root.join("blocker"), b"not a directory").unwrap();
        let bad_root = root.join("blocker").join("out");
        let error = execute(
            &paths,
            &bad_root,
            &json!({"code":"return 'x'.repeat(70000);"}),
            || false,
        )
        .unwrap_err();
        assert!(
            !error.is_empty() && error.len() < 4096,
            "storage failure must be an explicit, bounded error: {error}"
        );
        // Nothing is claimed complete: no large-result artifact was written.
        assert!(
            !root.join("blocker").join("out").join("large-result.json").exists()
        );
    }
    #[test]
    fn compute_exec_saves_utf8_files_and_rejects_path_escape() {
        let (root, paths) = fixture();
        let source = fs::read(&paths[0].1).unwrap();
        for name in [
            "../escape.txt",
            "C:\\escape.txt",
            "file.txt:stream",
            "CON.txt",
        ] {
            let code = format!(
                "saveFile({}, 'unsafe'); return true;",
                serde_json::to_string(name).unwrap()
            );
            assert!(execute(
                &paths,
                &root.join("rejected"),
                &json!({"code":code}),
                || false
            )
            .is_err());
        }
        let result=execute(&paths,&root.join("valid"),&json!({"code":"saveFile('报告.json',JSON.stringify({value:'中文😀'})); return {rows:attachments[0].sheets[0].rows.length-1};"}),||false).unwrap();
        assert_eq!(result["result"]["rows"], 2);
        let file = &result["files"][0];
        let bytes = fs::read(file["path"].as_str().unwrap()).unwrap();
        assert_eq!(file["bytes"].as_u64(), Some(bytes.len() as u64));
        assert_eq!(file["sha256"], hex::encode(Sha256::digest(&bytes)));
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"value":"中文😀"})
        );
        assert_eq!(fs::read(&paths[0].1).unwrap(), source);
    }
}

#[cfg(test)]
mod sparse_allocation_tests {
    use super::*;
    #[test]
    fn compute_sparse_far_cells_are_rejected_before_dense_allocation() {
        let mut cells = vec![
            ((0, 0), Data::String("header".into())),
            ((1_048_575, 16_383), Data::Float(1.0)),
        ]
        .into_iter();
        let mut budget = LoadBudget::default();
        let result = bounded_cell_matrix(
            || Ok(cells.next()),
            &mut budget,
            &|| false,
            Instant::now() + Duration::from_secs(1),
        );
        assert!(result.unwrap_err().contains("cell span"));
        assert_eq!(budget.cells, 0);
    }
    #[test]
    fn compute_sparse_rows_preserve_offsets_and_cached_values() {
        let mut cells = vec![
            ((9, 2), Data::String("name".into())),
            ((10, 3), Data::Float(12.5)),
        ]
        .into_iter();
        let mut budget = LoadBudget::default();
        let matrix = bounded_cell_matrix(
            || Ok(cells.next()),
            &mut budget,
            &|| false,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            matrix,
            vec![
                vec![json!("name"), Value::Null],
                vec![Value::Null, json!(12.5)]
            ]
        );
        assert_eq!(budget.cells, 4);
    }

    /// Real production input, read-only: the actual AGV workbook the analyst
    /// flow reads must load in the QuickJS compute engine at full scale
    /// (2,213 source rows). This is the exact first tool step the cloud run
    /// performs. `#[ignore]` because it reads a workspace fixture; it never
    /// writes (no saveFile) and never opens the CLI.
    #[test]
    #[ignore = "reads the real AGV workbook fixture: tests/execl-ceshi"]
    fn real_agv_workbook_loads_in_compute() {
        let xlsx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/execl-ceshi/AGV长时间任务汇总统计表.xlsx");
        assert!(xlsx.exists(), "real AGV fixture present: {}", xlsx.display());
        let root = std::env::temp_dir().join(format!("fox-real-agv-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let response = execute(
            &[("a".into(), xlsx)],
            &root.join("read"),
            &json!({"code": r#"
                const sheets = attachments[0].sheets;
                const first = sheets[0];
                return { sheetCount: sheets.length, sheetName: first.name,
                         rows: first.rows.length, cols: first.rows[0].length,
                         header: first.rows[0] };
            "#}),
            || false,
        )
        .unwrap_or_else(|e| panic!("COMPUTE_ERR::{e}"));
        let res = &response["result"];
        println!("real AGV workbook -> {}", serde_json::to_string(res).unwrap());
        assert_eq!(res["sheetCount"].as_u64().unwrap() >= 1, true);
        // 2,213 data rows (+ header row in the parsed grid) per 2026-09 history.
        assert!(
            res["rows"].as_u64().unwrap() >= 2_213,
            "expected at least 2,213 source rows, got {}", res["rows"]
        );
        assert!(res["cols"].as_u64().unwrap() >= 1);
        // No files: read-only inspection does not spill.
        assert_eq!(response["files"].as_array().unwrap().len(), 0);
    }
}

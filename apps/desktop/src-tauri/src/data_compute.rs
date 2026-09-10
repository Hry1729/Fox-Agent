//! In-process, bounded JavaScript data computation for pre-authorized attachments.
//!
//! The host supplies already-authorized attachment paths.  This module reads
//! workbook/text data in Rust, serializes only that data into a fresh QuickJS
//! runtime, and collects files from a small `saveFile` function.  JavaScript
//! has no filesystem, process, module, or network bindings.  Files are
//! validated and written by Rust after the interpreter has finished.

use calamine::{open_workbook_auto_from_rs, Data, Reader, Sheets};
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
const MAX_JS_MEMORY_BYTES: usize = 128 * 1024 * 1024;
const MAX_JS_STACK_BYTES: usize = 512 * 1024;

const MAX_CODE_BYTES: usize = 128 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 16;
const MAX_SHEETS_PER_ATTACHMENT: usize = 64;
const MAX_SHEET_ROWS: usize = 250_000;
const MAX_SHEET_COLUMNS: usize = 512;
const MAX_TOTAL_CELLS: usize = 1_000_000;
const MAX_TOTAL_TEXT_BYTES: usize = 32 * 1024 * 1024;

const MAX_FILES: usize = 32;
const MAX_FILE_NAME_BYTES: usize = 240;
const MAX_FILE_MEDIA_TYPE_BYTES: usize = 128;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_SCRIPT_RESULT_BYTES: usize = 24 * 1024 * 1024;
const MAX_FILENAME_CHARS: usize = 120;

const INTERRUPT_CANCELLED: u8 = 1;
const INTERRUPT_TIMEOUT: u8 = 2;

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

    let script = build_script(code);
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
                Err(error) => Err(format_js_error(ctx, error)),
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
                    "data-compute timed out after {} ms",
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
            "data-compute timed out after {} ms",
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
    if result_bytes.len() > MAX_RESULT_BYTES {
        return Err(format!(
            "data-compute result exceeds {} bytes",
            MAX_RESULT_BYTES
        ));
    }

    let output_specs = parse_output_specs(envelope.get("files"))?;
    let files = write_outputs(output_root, output_specs)?;
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

fn check_load_limits(cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<(), String> {
    if cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }
    if Instant::now() >= deadline {
        return Err("data-compute timed out while loading attachments".to_owned());
    }
    Ok(())
}

fn format_js_error<'js>(ctx: Ctx<'js>, error: JsError) -> String {
    const MAX_ERROR_BYTES: usize = 2 * 1024;
    if !matches!(error, JsError::Exception) {
        return format!(
            "JavaScript execution failed: {}",
            truncate_text(&format!("{error:?}"), MAX_ERROR_BYTES)
        );
    }

    let thrown = ctx.catch();
    if let Ok(exception) = Exception::from_js(&ctx, thrown.clone()) {
        let message = exception
            .message()
            .unwrap_or_else(|| "JavaScript exception".to_owned());
        let stack = exception.stack().unwrap_or_default();
        let detail = if stack.is_empty() {
            message
        } else {
            format!("{message}\n{stack}")
        };
        return format!(
            "JavaScript execution failed: {}",
            truncate_text(&detail, MAX_ERROR_BYTES)
        );
    }
    match Coerced::<String>::from_js(&ctx, thrown) {
        Ok(value) => format!(
            "JavaScript execution failed: {}",
            truncate_text(&value.0, MAX_ERROR_BYTES)
        ),
        Err(_) => "JavaScript execution failed with a non-error exception".to_owned(),
    }
}

fn truncate_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn selected_attachment_ids(
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
struct LoadBudget {
    cells: usize,
    text_bytes: usize,
}

impl LoadBudget {
    fn add_cells(&mut self, count: usize) -> Result<(), String> {
        self.cells = self
            .cells
            .checked_add(count)
            .ok_or_else(|| "attachment cell count overflowed".to_owned())?;
        if self.cells > MAX_TOTAL_CELLS {
            return Err(format!(
                "attachments contain more than {} cells",
                MAX_TOTAL_CELLS
            ));
        }
        Ok(())
    }

    fn add_text(&mut self, count: usize) -> Result<(), String> {
        self.text_bytes = self
            .text_bytes
            .checked_add(count)
            .ok_or_else(|| "attachment text size overflowed".to_owned())?;
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(format!(
                "attachments contain more than {} bytes of text",
                MAX_TOTAL_TEXT_BYTES
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
fn validate_zip_expansion(name: &str, bytes: &[u8]) -> Result<(), String> {
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

fn cell_to_value(cell: &Data, budget: &mut LoadBudget) -> Result<Value, String> {
    let value = match cell {
        Data::Int(number) => json!(number),
        Data::Float(number) if number.is_finite() => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::Float(_) => Value::Null,
        Data::String(text) => {
            budget.add_text(text.len())?;
            Value::String(text.clone())
        }
        Data::Bool(value) => Value::Bool(*value),
        Data::DateTime(value) => {
            let text = value.to_string();
            budget.add_text(text.len())?;
            Value::String(text)
        }
        Data::DateTimeIso(value) | Data::DurationIso(value) => {
            budget.add_text(value.len())?;
            Value::String(value.to_string())
        }
        Data::Error(error) => {
            let text = format!("#ERROR:{error:?}");
            budget.add_text(text.len())?;
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
struct OutputSpec {
    name: String,
    content: Vec<u8>,
    media_type: String,
}

fn parse_output_specs(value: Option<&Value>) -> Result<Vec<OutputSpec>, String> {
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
    for value in values {
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
        let content = object
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "saved file content must be a string or JSON value".to_owned())?
            .as_bytes()
            .to_vec();
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
        let media_type = object
            .get("mediaType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_media_type(&name).to_owned());
        if media_type.len() > MAX_FILE_MEDIA_TYPE_BYTES
            || media_type.chars().any(|character| character.is_control())
        {
            return Err(format!("output file '{name}' has an invalid media type"));
        }
        specs.push(OutputSpec {
            name,
            content,
            media_type,
        });
    }
    Ok(specs)
}

fn write_outputs(output_root: &Path, specs: Vec<OutputSpec>) -> Result<Vec<Value>, String> {
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
        files.push(json!({
            "path": path.to_string_lossy().into_owned(),
            "displayName": spec.name,
            "artifactType": "created_file",
            "bytes": bytes,
            "mediaType": spec.media_type,
            "sha256": sha256,
        }));
    }
    Ok(files)
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

fn build_script(code: &str) -> String {
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
  const result = (() => {
    "use strict";
"#,
    );
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
    script
}

fn validate_input_path(path: &Path) -> Result<(), String> {
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
        let script = build_script("return {ok: true};");
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

    fn fixture() -> (PathBuf, Vec<(String, PathBuf)>) {
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
        let error = execute(
            &paths,
            &root.join("large-result"),
            &json!({"code":"return 'x'.repeat(70000);"}),
            || false,
        )
        .unwrap_err();
        assert!(error.contains("result exceeds"), "{error}");
        assert!(error.len() < 1024);
        let result=execute(&paths,&root.join("globals"),&json!({"code":"return {process:typeof process,require:typeof require,fetch:typeof fetch};"}),||false).unwrap();
        assert_eq!(
            result["result"],
            json!({"process":"undefined","require":"undefined","fetch":"undefined"})
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
}

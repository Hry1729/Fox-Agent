//! Chunked attachment computation (`processing: "chunked"`).
//!
//! Rows stream from the on-disk snapshot through the bounded QuickJS runtime
//! one chunk at a time. Nothing materializes a whole sheet in Rust, in a
//! single JSON string, or in the JavaScript heap:
//!
//! * delimited files (csv/tsv) are parsed from a buffered reader with
//!   quote-aware scanning — true streaming, the file is never fully loaded;
//! * workbooks (xlsx/xlsb) are walked twice with calamine's streaming cell
//!   reader: once to measure the extent (bounded by the active profile),
//!   once to emit dense row chunks. This is chunked delivery from the
//!   package, documented as such — legacy xls/ods have no streaming reader
//!   and are rejected with an explicit alternative.
//!
//! The JavaScript contract differs from whole mode: the code defines
//! `onChunk(chunk)` (called once per chunk) and optionally `onFinish()`
//! (its return value is the result). `attachments` carries metadata only
//! (`sheets[].rowCount`, never row data).

use super::{
    cell_to_value_capped, check_load_limits, format_js_error, parse_output_specs,
    selected_attachment_ids, validate_input_path, validate_zip_expansion, write_outputs,
    LoadBudget, INTERRUPT_CANCELLED, INTERRUPT_TIMEOUT, MAX_CODE_BYTES, MAX_FILE_BYTES,
    MAX_JS_MEMORY_BYTES, MAX_JS_STACK_BYTES, MAX_RESULT_BYTES, MAX_SCRIPT_RESULT_BYTES,
    MAX_SHEETS_PER_ATTACHMENT, MAX_TOTAL_OUTPUT_BYTES,
};
use calamine::{open_workbook_auto_from_rs, Reader, Sheets};
use rquickjs::{Context, Error as JsError, Function, Runtime};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::Instant,
};

/// Cells targeted per delivered chunk (rows-per-chunk = cap / sheet width).
const CHUNK_TARGET_CELLS: usize = 65_536;
/// Hard row cap per chunk even for narrow sheets.
const CHUNK_MAX_ROWS: usize = 8_192;
/// One serialized chunk must stay comfortably below the QuickJS heap limit.
const CHUNK_MAX_JSON_BYTES: usize = 12 * 1024 * 1024;

/// Controlled tier for attachment computation. `standard` reproduces the
/// long-standing synchronous limits; `large` raises row/cell/text bounds for
/// chunked jobs while the QuickJS memory/stack and output limits stay fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Profile {
    pub name: &'static str,
    pub max_sheet_rows: usize,
    pub max_sheet_columns: usize,
    pub max_total_cells: usize,
    pub max_total_text_bytes: usize,
    pub max_attachment_bytes: u64,
}

pub(crate) fn profile(name: Option<&str>) -> Result<Profile, String> {
    match name.unwrap_or("standard") {
        "standard" => Ok(Profile {
            name: "standard",
            max_sheet_rows: 250_000,
            max_sheet_columns: 512,
            max_total_cells: 1_000_000,
            max_total_text_bytes: 32 * 1024 * 1024,
            max_attachment_bytes: 32 * 1024 * 1024,
        }),
        "large" => Ok(Profile {
            name: "large",
            max_sheet_rows: 1_000_000,
            max_sheet_columns: 512,
            max_total_cells: 4_000_000,
            max_total_text_bytes: 64 * 1024 * 1024,
            max_attachment_bytes: 32 * 1024 * 1024,
        }),
        other => Err(format!(
            "data-compute profile '{other}' 不存在（可用：standard、large）"
        )),
    }
}

/// Progress reported after every delivered chunk; the job layer persists it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkProgress {
    pub rows_done: u64,
    pub rows_total: Option<u64>,
    pub phase: &'static str,
}

/// 0-based extent plan for one sheet; `rows == 0` marks an empty sheet.
#[derive(Debug)]
struct SheetPlan {
    name: String,
    top: u32,
    left: u32,
    rows: u32,
    columns: u32,
}

#[derive(Debug)]
struct AttachmentPlan {
    id: String,
    name: String,
    kind: String,
    path: PathBuf,
    sheets: Vec<SheetPlan>,
}

/// Execute user JavaScript in chunked mode. See module docs for the contract.
pub(crate) fn execute<F>(
    attachment_paths: &[(String, PathBuf)],
    output_root: &Path,
    input: &Value,
    cancelled: F,
    deadline: Instant,
    progress: &dyn Fn(ChunkProgress),
) -> Result<Value, String>
where
    F: Fn() -> bool + 'static,
{
    let started = Instant::now();
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
    let tier = profile(input.get("profile").and_then(Value::as_str))?;
    let cancelled: Arc<dyn Fn() -> bool> = Arc::new(cancelled);
    if cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }

    let selected_ids = selected_attachment_ids(input, attachment_paths)?;

    // Measure phase: stream every input once to bound the work before any
    // JavaScript runs. Errors name the dimension, the active tier and the
    // executable alternative.
    let mut plans = Vec::with_capacity(selected_ids.len());
    let mut budget = LoadBudget::default();
    let mut total_rows: u64 = 0;
    for id in &selected_ids {
        check_load_limits(&*cancelled, deadline)?;
        let (_, path) = attachment_paths
            .iter()
            .find(|(candidate, _)| candidate == id)
            .ok_or_else(|| format!("attachment '{id}' was not supplied by the host"))?;
        let plan = measure_attachment(id, path, tier, &mut budget, &*cancelled, deadline)?;
        total_rows =
            total_rows.saturating_add(plan.sheets.iter().map(|s| u64::from(s.rows)).sum::<u64>());
        plans.push(plan);
    }
    progress(ChunkProgress {
        rows_done: 0,
        rows_total: Some(total_rows),
        phase: "measured",
    });

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

    let meta = plans
        .iter()
        .map(|plan| {
            json!({
                "id": plan.id,
                "name": plan.name,
                "kind": plan.kind,
                "sheets": plan.sheets.iter().map(|sheet| json!({
                    "name": sheet.name,
                    "rowCount": sheet.rows,
                    "columns": sheet.columns,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let meta_json = serde_json::to_string(&meta)
        .map_err(|error| format!("Unable to serialize attachment metadata: {error}"))?;
    let (script, code_line_offset) = build_chunked_script(code);

    let context = Context::full(&runtime)
        .map_err(|error| format!("Unable to create JavaScript context: {error:?}"))?;
    let envelope = context.with(|ctx| -> Result<String, String> {
        ctx.globals()
            .set("__foxAttachmentMeta", meta_json.as_str())
            .map_err(|error| format!("Unable to initialize JavaScript input: {error:?}"))?;
        ctx.globals()
            .set("__foxMaxFileChars", (MAX_FILE_BYTES * 2) as u64)
            .map_err(|error| format!("Unable to initialize JavaScript limits: {error:?}"))?;
        if let Err(error) = ctx.eval::<(), _>(script.as_str()) {
            return Err(format_js_error(ctx, error, code, code_line_offset));
        }
        let mut rows_done: u64 = 0;
        for plan in &plans {
            for sheet in &plan.sheets {
                if sheet.rows == 0 {
                    continue;
                }
                match plan.kind.as_str() {
                    "csv" | "tsv" => emit_delimited_chunks(
                        &ctx, plan, sheet, &mut rows_done, total_rows, &*cancelled,
                        deadline, progress, code, code_line_offset,
                    )?,
                    _ => emit_workbook_chunks(
                        &ctx, plan, sheet, tier, &mut budget, &mut rows_done, total_rows,
                        &*cancelled, deadline, progress, code, code_line_offset,
                    )?,
                }
            }
        }
        let finish: Function = ctx
            .globals()
            .get("__foxFinish")
            .map_err(|error| format!("Unable to finalize JavaScript: {error:?}"))?;
        match finish.call::<(), String>(()) {
            Ok(envelope) => Ok(envelope),
            Err(error) => Err(format_js_error(ctx, error, code, code_line_offset)),
        }
    });

    let envelope = match envelope {
        Ok(value) => value,
        Err(error) => {
            let interrupt = interrupted.load(Ordering::Relaxed);
            if interrupt == INTERRUPT_CANCELLED {
                return Err("data-compute was cancelled".to_owned());
            }
            if interrupt == INTERRUPT_TIMEOUT {
                return Err("[tool.computation_timed_out] data-compute timed out".to_owned());
            }
            return Err(error);
        }
    };
    if interrupted.load(Ordering::Relaxed) == INTERRUPT_CANCELLED || cancelled() {
        return Err("data-compute was cancelled".to_owned());
    }
    if interrupted.load(Ordering::Relaxed) == INTERRUPT_TIMEOUT || Instant::now() >= deadline {
        return Err("[tool.computation_timed_out] data-compute timed out".to_owned());
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
    let output_specs = super::parse_output_specs_with_limits(envelope.get("files"), &*cancelled, deadline)?;
    let files = write_outputs(output_root, output_specs)?;
    let output_bytes = files
        .iter()
        .filter_map(|file| file.get("bytes").and_then(Value::as_u64))
        .try_fold(0usize, |total, bytes| {
            total.checked_add(bytes as usize).ok_or(())
        })
        .unwrap_or(usize::MAX);
    if output_bytes > MAX_TOTAL_OUTPUT_BYTES {
        return Err(format!(
            "data-compute outputs exceed {} bytes",
            MAX_TOTAL_OUTPUT_BYTES
        ));
    }
    Ok(json!({
        "result": result,
        "files": files,
        "attachmentCount": plans.len(),
        "rowsProcessed": total_rows,
        "attachmentCells": budget.cells,
        "attachmentTextBytes": budget.text_bytes,
        "processing": "chunked",
        "profile": tier.name,
        "durationMs": started.elapsed().as_millis(),
        "computedBy": "fox-in-process-rquickjs",
    }))
}

/// Stream one input once to produce its chunk plan, enforcing the tier's
/// bounds. Every error names the exceeded dimension, the active tier and an
/// executable alternative.
fn measure_attachment(
    id: &str,
    path: &Path,
    tier: Profile,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<AttachmentPlan, String> {
    validate_input_path(path)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Unable to inspect attachment '{id}': {error}"))?;
    if !metadata.is_file() {
        return Err(format!("attachment '{id}' is not a regular file"));
    }
    if metadata.len() > tier.max_attachment_bytes {
        return Err(format!(
            "attachment '{id}' 为 {} 字节，超过 {} 档位的 {} 字节上限；请拆小文件或先用其他工具预筛",
            metadata.len(),
            tier.name,
            tier.max_attachment_bytes,
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
        "xlsx" | "xlsm" | "xlam" | "xlsb" => {
            let bytes = fs::read(path)
                .map_err(|error| format!("Unable to read attachment '{id}': {error}"))?;
            validate_zip_expansion(&name, &bytes)?;
            measure_workbook(id, name, extension, path, &bytes, tier, budget, cancelled, deadline)
        }
        "xls" | "ods" => Err(format!(
            "attachment '{id}' 是 {extension}，没有可用的流式读取器；请另存为 xlsx 后重试，或改用 whole 模式（standard 档位限制内）"
        )),
        "csv" | "tsv" => {
            let delimiter = if extension == "tsv" { b'\t' } else { b',' };
            let (rows, columns) =
                scan_delimited(path, delimiter, tier, budget, cancelled, deadline)?;
            budget.add_text_capped(name.len(), tier.max_total_text_bytes)?;
            Ok(AttachmentPlan {
                id: id.to_owned(),
                name,
                kind: extension,
                path: path.to_path_buf(),
                sheets: vec![SheetPlan {
                    name: "Sheet1".to_owned(),
                    top: 0,
                    left: 0,
                    rows: rows as u32,
                    columns: columns as u32,
                }],
            })
        }
        other => Err(format!(
            "attachment '{id}' 的类型 {other} 不支持分块处理；whole 模式支持 json/txt，或先把数据导出为 csv/xlsx"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_workbook(
    id: &str,
    name: String,
    extension: String,
    path: &Path,
    bytes: &[u8],
    tier: Profile,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<AttachmentPlan, String> {
    let mut workbook = open_workbook_auto_from_rs(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|error| format!("Unable to read workbook '{name}': {error:?}"))?;
    let names = workbook.sheet_names().to_owned();
    if names.is_empty() {
        return Err(format!("workbook '{name}' contains no worksheets"));
    }
    if names.len() > MAX_SHEETS_PER_ATTACHMENT {
        return Err(format!(
            "workbook '{name}' 有 {} 个工作表，超过 {MAX_SHEETS_PER_ATTACHMENT} 个上限；请拆分工作簿",
            names.len()
        ));
    }
    let mut sheets = Vec::with_capacity(names.len());
    for sheet_name in names {
        check_load_limits(cancelled, deadline)?;
        let Some((top, left, bottom, right)) =
            measure_sheet_extent(&mut workbook, &sheet_name, &name, tier, cancelled, deadline)?
        else {
            sheets.push(SheetPlan {
                name: sheet_name,
                top: 0,
                left: 0,
                rows: 0,
                columns: 0,
            });
            continue;
        };
        let rows = u64::from(bottom) - u64::from(top) + 1;
        let columns = u64::from(right) - u64::from(left) + 1;
        if rows > tier.max_sheet_rows as u64 || columns > tier.max_sheet_columns as u64 {
            return Err(format!(
                "工作表 '{sheet_name}' 为 {rows} 行 x {columns} 列，超过 {} 档位的 {} 行 / {} 列上限；请先筛列/分行导出为 csv，或减小数据范围",
                tier.name, tier.max_sheet_rows, tier.max_sheet_columns,
            ));
        }
        budget.add_cells_capped((rows * columns) as usize, tier.max_total_cells)?;
        budget.add_text_capped(sheet_name.len(), tier.max_total_text_bytes)?;
        sheets.push(SheetPlan {
            name: sheet_name,
            top,
            left,
            rows: rows as u32,
            columns: columns as u32,
        });
    }
    Ok(AttachmentPlan {
        id: id.to_owned(),
        name,
        kind: extension,
        path: path.to_path_buf(),
        sheets,
    })
}

/// Streaming extent of one worksheet via calamine's cell reader. A sparse
/// far-away cell enlarges the extent, not an allocation; the tier check runs
/// inside the loop so an over-extent sheet fails fast.
fn measure_sheet_extent(
    workbook: &mut Sheets<std::io::Cursor<Vec<u8>>>,
    sheet_name: &str,
    book_name: &str,
    tier: Profile,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<Option<(u32, u32, u32, u32)>, String> {
    macro_rules! scan {
        ($book:expr) => {{
            let mut reader = $book
                .worksheet_cells_reader(sheet_name)
                .map_err(|e| e.to_string())?;
            let mut bounds: Option<(u32, u32, u32, u32)> = None;
            loop {
                check_load_limits(cancelled, deadline)?;
                match reader.next_cell() {
                    Ok(Some(cell)) => {
                        // Parity with whole mode: styled-but-empty cells never
                        // enlarge the measured extent.
                        if matches!(cell.get_value(), calamine::DataRef::Empty) {
                            continue;
                        }
                        let (row, col) = cell.get_position();
                        let (top, left, bottom, right) = bounds.unwrap_or((row, col, row, col));
                        bounds = Some((top.min(row), left.min(col), bottom.max(row), right.max(col)));
                        let (t, l, b, r) = bounds.unwrap();
                        let rows = u64::from(b) - u64::from(t) + 1;
                        let columns = u64::from(r) - u64::from(l) + 1;
                        if rows > tier.max_sheet_rows as u64
                            || columns > tier.max_sheet_columns as u64
                            || rows.saturating_mul(columns) > tier.max_total_cells as u64
                        {
                            return Err(format!(
                                "工作表 '{sheet_name}' 的单元格跨度已达 {rows} 行 x {columns} 列，超过 {} 档位上限（{} 行 / {} 列 / {} 单元格）；请缩小数据范围或预筛",
                                tier.name, tier.max_sheet_rows, tier.max_sheet_columns, tier.max_total_cells,
                            ));
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        return Err(format!(
                            "Unable to stream worksheet '{sheet_name}' of '{book_name}': {error}"
                        ))
                    }
                }
            }
            bounds
        }};
    }
    let bounds = match workbook {
        Sheets::Xlsx(book) => scan!(book),
        Sheets::Xlsb(book) => scan!(book),
        _ => {
            return Err(format!(
                "workbook '{book_name}' 的格式没有流式读取器；请另存为 xlsx 后重试"
            ))
        }
    };
    Ok(bounds)
}

/// Quote-aware streaming scan of a delimited file: measures (rows, columns)
/// without ever holding more than one field's byte count.
fn scan_delimited(
    path: &Path,
    delimiter: u8,
    tier: Profile,
    budget: &mut LoadBudget,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<(u64, usize), String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("Unable to open delimited attachment: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut rows: u64 = 0;
    let mut columns: usize = 0;
    let mut row_fields: usize = 0;
    let mut field_bytes: usize = 0;
    let mut quoted = false;
    let mut quoted_had_content = false;
    let mut buf = [0u8; 8192];
    loop {
        check_load_limits(cancelled, deadline)?;
        let read = reader
            .read(&mut buf)
            .map_err(|error| format!("Unable to stream delimited attachment: {error}"))?;
        if read == 0 {
            break;
        }
        for &b in &buf[..read] {
            if quoted {
                if b == b'"' {
                    quoted = false;
                } else {
                    quoted_had_content = true;
                }
            } else if b == b'"' && field_bytes == 0 {
                quoted = true;
            } else if b == delimiter {
                row_fields += 1;
                field_bytes = 0;
                quoted_had_content = false;
            } else if b == b'\n' {
                row_fields += 1;
                columns = columns.max(row_fields);
                row_fields = 0;
                field_bytes = 0;
                quoted_had_content = false;
                rows += 1;
                if rows > tier.max_sheet_rows as u64 {
                    return Err(format!(
                        "分隔文本已有 {rows} 行，超过 {} 档位的 {} 行上限；请先拆分文件",
                        tier.name, tier.max_sheet_rows,
                    ));
                }
                if columns > tier.max_sheet_columns {
                    return Err(format!(
                        "分隔文本有 {columns} 列，超过 {} 档位的 {} 列上限；请按列拆分",
                        tier.name, tier.max_sheet_columns,
                    ));
                }
            } else {
                field_bytes += 1;
            }
        }
    }
    if quoted {
        return Err("分隔文本包含未闭合的引号字段".to_owned());
    }
    if row_fields > 0 || field_bytes > 0 || quoted_had_content {
        row_fields += 1;
        columns = columns.max(row_fields);
        rows += 1;
    }
    budget.add_text_capped(
        fs::metadata(path).map_err(|e| e.to_string())?.len() as usize,
        tier.max_total_text_bytes,
    )?;
    Ok((rows, columns))
}

/// Stream a delimited file a second time, delivering rectangular row chunks.
#[allow(clippy::too_many_arguments)]
fn emit_delimited_chunks(
    ctx: &rquickjs::Ctx,
    plan: &AttachmentPlan,
    sheet: &SheetPlan,
    rows_done: &mut u64,
    total_rows: u64,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
    progress: &dyn Fn(ChunkProgress),
    code: &str,
    code_line_offset: usize,
) -> Result<(), String> {
    let delimiter = if plan.kind == "tsv" { b'\t' } else { b',' };
    let file = fs::File::open(&plan.path)
        .map_err(|error| format!("Unable to open delimited attachment: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut chunk: Vec<Vec<Value>> = Vec::new();
    let mut chunk_start: u64 = 1;
    let mut row_index: u64 = 0;
    let mut row: Vec<Value> = Vec::new();
    let mut field = Vec::new();
    let mut quoted = false;
    let mut buf = [0u8; 8192];
    let capacity = chunk_capacity(sheet.columns as usize);
    let mut flush = |chunk: &mut Vec<Vec<Value>>,
                     chunk_start: &mut u64,
                     row_index: u64,
                     final_chunk: bool|
     -> Result<(), String> {
        if chunk.is_empty() && !final_chunk {
            return Ok(());
        }
        let width = chunk.iter().map(Vec::len).max().unwrap_or_default();
        for row in chunk.iter_mut() {
            row.resize(width, Value::Null);
        }
        let count = chunk.len() as u64;
        let payload = json!({
            "attachmentId": plan.id,
            "name": plan.name,
            "kind": plan.kind,
            "sheet": sheet.name,
            "startRow": *chunk_start,
            "rows": chunk,
            "final": final_chunk,
        })
        .to_string();
        deliver_chunk(
            ctx,
            payload,
            count,
            rows_done,
            total_rows,
            progress,
            code,
            code_line_offset,
        )?;
        chunk.clear();
        *chunk_start = row_index + 1;
        check_load_limits(cancelled, deadline)?;
        Ok(())
    };
    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|error| format!("Unable to stream delimited attachment: {error}"))?;
        if read == 0 {
            break;
        }
        for &b in &buf[..read] {
            if quoted {
                if b == b'"' {
                    quoted = false;
                } else {
                    field.push(b);
                }
            } else if b == b'"' && field.is_empty() {
                quoted = true;
            } else if b == delimiter {
                push_field(&mut row, &mut field)?;
            } else if b == b'\n' {
                if field.last() == Some(&b'\r') {
                    field.pop();
                }
                push_field(&mut row, &mut field)?;
                row_index += 1;
                chunk.push(std::mem::take(&mut row));
                if chunk.len() >= capacity {
                    flush(&mut chunk, &mut chunk_start, row_index, false)?;
                }
            } else {
                field.push(b);
            }
        }
    }
    if quoted {
        return Err("分隔文本包含未闭合的引号字段".to_owned());
    }
    if !field.is_empty() || !row.is_empty() {
        push_field(&mut row, &mut field)?;
        row_index += 1;
        chunk.push(std::mem::take(&mut row));
    }
    flush(&mut chunk, &mut chunk_start, row_index, true)
}

fn push_field(row: &mut Vec<Value>, field: &mut Vec<u8>) -> Result<(), String> {
    let text = String::from_utf8(std::mem::take(field))
        .map_err(|_| "分隔文本不是有效的 UTF-8".to_owned())?;
    row.push(Value::String(text));
    Ok(())
}

fn chunk_capacity(width: usize) -> usize {
    (CHUNK_TARGET_CELLS / width.max(1)).clamp(1, CHUNK_MAX_ROWS)
}

/// Second streaming pass over one worksheet, emitting dense row chunks that
/// cover the measured extent. Rows with no stored cells arrive as null rows,
/// so `startRow` always reflects the true 1-based sheet row of `rows[0]`.
#[allow(clippy::too_many_arguments)]
fn emit_workbook_chunks(
    ctx: &rquickjs::Ctx,
    plan: &AttachmentPlan,
    sheet: &SheetPlan,
    tier: Profile,
    budget: &mut LoadBudget,
    rows_done: &mut u64,
    total_rows: u64,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
    progress: &dyn Fn(ChunkProgress),
    code: &str,
    code_line_offset: usize,
) -> Result<(), String> {
    let bytes = fs::read(&plan.path)
        .map_err(|error| format!("Unable to read workbook '{}': {error}", plan.name))?;
    let mut workbook = open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
        .map_err(|error| format!("Unable to read workbook '{}': {error:?}", plan.name))?;
    let width = sheet.columns as usize;
    let rows_per_chunk = chunk_capacity(width) as u32;
    let bottom = sheet.top + sheet.rows - 1;

    macro_rules! emit {
        ($book:expr) => {{
            let mut reader = $book
                .worksheet_cells_reader(&sheet.name)
                .map_err(|e| e.to_string())?;
            let mut window_start = sheet.top;
            let mut matrix: Vec<Vec<Value>> = Vec::new();
            let mut flush_window = |matrix: &mut Vec<Vec<Value>>,
                                    window_start: &mut u32,
                                    window_end: u32|
             -> Result<(), String> {
                // Deliver rows [window_start, window_end): pad missing to dense.
                let window_rows = (window_end - *window_start) as usize;
                matrix.resize_with(window_rows, || vec![Value::Null; width]);
                for row in matrix.iter_mut() {
                    row.resize(width, Value::Null);
                }
                let payload = json!({
                    "attachmentId": plan.id,
                    "name": plan.name,
                    "kind": plan.kind,
                    "sheet": sheet.name,
                    "startRow": *window_start + 1,
                    "rows": matrix,
                    "final": window_end > bottom,
                })
                .to_string();
                deliver_chunk(
                    ctx,
                    payload,
                    window_rows as u64,
                    rows_done,
                    total_rows,
                    progress,
                    code,
                    code_line_offset,
                )?;
                matrix.clear();
                *window_start = window_end;
                check_load_limits(cancelled, deadline)?;
                Ok(())
            };
            loop {
                match reader.next_cell() {
                    Ok(Some(cell)) => {
                        // Styled-but-empty cells are just null slots; skipping
                        // them keeps the stream cheap and matches whole mode.
                        if matches!(cell.get_value(), calamine::DataRef::Empty) {
                            continue;
                        }
                        let (row, col) = cell.get_position();
                        if row < sheet.top || row < window_start {
                            // Rows above the extent are not part of the plan;
                            // a row behind the window means a non-ordered reader.
                            if row < window_start && row >= sheet.top {
                                return Err(
                                    "工作表单元格读取顺序异常，无法分块；请改用 whole 模式".to_owned()
                                );
                            }
                            continue;
                        }
                        while row >= window_start + rows_per_chunk {
                            let window_end = (window_start + rows_per_chunk).min(bottom + 1);
                            flush_window(&mut matrix, &mut window_start, window_end)?;
                        }
                        let local_row = (row - window_start) as usize;
                        let local_col = (col - sheet.left) as usize;
                        if local_col >= width {
                            return Err("工作表单元格超出测量范围，无法分块".to_owned());
                        }
                        if matrix.len() <= local_row {
                            matrix.resize_with(local_row + 1, Vec::new);
                        }
                        if matrix[local_row].len() < width {
                            matrix[local_row].resize(width, Value::Null);
                        }
                        matrix[local_row][local_col] = cell_to_value_capped(
                            &calamine::Data::from(cell.get_value().clone()),
                            budget,
                            tier.max_total_text_bytes,
                        )?;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        return Err(format!(
                            "Unable to stream worksheet '{}' of '{}': {error}",
                            sheet.name, plan.name
                        ))
                    }
                }
            }
            while window_start <= bottom {
                let window_end = (window_start + rows_per_chunk).min(bottom + 1);
                flush_window(&mut matrix, &mut window_start, window_end)?;
            }
            Ok::<(), String>(())
        }};
    }
    match &mut workbook {
        Sheets::Xlsx(book) => emit!(book),
        Sheets::Xlsb(book) => emit!(book),
        _ => Err("workbook 格式没有流式读取器；请另存为 xlsx 后重试".to_owned()),
    }
}

#[allow(clippy::too_many_arguments)]
fn deliver_chunk(
    ctx: &rquickjs::Ctx,
    payload: String,
    rows_in_chunk: u64,
    rows_done: &mut u64,
    total_rows: u64,
    progress: &dyn Fn(ChunkProgress),
    code: &str,
    code_line_offset: usize,
) -> Result<(), String> {
    if payload.len() > CHUNK_MAX_JSON_BYTES {
        return Err(format!(
            "单个数据块序列化后超过 {CHUNK_MAX_JSON_BYTES} 字节；请减少每块行数或列数"
        ));
    }
    let function: Function = ctx
        .globals()
        .get("__foxDeliverChunk")
        .map_err(|error| format!("Unable to deliver a chunk: {error:?}"))?;
    function
        .call::<(String,), ()>((payload,))
        .map_err(|error: JsError| format_js_error(ctx.clone(), error, code, code_line_offset))?;
    *rows_done += rows_in_chunk;
    progress(ChunkProgress {
        rows_done: *rows_done,
        rows_total: Some(total_rows),
        phase: "chunks",
    });
    Ok(())
}

/// The chunked-mode wrapper: the user code defines handlers instead of
/// returning a result. `attachments` carries metadata only. Handler pickup
/// happens INSIDE the user's scope — strict-mode function declarations do
/// not escape their IIFE.
fn build_chunked_script(code: &str) -> (String, usize) {
    let mut script = String::with_capacity(code.len() + 2_000);
    script.push_str(
        r#"(() => {
  const attachments = JSON.parse(__foxAttachmentMeta);
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
    script.push_str(super::TABLE_HELPERS);
    script.push_str(
        r#"
  (() => {
    "use strict";
"#,
    );
    let line_offset = script.bytes().filter(|byte| *byte == b'\n').count();
    script.push_str(code);
    script.push_str(
        r#"
  ;globalThis.__foxHandlers = {
    onChunk: (typeof onChunk === "function") ? onChunk : null,
    onFinish: (typeof onFinish === "function") ? onFinish : null,
  };
  })();
  const __foxHandlers = globalThis.__foxHandlers || {};
  if (typeof __foxHandlers.onChunk !== "function") {
    throw new Error("chunked processing requires the code to define onChunk(chunk); rows are delivered one chunk at a time and are never all in memory");
  }
  globalThis.__foxDeliverChunk = (payload) => { __foxHandlers.onChunk(JSON.parse(payload)); };
  globalThis.__foxFinish = () => {
    const __foxResult = (typeof __foxHandlers.onFinish === "function") ? __foxHandlers.onFinish() : null;
    return JSON.stringify({
      result: __foxResult === undefined ? null : __foxResult,
      files: __foxSavedFiles,
    });
  };
})()"#,
    );
    (script, line_offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    // Authored for a later user run; not executed by R3B.
    #[test]
    fn r3b_chunked_installs_the_same_table_export_and_hash_contract() {
        let root=workspace("r3b-table");let source=csv_fixture(&root,3);
        let response=execute(&[("a".to_owned(),source)],&root.join("out"),&json!({"code":r#"
            let count=0;
            function onChunk(chunk){for(const row of chunk.rows){if(row[0]!=='id')count++;}}
            function onFinish(){saveTable('summary.csv',{columns:['key','count'],keyColumns:['key'],rows:[[scalarKey('rows'),count]]});return count;}
        "#}),||false,deadline(5),&|_|{}).unwrap();
        assert_eq!(response["result"],3);
        let file=&response["files"][0];let bytes=fs::read(file["path"].as_str().unwrap()).unwrap();
        assert_eq!(hex::encode(sha2::Sha256::digest(&bytes)),file["renderedHash"].as_str().unwrap());
        assert_eq!(file["dataContract"]["keyColumns"],json!(["key"]));
    }
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    fn workspace(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-chunked-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn csv_fixture(root: &Path, rows: usize) -> PathBuf {
        let path = root.join("big.csv");
        let mut text = String::from("id,name,value\n");
        for index in 0..rows {
            text.push_str(&format!("{index},item-{index},{}\n", index % 100));
        }
        fs::write(&path, text).unwrap();
        path
    }

    // Minimal uncompressed ZIP so tests can build real workbooks without any
    // extra dependency. Calamine reads stored entries like any other zip.
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    fn zip_stored(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            for part in [
                0x0403_4b50u32.to_le_bytes().to_vec(),
                20u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                crc.to_le_bytes().to_vec(),
                (data.len() as u32).to_le_bytes().to_vec(),
                (data.len() as u32).to_le_bytes().to_vec(),
                (name.len() as u16).to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
            ] {
                out.extend_from_slice(&part);
            }
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            for part in [
                0x0201_4b50u32.to_le_bytes().to_vec(),
                20u16.to_le_bytes().to_vec(),
                20u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                crc.to_le_bytes().to_vec(),
                (data.len() as u32).to_le_bytes().to_vec(),
                (data.len() as u32).to_le_bytes().to_vec(),
                (name.len() as u16).to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u16.to_le_bytes().to_vec(),
                0u32.to_le_bytes().to_vec(),
                offset.to_le_bytes().to_vec(),
            ] {
                central.extend_from_slice(&part);
            }
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        for part in [
            0x0605_4b50u32.to_le_bytes().to_vec(),
            0u16.to_le_bytes().to_vec(),
            0u16.to_le_bytes().to_vec(),
            (entries.len() as u16).to_le_bytes().to_vec(),
            (entries.len() as u16).to_le_bytes().to_vec(),
            cd_size.to_le_bytes().to_vec(),
            cd_offset.to_le_bytes().to_vec(),
            0u16.to_le_bytes().to_vec(),
        ] {
            out.extend_from_slice(&part);
        }
        out
    }

    fn xlsx_fixture(sheet_xml: String) -> Vec<u8> {
        let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        let wb_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
        let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#;
        zip_stored(&[
            ("[Content_Types].xml", content_types.as_bytes().to_vec()),
            ("_rels/.rels", rels.as_bytes().to_vec()),
            ("xl/workbook.xml", workbook.as_bytes().to_vec()),
            ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes().to_vec()),
            ("xl/styles.xml", styles.as_bytes().to_vec()),
            ("xl/worksheets/sheet1.xml", sheet_xml.into_bytes()),
        ])
    }

    fn sheet_xml(rows: &[(u32, Vec<(u32, Value)>)]) -> String {
        fn col_name(mut column: u32) -> String {
            let mut letters = Vec::new();
            column += 1;
            while column > 0 {
                column -= 1;
                letters.push((b'A' + (column % 26) as u8) as char);
                column /= 26;
            }
            letters.iter().rev().collect()
        }
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        );
        for (row, cells) in rows {
            xml.push_str(&format!("<row r=\"{}\">", row + 1));
            for (col, value) in cells {
                let reference = format!("{}{}", col_name(*col), row + 1);
                match value {
                    Value::String(text) => {
                        let escaped = text
                            .replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;");
                        xml.push_str(&format!(
                            "<c r=\"{reference}\" t=\"inlineStr\"><is><t>{escaped}</t></is></c>"
                        ));
                    }
                    Value::Number(number) => {
                        xml.push_str(&format!("<c r=\"{reference}\"><v>{number}</v></c>"));
                    }
                    Value::Bool(flag) => {
                        xml.push_str(&format!(
                            "<c r=\"{reference}\" t=\"b\"><v>{}</v></c>",
                            if *flag { 1 } else { 0 }
                        ));
                    }
                    _ => {},
                }
            }
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData></worksheet>");
        xml
    }

    fn deadline(secs: u64) -> Instant {
        Instant::now() + Duration::from_secs(secs)
    }

    #[test]
    fn csv_streams_in_order_with_progress_and_outputs() {
        let root = workspace("csv");
        let csv = csv_fixture(&root, 120_000);
        let paths = vec![("a".to_owned(), csv)];
        let progress_calls = AtomicU64::new(0);
        let last = std::sync::Mutex::new((0u64, 0u64));
        let result = execute(
            &paths,
            &root.join("out"),
            &json!({
                "code": "let rows=0, sum=0, chunks=0;\nfunction onChunk(c){ chunks++; if(c.rows.length===0 && !c.final) throw new Error('empty chunk'); for(const r of c.rows){ rows++; if(r[0] !== 'id'){ sum += Number(r[2]); } } }\nfunction onFinish(){ saveFile('summary.json', JSON.stringify({rows:rows})); return {rows:rows, sum:sum, chunks:chunks, meta:attachments[0].sheets[0].rowCount}; }",
            }),
            || false,
            deadline(30),
            &|p: ChunkProgress| {
                progress_calls.fetch_add(1, Ordering::SeqCst);
                let mut slot = last.lock().unwrap();
                assert!(p.rows_done >= slot.0, "progress must be monotonic");
                *slot = (p.rows_done, p.rows_total.unwrap_or(0));
            },
        )
        .unwrap();
        assert_eq!(result["processing"], "chunked");
        assert_eq!(result["profile"], "standard");
        // 120,001 rows including the header.
        assert_eq!(result["rowsProcessed"], 120_001);
        assert_eq!(result["result"]["rows"], 120_001);
        assert_eq!(result["result"]["meta"], 120_001);
        assert!(result["result"]["chunks"].as_u64().unwrap() > 1, "multiple chunks delivered");
        let expected_sum: u64 = (0..120_000u64).map(|i| i % 100).sum();
        assert_eq!(result["result"]["sum"], expected_sum);
        assert_eq!(result["files"][0]["displayName"], "summary.json");
        assert!(progress_calls.load(Ordering::SeqCst) > 1);
        assert_eq!(last.lock().unwrap().0, 120_001);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn standard_tier_rejects_oversize_with_named_dimension_and_remedy() {
        let root = workspace("bounds");
        // 250,001 rows exceed the standard tier but not the large one.
        let csv = csv_fixture(&root, 250_001);
        let paths = vec![("a".to_owned(), csv)];
        let error = execute(
            &paths,
            &root.join("out"),
            &json!({"code": "function onChunk(c){}\n", "profile": "standard"}),
            || false,
            deadline(60),
            &|_| {},
        )
        .unwrap_err();
        assert!(error.contains("250"), "{error}");
        assert!(error.contains("standard"), "{error}");
        assert!(error.contains("拆分"), "{error}");
        // The same file passes on the large tier.
        let result = execute(
            &paths,
            &root.join("out2"),
            &json!({"code": "let n=0;\nfunction onChunk(c){ n+=c.rows.length; }\nfunction onFinish(){ return {rows:n}; }", "profile": "large"}),
            || false,
            deadline(120),
            &|_| {},
        )
        .unwrap();
        assert_eq!(result["result"]["rows"], 250_002);
        assert_eq!(result["profile"], "large");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workbook_delivers_dense_extent_with_true_row_numbers() {
        let root = workspace("xlsx");
        // Sparse workbook: header at row 1, data rows 2..=3 and 4,999..=5,000.
        let mut rows: Vec<(u32, Vec<(u32, Value)>)> = vec![
            (0, vec![(0, json!("name")), (1, json!("value"))]),
            (1, vec![(0, json!("alpha")), (1, json!(1))]),
            (2, vec![(0, json!("beta")), (1, json!(2))]),
            (4_998, vec![(0, json!("omega")), (1, json!(99))]),
        ];
        rows.push((4_999, vec![(0, json!("tail")), (1, json!(100))]));
        let bytes = xlsx_fixture(sheet_xml(&rows));
        let path = root.join("sparse.xlsx");
        fs::write(&path, bytes).unwrap();
        let paths = vec![("w".to_owned(), path)];
        let result = execute(
            &paths,
            &root.join("out"),
            &json!({"code": "let seen=[], count=0, finals=0, first=null, last=null;\nfunction onChunk(c){ count+=c.rows.length; if(c.final) finals++; if(first===null) first=c; last=c; seen.push([c.startRow, c.rows.length]); }\nfunction onFinish(){ const r2c1=last.rows[last.rows.length-1]; return {count:count, finals:finals, firstStart:first.startRow, firstCell:first.rows[0][0], lastStart:last.startRow, tail:r2c1[0], tailV:r2c1[1], chunkStarts:seen.length}; }"}),
            || false,
            deadline(30),
            &|_| {},
        )
        .unwrap();
        assert_eq!(result["result"]["count"], 5_000, "dense extent rows");
        assert_eq!(result["result"]["finals"], 1);
        assert_eq!(result["result"]["firstStart"], 1);
        assert_eq!(result["result"]["firstCell"], "name");
        assert!(result["result"]["chunkStarts"].as_u64().unwrap() >= 1);
        assert_eq!(result["result"]["tail"], "tail");
        assert_eq!(result["result"]["tailV"], 100);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_formats_are_rejected_with_an_explicit_alternative() {
        let root = workspace("legacy");
        let path = root.join("old.xls");
        fs::write(&path, b"not a real xls but enough for the kind check").unwrap();
        let paths = vec![("x".to_owned(), path)];
        let error = execute(
            &paths,
            &root.join("out"),
            &json!({"code": "function onChunk(c){}"}),
            || false,
            deadline(5),
            &|_| {},
        )
        .unwrap_err();
        assert!(error.contains("xlsx"), "{error}");
        assert!(error.contains("whole"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_onchunk_and_cancellation_and_timeout_are_distinct() {
        let root = workspace("control");
        let csv = csv_fixture(&root, 20_000);
        let paths = vec![("a".to_owned(), csv)];
        // No onChunk defined.
        let error = execute(
            &paths,
            &root.join("o1"),
            &json!({"code": "return 1;"}),
            || false,
            deadline(5),
            &|_| {},
        )
        .unwrap_err();
        assert!(error.contains("onChunk"), "{error}");
        // Cancellation between chunks.
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = Arc::clone(&flag);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            worker_flag.store(true, Ordering::SeqCst);
        });
        let error = execute(
            &paths,
            &root.join("o2"),
            &json!({"code": "function onChunk(c){ for(let i=0;i<500000;i++){ Math.sqrt(i); } }"}),
            move || flag.load(Ordering::SeqCst),
            deadline(60),
            &|_| {},
        )
        .unwrap_err();
        trigger.join().unwrap();
        assert!(error.contains("cancelled"), "{error}");
        // Past deadline.
        let error = execute(
            &paths,
            &root.join("o3"),
            &json!({"code": "function onChunk(c){}"}),
            || false,
            Instant::now() - Duration::from_secs(1),
            &|_| {},
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }
}

//! Business delivery verification for ordinary file-producing tasks.
//!
//! Three distinct end-of-turn states live here and must never be conflated:
//!
//! 1. **Model stop** — the model emitted a non-tool reply (`<fox-final/>` and
//!    the reply-end check are protocol concerns handled by the runtime).
//! 2. **Continuation review** — the bounded stop-review that asks a plausibly
//!    unfinished answer to keep going (see [`super::kernel_coordinator::live::CONTINUATION_PROMPT`]).
//! 3. **Business delivery** — this module: the task explicitly promised files,
//!    the Run's real artifacts are bound to checklist items, and Host runs
//!    deterministic checks (exists / parseable / non-empty / structural
//!    consistency).
//!
//! Failed items become one concrete repair request to the model. Repair rounds
//! ride the existing continuation transport but are tagged `delivery_repair`
//! so they are counted in `delivery_repair_rounds` / the delivery-repair event
//! count with [`DELIVERY_REPAIR_LIMIT`]; they are **never** mixed into the
//! stop-review continuation counter or into provider/turn model-failure
//! retries, and they never replace the reply-end marker check. A plain question
//! seeds no checklist and is never charged verification or repair work.

use crate::database::{
    Database, DeliveryArtifactRow, DeliveryChecklistItem, DeliveryChecklistSeed,
};
use crate::database::{
    DeliveryChecklistSeed as StoredChecklistSeed, DeliveryRequirement as StoredRequirement,
    RequirementKind as StoredRequirementKind,
};
use calamine::{open_workbook_auto_from_rs, Data as CellData, Reader};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Independent bounded-repair budget, separate from both the stop-review
/// continuation limit and from provider/turn model-failure retries.
pub(crate) const DELIVERY_REPAIR_LIMIT: i64 = 2;

/// File types an ordinary task can explicitly promise. Extension groups double
/// as the deterministic checks planned for the produced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactKind {
    Spreadsheet,
    Csv,
    Word,
    Slides,
    Pdf,
    Json,
    Text,
    Image,
    LegacyOffice,
    Other,
}

impl ArtifactKind {
    fn from_extension(extension: &str) -> Option<ArtifactKind> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "xlsx" | "xlsm" | "xlsb" | "xls" => ArtifactKind::Spreadsheet,
            "csv" | "tsv" => ArtifactKind::Csv,
            "docx" => ArtifactKind::Word,
            "doc" | "rtf" => ArtifactKind::LegacyOffice,
            "pptx" => ArtifactKind::Slides,
            "ppt" => ArtifactKind::LegacyOffice,
            "pdf" => ArtifactKind::Pdf,
            "json" => ArtifactKind::Json,
            "txt" | "md" | "markdown" | "html" | "htm" | "log" => ArtifactKind::Text,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => ArtifactKind::Image,
            _ => return None,
        })
    }

    fn from_keyword(keyword: &str) -> Option<(ArtifactKind, &'static str)> {
        Some(match keyword.to_lowercase().as_str() {
            "excel" | "电子表格" | "spreadsheet" => (ArtifactKind::Spreadsheet, "xlsx"),
            "word" | "文档" => (ArtifactKind::Word, "docx"),
            "ppt" | "powerpoint" | "演示文稿" | "幻灯片" => (ArtifactKind::Slides, "pptx"),
            "pdf" => (ArtifactKind::Pdf, "pdf"),
            _ => return None,
        })
    }

    fn extension_matches(&self, extension: &str) -> bool {
        matches!(
            (self, ArtifactKind::from_extension(extension)),
            (ArtifactKind::Spreadsheet, Some(ArtifactKind::Spreadsheet))
                | (ArtifactKind::Csv, Some(ArtifactKind::Csv))
                | (ArtifactKind::Word, Some(ArtifactKind::Word))
                | (ArtifactKind::Slides, Some(ArtifactKind::Slides))
                | (ArtifactKind::Pdf, Some(ArtifactKind::Pdf))
                | (ArtifactKind::Json, Some(ArtifactKind::Json))
                | (ArtifactKind::Text, Some(ArtifactKind::Text))
                | (ArtifactKind::Image, Some(ArtifactKind::Image))
        )
    }

    fn planned_checks(&self) -> Vec<&'static str> {
        match self {
            ArtifactKind::Spreadsheet | ArtifactKind::Word | ArtifactKind::Slides => {
                vec!["exists", "parseable", "nonempty"]
            }
            ArtifactKind::Csv => {
                vec!["exists", "parseable", "nonempty", "consistent_columns"]
            }
            ArtifactKind::Pdf | ArtifactKind::Json | ArtifactKind::Text | ArtifactKind::Image => {
                vec!["exists", "parseable", "nonempty"]
            }
            ArtifactKind::LegacyOffice | ArtifactKind::Other => vec!["exists", "nonempty"],
        }
    }
}

/// A production verb must appear somewhere in the task for document-count
/// language ("两份 Excel") to count as a promised deliverable; analysing or
/// reading an existing file never seeds a checklist.
const PRODUCTION_VERBS: &[&str] = &[
    "生成", "产出", "制作", "制备", "导出", "出具", "输出", "保存", "写成", "撰写", "编写",
    "整理成", "汇总成", "打印成", "交付", "create", "generate", "produce", "export", "save",
    "write", "build", "deliver",
];

const FILE_EXTENSIONS: &[&str] = &[
    "xlsx", "xlsm", "xlsb", "xls", "csv", "tsv", "docx", "doc", "rtf", "pptx", "ppt", "pdf",
    "json", "txt", "md", "markdown", "html", "htm", "log", "png", "jpg", "jpeg", "gif", "webp",
    "svg",
];

const KIND_KEYWORDS: &[&str] = &[
    "Excel",
    "电子表格",
    "spreadsheet",
    "Word",
    "文档",
    "PPT",
    "PowerPoint",
    "演示文稿",
    "幻灯片",
    "PDF",
];

const MAX_SEEDED_ITEMS: usize = 12;
const MAX_VERIFY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SCAN_DEPTH: usize = 5;
/// Backtracking from an explicit extension stops at these CJK characters,
/// which mark the prose before the actual file name (`生成AGV报告.xlsx`).
const CJK_NAME_STOPS: &[char] = &[
    '成', '为', '给', '把', '将', '在', '到', '请', '，', '。', '；', '：', '！', '？', '、',
    '的', '与', '和',
];

fn has_production_verb(text: &str) -> bool {
    let lowered = text.to_lowercase();
    PRODUCTION_VERBS
        .iter()
        .any(|verb| lowered.contains(&verb.to_lowercase()))
}

fn is_word_boundary(character: Option<char>) -> bool {
    match character {
        None => true,
        Some(value) => {
            value.is_whitespace()
                || matches!(
                    value,
                    '(' | ')' | '（' | '）' | '"' | '“' | '”' | '‘' | '’' | '「' | '」'
                        | '『' | '』' | '`' | '[' | ']' | '【' | '】' | ',' | '，' | ';' | '；'
                        | ':' | '：' | '、' | '。' | '！' | '？' | '…' | '·'
                        | '\n' | '\r' | '\t'
                )
        }
    }
}

fn is_filename_character(character: char) -> bool {
    if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ' ' | '/') {
        return true;
    }
    ('一'..='鿿').contains(&character) && !CJK_NAME_STOPS.contains(&character)
}

/// The character before an explicit file name must end prose: either a normal
/// token boundary, or one of the CJK stop characters (`生成AGV报告.docx`).
fn is_name_start_boundary(character: Option<char>) -> bool {
    match character {
        None => true,
        Some(value) => is_word_boundary(Some(value)) || CJK_NAME_STOPS.contains(&value),
    }
}

/// After the extension, ordinary boundaries plus any CJK ideograph are
/// accepted: Chinese prose continues without a space (`报告.docx并保存`).
fn is_extension_end_boundary(character: Option<char>) -> bool {
    match character {
        None => true,
        Some(value) => {
            is_word_boundary(Some(value)) || ('一'..='鿿').contains(&value)
        }
    }
}

/// Number of already-planned explicit files matching an artifact kind.
fn explicit_kind_count(seeds: &[DeliveryChecklistSeed], wanted: ArtifactKind) -> usize {
    seeds
        .iter()
        .filter(|seed| {
            Path::new(seed.target_path.as_deref().unwrap_or(""))
                .extension()
                .and_then(|value| value.to_str())
                .and_then(ArtifactKind::from_extension)
                .as_ref()
                == Some(&wanted)
        })
        .count()
}

/// Deterministically derive the lightweight delivery checklist for a task.
///
/// Two conservative signals are recognised:
/// * an explicit file name with a supported extension (`汇总表.xlsx`), and
/// * counted document language paired with a production verb
///   (`生成两份 Excel 和一份 Word`).
///
/// Anything else — including "分析这份 Excel" — seeds nothing, so ordinary
/// questions and read-only analysis bypass delivery verification entirely.
pub(crate) fn expectations_from_task(text: &str) -> Vec<DeliveryChecklistSeed> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut seeds: Vec<DeliveryChecklistSeed> = Vec::new();
    let mut used_keys = BTreeSet::new();

    let push = |seeds: &mut Vec<DeliveryChecklistSeed>,
                    used: &mut BTreeSet<String>,
                    seed: DeliveryChecklistSeed| {
        if seeds.len() < MAX_SEEDED_ITEMS && used.insert(seed.item_key.clone()) {
            seeds.push(seed);
        }
    };

    // 1) Explicit file names. A name is only accepted at a token boundary so
    //    URLs, e-mail addresses and prose such as "v1.2 release notes" cannot
    //    seed phantom deliverables.
    let mut hits: Vec<(usize, usize, &str)> = Vec::new();
    let lower_text = text.to_lowercase();
    for extension in FILE_EXTENSIONS {
        let needle = format!(".{extension}");
        let mut from = 0;
        while let Some(relative) = lower_text[from..].find(&needle) {
            let dot = from + relative;
            let end = dot + needle.len();
            from = end;
            if is_extension_end_boundary(text[end..].chars().next()) {
                hits.push((dot, end, extension));
            }
        }
    }
    hits.sort_by_key(|(dot, _, _)| *dot);
    let mut claimed_ranges: Vec<(usize, usize)> = Vec::new();
    for (dot, end, extension) in hits {
        let Some(kind) = ArtifactKind::from_extension(extension) else {
            continue;
        };
        let mut start = dot;
        for (index, character) in text.char_indices().rev().filter(|(i, _)| *i < dot) {
            let extends = if character == ' ' {
                // Spaces are legal inside a file name, but only between
                // ASCII name characters: "结果写入 report/x.csv" must
                // backtrack to "report/x.csv", not swallow the prose.
                let preceded_by_ascii = text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-' | '.' | '/'));
                let followed_by_ascii = text[start..]
                    .chars()
                    .next()
                    .is_some_and(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-' | '.' | '/'));
                preceded_by_ascii && followed_by_ascii
            } else {
                is_filename_character(character)
            };
            if extends {
                start = index;
            } else {
                break;
            }
        }
        let name = text[start..end].trim();
        let name_start = end - name.len();
        let traverses_root = name.replace('\\', "/").split('/').any(|segment| {
            segment.is_empty() || segment == "." || segment == ".."
        });
        // With '/' allowed inside relative names, backtracking can walk into a
        // URL ("https://host/report.docx"); the whitespace-delimited token
        // immediately before the name then carries the scheme colon.
        let preceding_token = text[..name_start]
            .rsplit(|value: char| value.is_whitespace())
            .next()
            .unwrap_or("");
        if name.is_empty()
            || name.starts_with('.')
            || name.contains("://")
            || name.contains('@')
            || traverses_root
            || preceding_token.contains("://")
            || preceding_token.contains(':')
            || preceding_token.contains('@')
            || !is_name_start_boundary(text[..name_start].chars().next_back())
        {
            continue;
        }
        if claimed_ranges
            .iter()
            .any(|(a, b)| name_start >= *a && name_start < *b)
        {
            continue;
        }
        claimed_ranges.push((name_start, end));
        let key = format!("file:{}", name.replace(['/', '\\'], "_").to_lowercase());
        push(
            &mut seeds,
            &mut used_keys,
            DeliveryChecklistSeed {
                item_key: key,
                target_path: Some(name.replace('\\', "/")),
                artifact_id: None,
                display_name: name.to_owned(),
                checks: kind
                    .planned_checks()
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                requirements: Vec::new(),
            },
        );
    }

    // 2) Counted document language, gated by a production verb. Explicit files
    //    of the same kind satisfy the count first, so "导出 a.xlsx 和 b.xlsx"
    //    does not also demand two phantom slots.
    if has_production_verb(text) {
        for keyword in KIND_KEYWORDS {
            let Some((kind, expected_extension)) = ArtifactKind::from_keyword(keyword) else {
                continue;
            };
            let needle = keyword.to_lowercase();
            let ascii_keyword = keyword.chars().all(|value| value.is_ascii_alphabetic());
            let mut search_from = 0;
            let mut slot_for_keyword = 0;
            'occurrences: while let Some(relative) = lower_text[search_from..].find(&needle) {
                let at = search_from + relative;
                search_from = at + needle.len();
                if ascii_keyword
                    && (!is_word_boundary(text[..at].chars().next_back())
                        || !is_word_boundary(text[search_from..].chars().next()))
                {
                    continue;
                }
                // Walk back over an optional measure word 份/个/张/篇/部.
                let mut cursor = at;
                let mut measure_seen = false;
                loop {
                    let rest = text[..cursor].trim_end();
                    let trimmed = cursor - (cursor - rest.len());
                    let Some(previous) = rest.chars().next_back() else {
                        break;
                    };
                    if !measure_seen && matches!(previous, '份' | '个' | '张' | '篇' | '部') {
                        measure_seen = true;
                        cursor = trimmed - previous.len_utf8();
                        continue;
                    }
                    break;
                }
                let rest = text[..cursor].trim_end();
                let count: i64 = match rest.chars().next_back() {
                    Some('一') => 1,
                    Some('二') | Some('两') => 2,
                    Some('三') => 3,
                    Some('四') => 4,
                    Some('五') => 5,
                    Some('六') => 6,
                    Some('七') => 7,
                    Some('八') => 8,
                    Some('九') => 9,
                    Some('十') => 10,
                    Some(digit) if digit.is_ascii_digit() => {
                        let mut number = String::new();
                        let mut end_num = rest.len();
                        for character in rest.chars().rev() {
                            if character.is_ascii_digit() {
                                number.insert(0, character);
                                end_num -= character.len_utf8();
                            } else {
                                break;
                            }
                        }
                        // Avoid swallowing a year or a long numeric id.
                        if number.len() > 2
                            || !is_word_boundary(text[..end_num].chars().next_back())
                        {
                            continue;
                        }
                        number.parse().unwrap_or(0)
                    }
                    _ => continue,
                };
                if !(1..=6).contains(&count) {
                    continue;
                }
                let explicit = explicit_kind_count(&seeds, kind) as i64;
                for ordinal in 1..=count {
                    if ordinal <= explicit {
                        continue;
                    }
                    slot_for_keyword += 1;
                    push(
                        &mut seeds,
                        &mut used_keys,
                        DeliveryChecklistSeed {
                            item_key: format!("slot:{expected_extension}:{slot_for_keyword}"),
                            target_path: None,
                            artifact_id: None,
                            display_name: format!("{keyword} 成果 {ordinal}/{count}"),
                            checks: kind
                                .planned_checks()
                                .iter()
                                .map(|value| (*value).to_owned())
                                .collect(),
                            requirements: Vec::new(),
                        },
                    );
                    if seeds.len() >= MAX_SEEDED_ITEMS {
                        break 'occurrences;
                    }
                }
            }
        }
    }

    // Deterministic order: explicit files first, then counted slots.
    seeds.sort_by(|left, right| left.item_key.cmp(&right.item_key));
    seeds
}

/// One item's deterministic verification result.
#[derive(Debug, Clone)]
pub(crate) struct ItemVerdict {
    pub item_key: String,
    pub passed: bool,
    pub finding_json: String,
    pub bound_path: Option<PathBuf>,
}

/// What the stop handler should do after the model's reply ended.
#[derive(Debug)]
pub(crate) enum DeliveryStop {
    /// Not a file-producing task (no checklist rows): normal stop handling.
    NoChecklist,
    /// Every promised artifact passed its deterministic checks.
    Passed { items: Vec<ItemVerdict> },
    /// Checks failed but the bounded repair budget is exhausted.
    Exhausted { items: Vec<ItemVerdict> },
    /// Checks failed; one more concrete repair request is armed.
    Repair {
        items: Vec<ItemVerdict>,
        prompt: String,
        findings_json: String,
    },
}

impl DeliveryStop {
    pub(crate) fn items(&self) -> &[ItemVerdict] {
        match self {
            DeliveryStop::NoChecklist => &[],
            DeliveryStop::Passed { items }
            | DeliveryStop::Exhausted { items }
            | DeliveryStop::Repair { items, .. } => items,
        }
    }
}

/// Run the delivery gate at a model stop. The live loop decides ordering
/// versus the generic stop-review; this function only reads durable facts and
/// the filesystem. Repair budget is derived from committed continuation
/// events tagged `delivery_repair`.
pub(crate) fn evaluate_stop(
    database: &Database,
    project_root: Option<&str>,
    run_id: &str,
) -> Result<DeliveryStop, String> {
    let checklist = database.delivery_checklist(run_id)?;
    if checklist.is_empty() {
        return Ok(DeliveryStop::NoChecklist);
    }
    let root = project_root.map(PathBuf::from);
    let started_at = database.run_created_at(run_id)?;
    // Structured demands the task text states in a machine-decidable form. When
    // it states none, every content demand it made is reported as unverified
    // rather than assumed satisfied by "the file opens".
    let requirements = stored_requirements(&database.delivery_requirements(run_id)?);

    // Real artifacts Host verified when tool calls settled (strongest evidence)
    // plus supported files produced under the project root during this Run
    // (catches files made by delegated child Runs).
    let mut candidates = artifact_candidates(database, run_id, root.as_deref())?;
    if let Some(root) = &root {
        scan_candidates(root, started_at, &mut candidates)?;
    }

    let mut verdicts = Vec::with_capacity(checklist.len());
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    for item in &checklist {
        verdicts.push(verify_item(
            database,
            run_id,
            item,
            root.as_deref(),
            &candidates,
            &mut assigned,
        )?);
    }
    apply_delivery_requirements(run_id, &checklist, &requirements, root.as_deref(), &mut verdicts)?;
    if verdicts.iter().all(|verdict| verdict.passed) {
        return Ok(DeliveryStop::Passed { items: verdicts });
    }
    // The repair budget ledger is written by the live loop right after the
    // continuation decision commits; kernel event lane counters stay
    // independent for the stop-review accounting and UI projections.
    let repairs = database.delivery_repair_round_count(run_id)?;
    if repairs >= DELIVERY_REPAIR_LIMIT {
        return Ok(DeliveryStop::Exhausted { items: verdicts });
    }
    let findings: Vec<Value> = verdicts
        .iter()
        .filter(|verdict| !verdict.passed)
        .map(|verdict| {
            serde_json::from_str(&verdict.finding_json)
                .unwrap_or_else(|_| json!({"reason": verdict.finding_json}))
        })
        .collect();
    let findings_json = serde_json::to_string(&findings).map_err(|error| error.to_string())?;
    Ok(DeliveryStop::Repair {
        items: verdicts,
        prompt: repair_prompt(&findings),
        findings_json,
    })
}

/// Persist item results and (for repairs) the bounded repair audit row. Called
/// by the live loop **after** the Kernel decision commits, so a crash before
/// this point fails closed (no phantom pass) instead of rewriting history; the
/// repair *budget* itself is derived from committed continuation events.
pub(crate) fn persist_outcome(
    database: &Database,
    run_id: &str,
    stop: &DeliveryStop,
    now: i64,
) -> Result<(), String> {
    for item in stop.items() {
        database.record_delivery_check(
            run_id,
            &item.item_key,
            item.passed,
            Some(&item.finding_json),
            now,
        )?;
    }
    if let DeliveryStop::Repair { findings_json, .. } = stop {
        database.record_delivery_repair_round(run_id, findings_json, now)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    artifact_id: Option<String>,
    artifact_sha: Option<String>,
    source: &'static str,
    modified_ms: i64,
}

fn artifact_candidates(
    database: &Database,
    run_id: &str,
    root: Option<&Path>,
) -> Result<Vec<Candidate>, String> {
    let rows: Vec<DeliveryArtifactRow> = database.run_artifacts(run_id)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let stored = PathBuf::from(&row.storage_path);
            let path = match (stored.is_absolute(), root) {
                (false, Some(root)) => root.join(&row.storage_path),
                _ => stored,
            };
            Candidate {
                path,
                artifact_id: Some(row.id),
                artifact_sha: row.sha256,
                source: "artifact",
                modified_ms: row.created_at,
            }
        })
        .collect())
}

fn scan_candidates(
    root: &Path,
    started_at: i64,
    candidates: &mut Vec<Candidate>,
) -> Result<(), String> {
    let mut known: BTreeSet<String> = candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.starts_with('.')
                || name.starts_with("~$")
                || name.contains(".fox-backup-")
                || matches!(name, "node_modules" | "target" | ".git")
            {
                continue;
            }
            if path.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("");
            if ArtifactKind::from_extension(extension).is_none() {
                continue;
            }
            let modified_ms = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(modified_ms_since_epoch)
                .unwrap_or(i64::MAX);
            // Filesystem timestamps have second granularity on Windows; tolerate
            // five seconds of clock wobble versus the Run start.
            if modified_ms != i64::MAX && modified_ms + 5_000 < started_at {
                continue;
            }
            if known.insert(name.to_owned()) {
                candidates.push(Candidate {
                    path,
                    artifact_id: None,
                    artifact_sha: None,
                    source: "scan",
                    modified_ms,
                });
            }
        }
    }
    Ok(())
}

fn modified_ms_since_epoch(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

#[allow(clippy::too_many_arguments)]
fn verify_item(
    database: &Database,
    run_id: &str,
    item: &DeliveryChecklistItem,
    root: Option<&Path>,
    candidates: &[Candidate],
    assigned: &mut BTreeSet<String>,
) -> Result<ItemVerdict, String> {
    // Pathless slots carry their expected extension in the key
    // (`slot:xlsx:1`); explicit items carry it in the target path.
    let kind = item
        .target_path
        .as_deref()
        .and_then(|path| Path::new(path).extension())
        .and_then(|value| value.to_str())
        .or_else(|| {
            item.item_key
                .strip_prefix("slot:")
                .and_then(|rest| rest.split(':').next())
        })
        .and_then(ArtifactKind::from_extension)
        .unwrap_or(ArtifactKind::Other);

    // A previously bound slot keeps verifying the exact same file even when
    // newer files appear during later repair rounds.
    let bound = if let Some(target) = item.target_path.as_deref() {
        let path = resolve_under_root(root, target)?;
        candidates
            .iter()
            .find(|candidate| candidate.path == path)
            .cloned()
            .or_else(|| {
                if path.is_file() {
                    Some(Candidate {
                        path,
                        artifact_id: item.artifact_id.clone(),
                        artifact_sha: None,
                        source: "declared",
                        modified_ms: 0,
                    })
                } else {
                    None
                }
            })
    } else {
        bind_pathless_slot(kind, candidates, assigned)
    };

    // The first successful slot binding is made durable so later rounds
    // re-verify the same artifact instead of drifting to a newer file.
    if item.target_path.is_none() {
        if let Some(candidate) = &bound {
            if let Some(relative) = root.and_then(|root| candidate.path.strip_prefix(root).ok()) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                database.bind_delivery_item(
                    run_id,
                    &item.item_key,
                    &relative,
                    candidate.artifact_id.as_deref(),
                )?;
            }
        }
    }

    let Some(candidate) = bound else {
        return Ok(missing_verdict(item));
    };

    let mut checks = serde_json::Map::new();
    let mut stats = json!({});
    let mut failure_reason: Option<String> = None;

    match fs::metadata(&candidate.path) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
            checks.insert("exists".to_owned(), json!("passed"));
            checks.insert("nonempty".to_owned(), json!("passed"));
        }
        Ok(_) => {
            checks.insert("exists".to_owned(), json!("passed"));
            checks.insert(
                "nonempty".to_owned(),
                json!({"state":"failed","reason":"文件为空"}),
            );
            failure_reason = Some(format!("交付项「{}」是空文件", item.display_name));
        }
        Err(error) => {
            checks.insert(
                "exists".to_owned(),
                json!({"state":"failed","reason":error.to_string()}),
            );
            failure_reason = Some(format!(
                "交付项「{}」的文件已不存在：{error}",
                item.display_name
            ));
        }
    }

    if failure_reason.is_none() && item.checks.iter().any(|name| name == "parseable") {
        match parse_check(&candidate.path, kind, &mut stats) {
            Ok(()) => {
                checks.insert("parseable".to_owned(), json!("passed"));
            }
            Err(reason) => {
                checks.insert(
                    "parseable".to_owned(),
                    json!({"state":"failed","reason":reason}),
                );
                failure_reason = Some(format!(
                    "交付项「{}」无法解析：{reason}",
                    item.display_name
                ));
            }
        }
    }
    if failure_reason.is_none()
        && item
            .checks
            .iter()
            .any(|name| name == "consistent_columns")
    {
        match csv_consistency(&candidate.path) {
            Ok(columns) => {
                stats["csvColumns"] = json!(columns);
                checks.insert("consistent_columns".to_owned(), json!("passed"));
            }
            Err(reason) => {
                checks.insert(
                    "consistent_columns".to_owned(),
                    json!({"state":"failed","reason":reason}),
                );
                failure_reason = Some(format!(
                    "交付项「{}」行列不一致：{reason}",
                    item.display_name
                ));
            }
        }
    }
    // When the artifact came from a settled tool result, the file on disk must
    // still be byte-identical to what Host verified: a real artifact binding,
    // not a path-shaped claim.
    if failure_reason.is_none() {
        if let Some(expected) = &candidate.artifact_sha {
            match sha256_file(&candidate.path) {
                Ok(actual) if &actual == expected => {
                    checks.insert("artifact_hash".to_owned(), json!("passed"));
                }
                Ok(actual) => {
                    checks.insert(
                        "artifact_hash".to_owned(),
                        json!({"state":"failed","expected":expected,"actual":actual}),
                    );
                    failure_reason = Some(format!(
                        "交付项「{}」与工具登记产物的内容哈希不一致",
                        item.display_name
                    ));
                }
                Err(error) => failure_reason = Some(format!("读取交付项失败：{error}")),
            }
        }
    }

    if failure_reason.is_none() {
        if let Err(reason) = content_floor(kind, &stats) {
            checks.insert(
                "nonempty".to_owned(),
                json!({"state":"failed","reason":reason}),
            );
            failure_reason = Some(format!("交付项「{}」{reason}", item.display_name));
        }
    }

    let passed = failure_reason.is_none();
    let finding = json!({
        "itemKey": item.item_key,
        "displayName": item.display_name,
        "boundPath": candidate.path.to_string_lossy(),
        "artifactId": candidate.artifact_id,
        "source": candidate.source,
        "checks": checks,
        "stats": stats,
        "reason": failure_reason,
    });
    Ok(ItemVerdict {
        item_key: item.item_key.clone(),
        passed,
        finding_json: finding.to_string(),
        bound_path: Some(candidate.path),
    })
}

fn missing_verdict(item: &DeliveryChecklistItem) -> ItemVerdict {
    let finding = json!({
        "itemKey": item.item_key,
        "displayName": item.display_name,
        "reason": format!("未找到与交付项「{}」绑定的真实产物（Run 期间项目目录内无匹配文件）", item.display_name),
        "checks": {},
        "stats": Value::Null,
    });
    ItemVerdict {
        item_key: item.item_key.clone(),
        passed: false,
        finding_json: finding.to_string(),
        bound_path: None,
    }
}

fn bind_pathless_slot(
    kind: ArtifactKind,
    candidates: &[Candidate],
    assigned: &mut BTreeSet<String>,
) -> Option<Candidate> {
    let mut matching = candidates
        .iter()
        .filter(|candidate| {
            let extension = candidate
                .path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            kind.extension_matches(extension)
                && !assigned.contains(&candidate.path.to_string_lossy().to_string())
        })
        .cloned()
        .collect::<Vec<_>>();
    // Prefer Host-verified artifacts over scan discoveries; within a source,
    // earliest production first (the order the Run actually made them).
    matching.sort_by(|left, right| {
        left.source
            .cmp(right.source)
            .then(left.modified_ms.cmp(&right.modified_ms))
            .then(left.path.cmp(&right.path))
    });
    matching.into_iter().find_map(|candidate| {
        let key = candidate.path.to_string_lossy().to_string();
        assigned.insert(key).then_some(candidate)
    })
}

fn resolve_under_root(root: Option<&Path>, target: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(target);
    let path = if candidate.is_absolute() {
        candidate
    } else {
        root.ok_or_else(|| "交付项使用了相对路径，但任务没有冻结项目目录".to_owned())?
            .join(candidate)
    };
    if let Some(root) = root {
        let normalized_root = normalize(root);
        let normalized = normalize(&path);
        if !normalized.starts_with(&normalized_root) {
            return Err(format!("交付路径越出冻结项目目录：{target}"));
        }
    }
    Ok(normalize(&path))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn parse_check(path: &Path, kind: ArtifactKind, stats: &mut Value) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_VERIFY_BYTES {
        return Err(format!(
            "文件超过 {} MiB 核验上限",
            MAX_VERIFY_BYTES / 1024 / 1024
        ));
    }
    match kind {
        ArtifactKind::Spreadsheet => {
            let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
                .map_err(|error| format!("工作簿无法打开：{error:?}"))?;
            let names = workbook.sheet_names().to_owned();
            if names.is_empty() {
                return Err("工作簿不包含任何工作表".to_owned());
            }
            let mut sheet_stats = Vec::new();
            let mut total_nonempty_rows = 0usize;
            for name in &names {
                let range = workbook
                    .worksheet_range(name)
                    .map_err(|error| format!("工作表 {name} 无法读取：{error:?}"))?;
                let nonempty_rows = range
                    .rows()
                    .filter(|row| row.iter().any(|cell| cell != &CellData::Empty))
                    .count();
                total_nonempty_rows += nonempty_rows;
                sheet_stats.push(json!({"name": name, "nonemptyRows": nonempty_rows,
                    "rows": range.height(), "columns": range.width()}));
            }
            stats["sheets"] = json!(sheet_stats);
            stats["nonemptyRows"] = json!(total_nonempty_rows);
            Ok(())
        }
        ArtifactKind::Pdf => {
            if bytes.len() < 8 || &bytes[..5] != b"%PDF-" {
                return Err("缺少 PDF 文件头（%PDF-）".to_owned());
            }
            // Count page objects without counting the page-tree "/Type/Pages".
            let mut pages = 0usize;
            for (index, window) in bytes.windows(b"/Type/Page".len()).enumerate() {
                if window == b"/Type/Page"
                    && bytes.get(index + b"/Type/Page".len()) != Some(&b's')
                {
                    pages += 1;
                }
            }
            for window in bytes.windows(b"/Type /Page".len()) {
                if window == b"/Type /Page" {
                    pages += 1;
                }
            }
            stats["pages"] = json!(pages);
            if pages == 0 {
                return Err("PDF 中未发现任何页面".to_owned());
            }
            Ok(())
        }
        ArtifactKind::Word | ArtifactKind::Slides => {
            let mime = if kind == ArtifactKind::Word {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            } else {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            };
            let document = crate::local_knowledge_import::ImportedDocument {
                display_name: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("delivery")
                    .to_owned(),
                relative_path: String::new(),
                content_hash: String::new(),
                file_size: bytes.len() as u64,
                mime_type: mime.to_owned(),
                stored_path: path.to_path_buf(),
            };
            let parsed = crate::local_knowledge_import::parse_imported_document(&document, 65536)
                .map_err(|error| format!("文档无法解析：{error}"))?;
            match parsed {
                crate::local_knowledge_import::ParsedDocument::Text { text, .. } => {
                    stats["textChars"] = json!(text.chars().count());
                    Ok(())
                }
                other => Err(format!("文档返回了意外的解析结果：{other:?}")),
            }
        }
        ArtifactKind::Json => {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            stats["jsonKind"] = json!(match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            });
            Ok(())
        }
        ArtifactKind::Text | ArtifactKind::Csv => {
            let text =
                String::from_utf8(bytes).map_err(|error| format!("不是合法 UTF-8：{error}"))?;
            let rows = text.lines().filter(|line| !line.trim().is_empty()).count();
            stats["textChars"] = json!(text.chars().count());
            stats["rows"] = json!(rows);
            Ok(())
        }
        ArtifactKind::Image => {
            let valid = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                || bytes.starts_with(b"\xFF\xD8\xFF")
                || bytes.starts_with(b"GIF87a")
                || bytes.starts_with(b"GIF89a")
                || bytes.starts_with(b"RIFF")
                || bytes.starts_with(b"<svg")
                || bytes.starts_with(b"<?xml");
            if !valid {
                return Err("图片文件头不合法".to_owned());
            }
            Ok(())
        }
        ArtifactKind::LegacyOffice | ArtifactKind::Other => Ok(()),
    }
}

fn content_floor(kind: ArtifactKind, stats: &Value) -> Result<(), String> {
    match kind {
        ArtifactKind::Spreadsheet => {
            if stats["nonemptyRows"].as_u64().unwrap_or(0) < 2 {
                return Err("缺少有效数据行（至少需要表头与一行数据）".to_owned());
            }
            Ok(())
        }
        ArtifactKind::Csv => {
            if stats["rows"].as_u64().unwrap_or(0) < 2 {
                return Err("缺少有效数据行（至少需要表头与一行数据）".to_owned());
            }
            Ok(())
        }
        ArtifactKind::Word | ArtifactKind::Slides | ArtifactKind::Text => {
            if stats["textChars"].as_u64().unwrap_or(0) == 0 {
                return Err("没有可读取的正文内容".to_owned());
            }
            Ok(())
        }
        ArtifactKind::Pdf => {
            if stats["pages"].as_u64().unwrap_or(0) == 0 {
                return Err("没有任何页面".to_owned());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn csv_consistency(path: &Path) -> Result<usize, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let rows = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Err("CSV 没有任何行".to_owned());
    }
    let columns = rows[0].split(',').count();
    for (index, row) in rows.iter().enumerate().skip(1) {
        let count = row.split(',').count();
        if count != columns {
            return Err(format!(
                "第 {} 行有 {count} 列，表头有 {columns} 列",
                index + 1
            ));
        }
    }
    Ok(columns)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

// ---------------------------------------------------------------------------
// Structured, independently verifiable requirements (review R6)
//
// The checklist above answers "was a real artifact produced and is it usable?".
// It deliberately does not answer "does it contain what the task demanded?".
// These requirements do — but only for demands the task states in a form a
// deterministic checker can decide (a counted number of charts, enumerated
// sections, a ratio that has to agree with a stated total). Anything else stays
// explicitly **unverified**: a task whose content demand cannot be parsed never
// silently passes as "verified".
//
// Expected values are always taken from the task text (or derived from the
// artifact's own declared total), never from a historical constant, and never
// from the artifact's own claim about itself.
// ---------------------------------------------------------------------------

/// Expected chart parts per artifact, counted inside the OOXML package.
const MAX_REQUIREMENT_CHARTS: usize = 64;
/// Upper bound on enumerated sections; a longer list is not a structured demand.
const MAX_REQUIREMENT_SECTIONS: usize = 12;
/// Number of leading document paragraphs searched for a required section label.
const SECTION_SCAN_PARAGRAPHS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequirementKind {
    /// At least `count` chart parts must be embedded in this artifact.
    Charts {
        count: usize,
        /// The demand is per worksheet, so it is checked against the artifact's
        /// own sheet count instead of the whole-file chart count.
        per_sheet: bool,
    },
    /// The document must contain every named section as a heading or leading
    /// standalone paragraph.
    Sections { names: Vec<String> },
    /// A stated ratio must be consistent with a total stated in the same task,
    /// in at least one delivered artifact.
    RatioConsistency { numerator: u64, total: u64 },
    /// A statistic recomputed from the source data the task named: the row
    /// distribution of `label_header` in `source` (optionally one `sheet`).
    SourceDistribution {
        source: String,
        /// Other readings of the same mention, tried in order when `source`
        /// names no existing file. Cleared once the requirement is bound.
        alternates: Vec<String>,
        source_sha256: String,
        label_header: String,
        sheet: Option<String>,
    },
    /// The task demanded statistics without naming a bindable source and column.
    SourceStatsUnbound { demand: String },
}

impl RequirementKind {
    fn id(&self) -> &'static str {
        match self {
            RequirementKind::Charts { .. } => "charts",
            RequirementKind::Sections { .. } => "sections",
            RequirementKind::RatioConsistency { .. } => "ratio_consistency",
            RequirementKind::SourceDistribution { .. } => "source_distribution",
            RequirementKind::SourceStatsUnbound { .. } => "source_statistics",
        }
    }

    /// The persisted form, which the checklist stores with the item it belongs
    /// to. One shape, one meaning: what was seeded is what gets verified.
    fn stored(&self) -> StoredRequirementKind {
        match self {
            RequirementKind::Charts { count, per_sheet } => StoredRequirementKind::Charts {
                count: *count,
                per_sheet: *per_sheet,
            },
            RequirementKind::Sections { names } => StoredRequirementKind::Sections {
                names: names.clone(),
            },
            RequirementKind::RatioConsistency { numerator, total } => {
                StoredRequirementKind::RatioConsistency {
                    numerator: *numerator,
                    total: *total,
                }
            }
            RequirementKind::SourceDistribution {
                source,
                alternates,
                source_sha256,
                label_header,
                sheet,
            } => StoredRequirementKind::SourceDistribution {
                source: source.clone(),
                alternates: alternates.clone(),
                source_sha256: source_sha256.clone(),
                label_header: label_header.clone(),
                sheet: sheet.clone(),
            },
            RequirementKind::SourceStatsUnbound { demand } => {
                StoredRequirementKind::SourceStatsUnbound {
                    demand: demand.clone(),
                }
            }
        }
    }

    fn from_stored(kind: &StoredRequirementKind) -> RequirementKind {
        match kind {
            StoredRequirementKind::Charts { count, per_sheet } => RequirementKind::Charts {
                count: *count,
                per_sheet: *per_sheet,
            },
            StoredRequirementKind::Sections { names } => RequirementKind::Sections {
                names: names.clone(),
            },
            StoredRequirementKind::RatioConsistency { numerator, total } => {
                RequirementKind::RatioConsistency {
                    numerator: *numerator,
                    total: *total,
                }
            }
            StoredRequirementKind::SourceDistribution {
                source,
                alternates,
                source_sha256,
                label_header,
                sheet,
            } => RequirementKind::SourceDistribution {
                source: source.clone(),
                alternates: alternates.clone(),
                source_sha256: source_sha256.clone(),
                label_header: label_header.clone(),
                sheet: sheet.clone(),
            },
            StoredRequirementKind::SourceStatsUnbound { demand } => {
                RequirementKind::SourceStatsUnbound {
                    demand: demand.clone(),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryRequirement {
    /// Checklist item this requirement belongs to; `None` means it spans items.
    pub item_key: Option<String>,
    pub id: String,
    pub kind: RequirementKind,
    /// The phrase in the task that produced this requirement, echoed in findings
    /// so the user can see what was demanded and what was verified.
    pub source_text: String,
}

/// Requirements the task text makes explicit. Conservative by construction:
/// an unparsed or ambiguous demand produces no requirement, and callers must
/// then report that demand as unverified rather than as satisfied.
pub(crate) fn requirements_from_task(text: &str) -> Vec<DeliveryRequirement> {
    let mut requirements = Vec::new();
    if text.trim().is_empty() {
        return requirements;
    }
    // Chart demands: only an explicitly bounded cardinal number counts, and a
    // per-worksheet demand keeps that granularity all the way to the check.
    if let Some((count, per_sheet)) = chart_count_request(text) {
        requirements.push(DeliveryRequirement {
            item_key: None,
            id: if per_sheet {
                format!("charts>=per-sheet:{count}")
            } else {
                format!("charts>={count}")
            },
            kind: RequirementKind::Charts { count, per_sheet },
            source_text: if per_sheet {
                format!("任务要求每个工作表至少 {count} 张图表")
            } else {
                format!("任务要求图表：至少 {count} 张")
            },
        });
    }
    // Named sections, in task order, de-duplicated.
    let sections = section_names_request(text);
    if !sections.is_empty() {
        requirements.push(DeliveryRequirement {
            item_key: None,
            id: format!("sections:{}", sections.len()),
            kind: RequirementKind::Sections {
                names: sections.clone(),
            },
            source_text: format!("任务要求章节：{}", sections.join("、")),
        });
    }
    // A ratio that must agree with a total stated in the same task.
    for (numerator, total) in ratio_pairs(text) {
        requirements.push(DeliveryRequirement {
            item_key: None,
            id: format!("ratio:{numerator}/{total}"),
            kind: RequirementKind::RatioConsistency { numerator, total },
            source_text: format!("任务声明比例 {numerator}/{total}"),
        });
    }
    // Statistics the task asks to compute from its own source data. The binding
    // (which file, which column, its bytes) is resolved by the Host against the
    // project folder, because only the Host has the authorized root; when the
    // task does not name both, the demand is recorded as **unbound** so the
    // report says 未核验 instead of staying silent.
    if let Some(demand) = source_statistics_demand(text) {
        requirements.push(demand);
    }
    requirements
}

/// Verbs that introduce a file as *input* to read, so a name introduced this way
/// is source data rather than something the task asks Fox to produce.
const SOURCE_INPUT_VERBS: &[&str] = &[
    "读取", "读", "基于", "根据", "依据", "参照", "按照", "依照", "输入", "来自", "来源",
    "分析", "解析",
];

/// What the task asks to compute from source data, as a requirement.
///
/// Two outcomes only, both explicit:
/// * a `SourceStatsUnbound` demand when statistics are requested but the task
///   does not name a data file together with the column to aggregate — the Host
///   then reports 未核验 rather than pretending "the file opens" is statistics;
/// * nothing at all when the task asks for no statistics.
///
/// The bindable form is produced by [`bind_source_distribution`], which the Host
/// calls with the project root (it needs the file's bytes and hash).
pub(crate) fn source_statistics_demand(text: &str) -> Option<DeliveryRequirement> {
    if !asks_for_statistics(text) {
        return None;
    }
    let (candidates, header) = source_statistics_binding(text)?;
    match (candidates.split_first(), header) {
        (Some((source, alternates)), Some(label_header)) => {
            let source_text = format!("任务要求按源数据统计：{source}");
            Some(DeliveryRequirement {
                item_key: None,
                id: format!("source:{source}#{label_header}"),
                kind: RequirementKind::SourceDistribution {
                    source: source.clone(),
                    alternates: alternates.to_vec(),
                    // Filled in by the Host, which can read the file.
                    source_sha256: String::new(),
                    label_header,
                    sheet: None,
                },
                source_text,
            })
        }
        _ => Some(DeliveryRequirement {
            item_key: None,
            id: "source:unbound".to_owned(),
            kind: RequirementKind::SourceStatsUnbound {
                demand: text.trim().chars().take(200).collect(),
            },
            source_text: "任务要求统计，但未指明可核验的源文件与统计列".to_owned(),
        }),
    }
}

/// True when the task asks for computed statistics rather than a plain artifact.
fn asks_for_statistics(text: &str) -> bool {
    ["统计", "汇总", "分布", "占比", "比例", "频次"]
        .iter()
        .any(|marker| text.contains(marker))
}

/// The source file candidates and aggregation column the task names.
///
/// Deliberately narrow: a statistic is only bindable when the task says which
/// data file to read and which column to group by ("按 X 列/字段/分组"). Anything
/// vaguer stays unbound, because guessing the口径 would manufacture expectations
/// the user never stated.
///
/// More than one candidate name can come out of one mention: Chinese prose and
/// file names share their characters, so the parser reports both the
/// conservative reading (stopping at prose markers) and the maximal reading, and
/// the Host binds whichever one **really exists** under the project root. A
/// candidate that names no file is never used, so a wrong guess cannot become an
/// expectation.
fn source_statistics_binding(text: &str) -> Option<(Vec<String>, Option<String>)> {
    let deliverables = expectations_from_task(text)
        .into_iter()
        .filter_map(|seed| seed.target_path)
        .map(|path| path.rsplit('/').next().unwrap_or(&path).to_lowercase())
        .collect::<Vec<_>>();

    let mut candidates: Vec<String> = Vec::new();
    let lower = text.to_lowercase();
    for extension in ["xlsx", "xlsm", "csv", "tsv"] {
        let needle = format!(".{extension}");
        let mut from = 0;
        while let Some(relative) = lower[from..].find(&needle) {
            let end = from + relative + needle.len();
            from = end;
            // Two readings of the same mention, in order of preference. Both
            // include the extension itself.
            let strict = backtrack_name(text, end, true);
            let maximal = backtrack_name(text, end, false);
            for (name, strict_reading) in [(&strict, true), (&maximal, false)] {
                let name = name.trim();
                if name.len() <= needle.len()
                    || name.starts_with('.')
                    || name.contains("://")
                    || name.contains('@')
                {
                    continue;
                }
                let bare = name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase();
                // A file the task asks Fox to *produce* is a deliverable, not a
                // source — but the same name can also be introduced as input
                // ("读取 X" vs "生成 X"), so the verb in front of it decides for
                // the conservative reading. The maximal reading is offered too:
                // Chinese file names may contain the characters prose uses, and
                // only a name that exists on disk is ever bound.
                if strict_reading && !introduced_as_input(text, name) {
                    if deliverables.iter().any(|deliverable| deliverable == &bare) {
                        continue;
                    }
                }
                if !candidates.iter().any(|existing| existing == name) {
                    candidates.push(name.to_owned());
                }
            }
            if !candidates.is_empty() {
                break;
            }
        }
        if !candidates.is_empty() {
            break;
        }
    }

    // The aggregation column, in the forms a person actually writes.
    let mut header = None;
    for marker in ["列", "字段", "分组", "维度"] {
        let mut from = 0;
        while let Some(relative) = text[from..].find(marker) {
            let at = from + relative;
            from = at + marker.len();
            // Walk back to the start of the name that precedes the marker,
            // stopping at the verb that introduces it.
            let head = &text[..at];
            let cut = head
                .char_indices()
                .rev()
                .find(|(_, character)| {
                    matches!(character, '按' | '以' | '：' | ':' | '，' | ',' | '（' | '(' | '、')
                })
                .map(|(index, character)| index + character.len_utf8())
                .unwrap_or(0);
            let candidate = head[cut..].trim();
            let candidate = candidate
                .trim_matches(|character: char| {
                    matches!(character, '“' | '”' | '"' | '\'' | '《' | '》')
                })
                .trim();
            if candidate.chars().count() >= 2
                && candidate.chars().count() <= 16
                && !candidate.chars().any(|character| character.is_ascii_digit())
                && !candidate.contains(char::is_whitespace)
            {
                header = Some(candidate.to_owned());
                break;
            }
        }
        if header.is_some() {
            break;
        }
    }
    if candidates.is_empty() {
        return Some((candidates, header));
    }
    Some((candidates, header))
}

/// The name that ends at `end`, walking back over filename characters. With
/// `stop_at_prose` the walk also stops at the CJK characters that mark prose
/// before a name (`生成AGV报告.xlsx`); without it, only whitespace and
/// punctuation stop the walk, so a name containing such a character is still
/// found.
fn backtrack_name(text: &str, end: usize, stop_at_prose: bool) -> String {
    let mut start = end;
    let preceding: Vec<(usize, char)> = text
        .char_indices()
        .rev()
        .filter(|(index, _)| *index < start)
        .collect();
    for (index, character) in preceding {
        let allowed = if stop_at_prose {
            is_filename_character(character) && !character.is_whitespace()
        } else {
            is_source_name_character(character)
        };
        if allowed {
            start = index;
        } else {
            break;
        }
    }
    text[start..end].to_owned()
}

/// A character that can appear inside a data file's name: letters, digits and
/// the characters Chinese file names actually use, but nothing that ends prose.
fn is_source_name_character(character: char) -> bool {
    if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | '(' | ')') {
        return true;
    }
    ('\u{4e00}'..='\u{9fff}').contains(&character)
        && !matches!(
            character,
            '，' | '。' | '；' | '：' | '！' | '？' | '、' | '“' | '”' | '（' | '）' | '《' | '》'
        )
}

/// True when the token in front of `name` introduces it as input to read.
fn introduced_as_input(text: &str, name: &str) -> bool {
    let Some(start) = text.find(name) else {
        return false;
    };
    let tail = text[..start].trim_end();
    let tail = tail
        .char_indices()
        .rev()
        .take(6)
        .map(|(index, _)| index)
        .last()
        .map(|index| &tail[index..])
        .unwrap_or(tail);
    SOURCE_INPUT_VERBS.iter().any(|verb| tail.ends_with(verb))
}

/// Bind a `SourceDistribution` requirement to a real file under the project
/// root: existence, hash and a column that is really present. Returns the bound
/// requirement, or an explicit unbound one.
pub(crate) fn bind_source_distribution(
    root: &Path,
    requirement: DeliveryRequirement,
) -> DeliveryRequirement {
    let RequirementKind::SourceDistribution {
        source,
        alternates,
        label_header,
        ..
    } = &requirement.kind
    else {
        return requirement;
    };
    // The parser cannot know which reading of the mention is a real file; the
    // Host can. Only an existing file with the declared column is ever bound, so
    // a wrong reading cannot become an expectation.
    let mut rejected: Vec<String> = Vec::new();
    for candidate in std::iter::once(source).chain(alternates.iter()) {
        let path = root.join(candidate.replace('\\', "/"));
        let Ok(bytes) = fs::read(&path) else {
            rejected.push(candidate.clone());
            continue;
        };
        if let Err(reason) = source_distribution_records(&bytes, None, label_header) {
            return unbound_requirement(format!(
                "源文件 {candidate} 中无法按「{label_header}」统计：{reason}"
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = hex::encode(hasher.finalize());
        return DeliveryRequirement {
            item_key: requirement.item_key,
            id: format!("source:{candidate}#{label_header}"),
            kind: RequirementKind::SourceDistribution {
                source: candidate.clone(),
                // Bound: the alternatives have served their purpose.
                alternates: Vec::new(),
                source_sha256: sha256,
                label_header: label_header.clone(),
                sheet: None,
            },
            source_text: requirement.source_text,
        };
    }
    unbound_requirement(format!(
        "任务按源文件统计，但项目目录中找不到该文件（尝试过：{}）",
        rejected.join("、")
    ))
}

fn unbound_requirement(demand: String) -> DeliveryRequirement {
    DeliveryRequirement {
        item_key: None,
        id: "source:unbound".to_owned(),
        kind: RequirementKind::SourceStatsUnbound { demand },
        source_text: "任务要求统计，但未能绑定可核验的源数据与统计列".to_owned(),
    }
}

/// Rows scanned for the header, so a stray value deep in a sheet cannot define
/// the口径.
const SOURCE_HEADER_SCAN_ROWS: usize = 20;
/// Rounding allowance when comparing a stated share with the share the source
/// implies (percentage points).
const SOURCE_RATIO_TOLERANCE_PERCENT: f64 = 0.5;

/// Row distribution of `label_header` in one source workbook.
///
/// Independent of every artifact: the counts come from the source records
/// themselves (one record per non-empty cell under the header), so a summary
/// that repeats an old number cannot verify itself.
fn source_distribution_records(
    bytes: &[u8],
    sheet: Option<&str>,
    label_header: &str,
) -> Result<Vec<(String, u64)>, String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|error| format!("源工作簿无法打开：{error:?}"))?;
    let names = workbook.sheet_names().to_owned();
    let candidates: Vec<String> = match sheet {
        Some(name) => names
            .iter()
            .filter(|candidate| candidate.as_str() == name)
            .cloned()
            .collect(),
        None => names.clone(),
    };
    if candidates.is_empty() {
        return Err(match sheet {
            Some(name) => format!("源工作簿中没有工作表 {name}"),
            None => "源工作簿没有工作表".to_owned(),
        });
    }
    let mut last_error = String::new();
    for name in candidates {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        let rows = range
            .rows()
            .take(SOURCE_HEADER_SCAN_ROWS)
            .enumerate()
            .collect::<Vec<_>>();
        let Some((header_row, header_column)) = rows.iter().find_map(|(index, row)| {
            row.iter()
                .position(|cell| cell_text(cell).trim() == label_header.trim())
                .map(|column| (*index, column))
        }) else {
            last_error = format!("工作表 {name} 没有「{label_header}」表头");
            continue;
        };
        let mut counts: Vec<(String, u64)> = Vec::new();
        for row in range.rows().skip(header_row + 1) {
            let label = row.get(header_column).map(cell_text).unwrap_or_default();
            let label = label.trim();
            if label.is_empty() {
                continue;
            }
            match counts.iter_mut().find(|(existing, _)| existing == label) {
                Some((_, count)) => *count += 1,
                None => counts.push((label.to_owned(), 1)),
            }
        }
        if counts.is_empty() {
            last_error = format!("工作表 {name} 在「{label_header}」列下没有记录");
            continue;
        }
        return Ok(counts);
    }
    Err(if last_error.is_empty() {
        "源工作簿中没有匹配的工作表".to_owned()
    } else {
        last_error
    })
}

/// A cardinal number written in digits or Chinese numerals, at a text offset.
fn parse_cardinal_at(text: &str, end: usize) -> Option<(u64, usize)> {
    let head = &text[..end];
    let mut digits = String::new();
    let mut cursor = end;
    for character in head.chars().rev() {
        if character.is_ascii_digit() {
            digits.insert(0, character);
            cursor -= character.len_utf8();
            if digits.len() > 6 {
                return None;
            }
        } else {
            break;
        }
    }
    if !digits.is_empty() {
        // A run longer than six digits is an id or a year fragment, not a count.
        let value = digits.parse::<u64>().ok()?;
        return (value > 0 && value <= 4096).then_some((value, cursor));
    }
    let previous = head.chars().next_back()?;
    let value = match previous {
        '一' | '两' => 1,
        '二' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '七' => 7,
        '八' => 8,
        '九' => 9,
        '十' => 10,
        _ => return None,
    };
    Some((value as u64, cursor - previous.len_utf8()))
}

/// How many chart parts the task demands, and whether it demanded them per
/// worksheet ("每个 Sheet 配一张图表").
///
/// The granularity is part of the demand: a whole-file chart count cannot verify
/// a per-sheet requirement, so it travels with the requirement instead of being
/// flattened to "at least one chart somewhere".
fn chart_count_request(text: &str) -> Option<(usize, bool)> {
    let lowered = text.to_lowercase();
    let per_sheet_marker = lowered.contains("每个 sheet")
        || lowered.contains("每个工作表")
        || lowered.contains("每张工作表")
        || lowered.contains("每个sheet")
        || lowered.contains("每张表")
        || lowered.contains("每个表格")
        || lowered.contains("per sheet")
        || lowered.contains("each sheet")
        || lowered.contains("每页");
    for keyword in ["图表", "chart", "图"] {
        let mut from = 0;
        while let Some(relative) = lowered[from..].find(keyword) {
            let at = from + relative;
            from = at + keyword.len();
            // Look back over an optional measure word ("张/幅/个").
            let mut cursor = at;
            if let Some(previous) = text[..cursor].chars().next_back() {
                if matches!(previous, '张' | '幅' | '个') {
                    cursor -= previous.len_utf8();
                }
            }
            if let Some((count, _)) = parse_cardinal_at(text, cursor) {
                let count = count as usize;
                if (1..=MAX_REQUIREMENT_CHARTS).contains(&count) {
                    return Some((count, per_sheet_marker));
                }
            }
        }
    }
    None
}

/// Enumerated section names, e.g. "包含：分析概述、统计结果、结论与建议 三个章节".
fn section_names_request(text: &str) -> Vec<String> {
    const SECTION_MARKERS: &[&str] = &[
        "章节", "部分", "小节", "section", "Section",
    ];
    for marker in SECTION_MARKERS {
        let mut from = 0;
        while let Some(relative) = text[from..].find(marker) {
            let at = from + relative;
            from = at + marker.len();
            // Walk back to the start of the enumeration.
            let head = &text[..at];
            let start = head
                .char_indices()
                .rev()
                .find(|(_, character)| matches!(character, '：' | ':' | '，' | ',' | '。' | '\n' | '（' | '('))
                .map(|(index, character)| index + character.len_utf8())
                .unwrap_or(0);
            let segment = &head[start..];
            let names = split_name_list(segment);
            if names.len() >= 2 && names.len() <= MAX_REQUIREMENT_SECTIONS {
                return names;
            }
        }
    }
    Vec::new()
}

fn split_name_list(segment: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for raw in segment.split(['、', ',', '，', '/', '和', '及']) {
        let name = raw
            .trim()
            .trim_start_matches(|character: char| {
                character.is_ascii_digit() || matches!(character, '第' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十')
            })
            .trim_matches(|character: char| matches!(character, '《' | '》' | '“' | '”' | '"'))
            .trim();
        // A trailing counter ("三个章节") belongs to the demand's shape, not to
        // the section's name.
        let name = strip_trailing_counter(name);
        if name.chars().count() < 2
            || name.chars().count() > 24
            || name.chars().any(|character| character.is_ascii_digit())
        {
            continue;
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// Drop a trailing measure-word counter from a name candidate.
fn strip_trailing_counter(value: &str) -> &str {
    let trimmed = value.trim();
    let Some(without_measure) = trimmed.strip_suffix(['个', '份', '项', '节', '章']) else {
        return trimmed;
    };
    let stripped = without_measure.trim_end().trim_end_matches(|character: char| {
        character.is_ascii_digit()
            || matches!(character, '一' | '二' | '两' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十')
    });
    stripped.trim_end()
}

/// `<numerator>/<total>`-shaped statements, e.g. "卸船 548 台，占总计 927 台的 59.12%".
///
/// Only a *single clause* that names both counts and a ratio word produces a
/// requirement: taking the largest number in the whole task as the denominator
/// let dates, ids and unrelated counts be read as the total of an unrelated
/// share.
fn ratio_pairs(text: &str) -> Vec<(u64, u64)> {
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    for clause in text.split(['。', '；', ';', '\n', '！', '？', '!', '?']) {
        let mut pieces: Vec<&str> = vec![clause];
        // A comma usually separates the count from its ratio, so try both the
        // whole sentence and each comma-separated part.
        pieces.extend(clause.split(['，', ',']));
        for piece in pieces {
            let lowered = piece.to_lowercase();
            let has_ratio_word = ["占", "比例", "占比", "proportion", "percent", "%"]
                .iter()
                .any(|marker| lowered.contains(marker));
            if !has_ratio_word {
                continue;
            }
            let mut values = stated_cardinals(piece);
            values.sort_unstable();
            values.dedup();
            if values.len() < 2 {
                continue;
            }
            let total = *values.last().expect("non-empty");
            for numerator in values.iter().take(values.len() - 1) {
                if *numerator == 0 || *numerator >= total {
                    continue;
                }
                if !pairs.contains(&(*numerator, total)) {
                    pairs.push((*numerator, total));
                }
                if pairs.len() >= 8 {
                    return pairs;
                }
            }
        }
    }
    pairs
}

fn stated_cardinals(text: &str) -> Vec<u64> {
    let mut values = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index - start <= 6 {
            if let Ok(value) = text[start..index].parse::<u64>() {
                // A count carries a unit or a measure word right after it; a
                // bare number inside prose is not a stated total. A percentage
                // is a share, not a count, so it is excluded here.
                let follows = text[index..].chars().next();
                if matches!(follows, Some(' ') | Some('台') | Some('个') | Some('条') | Some('项')
                    | Some('次') | Some('辆') | Some('张') | Some('份')) {
                    values.push(value);
                }
            }
        }
    }
    values
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactFacts {
    pub item_key: String,
    pub display_name: String,
    pub path: PathBuf,
    pub kind: ArtifactKind,
    /// Chart parts found in the OOXML package.
    pub charts: usize,
    /// Sheet names (spreadsheets only).
    pub sheets: Vec<String>,
    /// Charts attributed to each worksheet through the package's own
    /// relationships. `None` means that chain could not be read — an unknown
    /// state, not "zero charts".
    pub sheet_charts: Option<SheetChartCounts>,
}

/// Charts attached to each worksheet, resolved through the OOXML relationship
/// chain a viewer follows: `workbook.xml` -> sheet part -> its drawing(s) -> the
/// chart parts those drawings reference.
///
/// A chart part that no worksheet references is *not* attributable to a sheet,
/// so it cannot satisfy a per-sheet demand even though it exists in the package.
#[derive(Debug, Clone, Default)]
pub(crate) struct SheetChartCounts {
    /// `(sheet name, number of chart parts that sheet's drawings reference)`.
    pub per_sheet: Vec<(String, usize)>,
    /// Chart parts present in the package but unreachable from any worksheet.
    pub orphan_charts: usize,
}

impl SheetChartCounts {
    /// The fewest charts any one sheet carries.
    pub(crate) fn min_per_sheet(&self) -> Option<usize> {
        self.per_sheet.iter().map(|(_, count)| *count).min()
    }
}

/// Structural facts read straight out of the artifact's own package. This is
/// deliberately independent of anything the artifact or the model *says* about
/// itself.
pub(crate) fn collect_artifact_facts(
    item_key: &str,
    display_name: &str,
    path: &Path,
) -> Result<ArtifactFacts, String> {
    let kind = path
        .extension()
        .and_then(|value| value.to_str())
        .and_then(ArtifactKind::from_extension)
        .unwrap_or(ArtifactKind::Other);
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_VERIFY_BYTES {
        return Err("文件超过核验上限".to_owned());
    }
    let charts = ooxml_chart_parts(&bytes)
        .map(|names| names.len())
        .unwrap_or(0);
    let sheets = if kind == ArtifactKind::Spreadsheet {
        open_workbook_auto_from_rs(Cursor::new(bytes.clone()))
            .map(|workbook| workbook.sheet_names().to_owned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Per-sheet chart attribution, resolved through the package's own
    // relationships. `None` means the relation chain could not be read, which is
    // reported as *unverified* rather than as "no charts" or as a pass.
    let sheet_charts = if kind == ArtifactKind::Spreadsheet {
        ooxml_sheet_charts(&bytes).ok()
    } else {
        None
    };
    Ok(ArtifactFacts {
        item_key: item_key.to_owned(),
        display_name: display_name.to_owned(),
        path: path.to_path_buf(),
        kind,
        charts,
        sheets,
        sheet_charts,
    })
}

fn ooxml_chart_parts(bytes: &[u8]) -> Result<Vec<String>, String> {
    Ok(crate::local_knowledge_import::zip_entry_names(bytes)?
        .into_iter()
        .filter(|name| {
            (name.starts_with("xl/drawings/charts/chart") || name.starts_with("xl/charts/chart"))
                && name.ends_with(".xml")
        })
        .collect())
}

/// Charts attached to every worksheet of a workbook, following the same
/// relationship chain a spreadsheet viewer follows.
fn ooxml_sheet_charts(bytes: &[u8]) -> Result<SheetChartCounts, String> {
    let workbook = crate::local_knowledge_import::zip_entry_text(bytes, "xl/workbook.xml")?
        .ok_or_else(|| "workbook.xml missing".to_owned())?;
    let workbook_rels =
        crate::local_knowledge_import::zip_entry_text(bytes, "xl/_rels/workbook.xml.rels")?
            .ok_or_else(|| "workbook relationships missing".to_owned())?;
    let part_of = relationship_targets(&workbook_rels, "xl");
    let mut per_sheet = Vec::new();
    let mut attributed = 0usize;
    for (name, rel_id) in sheet_relationships(&workbook) {
        let Some(part) = part_of.get(&rel_id) else {
            return Err(format!("sheet {name} has no resolvable worksheet part"));
        };
        let sheet_xml = crate::local_knowledge_import::zip_entry_text(bytes, part)?
            .ok_or_else(|| format!("worksheet part {part} is missing"))?;
        let mut charts = 0usize;
        for drawing_rel in element_attribute_values(&sheet_xml, "drawing", "r:id") {
            let sheet_rels_path = rels_path_of(part)?;
            let sheet_rels = crate::local_knowledge_import::zip_entry_text(bytes, &sheet_rels_path)?
                .ok_or_else(|| format!("worksheet relationships missing for {part}"))?;
            let drawings_of = relationship_targets(&sheet_rels, parent_dir(part));
            let Some(drawing_part) = drawings_of.get(&drawing_rel) else {
                return Err(format!("drawing {drawing_rel} of {part} is unresolved"));
            };
            let drawing_xml = crate::local_knowledge_import::zip_entry_text(bytes, drawing_part)?
                .ok_or_else(|| format!("drawing part {drawing_part} is missing"))?;
            let chart_refs = element_attribute_values(&drawing_xml, "chart", "r:id");
            if chart_refs.is_empty() {
                continue;
            }
            let drawing_rels_path = rels_path_of(drawing_part)?;
            let drawing_rels =
                crate::local_knowledge_import::zip_entry_text(bytes, &drawing_rels_path)?
                    .ok_or_else(|| format!("drawing relationships missing for {drawing_part}"))?;
            let charts_of = relationship_targets(&drawing_rels, parent_dir(drawing_part));
            for chart_rel in chart_refs {
                let Some(chart_part) = charts_of.get(&chart_rel) else {
                    return Err(format!("chart {chart_rel} of {drawing_part} is unresolved"));
                };
                // The reference alone does not put a chart on the sheet: the part
                // has to exist.
                if crate::local_knowledge_import::zip_entry_text(bytes, chart_part)?.is_none() {
                    return Err(format!("chart part {chart_part} is missing"));
                }
                charts += 1;
            }
        }
        attributed += charts;
        per_sheet.push((name, charts));
    }
    let total = ooxml_chart_parts(bytes)?.len();
    Ok(SheetChartCounts {
        per_sheet,
        orphan_charts: total.saturating_sub(attributed),
    })
}

/// `<sheet name="..." r:id="..."/>` entries of `workbook.xml`, in document order.
fn sheet_relationships(workbook: &str) -> Vec<(String, String)> {
    let mut sheets = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative) = workbook[cursor..].find("<sheet ") {
        let start = cursor + relative;
        let Some(end_relative) = workbook[start..].find('>') else {
            break;
        };
        let element = &workbook[start..start + end_relative];
        cursor = start + end_relative;
        if let (Some(name), Some(rel_id)) =
            (attribute_value(element, "name"), attribute_value(element, "r:id"))
        {
            sheets.push((name, rel_id));
        }
    }
    sheets
}

/// Values of `attribute` for every `<local ...>` (or `<ns:local ...>`) element in
/// `xml`. OOXML uses namespace prefixes (`<c:chart>`), so the local name is
/// matched with or without one.
fn element_attribute_values(xml: &str, local: &str, attribute: &str) -> Vec<String> {
    let mut values = Vec::new();
    for needle in [format!("<{local} "), format!(":{local} ")] {
        let mut cursor = 0usize;
        while let Some(relative) = xml[cursor..].find(&needle) {
            let start = cursor + relative;
            let Some(end_relative) = xml[start..].find('>') else {
                break;
            };
            let element = &xml[start..start + end_relative];
            cursor = start + end_relative;
            if let Some(value) = attribute_value(element, attribute) {
                values.push(value);
            }
        }
        if !values.is_empty() {
            break;
        }
    }
    values
}

fn attribute_value(element: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=\"");
    let start = element.find(&needle)? + needle.len();
    let end = element[start..].find('"')? + start;
    Some(element[start..end].to_owned())
}

/// `Relationship` entries of a `.rels` document: `Id -> part path`, resolved
/// against `base` (the directory that part's own targets are relative to).
fn relationship_targets(rels: &str, base: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut cursor = 0usize;
    while let Some(relative) = rels[cursor..].find("<Relationship ") {
        let start = cursor + relative;
        let Some(end_relative) = rels[start..].find('>') else {
            break;
        };
        let element = &rels[start..start + end_relative];
        cursor = start + end_relative;
        if element.contains("TargetMode=\"External\"") {
            continue;
        }
        if let (Some(id), Some(target)) = (
            attribute_value(element, "Id"),
            attribute_value(element, "Target"),
        ) {
            map.insert(id, resolve_part_path(base, &target));
        }
    }
    map
}

/// Resolve an OOXML relationship target against `base` (targets may climb with
/// `../`).
fn resolve_part_path(base: &str, target: &str) -> String {
    let target = target.trim_start_matches('/');
    let mut segments: Vec<String> = base
        .trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_owned()),
        }
    }
    segments.join("/")
}

fn parent_dir(part: &str) -> &str {
    part.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// `xl/worksheets/sheet1.xml` -> `xl/worksheets/_rels/sheet1.xml.rels`
fn rels_path_of(part: &str) -> Result<String, String> {
    let (dir, file) = part
        .rsplit_once('/')
        .ok_or_else(|| format!("part {part} has no directory"))?;
    Ok(format!("{dir}/_rels/{file}.rels"))
}

/// Attach one real chart to **every worksheet** of an existing workbook, wired
/// through the same relationships a viewer follows (worksheet -> drawing ->
/// chart), including the package content types.
///
/// Tests need this to build a deliverable that genuinely satisfies a per-sheet
/// chart demand: a chart part that no worksheet references is not something the
/// user can see, and the gate is right to reject it.
#[cfg(test)]
pub(crate) fn zip_attach_chart_per_sheet(bytes: &[u8]) -> Vec<u8> {
    let mut entries = match crate::local_knowledge_import::zip_entries(bytes) {
        Ok(entries) => entries,
        Err(_) => return bytes.to_vec(),
    };
    let workbook = crate::local_knowledge_import::zip_entry_text(bytes, "xl/workbook.xml")
        .ok()
        .flatten()
        .unwrap_or_default();
    let workbook_rels =
        crate::local_knowledge_import::zip_entry_text(bytes, "xl/_rels/workbook.xml.rels")
            .ok()
            .flatten()
            .unwrap_or_default();
    let part_of = relationship_targets(&workbook_rels, "xl");
    let mut content_type_overrides: Vec<String> = Vec::new();
    for (index, (_name, rel_id)) in sheet_relationships(&workbook).into_iter().enumerate() {
        let Some(part) = part_of.get(&rel_id) else {
            continue;
        };
        let Some(sheet_xml) = crate::local_knowledge_import::zip_entry_text(bytes, part)
            .ok()
            .flatten()
        else {
            continue;
        };
        let drawing = index + 1;
        let rel_id_drawing = format!("rIdDrw{drawing}");
        if !sheet_xml.contains("<drawing ") {
            let Some(at) = sheet_xml.rfind("</worksheet>") else {
                continue;
            };
            let with_drawing = format!(
                r#"{}{}<drawing r:id="{rel_id_drawing}"/></worksheet>"#,
                &sheet_xml[..at],
                ""
            );
            if let Some(entry) = entries.iter_mut().find(|(name, _)| name == part) {
                entry.1 = with_drawing.into_bytes();
            }
        }
        let Ok(sheet_rels_path) = rels_path_of(part) else {
            continue;
        };
        let existing_rels = crate::local_knowledge_import::zip_entry_text(bytes, &sheet_rels_path)
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#.to_owned()
            });
        let rels = existing_rels.replacen(
            "</Relationships>",
            &format!(
                r#"<Relationship Id="{rel_id_drawing}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing{drawing}.xml"/></Relationships>"#
            ),
            1,
        );
        set_entry(&mut entries, &sheet_rels_path, rels);
        let drawing_part = format!("xl/drawings/drawing{drawing}.xml");
        set_entry(
            &mut entries,
            &drawing_part,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart r:id="rIdChart{drawing}"/></xdr:wsDr>"#
            ),
        );
        let drawing_rels = format!("xl/drawings/_rels/drawing{drawing}.xml.rels");
        set_entry(
            &mut entries,
            &drawing_rels,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdChart{drawing}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart{drawing}.xml"/></Relationships>"#
            ),
        );
        let chart_part = format!("xl/charts/chart{drawing}.xml");
        set_entry(
            &mut entries,
            &chart_part,
            r#"<?xml version="1.0" encoding="UTF-8"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#
                .to_owned(),
        );
        content_type_overrides.push(format!(
            r#"<Override PartName="/{drawing_part}" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/{chart_part}" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>"#
        ));
    }
    if !content_type_overrides.is_empty() {
        if let Some(existing) = crate::local_knowledge_import::zip_entry_text(
            bytes,
            "[Content_Types].xml",
        )
        .ok()
        .flatten()
        {
            if existing.contains("</Types>") {
                let updated = existing.replacen(
                    "</Types>",
                    &format!("{}</Types>", content_type_overrides.join("")),
                    1,
                );
                set_entry(&mut entries, "[Content_Types].xml", updated);
            }
        }
    }
    stored_zip(&entries)
}

#[cfg(test)]
fn set_entry(entries: &mut Vec<(String, Vec<u8>)>, name: &str, body: String) {
    match entries.iter_mut().find(|(existing, _)| existing == name) {
        Some(entry) => entry.1 = body.into_bytes(),
        None => entries.push((name.to_owned(), body.into_bytes())),
    }
}

/// Paragraph-level text of a Word document, in document order.
fn document_paragraphs(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let document = crate::local_knowledge_import::zip_entry_text(&bytes, "word/document.xml")?
        .ok_or_else(|| "docx 缺少 word/document.xml".to_owned())?;
    let text = xml_to_text(&document);
    Ok(text
        .split('\n')
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

/// Text content of an OOXML part, one line per `w:p` paragraph. Tag text is
/// dropped and the usual entities decoded; this is only used to decide whether a
/// named section exists, never as the document's rendered content.
fn xml_to_text(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    for character in xml.chars() {
        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                if tag.starts_with("/w:p") {
                    out.push('\n');
                }
                in_tag = false;
                tag.clear();
            }
            other if in_tag => {
                if tag.len() < 32 {
                    tag.push(other);
                }
            }
            '&' => out.push('&'),
            other => out.push(other),
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Read the whole artifact as searchable text, for consistency checks that span
/// artifacts. Structured parsers are used where they exist; the OOXML text
/// extraction is only a fallback for cross-artifact comparison, never the basis
/// of a per-item pass.
fn artifact_text(path: &Path, kind: ArtifactKind) -> Result<String, String> {
    match kind {
        ArtifactKind::Word => Ok(document_paragraphs(path)?.join("\n")),
        ArtifactKind::Spreadsheet => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
                .map_err(|error| format!("工作簿无法打开：{error:?}"))?;
            let mut out = String::new();
            for name in workbook.sheet_names().to_owned() {
                if let Ok(range) = workbook.worksheet_range(&name) {
                    for row in range.rows() {
                        let line = row
                            .iter()
                            .map(cell_text)
                            .collect::<Vec<_>>()
                            .join("\t");
                        if !line.trim().is_empty() {
                            out.push_str(&line);
                            out.push('\n');
                        }
                    }
                }
            }
            Ok(out)
        }
        _ => fs::read_to_string(path).map_err(|error| error.to_string()),
    }
}

fn cell_text(cell: &CellData) -> String {
    match cell {
        CellData::Empty => String::new(),
        CellData::String(value) => value.clone(),
        CellData::Float(value) => {
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                format!("{value}")
            }
        }
        CellData::Int(value) => value.to_string(),
        CellData::Bool(value) => value.to_string(),
        CellData::DateTime(value) => format!("{value:?}"),
        CellData::Error(value) => format!("{value:?}"),
        other => format!("{other:?}"),
    }
}

/// Tolerance (percentage points) allowed when comparing a stated share with the
/// expected one. Matches the independent evaluation checker's 0.0005 fraction
/// tolerance, expressed in percent.
const RATIO_TOLERANCE_PERCENT: f64 = 0.05;

/// The percentage a stated share implies, computed here rather than read from
/// the artifact: `numerator / total * 100`.
fn ratio_percent(numerator: u64, total: u64) -> Option<f64> {
    if total == 0 || numerator > total {
        return None;
    }
    Some(numerator as f64 / total as f64 * 100.0)
}

/// Percentages stated on one line of text: `59.12%`, `59,12 %`, or a bare
/// fraction when the line explicitly says 比例/占比/占.
fn percentages_in(line: &str) -> Vec<f64> {
    let mut values = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_digit() || bytes[index] == b'.' || bytes[index] == b',')
        {
            index += 1;
        }
        let raw = line[start..index].replace(',', "");
        // A percentage has to say so, either with a sign or with a ratio word
        // right after the number.
        let rest = &line[index..];
        let signed = rest.trim_start().starts_with('%');
        let worded = rest.trim_start().starts_with("个百分点")
            || rest.trim_start().starts_with("％");
        if !signed && !worded {
            continue;
        }
        if let Ok(value) = raw.parse::<f64>() {
            values.push(value);
        }
    }
    values
}

/// True when `haystack` states the number `value` as its own token.
fn text_states_number(haystack: &str, value: u64) -> bool {    let needle = value.to_string();
    let mut from = 0;
    while let Some(relative) = haystack[from..].find(&needle) {
        let at = from + relative;
        let end = at + needle.len();
        from = end;
        let before_ok = haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_digit() && character != '.');
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_ascii_digit() && character != '.');
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// The verdict for one requirement. `Unverified` is a first-class outcome: a
/// demand whose evidence could not be read or bound must never be reported as
/// met (which is exactly what "the file opens" used to do by accident).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequirementVerdict {
    Met,
    Unmet(String),
    Unverified(String),
}

impl RequirementVerdict {
    pub(crate) fn reason(&self) -> Option<String> {
        match self {
            RequirementVerdict::Met => None,
            RequirementVerdict::Unmet(reason) | RequirementVerdict::Unverified(reason) => {
                Some(reason.clone())
            }
        }
    }
}

/// The verdict of one requirement against the artifacts it applies to.
///
/// `root` is the Run's authorized project folder: a requirement bound to source
/// data resolves that file against it, so the same relative name in two projects
/// cannot silently point at the same bytes.
fn evaluate_requirement(
    requirement: &DeliveryRequirement,
    facts: &[ArtifactFacts],
    root: Option<&Path>,
) -> RequirementVerdict {
    match &requirement.kind {
        RequirementKind::Charts { count, per_sheet } => {
            let Some(best) = facts.iter().max_by_key(|fact| fact.charts) else {
                return RequirementVerdict::Unverified(
                    "没有可读取的交付项，图表要求未核验".to_owned(),
                );
            };
            if *per_sheet {
                // A per-worksheet demand is only verifiable through the package's
                // own sheet -> drawing -> chart relationships: a whole-file count
                // cannot say which sheet a chart belongs to, and a chart part no
                // worksheet references is not something the user can see.
                let Some(attribution) = best.sheet_charts.as_ref() else {
                    return RequirementVerdict::Unverified(format!(
                        "交付项「{}」的工作表->图表关系无法解析，未核验每个工作表 {count} 张图表的要求",
                        best.display_name
                    ));
                };
                if attribution.per_sheet.is_empty() {
                    return RequirementVerdict::Unverified(format!(
                        "交付项「{}」没有可识别的工作表，逐表图表要求未核验",
                        best.display_name
                    ));
                }
                let short: Vec<String> = attribution
                    .per_sheet
                    .iter()
                    .filter(|(_, charts)| *charts < *count)
                    .map(|(name, charts)| format!("{name}：{charts} 张"))
                    .collect();
                if short.is_empty() {
                    return RequirementVerdict::Met;
                }
                return RequirementVerdict::Unmet(format!(
                    "任务要求每个工作表至少 {count} 张图表，交付项「{}」按工作表实际挂载的图表为 {}（未达标：{}）；包内另有 {} 个未被任何工作表引用的图表部件，不计入",
                    best.display_name,
                    attribution
                        .per_sheet
                        .iter()
                        .map(|(name, charts)| format!("{name}={charts}"))
                        .collect::<Vec<_>>()
                        .join("、"),
                    short.join("、"),
                    attribution.orphan_charts
                ));
            }
            if best.charts >= *count {
                RequirementVerdict::Met
            } else {
                RequirementVerdict::Unmet(format!(
                    "任务要求至少 {count} 张图表，交付项「{}」的 OOXML 包中只发现 {} 个图表部件",
                    best.display_name, best.charts
                ))
            }
        }
        RequirementKind::Sections { names } => {
            let word_artifacts: Vec<&ArtifactFacts> = facts
                .iter()
                .filter(|fact| fact.kind == ArtifactKind::Word)
                .collect();
            if word_artifacts.is_empty() {
                return RequirementVerdict::Unverified(format!(
                    "任务要求文档包含章节：{}，但没有可读取的 Word 交付项可供核验",
                    names.join("、")
                ));
            }
            let mut readable = 0usize;
            let mut best_missing = names.clone();
            for fact in word_artifacts {
                let Ok(paragraphs) = document_paragraphs(&fact.path) else {
                    continue;
                };
                readable += 1;
                let missing: Vec<String> = names
                    .iter()
                    .filter(|name| {
                        !paragraphs
                            .iter()
                            .take(SECTION_SCAN_PARAGRAPHS)
                            .any(|paragraph| paragraph.contains(name.as_str()))
                    })
                    .cloned()
                    .collect();
                if missing.is_empty() {
                    return RequirementVerdict::Met;
                }
                if missing.len() < best_missing.len() {
                    best_missing = missing;
                }
            }
            if readable == 0 {
                return RequirementVerdict::Unverified(
                    "Word 交付项无法解析，章节要求未核验".to_owned(),
                );
            }
            RequirementVerdict::Unmet(format!(
                "任务要求文档包含章节：{}，文档中未找到：{}",
                names.join("、"),
                best_missing.join("、")
            ))
        }
        RequirementKind::RatioConsistency { numerator, total } => {
            let Some(expected) = ratio_percent(*numerator, *total) else {
                return RequirementVerdict::Unverified(format!(
                    "声明的 {numerator}/{total} 不是可计算的比例，未核验"
                ));
            };
            let mut saw_artifact = false;
            // Artifacts whose stated share agrees with the arithmetic.
            let mut stated: Vec<(String, f64)> = Vec::new();
            // An artifact that named both counts but a share that contradicts the
            // arithmetic. One contradiction is a hard failure, even when another
            // artifact got it right: the delivery says two different things.
            let mut wrong_share: Option<String> = None;
            // An artifact that named both counts and no share at all.
            let mut absent_share: Option<String> = None;
            // The numerator appears without the total it is a share of.
            let mut disjoint: Option<String> = None;
            let mut saw_metric = false;
            for fact in facts {
                let Ok(text) = artifact_text(&fact.path, fact.kind) else {
                    continue;
                };
                saw_artifact = true;
                for line in text.lines() {
                    let has_total = text_states_number(line, *total);
                    let has_numerator = text_states_number(line, *numerator);
                    if !has_total && !has_numerator {
                        continue;
                    }
                    saw_metric = true;
                    if !has_total {
                        // A numerator is only evidence about this metric when the
                        // total it is a share of is stated with it.
                        disjoint.get_or_insert_with(|| {
                            format!(
                                "交付项「{}」出现了分子 {numerator}，但同一行没有声明总数 {total}",
                                fact.display_name
                            )
                        });
                        continue;
                    }
                    if !has_numerator {
                        continue;
                    }
                    let stated_percents = percentages_in(line);
                    match stated_percents
                        .iter()
                        .copied()
                        .find(|value| (value - expected).abs() <= RATIO_TOLERANCE_PERCENT)
                    {
                        Some(actual) => stated.push((fact.display_name.clone(), actual)),
                        None => {
                            let shown = stated_percents
                                .iter()
                                .map(|value| format!("{value:.4}%"))
                                .collect::<Vec<_>>()
                                .join("/");
                            let message = if shown.is_empty() {
                                format!(
                                    "交付项「{}」在同一行给出 {numerator}/{total}，但没有给出比例；应为 {expected:.4}%",
                                    fact.display_name
                                )
                            } else {
                                format!(
                                    "交付项「{}」声明 {numerator}/{total} 的比例为 {shown}，与按该分子分母算出的 {expected:.4}% 不符",
                                    fact.display_name
                                )
                            };
                            if shown.is_empty() {
                                absent_share.get_or_insert(message);
                            } else {
                                wrong_share.get_or_insert(message);
                            }
                        }
                    }
                }
            }
            if !saw_artifact {
                return RequirementVerdict::Unverified(
                    "没有可核验的交付项，无法核对声明的总数与比例".to_owned(),
                );
            }
            // A share that contradicts the stated counts is never acceptable.
            if let Some(failure) = wrong_share {
                return RequirementVerdict::Unmet(failure);
            }
            if !saw_metric {
                return RequirementVerdict::Unverified(format!(
                    "交付文件中没有声明总数 {total} 与分子 {numerator}，比例未核验"
                ));
            }
            // Cross-artifact consistency is enforced by the rule above: every
            // artifact that states this metric must state the share the counts
            // imply, so a table and a report cannot disagree about it — the wrong
            // one is reported by name.
            if stated.is_empty() {
                return RequirementVerdict::Unmet(
                    absent_share
                        .or(disjoint)
                        .unwrap_or_else(|| format!("交付文件中没有可核对的 {numerator}/{total} 比例")),
                );
            }
            RequirementVerdict::Met
        }
        RequirementKind::SourceStatsUnbound { demand } => RequirementVerdict::Unverified(format!(
            "任务要求统计，但未能绑定可核验的源数据与统计列，因此未核验：{}",
            demand.chars().take(120).collect::<String>()
        )),
        RequirementKind::SourceDistribution {
            source,
            alternates: _,
            source_sha256,
            label_header,
            sheet,
        } => {
            let mut saw_artifact = false;
            // The source is resolved inside the Run's authorized project folder.
            let resolved = {
                let declared = Path::new(source.as_str());
                match root {
                    Some(root) if declared.is_relative() => root.join(declared),
                    _ => declared.to_path_buf(),
                }
            };
            let Ok(bytes) = fs::read(&resolved) else {
                return RequirementVerdict::Unverified(format!(
                    "源数据 {source} 在核验时不可读（{}），统计口径无法重算",
                    resolved.display()
                ));
            };
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let current_sha = hex::encode(hasher.finalize());
            // Expectations are recomputed from the source records *now*: a source
            // that changed since the task ran simply produces different expected
            // values, and an artifact still holding the old ones fails below.
            let expected =
                match source_distribution_records(&bytes, sheet.as_deref(), label_header) {
                    Ok(records) => records,
                    Err(reason) => {
                        return RequirementVerdict::Unverified(format!(
                            "无法从源数据 {source} 重算「{label_header}」的统计：{reason}"
                        ))
                    }
                };
            let total: u64 = expected.iter().map(|(_, count)| *count).sum();
            if total == 0 {
                return RequirementVerdict::Unverified(format!(
                    "源数据 {source} 的「{label_header}」没有任何记录，统计口径为空"
                ));
            }
            let mut problems: Vec<String> = Vec::new();
            for fact in facts {
                let Ok(text) = artifact_text(&fact.path, fact.kind) else {
                    continue;
                };
                saw_artifact = true;
                // The total has to be stated, otherwise the percentages cannot be
                // anchored to anything.
                if !text.lines().any(|line| text_states_number(line, total)) {
                    problems.push(format!(
                        "交付项「{}」没有声明源数据算出的总数 {total}",
                        fact.display_name
                    ));
                    continue;
                }
                for (label, count) in &expected {
                    let expected_ratio = ratio_percent(*count, total).unwrap_or(0.0);
                    let line = text
                        .lines()
                        .find(|line| line.contains(label.as_str()))
                        .unwrap_or_default();
                    if line.is_empty() {
                        problems.push(format!(
                            "交付项「{}」没有出现源数据中的「{label}」",
                            fact.display_name
                        ));
                        continue;
                    }
                    if !text_states_number(line, *count) {
                        problems.push(format!(
                            "交付项「{}」中「{label}」的数量与源数据不一致（源数据为 {count}）",
                            fact.display_name
                        ));
                        continue;
                    }
                    let ratios = percentages_in(line);
                    if !ratios.iter().any(|value| {
                        (value - expected_ratio).abs() <= SOURCE_RATIO_TOLERANCE_PERCENT
                    }) {
                        problems.push(format!(
                            "交付项「{}」中「{label}」的占比与源数据不一致（源数据为 {expected_ratio:.2}%，文件写作 {}）",
                            fact.display_name,
                            if ratios.is_empty() {
                                "未给出比例".to_owned()
                            } else {
                                ratios
                                    .iter()
                                    .map(|value| format!("{value:.2}%"))
                                    .collect::<Vec<_>>()
                                    .join("/")
                            }
                        ));
                    }
                }
            }
            if problems.is_empty() && !saw_artifact {
                return RequirementVerdict::Unverified(
                    "没有可读取的交付项，源数据统计未核验".to_owned(),
                );
            }
            if problems.is_empty() {
                return RequirementVerdict::Met;
            }
            let drift = if current_sha == *source_sha256 {
                String::new()
            } else {
                format!(
                    "（源数据在任务开始后已变化：登记 {}，当前 {}；期望值按当前源数据重算）",
                    &source_sha256[..8.min(source_sha256.len())],
                    &current_sha[..8.min(current_sha.len())]
                )
            };
            RequirementVerdict::Unmet(format!(
                "按源数据 {source} 的「{label_header}」独立重算（共 {total} 条记录）后，交付产物不一致：{}{drift}",
                problems.join("；")
            ))
        }
    }
}

/// Evaluate one item's requirements. `item_facts` holds that item's own facts;
/// `all_facts` carries every bound artifact, for requirements that span items.
pub(crate) fn evaluate_requirements(
    requirements: &[DeliveryRequirement],
    item_facts: &[ArtifactFacts],
    all_facts: &[ArtifactFacts],
) -> Vec<Value> {
    requirements
        .iter()
        .filter_map(|requirement| {
            let facts = match requirement.kind {
                RequirementKind::RatioConsistency { .. }
                | RequirementKind::SourceDistribution { .. }
                | RequirementKind::SourceStatsUnbound { .. } => all_facts,
                _ => item_facts,
            };
            let verdict = evaluate_requirement(requirement, facts, None);
            let (state, reason) = match &verdict {
                RequirementVerdict::Met => ("passed", None),
                RequirementVerdict::Unmet(reason) => ("failed", Some(reason.clone())),
                RequirementVerdict::Unverified(reason) => ("unverified", Some(reason.clone())),
            };
            Some(json!({
                "id": requirement.id,
                "check": requirement.kind.id(),
                "sourceText": requirement.source_text,
                "verified": matches!(
                    verdict,
                    RequirementVerdict::Met | RequirementVerdict::Unmet(_)
                ),
                "state": state,
                "reason": reason,
            }))
        })
        .collect()
}

/// Requirements that had to be checked across several deliverables. Returned
/// separately so one failure is reported once, not once per item.
pub(crate) fn cross_artifact_requirements(
    requirements: &[DeliveryRequirement],
) -> Vec<DeliveryRequirement> {
    requirements
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.kind,
                RequirementKind::RatioConsistency { .. }
                    | RequirementKind::SourceDistribution { .. }
                    | RequirementKind::SourceStatsUnbound { .. }
            )
        })
        .cloned()
        .collect()
}

/// Requirements that apply to exactly one delivered artifact each.
pub(crate) fn item_requirements(
    requirements: &[DeliveryRequirement],
    item_key: &str,
    item_kind: ArtifactKind,
) -> Vec<DeliveryRequirement> {
    requirements
        .iter()
        .filter(|requirement| match &requirement.kind {
            RequirementKind::Charts { .. } => {
                matches!(item_kind, ArtifactKind::Spreadsheet | ArtifactKind::Slides)
            }
            RequirementKind::Sections { .. } => item_kind == ArtifactKind::Word,
            RequirementKind::RatioConsistency { .. }
            | RequirementKind::SourceDistribution { .. }
            | RequirementKind::SourceStatsUnbound { .. } => false,
        })
        .map(|requirement| {
            let mut scoped = requirement.clone();
            scoped.item_key = Some(item_key.to_owned());
            scoped
        })
        .collect()
}

/// Scope the task's requirements to one checklist item for storage. Only the
/// requirements this item can actually satisfy are attached, so the gate never
/// checks a chart demand against a Word document.
pub(crate) fn attach_requirements(
    seed: &mut StoredChecklistSeed,
    requirements: &[DeliveryRequirement],
) {
    let kind = seed
        .target_path
        .as_deref()
        .and_then(|path| Path::new(path).extension())
        .and_then(|value| value.to_str())
        .or_else(|| {
            seed.item_key
                .strip_prefix("slot:")
                .and_then(|rest| rest.split(':').next())
        })
        .and_then(ArtifactKind::from_extension)
        .unwrap_or(ArtifactKind::Other);
    seed.requirements = item_requirements(requirements, &seed.item_key, kind)
        .iter()
        .map(|requirement| StoredRequirement {
            item_key: requirement.item_key.clone(),
            id: requirement.id.clone(),
            kind: requirement.kind.stored(),
            source_text: requirement.source_text.clone(),
        })
        .collect();
}

/// Rehydrate stored requirements into the runtime form the checkers use.
pub(crate) fn stored_requirements(
    stored: &[StoredRequirement],
) -> Vec<DeliveryRequirement> {
    stored
        .iter()
        .map(|item| DeliveryRequirement {
            item_key: item.item_key.clone(),
            id: item.id.clone(),
            kind: RequirementKind::from_stored(&item.kind),
            source_text: item.source_text.clone(),
        })
        .collect()
}

/// What the delivery gate can say about a requirement: met, unmet, or not
/// machine-verifiable. The third state exists so an unparsed demand is never
/// reported as satisfied by "the file opens".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequirementOutcome {
    Met,
    Unmet,
    Unverified,
}

impl RequirementOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RequirementOutcome::Met => "passed",
            RequirementOutcome::Unmet => "failed",
            RequirementOutcome::Unverified => "unverified",
        }
    }
}

/// Persist one requirement's outcome in the item's finding so the UI can show
/// met / unmet / unverified without inventing a second ledger.
pub(crate) fn apply_requirement_outcomes(
    finding_json: &str,
    outcomes: &[(DeliveryRequirement, RequirementOutcome, Option<String>)],
) -> Option<(String, Option<String>)> {
    if outcomes.is_empty() {
        return None;
    }
    let mut finding: Value = serde_json::from_str(finding_json).ok()?;
    let checks = finding["checks"].as_object_mut()?;
    let mut failure: Option<String> = None;
    let mut listed = Vec::new();
    for (requirement, outcome, reason) in outcomes {
        checks.insert(
            requirement.id.clone(),
            json!({
                "state": outcome.as_str(),
                "check": requirement.kind.id(),
                "sourceText": requirement.source_text,
                "reason": reason,
            }),
        );
        listed.push(json!({
            "id": requirement.id,
            "check": requirement.kind.id(),
            "state": outcome.as_str(),
            "sourceText": requirement.source_text,
            "reason": reason,
        }));
        if *outcome == RequirementOutcome::Unmet && failure.is_none() {
            failure = reason.clone().or_else(|| Some(requirement.source_text.clone()));
        }
    }
    finding["requirements"] = json!(listed);
    Some((finding.to_string(), failure))
}

/// Evaluate the task's structured requirements against the bound artifacts and
/// fold the outcomes into the item findings.
///
/// The pass runs in the production gate — not in a test-only checker — so a
/// parseable but incomplete deliverable (missing charts, a missing named
/// section, a ratio that disagrees with the stated total) reaches the existing
/// bounded delivery repair with a concrete reason. Requirements that no checker
/// can decide stay explicitly `unverified`: the gate never upgrades silence into
/// a pass.
fn apply_delivery_requirements(
    run_id: &str,
    checklist: &[DeliveryChecklistItem],
    requirements: &[DeliveryRequirement],
    root: Option<&Path>,
    verdicts: &mut [ItemVerdict],
) -> Result<(), String> {
    if requirements.is_empty() {
        return Ok(());
    }
    let kinds: std::collections::HashMap<&str, ArtifactKind> = checklist
        .iter()
        .map(|item| {
            let kind = item
                .target_path
                .as_deref()
                .and_then(|path| Path::new(path).extension())
                .and_then(|value| value.to_str())
                .or_else(|| {
                    item.item_key
                        .strip_prefix("slot:")
                        .and_then(|rest| rest.split(':').next())
                })
                .and_then(ArtifactKind::from_extension)
                .unwrap_or(ArtifactKind::Other);
            (item.item_key.as_str(), kind)
        })
        .collect();

    // Facts for every readable bound artifact: item-level requirements use the
    // item's own artifact, cross-artifact requirements compare all of them.
    let mut all_facts: Vec<ArtifactFacts> = Vec::new();
    let mut facts_by_item: std::collections::HashMap<String, Vec<ArtifactFacts>> =
        std::collections::HashMap::new();
    let mut unreadable: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for verdict in verdicts.iter() {
        let Some(path) = verdict.bound_path.as_deref() else {
            continue;
        };
        let kind = kinds
            .get(verdict.item_key.as_str())
            .copied()
            .unwrap_or(ArtifactKind::Other);
        match collect_artifact_facts(&verdict.item_key, &verdict.item_key, path) {
            Ok(facts) => {
                facts_by_item
                    .entry(verdict.item_key.clone())
                    .or_default()
                    .push(facts.clone());
                all_facts.push(facts);
            }
            Err(reason) => {
                unreadable.insert(verdict.item_key.clone(), reason);
            }
        }
        let _ = kind;
    }

    for verdict in verdicts.iter_mut() {
        let kind = kinds
            .get(verdict.item_key.as_str())
            .copied()
            .unwrap_or(ArtifactKind::Other);
        let scoped = item_requirements(requirements, &verdict.item_key, kind);
        let empty: Vec<ArtifactFacts> = Vec::new();
        let item_facts = facts_by_item
            .get(&verdict.item_key)
            .unwrap_or(&empty);
        let mut outcomes: Vec<(DeliveryRequirement, RequirementOutcome, Option<String>)> = Vec::new();
        if item_facts.is_empty() {
            // Nothing readable to check against: state that plainly instead of
            // reporting the demand as met.
            let reason = unreadable.get(&verdict.item_key).cloned();
            for requirement in scoped {
                outcomes.push((
                    requirement.clone(),
                    RequirementOutcome::Unverified,
                    Some(match &reason {
                        Some(reason) => format!("无法读取交付项，未核验：{reason}"),
                        None => "交付项未绑定可读取的产物，未核验".to_owned(),
                    }),
                ));
            }
        } else {
            for requirement in scoped {
                let (outcome, reason) = match evaluate_requirement(&requirement, item_facts, root) {
                    RequirementVerdict::Met => (RequirementOutcome::Met, None),
                    RequirementVerdict::Unmet(reason) => {
                        (RequirementOutcome::Unmet, Some(reason))
                    }
                    RequirementVerdict::Unverified(reason) => {
                        (RequirementOutcome::Unverified, Some(reason))
                    }
                };
                outcomes.push((requirement, outcome, reason));
            }
        }
        if let Some((finding_json, failure)) =
            apply_requirement_outcomes(&verdict.finding_json, &outcomes)
        {
            verdict.finding_json = finding_json;
            if failure.is_some() {
                verdict.passed = false;
            }
        }
    }
    // Requirements that span artifacts are checked once, against every artifact
    // the Run actually bound, and reported on the first item so exactly one
    // repair item names the inconsistency.
    let cross = cross_artifact_requirements(requirements);
    if cross.is_empty() {
        return Ok(());
    }
    let Some(first) = verdicts.first_mut() else {
        return Ok(());
    };
    let mut outcomes = Vec::new();
    for requirement in cross {
        if all_facts.is_empty() {
            outcomes.push((
                requirement.clone(),
                RequirementOutcome::Unverified,
                Some("没有任何可读取的交付项，无法核验该要求".to_owned()),
            ));
            continue;
        }
        let (outcome, reason) = match evaluate_requirement(&requirement, &all_facts, root) {
            RequirementVerdict::Met => (RequirementOutcome::Met, None),
            RequirementVerdict::Unmet(reason) => (RequirementOutcome::Unmet, Some(reason)),
            RequirementVerdict::Unverified(reason) => {
                (RequirementOutcome::Unverified, Some(reason))
            }
        };
        outcomes.push((requirement, outcome, reason));
    }
    if let Some((finding_json, failure)) = apply_requirement_outcomes(&first.finding_json, &outcomes) {
        first.finding_json = finding_json;
        if failure.is_some() {
            first.passed = false;
        }
    }
    let _ = run_id;
    Ok(())
}

/// Write a stored-only OOXML/ZIP container from `(name, bytes)` entries.
///
/// Test fixtures use this to build or extend a real package without a
/// compressor: the deterministic readers accept method 0, so a fixture can add
/// exactly one OOXML part (for example `xl/charts/chart1.xml`) and prove the
/// gate reacts to that structural fact rather than to a label.
#[cfg(test)]
pub(crate) fn stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]); // local header
        out.extend_from_slice(&[20, 0]); // version needed
        out.extend_from_slice(&[0, 0]); // flags
        out.extend_from_slice(&[0, 0]); // method: stored
        out.extend_from_slice(&[0, 0, 0, 0]); // time
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]); // extra length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        central.extend_from_slice(&[20, 0]);
        central.extend_from_slice(&[20, 0]);
        central.extend_from_slice(&[0, 0]);
        central.extend_from_slice(&[0, 0]);
        central.extend_from_slice(&[0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0, 0]); // extra
        central.extend_from_slice(&[0, 0]); // comment
        central.extend_from_slice(&[0, 0]); // disk
        central.extend_from_slice(&[0, 0]); // internal attrs
        central.extend_from_slice(&[0, 0, 0, 0]); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

/// Copy an existing OOXML package and add one stored part to it.
#[cfg(test)]
pub(crate) fn zip_with_extra_part(bytes: &[u8], name: &str, body: &str) -> Vec<u8> {
    let mut entries = crate::local_knowledge_import::zip_entries(bytes).expect("read package");
    entries.push((name.to_owned(), body.as_bytes().to_vec()));
    stored_zip(&entries)
}

fn repair_prompt(findings: &[Value]) -> String {
    let mut body = String::from(
        "Fox 交付核验：你承诺的文件成果尚未全部通过 Host 的确定性核验。请只修复下列具体未完成项（在已授权工具范围内），不要重复已经完成的操作，也不要用一句“已完成”代替真实产物：\n",
    );
    for finding in findings {
        let name = finding["displayName"].as_str().unwrap_or("交付项");
        let reason = finding["reason"].as_str().unwrap_or("核验未通过");
        body.push_str(&format!("- {name}：{reason}\n"));
        if let Some(checks) = finding["checks"].as_object() {
            for (check, result) in checks {
                if result["state"] == "failed" {
                    let detail = result["reason"].as_str().unwrap_or("");
                    body.push_str(&format!("    · {check} 未通过：{detail}\n"));
                }
            }
        }
    }
    body.push_str("修复并确认文件真实落盘后，再给出最终答复。");
    // request_continuation caps the prompt at 16 KiB; trim at a byte boundary.
    while body.len() > 15_500 {
        body.pop();
    }
    body
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::now_ms;

    fn seed_by_key<'a>(
        seeds: &'a [DeliveryChecklistSeed],
        key: &str,
    ) -> &'a DeliveryChecklistSeed {
        seeds
            .iter()
            .find(|seed| seed.item_key == key)
            .unwrap_or_else(|| panic!("missing seed {key}"))
    }

    #[test]
    fn counts_agv_style_excel_and_word_deliverables() {
        let seeds = expectations_from_task(
            "请基于当前原文件生成两份 Excel 汇总表与一份 Word 报告，包含图表，保存到项目目录。",
        );
        let keys: Vec<&str> = seeds.iter().map(|seed| seed.item_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["slot:docx:1", "slot:xlsx:1", "slot:xlsx:2"],
            "{keys:?}"
        );
        let excel = seed_by_key(&seeds, "slot:xlsx:1");
        assert_eq!(excel.checks, vec!["exists", "parseable", "nonempty"]);
        assert_eq!(excel.target_path, None);
        assert!(seeds
            .iter()
            .any(|seed| seed.display_name == "Excel 成果 1/2"));
        assert!(seeds
            .iter()
            .any(|seed| seed.display_name == "Word 成果 1/1"));
    }

    #[test]
    fn explicit_filenames_bind_real_targets_and_do_not_double_seed_slots() {
        let seeds = expectations_from_task(
            "请把结果写入 report/summary.csv 和 stats.json，再导出两份 PDF：a.pdf 与 b.pdf。",
        );
        let by_path = |path: &str| {
            seeds
                .iter()
                .find(|seed| seed.target_path.as_deref() == Some(path))
        };
        let csv = by_path("report/summary.csv").expect("csv target");
        assert!(csv
            .checks
            .contains(&"consistent_columns".to_owned()));
        assert!(by_path("stats.json").is_some());
        assert!(by_path("a.pdf").is_some());
        assert!(by_path("b.pdf").is_some());
        // Both PDFs are explicit; counted language must not add phantom slots.
        assert!(!seeds
            .iter()
            .any(|seed| seed.item_key.starts_with("slot:pdf")));
    }

    #[test]
    fn questions_analysis_urls_and_vague_requests_never_seed_a_checklist() {
        for text in [
            "请解释一下 AGV 长时间任务的主要原因。",
            "请分析这份 Excel 里两班卸船的差异。",
            "文档在 https://fox.example.com/report.docx 里，帮我读一下。",
            "请生成一份结论摘要。",
            "联系我：user@example.com",
        ] {
            assert!(expectations_from_task(text).is_empty(), "seeded for: {text}");
        }
    }

    #[test]
    fn chinese_numerals_plain_one_and_explicit_name_backtracking() {
        assert_eq!(expectations_from_task("请生成一份 Excel 表格").len(), 1);
        assert_eq!(expectations_from_task("请产出三个 PDF 文件").len(), 3);
        let named = expectations_from_task("请生成AGV分析报告.docx并保存");
        let seed = named
            .iter()
            .find(|seed| seed.target_path.as_deref() == Some("AGV分析报告.docx"))
            .expect("filename backtracked to the real name");
        assert_eq!(seed.display_name, "AGV分析报告.docx");
    }

    fn conversation_run(root: &Path) -> (Database, String) {
        let db = Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(
                db.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some("read_only"),
            )
            .unwrap();
        let run = db
            .create_run(&conversation.id, "生成一份 Excel 成果", None)
            .unwrap()
            .run
            .id;
        (db, run)
    }

    fn excel_slot_seed() -> DeliveryChecklistSeed {
        DeliveryChecklistSeed {
            item_key: "slot:xlsx:1".into(),
            target_path: None,
            artifact_id: None,
            display_name: "Excel 成果 1/1".into(),
            checks: vec!["exists".into(), "parseable".into(), "nonempty".into()],
            requirements: Vec::new(),
        }
    }

    #[test]
    fn missing_artifact_fails_with_concrete_reason_and_repair_is_bounded() {
        let root = std::env::temp_dir()
            .join(format!("fox-delivery-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        db.seed_delivery_checklist(&run_id, &[excel_slot_seed()], now_ms())
            .unwrap();

        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        assert!(matches!(stop, DeliveryStop::Repair { .. }));
        if let DeliveryStop::Repair { prompt, items, .. } = &stop {
            assert!(prompt.contains("交付核验"));
            assert!(prompt.contains("Excel 成果 1/1"));
            assert_eq!(items.len(), 1);
            assert!(!items[0].passed);
        }
        persist_outcome(&db, &run_id, &stop, now_ms()).unwrap();
        assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);

        // Second failure is still within the bounded budget.
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        assert!(matches!(stop, DeliveryStop::Repair { .. }));
        persist_outcome(&db, &run_id, &stop, now_ms()).unwrap();

        // Third failure exhausts the repair budget: the run may finish, but its
        // business delivery stays visibly failed rather than silently passing.
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        match stop {
            DeliveryStop::Exhausted { items } => assert!(!items[0].passed),
            other => panic!("expected exhausted, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn real_workbook_passes_and_gets_bound_to_the_slot_durably() {
        let root =
            std::env::temp_dir().join(format!("fox-delivery-xlsx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        // Rewrite the bytes instead of fs::copy: Windows preserves the source
        // file's mtime on copy, and the scan gate only accepts artifacts
        // modified at or after the run was created.
        let fixture_bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/office-reading.xlsx"),
        )
        .unwrap();
        std::fs::write(root.join("AGV汇总.xlsx"), fixture_bytes).unwrap();
        let seeds = expectations_from_task("请生成一份 Excel 成果");
        assert_eq!(seeds.len(), 1);
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();

        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        match &stop {
            DeliveryStop::Passed { items } => {
                assert!(items[0].passed, "{}", items[0].finding_json);
                assert!(items[0]
                    .bound_path
                    .as_ref()
                    .unwrap()
                    .ends_with("AGV汇总.xlsx"));
            }
            other => panic!("expected pass, got: {other:?}"),
        }
        persist_outcome(&db, &run_id, &stop, now_ms()).unwrap();
        let rows = db.delivery_checklist(&run_id).unwrap();
        assert_eq!(rows[0].status, "passed");
        assert!(rows[0].checked_at.is_some());
        // The slot is now durably bound to the real relative path.
        assert_eq!(rows[0].target_path.as_deref(), Some("AGV汇总.xlsx"));
        assert!(rows[0]
            .finding
            .as_ref()
            .unwrap()
            .contains("nonemptyRows"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_workbook_fails_parseability_not_just_existence() {
        let root =
            std::env::temp_dir().join(format!("fox-delivery-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        std::fs::write(root.join("broken.xlsx"), b"not a real workbook").unwrap();
        let seeds = expectations_from_task("请生成 broken.xlsx");
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        match stop {
            DeliveryStop::Repair { items, .. } => {
                let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
                assert_eq!(finding["checks"]["parseable"]["state"], json!("failed"));
                assert_eq!(finding["checks"]["exists"], json!("passed"));
            }
            other => panic!("expected repair, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn csv_column_mismatch_is_a_structural_failure() {
        let root =
            std::env::temp_dir().join(format!("fox-delivery-csv-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        std::fs::write(root.join("table.csv"), "a,b,c\n1,2,3\n4,5\n").unwrap();
        let seeds = expectations_from_task("请生成 table.csv");
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        match stop {
            DeliveryStop::Repair { items, .. } => {
                let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
                assert_eq!(
                    finding["checks"]["consistent_columns"]["state"],
                    json!("failed")
                );
            }
            other => panic!("expected repair, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_checklist_means_no_delivery_gate() {
        let root =
            std::env::temp_dir().join(format!("fox-delivery-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        assert!(matches!(stop, DeliveryStop::NoChecklist));
        assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ledger_survives_reopen() {
        let root = std::env::temp_dir()
            .join(format!("fox-delivery-reopen-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let run_id = {
            let (db, run_id) = conversation_run(&root);
            db.seed_delivery_checklist(&run_id, &[excel_slot_seed()], now_ms())
                .unwrap();
            let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
            persist_outcome(&db, &run_id, &stop, now_ms()).unwrap();
            run_id
        };
        let db = Database::open(root.join("facts.db")).unwrap();
        assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
        let items = db.delivery_checklist(&run_id).unwrap();
        assert_eq!(items[0].status, "failed");
        assert!(items[0].finding.as_ref().unwrap().contains("未找到"));
        let _ = std::fs::remove_dir_all(root);
    }

    // -----------------------------------------------------------------------
    // R6: structured requirements in the production delivery gate
    // -----------------------------------------------------------------------

    /// Write a stored-only OOXML/ZIP container. The deterministic readers accept
    /// method 0, so a test fixture can be built without a compressor.
    fn text_zip(entries: &[(&str, String)]) -> Vec<u8> {
        let owned = entries
            .iter()
            .map(|(name, body)| ((*name).to_owned(), body.as_bytes().to_vec()))
            .collect::<Vec<_>>();
        stored_zip(&owned)
    }

    fn docx_bytes(paragraphs: &[&str]) -> Vec<u8> {
        let body = paragraphs
            .iter()
            .map(|text| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"))
            .collect::<String>();
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        );
        text_zip(&[("word/document.xml", document)])
    }

    /// Test view of one requirement: `None` when met, otherwise the reason
    /// (whether the state was a failure or unverified).
    fn requirement_failure(
        requirement: &DeliveryRequirement,
        facts: &[ArtifactFacts],
    ) -> Option<String> {
        evaluate_requirement(requirement, facts, None).reason()
    }

    /// A workbook whose charts are wired the way a viewer resolves them:
    /// `workbook.xml` -> sheet part -> `<drawing>` -> drawing part -> chart
    /// parts, through the package's own relationship files. `orphans` extra chart
    /// parts are added that no worksheet references.
    fn workbook_with_sheet_charts(sheets: &[(&str, usize)], orphans: usize) -> Vec<u8> {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut push = |name: String, body: String| entries.push((name, body.into_bytes()));
        let header = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
        let rels_ns = "http://schemas.openxmlformats.org/package/2006/relationships";
        let r_ns = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

        let sheet_elements = sheets
            .iter()
            .enumerate()
            .map(|(index, (name, _))| {
                format!(
                    r#"<sheet name="{name}" sheetId="{}" r:id="rId{}"/>"#,
                    index + 1,
                    index + 1
                )
            })
            .collect::<String>();
        push(
            "xl/workbook.xml".into(),
            format!(
                r#"{header}<workbook xmlns:r="{r_ns}"><sheets>{sheet_elements}</sheets></workbook>"#
            ),
        );
        let workbook_rels = sheets
            .iter()
            .enumerate()
            .map(|(index, _)| {
                format!(
                    r#"<Relationship Id="rId{}" Type="{r_ns}/worksheet" Target="worksheets/sheet{}.xml"/>"#,
                    index + 1,
                    index + 1
                )
            })
            .collect::<String>();
        push(
            "xl/_rels/workbook.xml.rels".into(),
            format!(r#"{header}<Relationships xmlns="{rels_ns}">{workbook_rels}</Relationships>"#),
        );

        for (index, (_, charts)) in sheets.iter().enumerate() {
            let sheet = index + 1;
            let drawing_ref = if *charts > 0 {
                format!(r#"<drawing r:id="rIdDrw{sheet}"/>"#)
            } else {
                String::new()
            };
            push(
                format!("xl/worksheets/sheet{sheet}.xml"),
                format!(
                    r#"{header}<worksheet xmlns:r="{r_ns}"><sheetData/>{drawing_ref}</worksheet>"#
                ),
            );
            if *charts == 0 {
                continue;
            }
            push(
                format!("xl/worksheets/_rels/sheet{sheet}.xml.rels"),
                format!(
                    r#"{header}<Relationships xmlns="{rels_ns}"><Relationship Id="rIdDrw{sheet}" Type="{r_ns}/drawing" Target="../drawings/drawing{sheet}.xml"/></Relationships>"#
                ),
            );
            let chart_refs = (1..=*charts)
                .map(|chart| format!(r#"<c:chart r:id="rIdC{sheet}_{chart}"/>"#))
                .collect::<String>();
            push(
                format!("xl/drawings/drawing{sheet}.xml"),
                format!(
                    r#"{header}<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="{r_ns}">{chart_refs}</xdr:wsDr>"#
                ),
            );
            let chart_rels = (1..=*charts)
                .map(|chart| {
                    format!(
                        r#"<Relationship Id="rIdC{sheet}_{chart}" Type="{r_ns}/chart" Target="../charts/chart{sheet}-{chart}.xml"/>"#
                    )
                })
                .collect::<String>();
            push(
                format!("xl/drawings/_rels/drawing{sheet}.xml.rels"),
                format!(r#"{header}<Relationships xmlns="{rels_ns}">{chart_rels}</Relationships>"#),
            );
            for chart in 1..=*charts {
                push(
                    format!("xl/charts/chart{sheet}-{chart}.xml"),
                    format!(
                        r#"{header}<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#
                    ),
                );
            }
        }
        for orphan in 0..orphans {
            push(
                format!("xl/charts/chart9{orphan}.xml"),
                format!(
                    r#"{header}<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#
                ),
            );
        }
        stored_zip(&entries)
    }

    fn write_workbook(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// A source workbook with a real header row and records.
    ///
    /// Built by taking a real xlsx fixture (so the container is exactly what a
    /// spreadsheet app writes) and replacing one worksheet's `sheetData` with the
    /// header and records. That keeps the expectation derived by an independent
    /// parser — calamine — rather than by the test.
    fn source_workbook(header: &str, records: &[&str]) -> (Vec<u8>, String) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/office-reading.xlsx");
        let base = std::fs::read(&fixture).expect("test fixture workbook");
        let mut entries =
            crate::local_knowledge_import::zip_entries(&base).expect("fixture package");
        let workbook = entries
            .iter()
            .find(|(name, _)| name == "xl/workbook.xml")
            .map(|(_, body)| String::from_utf8_lossy(body).into_owned())
            .expect("fixture workbook.xml");
        let (sheet_name, _) = sheet_relationships(&workbook)
            .into_iter()
            .next()
            .expect("fixture has a sheet");
        let index = entries
            .iter()
            .position(|(name, _)| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
            .expect("fixture has a worksheet part");
        let sheet = String::from_utf8_lossy(&entries[index].1).into_owned();
        let start = sheet.find("<sheetData").expect("worksheet sheetData");
        let end =
            sheet.find("</sheetData>").expect("worksheet sheetData end") + "</sheetData>".len();
        let escape = |value: &str| {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let mut rows = format!(
            r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{}</t></is></c><c r="B1" t="inlineStr"><is><t>记录编号</t></is></c></row>"#,
            escape(header)
        );
        for (position, label) in records.iter().enumerate() {
            let row = position + 2;
            rows.push_str(&format!(
                r#"<row r="{row}"><c r="A{row}" t="inlineStr"><is><t>{}</t></is></c><c r="B{row}"><v>{row}</v></c></row>"#,
                escape(label)
            ));
        }
        rows.push_str("</sheetData>");
        let rebuilt = format!("{}{}{}", &sheet[..start], rows, &sheet[end..]);
        entries[index].1 = rebuilt.into_bytes();
        (stored_zip(&entries), sheet_name)
    }

    #[test]
    fn explicit_chart_and_section_demands_become_structured_requirements() {
        let requirements = requirements_from_task(
            "请基于源数据生成 AGV长时间任务统计分布.xlsx（每个 Sheet 配一张图表）和 AGV长时间任务分析报告.docx，\
             报告需包含：分析概述、统计结果、结论与建议 三个章节。",
        );
        assert!(
            requirements.iter().any(|requirement| matches!(
                requirement.kind,
                RequirementKind::Charts { count, per_sheet: true } if count == 1
            )),
            "a per-sheet chart demand must keep its granularity: {requirements:?}"
        );
        let sections = requirements
            .iter()
            .find_map(|requirement| match &requirement.kind {
                RequirementKind::Sections { names } => Some(names.clone()),
                _ => None,
            })
            .expect("section requirement");
        assert_eq!(
            sections.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["分析概述", "统计结果", "结论与建议"]
        );
        // A task that states no decidable demand produces none, and the gate then
        // reports those demands unverified instead of passing them.
        assert!(requirements_from_task("请生成 summary.xlsx").is_empty());
        assert!(requirements_from_task("分析这份 Excel 的图表数量").is_empty());
    }

    /// A parseable workbook that meets every structural check but carries no
    /// chart must fail when the task demanded one, and must reach the bounded
    /// delivery repair through the real gate.
    #[test]
    fn a_parseable_but_chartless_deliverable_fails_the_chart_requirement() {
        let root = std::env::temp_dir().join(format!("fox-delivery-chart-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        let fixture =
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/office-reading.xlsx"))
                .unwrap();
        std::fs::write(root.join("统计分布.xlsx"), fixture).unwrap();

        let task = "请生成 统计分布.xlsx，其中每个 Sheet 配一张图表。";
        let mut seeds = expectations_from_task(task);
        assert_eq!(seeds.len(), 1);
        assert!(
            seeds[0].requirements.is_empty(),
            "requirements are attached by the Host, not by the seeder"
        );
        let requirements = requirements_from_task(task);
        assert!(!requirements.is_empty(), "the chart demand must be parsed");
        for seed in seeds.iter_mut() {
            attach_requirements(seed, &requirements);
        }
        assert_eq!(seeds[0].requirements.len(), 1);
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();

        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        match stop {
            DeliveryStop::Repair { items, prompt, .. } => {
                let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
                assert_eq!(finding["checks"]["parseable"], json!("passed"));
                let requirement_finding = finding["requirements"]
                    .as_array()
                    .expect("requirement findings")
                    .iter()
                    .find(|entry| entry["check"] == json!("charts"))
                    .expect("chart requirement reported");
                assert_eq!(requirement_finding["state"], json!("failed"));
                assert!(
                    requirement_finding["reason"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("图表"),
                    "the failure must name the missing charts"
                );
                assert!(prompt.contains("图表"), "repair must name the demand");
            }
            other => panic!("expected a bounded repair, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// The same task with a document that really carries the demanded section
    /// passes, and an artifact that is missing one named section fails with that
    /// section named.
    #[test]
    fn a_missing_named_section_is_reported_and_a_complete_document_passes() {
        let task = "请撰写 分析报告.docx，报告需包含：分析概述、统计结果、结论与建议 三个章节。";
        let mut seeds = expectations_from_task(task);
        assert_eq!(seeds.len(), 1, "the docx must be seeded as a deliverable");
        let requirements = requirements_from_task(task);
        for seed in seeds.iter_mut() {
            attach_requirements(seed, &requirements);
        }

        // Complete document: passes.
        let good_root =
            std::env::temp_dir().join(format!("fox-delivery-sections-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&good_root).unwrap();
        let (db, run_id) = conversation_run(&good_root);
        std::fs::write(
            good_root.join("分析报告.docx"),
            docx_bytes(&["分析概述", "本文档说明统计口径。", "统计结果", "共 12 条记录。", "结论与建议", "建议持续复核。"]),
        )
        .unwrap();
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();
        let stop = evaluate_stop(&db, Some(good_root.to_str().unwrap()), &run_id).unwrap();
        match &stop {
            DeliveryStop::Passed { items } => {
                let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
                let entry = finding["requirements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["check"] == json!("sections"))
                    .expect("section requirement reported");
                assert_eq!(entry["state"], json!("passed"));
            }
            other => panic!("a complete document must pass, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(good_root);

        // Missing one named section: repair, naming the missing section.
        let bad_root =
            std::env::temp_dir().join(format!("fox-delivery-sections-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&bad_root).unwrap();
        let (db, run_id) = conversation_run(&bad_root);
        std::fs::write(
            bad_root.join("分析报告.docx"),
            docx_bytes(&["分析概述", "只有概述和结果。", "统计结果", "共 12 条记录。"]),
        )
        .unwrap();
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();
        let stop = evaluate_stop(&db, Some(bad_root.to_str().unwrap()), &run_id).unwrap();
        match stop {
            DeliveryStop::Repair { items, .. } => {
                let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
                let entry = finding["requirements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["check"] == json!("sections"))
                    .expect("section requirement reported");
                assert_eq!(entry["state"], json!("failed"));
                assert!(
                    entry["reason"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("结论与建议"),
                    "the failure must name the missing section: {entry}"
                );
            }
            other => panic!("a document missing a named section must repair, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(bad_root);
    }

    /// A ratio the task states about its own total has to appear in the delivered
    /// artifacts *with the arithmetic to match*; a wrong or absent ratio fails.
    /// The expected share is computed here from the stated counts, never read
    /// from the artifact and never a stored constant.
    #[test]
    fn a_stated_ratio_is_computed_and_compared_instead_of_merely_present() {
        let task = "请生成 分布.xlsx，其中卸船 4 台，占总计 10 台的比例必须与源数据一致。";
        let requirements = requirements_from_task(task);
        let ratio = requirements
            .iter()
            .find(|requirement| matches!(requirement.kind, RequirementKind::RatioConsistency { .. }))
            .expect("the stated ratio must be parsed");
        assert!(
            matches!(
                ratio.kind,
                RequirementKind::RatioConsistency { numerator: 4, total: 10 }
            ),
            "unexpected requirement: {ratio:?}"
        );
        // The expectation itself is arithmetic on the task's own numbers.
        assert_eq!(ratio_percent(4, 10), Some(40.0));
        assert_eq!(ratio_percent(548, 927), Some(548.0 / 927.0 * 100.0));
        assert_eq!(ratio_percent(1, 0), None);
        assert_eq!(ratio_percent(5, 4), None);

        let root = std::env::temp_dir().join(format!("fox-delivery-ratio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let facts_of = |name: &str, body: &str| {
            let path = root.join(name);
            std::fs::write(&path, body).unwrap();
            vec![collect_artifact_facts("item", name, &path).unwrap()]
        };

        // Consistent: the share is stated, and 4/10 is 40%.
        assert!(requirement_failure(&ratio, &facts_of("一致.txt", "卸船 4 台，总计 10 台，占比 40%")).is_none());

        // THE case the old check let through: right counts, wrong percentage.
        let failure = requirement_failure(&ratio, &facts_of("错比例.txt", "卸船 4 台，总计 10 台，占比 70%"))
            .expect("right counts with a wrong share must fail");
        assert!(
            failure.contains("70") && failure.contains("40"),
            "the failure must name the stated and expected share: {failure}"
        );

        // A row that names both counts but never states a share is unmet too: the
        // task asked for a ratio.
        let failure = requirement_failure(&ratio, &facts_of("无比例.txt", "卸船 4 台，总计 10 台"))
            .expect("a required but absent share must fail");
        assert!(failure.contains("没有给出比例"), "{failure}");

        // Numbers scattered in unrelated places are not one statement.
        let failure = requirement_failure(
            &ratio,
            &facts_of("散落.txt", "报告日期 2026 年\n卸船 4 台\n备注：总计 10 台见附表\n装载率 40%"),
        )
        .expect("scattered numbers must not satisfy the metric");
        assert!(failure.contains("同一行"), "{failure}");

        // A wrong numerator with a matching-looking share still fails, because the
        // share has to belong to the stated counts.
        let failure = requirement_failure(&ratio, &facts_of("错分子.txt", "卸船 7 台，总计 10 台，占比 70%"))
            .expect("a mismatched numerator must fail");
        assert!(failure.contains("4") && failure.contains("10"), "{failure}");

        // Two artifacts claiming the same metric with different shares: the one
        // that contradicts the arithmetic is rejected by name, so a report can
        // never quietly disagree with the table it summarises.
        let a = root.join("表.txt");
        std::fs::write(&a, "卸船 4 台，总计 10 台，占比 40%").unwrap();
        let b = root.join("报告.txt");
        std::fs::write(&b, "卸船 4 台，总计 10 台，占比 59.12%").unwrap();
        let facts = vec![
            collect_artifact_facts("a", "表.txt", &a).unwrap(),
            collect_artifact_facts("b", "报告.txt", &b).unwrap(),
        ];
        let failure = requirement_failure(&ratio, &facts).expect("a contradicting share must fail");
        assert!(
            failure.contains("报告.txt") && failure.contains("59.12"),
            "the rejection must name the artifact and its share: {failure}"
        );

        // …and agreement across artifacts passes.
        std::fs::write(&b, "卸船 4 台，总计 10 台，占比 40.0%").unwrap();
        let facts = vec![
            collect_artifact_facts("a", "表.txt", &a).unwrap(),
            collect_artifact_facts("b", "报告.txt", &b).unwrap(),
        ];
        assert!(requirement_failure(&ratio, &facts).is_none());

        // Rounding inside tolerance is accepted; a whole point off is not.
        assert!(requirement_failure(&ratio, &facts_of("舍入.txt", "卸船 4 台，总计 10 台，占比 40.01%")).is_none());
        assert!(requirement_failure(&ratio, &facts_of("差一点.txt", "卸船 4 台，总计 10 台，占比 41%")).is_some());

        let _ = std::fs::remove_dir_all(root);
    }

    /// The requirement is derived from one clause that states counts *and* a
    /// ratio word; dates, ids and unrelated counts cannot become a denominator.
    #[test]
    fn only_a_self_contained_share_statement_becomes_a_ratio_requirement() {
        let parsed = |task: &str| {
            requirements_from_task(task)
                .into_iter()
                .filter_map(|requirement| match requirement.kind {
                    RequirementKind::RatioConsistency { numerator, total } => {
                        Some((numerator, total))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // A real share statement, in one clause.
        assert_eq!(parsed("卸船 548 台，占总计 927 台。"), vec![(548, 927)]);
        // Numbers in another clause are not the denominator of this share.
        assert!(
            parsed("报告日期 2026 年 9 月 14 日。请生成 分布.xlsx。").is_empty(),
            "dates must not become a share"
        );
        // A counted share without any ratio word is not a ratio demand.
        assert!(parsed("请生成 分布.xlsx，一共 10 台，其中 4 台。").is_empty());
        // Two ratios in one task stay separate.
        assert_eq!(
            parsed("卸船 4 台，占总计 10 台的 40%；装船 6 台，占总计 10 台的 60%。"),
            vec![(4, 10), (6, 10)]
        );
    }

    /// N2/F4: a per-sheet chart demand is verified per sheet, through the
    /// package's own relationships. A whole-file count cannot stand in for it,
    /// and a chart part no worksheet references does not count.
    #[test]
    fn a_per_sheet_chart_demand_is_checked_per_sheet() {
        let task = "请生成 分布.xlsx，其中每个 Sheet 配一张图表。";
        let requirements = requirements_from_task(task);
        let chart = requirements
            .iter()
            .find(|requirement| matches!(requirement.kind, RequirementKind::Charts { .. }))
            .expect("chart requirement");
        match &chart.kind {
            RequirementKind::Charts { count, per_sheet } => {
                // The task says "each sheet", so the demand is one chart per
                // sheet and the granularity must survive into the requirement.
                assert_eq!(*count, 1);
                assert!(per_sheet, "the per-sheet wording must be recorded");
            }
            other => panic!("unexpected: {other:?}"),
        }

        let root =
            std::env::temp_dir().join(format!("fox-delivery-chart-{}", uuid::Uuid::new_v4()));
        let facts_of = |name: &str, sheets: &[(&str, usize)], orphans: usize| {
            let bytes = workbook_with_sheet_charts(sheets, orphans);
            let path = write_workbook(&root, name, &bytes);
            vec![collect_artifact_facts("item", name, &path).unwrap()]
        };

        // Two sheets, one chart each: met, and the attribution is real.
        let good = facts_of("两表各一图.xlsx", &[("Sheet1", 1), ("Sheet2", 1)], 0);
        assert_eq!(
            good[0].sheet_charts.as_ref().unwrap().per_sheet,
            vec![("Sheet1".to_owned(), 1), ("Sheet2".to_owned(), 1)]
        );
        assert_eq!(good[0].sheet_charts.as_ref().unwrap().orphan_charts, 0);
        assert!(requirement_failure(chart, &good).is_none());

        // Two charts, both on Sheet1 and none on Sheet2: the per-sheet demand is
        // NOT met, even though the file-wide count (2) equals the old shortcut's
        // `1 x sheets`.
        let both_on_one = facts_of("两图同表.xlsx", &[("Sheet1", 2), ("Sheet2", 0)], 0);
        assert_eq!(both_on_one[0].charts, 2);
        let failure = requirement_failure(chart, &both_on_one)
            .expect("two charts on one sheet must not satisfy a per-sheet demand");
        assert!(
            failure.contains("Sheet2") && failure.contains("未达标"),
            "the failure must name the sheet that has no chart: {failure}"
        );

        // An orphan chart part (present but referenced by no worksheet) does not
        // count toward any sheet.
        let with_orphan = facts_of("孤立图表.xlsx", &[("Sheet1", 1), ("Sheet2", 0)], 1);
        assert_eq!(with_orphan[0].charts, 2);
        let attribution = with_orphan[0].sheet_charts.as_ref().unwrap();
        assert_eq!(
            attribution.per_sheet,
            vec![("Sheet1".to_owned(), 1), ("Sheet2".to_owned(), 0)]
        );
        assert_eq!(attribution.orphan_charts, 1);
        let failure = requirement_failure(chart, &with_orphan)
            .expect("an orphan chart must not satisfy a sheet");
        assert!(failure.contains("未被任何工作表引用"), "{failure}");

        // A relationship chain that cannot be resolved is reported unverified,
        // never as met and never as a failure the user cannot act on.
        let broken = root.join("关系损坏.xlsx");
        let mut entries =
            crate::local_knowledge_import::zip_entries(&workbook_with_sheet_charts(
                &[("Sheet1", 1)],
                0,
            ))
            .unwrap();
        // Drop the drawing part the worksheet references.
        entries.retain(|(name, _)| name != "xl/drawings/drawing1.xml");
        std::fs::write(&broken, stored_zip(&entries)).unwrap();
        let facts = vec![collect_artifact_facts("item", "关系损坏.xlsx", &broken).unwrap()];
        assert!(
            facts[0].sheet_charts.is_none(),
            "a broken relationship chain must not be reported as zero charts"
        );
        match evaluate_requirement(chart, &facts, None) {
            RequirementVerdict::Unverified(reason) => {
                assert!(reason.contains("无法解析"), "{reason}")
            }
            other => panic!("a broken chain must be unverified, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// F3: the statistic the task asks for is recomputed from the source data it
    /// names, so an artifact can neither verify itself nor survive a source the
    /// task no longer matches. The expectation never comes from the artifact and
    /// never from the model's own summary.
    #[test]
    fn source_statistics_are_recomputed_from_the_named_source() {
        let root =
            std::env::temp_dir().join(format!("fox-delivery-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        // Source: 3 卸船 / 1 装船 = 4 records, so the shares are 75% and 25%.
        let source = root.join("AGV源数据.xlsx");
        let (source_bytes, _sheet) =
            source_workbook("任务类型", &["卸船", "卸船", "装船", "卸船"]);
        std::fs::write(&source, &source_bytes).unwrap();
        let task = "请读取 AGV源数据.xlsx，按 任务类型 列统计数量与占比，生成 分布.xlsx。";

        // The expectation comes from the source records themselves.
        match source_distribution_records(&std::fs::read(&source).unwrap(), None, "任务类型") {
            Ok(records) => assert_eq!(
                records,
                vec![("卸船".to_owned(), 3), ("装船".to_owned(), 1)]
            ),
            Err(reason) => panic!("the source workbook must be readable: {reason}"),
        }

        // The task binds a source distribution requirement…
        let parsed = requirements_from_task(task);
        let requirement = parsed
            .iter()
            .find(|requirement| {
                matches!(requirement.kind, RequirementKind::SourceDistribution { .. })
            })
            .unwrap_or_else(|| {
                panic!(
                    "a named source and column must bind a statistic; parsed: {:?}",
                    parsed
                        .iter()
                        .map(|requirement| requirement.id.clone())
                        .collect::<Vec<_>>()
                )
            });
        match &requirement.kind {
            RequirementKind::SourceDistribution {
                source, label_header, ..
            } => {
                assert_eq!(source, "AGV源数据.xlsx");
                assert_eq!(label_header, "任务类型");
            }
            other => panic!("unexpected: {other:?}"),
        }
        // …and the Host binds it against the real file, hashing its bytes.
        let bound = bind_source_distribution(&root, requirement.clone());
        match &bound.kind {
            RequirementKind::SourceDistribution { source_sha256, .. } => {
                let bytes = std::fs::read(&source).unwrap();
                assert_eq!(source_sha256, &hex::encode(Sha256::digest(&bytes)));
                assert!(!source_sha256.is_empty());
            }
            other => panic!("binding must stay a source distribution: {other:?}"),
        }

        let facts_of = |name: &str, body: &str| {
            let path = root.join(name);
            std::fs::write(&path, body).unwrap();
            vec![collect_artifact_facts("item", name, &path).unwrap()]
        };
        let verdict = |body: &str| {
            evaluate_requirement(
                &bound,
                &facts_of("产物.txt", body),
                Some(root.as_path()),
            )
        };

        // A correct recomputation passes (75% / 25%, rounded to one decimal).
        match verdict("总计 4 条\n卸船 3 条，占比 75.0%\n装船 1 条，占比 25.0%") {
            RequirementVerdict::Met => {}
            other => panic!("a correct recomputation must pass, got {other:?}"),
        }

        // An artifact writing its own *wrong* numbers fails: each is compared
        // against the source, so two artifacts agreeing on the same wrong number
        // still fail.
        let failure = verdict("总计 4 条\n卸船 2 条，占比 50.0%\n装船 2 条，占比 50.0%")
            .reason()
            .expect("a wrong count must fail against the source");
        assert!(failure.contains("源数据"), "{failure}");
        assert!(failure.contains("卸船"), "{failure}");

        // A right count with a wrong share fails too.
        let failure = verdict("总计 4 条\n卸船 3 条，占比 50%\n装船 1 条，占比 50%")
            .reason()
            .expect("a wrong share must fail");
        assert!(failure.contains("占比"), "{failure}");

        // The source record changes: an artifact still carrying the old numbers
        // must now fail, because the expectation is recomputed from the source.
        let (replaced, _) = source_workbook("任务类型", &["卸船", "装船", "装船", "装船"]);
        std::fs::write(&source, &replaced).unwrap();
        let failure = verdict("总计 4 条\n卸船 3 条，占比 75.0%\n装船 1 条，占比 25.0%")
            .reason()
            .expect("stale numbers must fail after the source changed");
        assert!(failure.contains("源数据"), "{failure}");
        assert!(
            failure.contains("已变化"),
            "the report must state the source drift: {failure}"
        );
        // …and the recomputed numbers pass.
        match verdict("总计 4 条\n卸船 1 条，占比 25.0%\n装船 3 条，占比 75.0%") {
            RequirementVerdict::Met => {}
            other => panic!("the recomputed numbers must pass, got {other:?}"),
        }

        // A statistics demand the task does not make bindable is reported
        // unverified, with the demand text, instead of disappearing.
        let vague = requirements_from_task("请统计任务情况，生成 分布.xlsx。");
        let unbound = vague
            .iter()
            .find(|requirement| {
                matches!(requirement.kind, RequirementKind::SourceStatsUnbound { .. })
            })
            .expect("an unspecifiable statistic stays visible as unverified");
        match evaluate_requirement(unbound, &[], Some(root.as_path())) {
            RequirementVerdict::Unverified(reason) => {
                assert!(reason.contains("未核验"), "{reason}")
            }
            other => panic!("an unbound statistic must be unverified, got {other:?}"),
        }
        // A task whose source file does not exist cannot be bound either.
        let missing_task = "请读取 不存在.xlsx，按 任务类型 列统计数量，生成 分布.xlsx。";
        let missing_parsed = requirements_from_task(missing_task);
        let missing = bind_source_distribution(
            &root,
            missing_parsed
                .iter()
                .find(|requirement| {
                    matches!(requirement.kind, RequirementKind::SourceDistribution { .. })
                })
                .unwrap_or_else(|| {
                    panic!(
                        "the missing-source task must still parse a binding; parsed: {:?}",
                        missing_parsed
                            .iter()
                            .map(|requirement| requirement.id.clone())
                            .collect::<Vec<_>>()
                    )
                })
                .clone(),
        );
        assert!(matches!(
            evaluate_requirement(&missing, &[], Some(root.as_path())),
            RequirementVerdict::Unverified(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A requirement that cannot be read is reported unverified — never passed —
    /// and does not by itself fail the item.
    #[test]
    fn an_unreadable_artifact_reports_requirements_as_unverified() {
        let root = std::env::temp_dir()
            .join(format!("fox-delivery-unverified-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (db, run_id) = conversation_run(&root);
        // No file at all: the item is missing, so the requirement cannot be read.
        let task = "请生成 统计分布.xlsx，其中每个 Sheet 配一张图表。";
        let mut seeds = expectations_from_task(task);
        let requirements = requirements_from_task(task);
        for seed in seeds.iter_mut() {
            attach_requirements(seed, &requirements);
        }
        db.seed_delivery_checklist(&run_id, &seeds, now_ms()).unwrap();
        let stop = evaluate_stop(&db, Some(root.to_str().unwrap()), &run_id).unwrap();
        let items = stop.items();
        let finding: Value = serde_json::from_str(&items[0].finding_json).unwrap();
        let entries = finding["requirements"].as_array().expect("requirement findings");
        assert!(!entries.is_empty(), "the demand must still be reported");
        assert!(
            entries.iter().all(|entry| entry["state"] == json!("unverified")),
            "an unreadable item must report unverified, got {entries:?}"
        );
        // The missing artifact itself is still a failure, so the run repairs.
        assert!(!items[0].passed);
        let _ = std::fs::remove_dir_all(root);
    }
}

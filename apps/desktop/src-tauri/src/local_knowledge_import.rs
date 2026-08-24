use flate2::read::{DeflateDecoder, ZlibDecoder};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf, Prefix},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_MAX_FILE_SIZE: u64 = 256 * 1024 * 1024;
pub const DEFAULT_CHUNK_MAX_BYTES: usize = 4 * 1024;

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const STAGING_DIRECTORY: &str = "staging";
const KNOWLEDGE_BASES_DIRECTORY: &str = "knowledge-bases";
const DOCUMENTS_DIRECTORY: &str = "documents";

const MAX_ARCHIVE_ENTRIES: usize = 512;
const MAX_ARCHIVE_ENTRY_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: u64 = 100;
const MAX_XML_DEPTH: usize = 256;
const MAX_XML_TAG_BYTES: usize = 1024 * 1024;
const MAX_EXTRACTED_TEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_PDF_STREAMS: usize = 4096;
const MAX_PDF_STREAM_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const CODE_SOURCE_MISSING: &str = "local_knowledge.import_source_missing";
const CODE_SOURCE_NOT_FILE: &str = "local_knowledge.import_source_not_file";
const CODE_SOURCE_UNREADABLE: &str = "local_knowledge.import_source_unreadable";
const CODE_UNSAFE_PATH: &str = "local_knowledge.import_unsafe_path";
const CODE_UNSUPPORTED_EXTENSION: &str = "local_knowledge.import_unsupported_extension";
const CODE_FILE_TOO_LARGE: &str = "local_knowledge.import_file_too_large";
const CODE_STORAGE_FAILED: &str = "local_knowledge.import_storage_failed";
const CODE_COPY_FAILED: &str = "local_knowledge.import_copy_failed";
const CODE_COMMIT_FAILED: &str = "local_knowledge.import_commit_failed";
const CODE_DESTINATION_EXISTS: &str = "local_knowledge.import_destination_exists";
const CODE_CANCELLED: &str = "local_knowledge.import_cancelled";
const CODE_PARSE_FAILED: &str = "local_knowledge.import_parse_failed";
const CODE_INVALID_CHUNK_CONFIG: &str = "local_knowledge.import_invalid_chunk_config";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    pub code: &'static str,
    pub message: String,
}

impl ImportError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

pub trait ImportCancellation {
    fn is_cancelled(&self) -> bool;
}

impl<F> ImportCancellation for F
where
    F: Fn() -> bool,
{
    fn is_cancelled(&self) -> bool {
        self()
    }
}

#[derive(Clone, Default)]
pub struct ImportCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl ImportCancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Release);
    }
}

impl ImportCancellation for ImportCancellationToken {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub struct ImportOptions<'a> {
    pub max_file_size: u64,
    pub cancellation: Option<&'a dyn ImportCancellation>,
}

impl<'a> Default for ImportOptions<'a> {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            cancellation: None,
        }
    }
}

impl<'a> ImportOptions<'a> {
    pub fn with_max_file_size(mut self, max_file_size: u64) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    pub fn with_cancellation(mut self, cancellation: &'a dyn ImportCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportedDocument {
    pub display_name: String,
    pub relative_path: String,
    pub content_hash: String,
    pub file_size: u64,
    pub mime_type: String,
    #[serde(skip)]
    pub stored_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextChunk {
    pub chunk_index: usize,
    pub text: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub anchor: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ParsedDocument {
    Text {
        text: String,
        chunks: Vec<TextChunk>,
    },
    PendingSpecializedParser {
        mime_type: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecializedDocumentKind {
    Docx,
    Pptx,
    Xlsx,
}

pub fn import_document(
    managed_root: impl AsRef<Path>,
    knowledge_base_id: &str,
    source_path: impl AsRef<Path>,
    options: ImportOptions<'_>,
) -> Result<ImportedDocument, ImportError> {
    check_cancelled(&options)?;

    let managed_root = managed_root.as_ref();
    let source_path = source_path.as_ref();
    validate_source_path(source_path)?;
    validate_path_component(knowledge_base_id, "knowledge base ID")?;

    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ImportError::new(CODE_SOURCE_MISSING, "the source file does not exist")
        } else {
            ImportError::new(
                CODE_SOURCE_UNREADABLE,
                format!("cannot inspect the source file: {error}"),
            )
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(ImportError::new(
            CODE_SOURCE_NOT_FILE,
            "the source must be a regular file",
        ));
    }

    let display_name = source_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            ImportError::new(CODE_UNSAFE_PATH, "the source filename is not valid UTF-8")
        })?
        .to_owned();
    validate_path_component(&display_name, "source filename")?;

    let mime_type = mime_type_for_filename(&display_name)?;
    let file_size = metadata.len();
    if file_size > options.max_file_size {
        return Err(ImportError::new(
            CODE_FILE_TOO_LARGE,
            format!(
                "the source file is {file_size} bytes but the limit is {} bytes",
                options.max_file_size
            ),
        ));
    }

    let staging_parent = ensure_managed_directory(managed_root, &[STAGING_DIRECTORY])?;
    let documents_directory = ensure_managed_directory(
        managed_root,
        &[
            KNOWLEDGE_BASES_DIRECTORY,
            knowledge_base_id,
            DOCUMENTS_DIRECTORY,
        ],
    )?;
    let final_path = documents_directory.join(&display_name);
    if destination_conflicts(&documents_directory, &display_name)? {
        return Err(ImportError::new(
            CODE_DESTINATION_EXISTS,
            format!("the destination already exists: {display_name}"),
        ));
    }

    check_cancelled(&options)?;
    let staging_directory = create_staging_directory(&staging_parent)?;
    let staging_path = staging_directory.join(&display_name);
    let result = import_into_staging(
        source_path,
        &staging_path,
        &final_path,
        display_name,
        mime_type,
        options,
    );

    match result {
        Ok(document) => {
            if let Err(error) = fs::remove_dir_all(&staging_directory) {
                return Err(ImportError::new(
                    CODE_STORAGE_FAILED,
                    format!("the staging directory could not be cleaned: {error}"),
                ));
            }
            Ok(document)
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_directory);
            Err(error)
        }
    }
}

fn import_into_staging(
    source_path: &Path,
    staging_path: &Path,
    final_path: &Path,
    display_name: String,
    mime_type: String,
    options: ImportOptions<'_>,
) -> Result<ImportedDocument, ImportError> {
    let (file_size, content_hash) = copy_and_hash(source_path, staging_path, &options)?;
    check_cancelled(&options)?;

    if destination_conflicts(
        final_path.parent().unwrap_or_else(|| Path::new(".")),
        &display_name,
    )? {
        return Err(ImportError::new(
            CODE_DESTINATION_EXISTS,
            format!("the destination already exists: {display_name}"),
        ));
    }
    fs::rename(staging_path, final_path).map_err(|error| {
        ImportError::new(
            CODE_COMMIT_FAILED,
            format!("the staged document could not be committed: {error}"),
        )
    })?;

    Ok(ImportedDocument {
        display_name,
        relative_path: final_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_owned(),
        content_hash,
        file_size,
        mime_type,
        stored_path: final_path.to_path_buf(),
    })
}

fn copy_and_hash(
    source_path: &Path,
    staging_path: &Path,
    options: &ImportOptions<'_>,
) -> Result<(u64, String), ImportError> {
    let mut source = File::open(source_path).map_err(|error| {
        ImportError::new(
            CODE_SOURCE_UNREADABLE,
            format!("the source file could not be opened: {error}"),
        )
    })?;
    let mut staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging_path)
        .map_err(|error| {
            ImportError::new(
                CODE_COPY_FAILED,
                format!("the staging file could not be created: {error}"),
            )
        })?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    let mut total = 0u64;
    loop {
        check_cancelled(options)?;
        let bytes_read = source.read(&mut buffer).map_err(|error| {
            ImportError::new(
                CODE_COPY_FAILED,
                format!("the source file could not be read: {error}"),
            )
        })?;
        if bytes_read == 0 {
            break;
        }

        total = total.checked_add(bytes_read as u64).ok_or_else(|| {
            ImportError::new(CODE_FILE_TOO_LARGE, "the source file size overflowed")
        })?;
        if total > options.max_file_size {
            return Err(ImportError::new(
                CODE_FILE_TOO_LARGE,
                format!(
                    "the source file exceeds the limit of {} bytes",
                    options.max_file_size
                ),
            ));
        }

        staged.write_all(&buffer[..bytes_read]).map_err(|error| {
            ImportError::new(
                CODE_COPY_FAILED,
                format!("the staging file could not be written: {error}"),
            )
        })?;
        hasher.update(&buffer[..bytes_read]);
    }
    check_cancelled(options)?;
    staged.sync_all().map_err(|error| {
        ImportError::new(
            CODE_COPY_FAILED,
            format!("the staging file could not be flushed: {error}"),
        )
    })?;

    Ok((total, hex::encode(hasher.finalize())))
}

pub fn parse_imported_document(
    document: &ImportedDocument,
    max_chunk_bytes: usize,
) -> Result<ParsedDocument, ImportError> {
    let bytes = read_stored_document(&document.stored_path)?;
    let text = match document.mime_type.as_str() {
        "text/plain" | "text/markdown" => parse_utf8_text(&bytes)?,
        "application/pdf" => parse_pdf_text(&bytes)?,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            parse_zip_based_text(&bytes, SpecializedDocumentKind::Docx)?
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            parse_zip_based_text(&bytes, SpecializedDocumentKind::Pptx)?
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            parse_zip_based_text(&bytes, SpecializedDocumentKind::Xlsx)?
        }
        _ => {
            return Ok(ParsedDocument::PendingSpecializedParser {
                mime_type: document.mime_type.clone(),
            });
        }
    };
    let chunks = chunk_utf8_text(&text, max_chunk_bytes)?;
    Ok(ParsedDocument::Text { text, chunks })
}

fn read_stored_document(path: &Path) -> Result<Vec<u8>, ImportError> {
    let file = File::open(path).map_err(|error| {
        ImportError::new(
            CODE_PARSE_FAILED,
            format!("the imported document could not be read: {error}"),
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        ImportError::new(
            CODE_PARSE_FAILED,
            format!("the imported document metadata could not be read: {error}"),
        )
    })?;
    if metadata.len() > DEFAULT_MAX_FILE_SIZE {
        return Err(ImportError::new(
            CODE_FILE_TOO_LARGE,
            format!("the imported document exceeds the limit of {DEFAULT_MAX_FILE_SIZE} bytes"),
        ));
    }
    let capacity = usize::try_from(metadata.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(DEFAULT_MAX_FILE_SIZE.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ImportError::new(
                CODE_PARSE_FAILED,
                format!("the imported document could not be read: {error}"),
            )
        })?;
    if bytes.len() as u64 > DEFAULT_MAX_FILE_SIZE {
        return Err(ImportError::new(
            CODE_FILE_TOO_LARGE,
            format!("the imported document exceeds the limit of {DEFAULT_MAX_FILE_SIZE} bytes"),
        ));
    }
    Ok(bytes)
}

fn parse_utf8_text(bytes: &[u8]) -> Result<String, ImportError> {
    String::from_utf8(bytes.to_vec()).map_err(|error| {
        ImportError::new(
            CODE_PARSE_FAILED,
            format!("the text document is not valid UTF-8: {error}"),
        )
    })
}

#[derive(Debug, Clone)]
struct ZipEntry {
    name: String,
    flags: u16,
    method: u16,
    compressed_size: u64,
    uncompressed_size: u64,
    crc32: u32,
    local_header_offset: u64,
}

struct SafeZipArchive<'a> {
    bytes: &'a [u8],
    entries: Vec<ZipEntry>,
}

impl<'a> SafeZipArchive<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, ImportError> {
        const END_OF_CENTRAL_DIRECTORY_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
        const CENTRAL_DIRECTORY_SIGNATURE: &[u8; 4] = b"PK\x01\x02";
        const END_OF_CENTRAL_DIRECTORY_SIZE: usize = 22;
        const MAX_ZIP_COMMENT_BYTES: usize = u16::MAX as usize;

        if bytes.len() < END_OF_CENTRAL_DIRECTORY_SIZE {
            return Err(parse_error("the ZIP archive is truncated"));
        }

        let search_start = bytes
            .len()
            .saturating_sub(END_OF_CENTRAL_DIRECTORY_SIZE + MAX_ZIP_COMMENT_BYTES);
        let search_end = bytes.len() - END_OF_CENTRAL_DIRECTORY_SIZE;
        let end_offset = (search_start..=search_end)
            .rev()
            .find(|&offset| bytes[offset..].starts_with(END_OF_CENTRAL_DIRECTORY_SIGNATURE))
            .ok_or_else(|| parse_error("the ZIP end record is missing"))?;
        let end_record = &bytes[end_offset..];
        let comment_length = read_u16(end_record, 20)
            .ok_or_else(|| parse_error("the ZIP end record is truncated"))?
            as usize;
        if end_offset
            .checked_add(END_OF_CENTRAL_DIRECTORY_SIZE)
            .and_then(|offset| offset.checked_add(comment_length))
            != Some(bytes.len())
        {
            return Err(parse_error(
                "the ZIP end record has an invalid comment length",
            ));
        }

        let disk_number =
            read_u16(end_record, 4).ok_or_else(|| parse_error("the ZIP disk number is missing"))?;
        let central_directory_disk = read_u16(end_record, 6)
            .ok_or_else(|| parse_error("the ZIP central directory disk is missing"))?;
        let entries_on_disk =
            read_u16(end_record, 8).ok_or_else(|| parse_error("the ZIP entry count is missing"))?;
        let total_entries = read_u16(end_record, 10)
            .ok_or_else(|| parse_error("the ZIP total entry count is missing"))?;
        if disk_number != 0
            || central_directory_disk != 0
            || entries_on_disk != total_entries
            || total_entries == u16::MAX
            || total_entries as usize > MAX_ARCHIVE_ENTRIES
        {
            return Err(parse_error(
                "multi-disk and ZIP64 archives are not supported",
            ));
        }
        let central_directory_size = read_u32(end_record, 12)
            .ok_or_else(|| parse_error("the ZIP central directory size is missing"))?
            as usize;
        let central_directory_offset = read_u32(end_record, 16)
            .ok_or_else(|| parse_error("the ZIP central directory offset is missing"))?
            as usize;
        let central_directory_end = central_directory_offset
            .checked_add(central_directory_size)
            .ok_or_else(|| parse_error("the ZIP central directory size overflowed"))?;
        if central_directory_end > bytes.len() || central_directory_end > end_offset {
            return Err(parse_error(
                "the ZIP central directory is outside the archive",
            ));
        }

        let mut entries = Vec::with_capacity(total_entries as usize);
        let mut names = HashSet::with_capacity(total_entries as usize);
        let mut total_uncompressed_size = 0u64;
        let mut cursor = central_directory_offset;
        for _ in 0..total_entries {
            if cursor
                .checked_add(46)
                .is_none_or(|end| end > central_directory_end)
                || !bytes[cursor..].starts_with(CENTRAL_DIRECTORY_SIGNATURE)
            {
                return Err(parse_error("the ZIP central directory entry is malformed"));
            }
            let entry = &bytes[cursor..];
            let flags =
                read_u16(entry, 8).ok_or_else(|| parse_error("the ZIP entry flags are missing"))?;
            if flags & 1 != 0 {
                return Err(parse_error("encrypted ZIP entries are not supported"));
            }
            let method = read_u16(entry, 10)
                .ok_or_else(|| parse_error("the ZIP compression method is missing"))?;
            let crc32 =
                read_u32(entry, 16).ok_or_else(|| parse_error("the ZIP checksum is missing"))?;
            let compressed_size = read_u32(entry, 20)
                .ok_or_else(|| parse_error("the ZIP compressed size is missing"))?
                as u64;
            let uncompressed_size = read_u32(entry, 24)
                .ok_or_else(|| parse_error("the ZIP uncompressed size is missing"))?
                as u64;
            let name_length = read_u16(entry, 28)
                .ok_or_else(|| parse_error("the ZIP entry name length is missing"))?
                as usize;
            let extra_length = read_u16(entry, 30)
                .ok_or_else(|| parse_error("the ZIP entry extra length is missing"))?
                as usize;
            let comment_length = read_u16(entry, 32)
                .ok_or_else(|| parse_error("the ZIP entry comment length is missing"))?
                as usize;
            let local_header_offset = read_u32(entry, 42)
                .ok_or_else(|| parse_error("the ZIP local header offset is missing"))?
                as u64;
            let record_length = 46usize
                .checked_add(name_length)
                .and_then(|length| length.checked_add(extra_length))
                .and_then(|length| length.checked_add(comment_length))
                .ok_or_else(|| parse_error("the ZIP entry record size overflowed"))?;
            if cursor
                .checked_add(record_length)
                .is_none_or(|end| end > central_directory_end)
            {
                return Err(parse_error("the ZIP entry record is truncated"));
            }

            let name_bytes = &entry[46..46 + name_length];
            let name = String::from_utf8(name_bytes.to_vec())
                .map_err(|_| parse_error("the ZIP entry name is not valid UTF-8"))?;
            validate_archive_entry_name(&name)?;
            if !names.insert(name.clone()) {
                return Err(parse_error(
                    "the ZIP archive contains duplicate entry names",
                ));
            }
            if uncompressed_size > MAX_ARCHIVE_ENTRY_UNCOMPRESSED_BYTES {
                return Err(parse_error(format!(
                    "ZIP entry {name} exceeds the uncompressed size limit"
                )));
            }
            if compressed_size == 0 && uncompressed_size > 0 {
                return Err(parse_error(format!(
                    "ZIP entry {name} has an invalid compression ratio"
                )));
            }
            if uncompressed_size > compressed_size.saturating_mul(MAX_COMPRESSION_RATIO) {
                return Err(parse_error(format!(
                    "ZIP entry {name} exceeds the compression ratio limit"
                )));
            }
            total_uncompressed_size = total_uncompressed_size
                .checked_add(uncompressed_size)
                .ok_or_else(|| parse_error("ZIP uncompressed size overflowed"))?;
            if total_uncompressed_size > MAX_ARCHIVE_TOTAL_UNCOMPRESSED_BYTES {
                return Err(parse_error(
                    "the ZIP archive exceeds the total uncompressed size limit",
                ));
            }
            if local_header_offset > usize::MAX as u64
                || local_header_offset as usize >= bytes.len()
            {
                return Err(parse_error(format!(
                    "ZIP entry {name} points outside the archive"
                )));
            }

            entries.push(ZipEntry {
                name,
                flags,
                method,
                compressed_size,
                uncompressed_size,
                crc32,
                local_header_offset,
            });
            cursor += record_length;
        }
        if cursor != central_directory_end {
            return Err(parse_error(
                "the ZIP central directory has trailing records",
            ));
        }

        Ok(Self { bytes, entries })
    }

    fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    fn read_entry(&self, name: &str) -> Result<Option<Vec<u8>>, ImportError> {
        let Some(entry) = self.entries.iter().find(|entry| entry.name == name) else {
            return Ok(None);
        };
        let local_offset = usize::try_from(entry.local_header_offset)
            .map_err(|_| parse_error("the ZIP local header offset is too large"))?;
        if local_offset
            .checked_add(30)
            .is_none_or(|end| end > self.bytes.len())
            || !self.bytes[local_offset..].starts_with(b"PK\x03\x04")
        {
            return Err(parse_error(format!(
                "ZIP entry {name} has a malformed local header"
            )));
        }
        let local_header = &self.bytes[local_offset..];
        let local_flags = read_u16(local_header, 6)
            .ok_or_else(|| parse_error("the ZIP local flags are missing"))?;
        let local_method = read_u16(local_header, 8)
            .ok_or_else(|| parse_error("the ZIP local compression method is missing"))?;
        let local_name_length = read_u16(local_header, 26)
            .ok_or_else(|| parse_error("the ZIP local name length is missing"))?
            as usize;
        let local_extra_length = read_u16(local_header, 28)
            .ok_or_else(|| parse_error("the ZIP local extra length is missing"))?
            as usize;
        let data_offset = local_offset
            .checked_add(30)
            .and_then(|offset| offset.checked_add(local_name_length))
            .and_then(|offset| offset.checked_add(local_extra_length))
            .ok_or_else(|| parse_error("the ZIP data offset overflowed"))?;
        let data_length = usize::try_from(entry.compressed_size)
            .map_err(|_| parse_error("the ZIP compressed size is too large"))?;
        let data_end = data_offset
            .checked_add(data_length)
            .ok_or_else(|| parse_error("the ZIP data range overflowed"))?;
        if data_end > self.bytes.len() {
            return Err(parse_error(format!("ZIP entry {name} data is truncated")));
        }
        if local_flags & 1 != 0 || local_flags != entry.flags || local_method != entry.method {
            return Err(parse_error(format!(
                "ZIP entry {name} has inconsistent local metadata"
            )));
        }
        let local_name_end = 30usize
            .checked_add(local_name_length)
            .ok_or_else(|| parse_error("the ZIP local name range overflowed"))?;
        if local_name_end > local_header.len()
            || &local_header[30..local_name_end] != name.as_bytes()
        {
            return Err(parse_error(format!(
                "ZIP entry {name} has inconsistent names"
            )));
        }

        let compressed = &self.bytes[data_offset..data_end];
        let uncompressed_size = usize::try_from(entry.uncompressed_size)
            .map_err(|_| parse_error("the ZIP uncompressed size is too large"))?;
        let data = match entry.method {
            0 => {
                if compressed.len() != uncompressed_size {
                    return Err(parse_error(format!(
                        "stored ZIP entry {name} has an invalid size"
                    )));
                }
                compressed.to_vec()
            }
            8 => {
                let limit = uncompressed_size
                    .checked_add(1)
                    .ok_or_else(|| parse_error("the ZIP uncompressed size overflowed"))?;
                let decoder = DeflateDecoder::new(compressed);
                let mut data = Vec::with_capacity(uncompressed_size.min(1024 * 1024));
                decoder
                    .take(limit as u64)
                    .read_to_end(&mut data)
                    .map_err(|error| {
                        parse_error(format!("ZIP entry {name} failed to inflate: {error}"))
                    })?;
                if data.len() != uncompressed_size {
                    return Err(parse_error(format!(
                        "ZIP entry {name} has an invalid inflated size"
                    )));
                }
                data
            }
            method => {
                return Err(parse_error(format!(
                    "ZIP entry {name} uses unsupported compression method {method}"
                )));
            }
        };
        if crc32_ieee(&data) != entry.crc32 {
            return Err(parse_error(format!("ZIP entry {name} failed its checksum")));
        }
        Ok(Some(data))
    }
}

fn parse_zip_based_text(
    bytes: &[u8],
    kind: SpecializedDocumentKind,
) -> Result<String, ImportError> {
    let archive = SafeZipArchive::new(bytes)?;
    let mut text = String::new();
    match kind {
        SpecializedDocumentKind::Docx => {
            let mut names = archive
                .entry_names()
                .filter(|name| {
                    *name == "word/document.xml"
                        || (name.starts_with("word/header") && name.ends_with(".xml"))
                        || (name.starts_with("word/footer") && name.ends_with(".xml"))
                        || *name == "word/footnotes.xml"
                        || *name == "word/endnotes.xml"
                })
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !names.iter().any(|name| name == "word/document.xml") {
                return Err(parse_error("DOCX main document XML is missing"));
            }
            names.sort_by(|left, right| {
                if left == "word/document.xml" {
                    std::cmp::Ordering::Less
                } else if right == "word/document.xml" {
                    std::cmp::Ordering::Greater
                } else {
                    left.cmp(right)
                }
            });
            for name in names {
                let xml = archive
                    .read_entry(&name)?
                    .ok_or_else(|| parse_error(format!("DOCX entry {name} is missing")))?;
                let section = extract_xml_text(&xml, &["t"])?;
                append_text_section(&mut text, &section)?;
            }
        }
        SpecializedDocumentKind::Pptx => {
            let mut names = archive
                .entry_names()
                .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            names.sort_by_key(|name| {
                name.strip_prefix("ppt/slides/slide")
                    .and_then(|value| value.strip_suffix(".xml"))
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(u32::MAX)
            });
            if names.is_empty() {
                return Err(parse_error("PPTX contains no slide XML"));
            }
            for name in names {
                let xml = archive
                    .read_entry(&name)?
                    .ok_or_else(|| parse_error(format!("PPTX entry {name} is missing")))?;
                let section = extract_xml_text(&xml, &["t"])?;
                append_text_section(&mut text, &section)?;
            }
        }
        SpecializedDocumentKind::Xlsx => {
            let mut names = archive
                .entry_names()
                .filter(|name| {
                    *name == "xl/sharedStrings.xml"
                        || (name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
                })
                .map(str::to_owned)
                .collect::<Vec<_>>();
            names.sort();
            if names.is_empty() {
                return Err(parse_error("XLSX contains no worksheet XML"));
            }
            for name in names {
                let xml = archive
                    .read_entry(&name)?
                    .ok_or_else(|| parse_error(format!("XLSX entry {name} is missing")))?;
                let allowed = if name == "xl/sharedStrings.xml" {
                    &["t"][..]
                } else {
                    &["t", "v"][..]
                };
                let section = extract_xml_text(&xml, allowed)?;
                append_text_section(&mut text, &section)?;
            }
        }
    }
    Ok(text)
}

fn append_text_section(output: &mut String, section: &str) -> Result<(), ImportError> {
    let section = section.trim();
    if section.is_empty() {
        return Ok(());
    }
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    append_bounded_text(output, section.as_bytes())
}

fn parse_error(message: impl Into<String>) -> ImportError {
    ImportError::new(CODE_PARSE_FAILED, message)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let bytes = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn validate_archive_entry_name(name: &str) -> Result<(), ImportError> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') {
        return Err(parse_error("the ZIP archive contains an unsafe entry path"));
    }
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty() {
        return Ok(());
    }
    for component in name.split('/') {
        validate_path_component(component, "ZIP entry path component")?;
    }
    Ok(())
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
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

fn extract_xml_text(xml: &[u8], allowed_local_names: &[&str]) -> Result<String, ImportError> {
    let xml = xml.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(xml);
    let xml = std::str::from_utf8(xml)
        .map_err(|error| parse_error(format!("OOXML XML is not valid UTF-8: {error}")))?;
    let mut output = String::new();
    let mut stack = Vec::<String>::new();
    let mut cursor = 0usize;

    while cursor < xml.len() {
        let Some(relative_open) = xml[cursor..].find('<') else {
            if is_xml_text_context(&stack, allowed_local_names) {
                append_decoded_xml_text(&mut output, &xml[cursor..])?;
            }
            break;
        };
        let open = cursor + relative_open;
        if open > cursor && is_xml_text_context(&stack, allowed_local_names) {
            append_decoded_xml_text(&mut output, &xml[cursor..open])?;
        }

        if xml[open..].starts_with("<!--") {
            let comment_end = xml[open + 4..]
                .find("-->")
                .map(|offset| open + 4 + offset + 3)
                .ok_or_else(|| parse_error("OOXML XML contains an unterminated comment"))?;
            cursor = comment_end;
            continue;
        }
        if xml[open..].starts_with("<![CDATA[") {
            let cdata_start = open + "<![CDATA[".len();
            let cdata_end = xml[cdata_start..]
                .find("]]>")
                .map(|offset| cdata_start + offset)
                .ok_or_else(|| parse_error("OOXML XML contains an unterminated CDATA section"))?;
            if is_xml_text_context(&stack, allowed_local_names) {
                append_bounded_text(&mut output, &xml[cdata_start..cdata_end].as_bytes())?;
            }
            cursor = cdata_end + 3;
            continue;
        }

        let tag_end = find_xml_tag_end(xml, open + 1)
            .ok_or_else(|| parse_error("OOXML XML contains an unterminated tag"))?;
        if tag_end.saturating_sub(open + 1) > MAX_XML_TAG_BYTES {
            return Err(parse_error("OOXML XML tag exceeds the size limit"));
        }
        let raw_tag = &xml[open..=tag_end];
        let trimmed_tag = raw_tag.trim();
        if trimmed_tag.starts_with("<!DOCTYPE") || trimmed_tag.starts_with("<!doctype") {
            return Err(parse_error("OOXML XML document types are not supported"));
        }
        if trimmed_tag.starts_with("<?") || trimmed_tag.starts_with("<!") {
            cursor = tag_end + 1;
            continue;
        }
        let (closing, self_closing, name) = parse_xml_tag(raw_tag)?;
        let local_name = xml_local_name(name);
        if closing {
            let Some(open_name) = stack.pop() else {
                return Err(parse_error("OOXML XML has an unexpected closing tag"));
            };
            if open_name != name {
                return Err(parse_error("OOXML XML tags are not properly nested"));
            }
            if matches!(local_name, "p" | "tr" | "row") {
                append_bounded_text(&mut output, b"\n")?;
            }
        } else {
            if stack.len() >= MAX_XML_DEPTH {
                return Err(parse_error("OOXML XML nesting exceeds the depth limit"));
            }
            if matches!(local_name, "br" | "cr") && !stack.is_empty() {
                append_bounded_text(&mut output, b"\n")?;
            } else if local_name == "tab" && !stack.is_empty() {
                append_bounded_text(&mut output, b"\t")?;
            }
            if !self_closing {
                stack.push(name.to_owned());
            } else if matches!(local_name, "p" | "tr" | "row") {
                append_bounded_text(&mut output, b"\n")?;
            }
        }
        cursor = tag_end + 1;
    }

    if !stack.is_empty() {
        return Err(parse_error("OOXML XML has unclosed tags"));
    }
    Ok(normalize_extracted_text(output))
}

fn append_decoded_xml_text(output: &mut String, text: &str) -> Result<(), ImportError> {
    let decoded = decode_xml_entities(text)?;
    append_bounded_text(output, decoded.as_bytes())
}

fn append_bounded_text(output: &mut String, bytes: &[u8]) -> Result<(), ImportError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| parse_error(format!("extracted document text is not UTF-8: {error}")))?;
    if output
        .len()
        .checked_add(text.len())
        .is_none_or(|length| length > MAX_EXTRACTED_TEXT_BYTES)
    {
        return Err(parse_error(
            "extracted document text exceeds the size limit",
        ));
    }
    output.push_str(text);
    Ok(())
}

fn decode_xml_entities(text: &str) -> Result<String, ImportError> {
    if !text.contains('&') {
        return Ok(text.to_owned());
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(relative_ampersand) = text[cursor..].find('&') {
        let ampersand = cursor + relative_ampersand;
        output.push_str(&text[cursor..ampersand]);
        let semicolon = text[ampersand + 1..]
            .find(';')
            .map(|offset| ampersand + 1 + offset)
            .ok_or_else(|| parse_error("OOXML XML contains an unterminated entity"))?;
        let entity = &text[ampersand + 1..semicolon];
        let character = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                let value = u32::from_str_radix(&entity[2..], 16)
                    .map_err(|_| parse_error("OOXML XML contains an invalid numeric entity"))?;
                char::from_u32(value)
                    .ok_or_else(|| parse_error("OOXML XML contains an invalid Unicode entity"))?
            }
            _ if entity.starts_with('#') => {
                let value = entity[1..]
                    .parse::<u32>()
                    .map_err(|_| parse_error("OOXML XML contains an invalid numeric entity"))?;
                char::from_u32(value)
                    .ok_or_else(|| parse_error("OOXML XML contains an invalid Unicode entity"))?
            }
            _ => return Err(parse_error("OOXML XML contains an unknown entity")),
        };
        output.push(character);
        cursor = semicolon + 1;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

fn is_xml_text_context(stack: &[String], allowed_local_names: &[&str]) -> bool {
    stack.iter().any(|name| {
        let local_name = xml_local_name(name);
        allowed_local_names
            .iter()
            .any(|allowed| *allowed == local_name)
    })
}

fn xml_local_name(name: &str) -> &str {
    name.rsplit_once(':')
        .map_or(name, |(_, local_name)| local_name)
}

fn parse_xml_tag(tag: &str) -> Result<(bool, bool, &str), ImportError> {
    let body = tag
        .strip_prefix('<')
        .and_then(|tag| tag.strip_suffix('>'))
        .ok_or_else(|| parse_error("OOXML XML tag delimiters are invalid"))?
        .trim();
    let closing = body.starts_with('/');
    let body = body.strip_prefix('/').unwrap_or(body).trim_start();
    let self_closing = !closing && body.ends_with('/');
    let body = if self_closing {
        body[..body.len() - 1].trim_end()
    } else {
        body
    };
    let name_length = body
        .char_indices()
        .find(|(_, character)| character.is_whitespace() || *character == '/')
        .map_or(body.len(), |(index, _)| index);
    if name_length == 0 {
        return Err(parse_error("OOXML XML tag name is missing"));
    }
    let name = &body[..name_length];
    if !name.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, ':' | '_' | '-' | '.')
    }) {
        return Err(parse_error("OOXML XML tag name is invalid"));
    }
    Ok((closing, self_closing, name))
}

fn find_xml_tag_end(xml: &str, start: usize) -> Option<usize> {
    let bytes = xml.as_bytes();
    let mut quote = None;
    for (offset, byte) in bytes.get(start..)?.iter().enumerate() {
        match quote {
            Some(current_quote) if *byte == current_quote => quote = None,
            None if *byte == b'"' || *byte == b'\'' => quote = Some(*byte),
            None if *byte == b'>' => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn normalize_extracted_text(mut text: String) -> String {
    if text.contains('\r') {
        text = text.replace("\r\n", "\n").replace('\r', "\n");
    }
    text.trim().to_owned()
}

fn parse_pdf_text(bytes: &[u8]) -> Result<String, ImportError> {
    let header_limit = bytes.len().min(1024);
    if find_subslice(&bytes[..header_limit], b"%PDF-").is_none() {
        return Err(parse_error("the PDF header is missing"));
    }

    let mut output = String::new();
    let mut cursor = 0usize;
    let mut stream_count = 0usize;
    while let Some(relative_stream) = find_pdf_keyword(&bytes[cursor..], b"stream") {
        if stream_count >= MAX_PDF_STREAMS {
            return Err(parse_error("the PDF contains too many streams"));
        }
        let stream_offset = cursor + relative_stream;
        let dictionary = find_pdf_stream_dictionary(bytes, stream_offset);
        let data_start = skip_pdf_line_ending(bytes, stream_offset + b"stream".len())
            .ok_or_else(|| parse_error("the PDF stream start is truncated"))?;
        let (data_end, endstream_end) = pdf_stream_data_end(bytes, data_start, dictionary)?;
        if data_end < data_start || data_end > bytes.len() {
            return Err(parse_error("the PDF stream range is invalid"));
        }
        let raw_stream = &bytes[data_start..data_end];
        let stream = if dictionary
            .is_some_and(|dictionary| find_subslice(dictionary, b"/FlateDecode").is_some())
        {
            inflate_pdf_stream(raw_stream)?
        } else if dictionary.is_some_and(|dictionary| {
            find_subslice(dictionary, b"/Filter").is_some()
                && find_subslice(dictionary, b"/Subtype /Image").is_none()
        }) {
            return Err(parse_error(
                "the PDF uses an unsupported stream filter for text extraction",
            ));
        } else {
            raw_stream.to_vec()
        };
        let section = extract_pdf_text_object(&stream)?;
        if !section.is_empty() {
            append_text_section(&mut output, &section)?;
        }
        stream_count += 1;
        cursor = endstream_end;
    }
    if stream_count == 0 {
        return Err(parse_error("the PDF contains no readable streams"));
    }
    Ok(normalize_extracted_text(output))
}

fn find_pdf_stream_dictionary<'a>(bytes: &'a [u8], stream_offset: usize) -> Option<&'a [u8]> {
    let before_stream = &bytes[..stream_offset];
    let dictionary_start = find_last_subslice(before_stream, b"<<")?;
    let dictionary_end =
        find_subslice(&before_stream[dictionary_start + 2..], b">>")? + dictionary_start + 2;
    if dictionary_end > stream_offset {
        return None;
    }
    Some(&bytes[dictionary_start..dictionary_end + 2])
}

fn pdf_stream_data_end(
    bytes: &[u8],
    data_start: usize,
    dictionary: Option<&[u8]>,
) -> Result<(usize, usize), ImportError> {
    if let Some(length) = dictionary.and_then(parse_pdf_length) {
        let data_end = data_start
            .checked_add(length)
            .ok_or_else(|| parse_error("the PDF stream length overflowed"))?;
        if data_end > bytes.len() {
            return Err(parse_error("the PDF stream length exceeds the file"));
        }
        let endstream_start = skip_pdf_whitespace(bytes, data_end);
        if !bytes[endstream_start..].starts_with(b"endstream") {
            return Err(parse_error("the PDF stream end marker is missing"));
        }
        return Ok((data_end, endstream_start + b"endstream".len()));
    }
    let relative_endstream = find_pdf_keyword(&bytes[data_start..], b"endstream")
        .ok_or_else(|| parse_error("the PDF stream end marker is missing"))?;
    let mut data_end = data_start + relative_endstream;
    while data_end > data_start && matches!(bytes[data_end - 1], b'\r' | b'\n' | b' ' | b'\t') {
        data_end -= 1;
    }
    Ok((
        data_end,
        data_start + relative_endstream + b"endstream".len(),
    ))
}

fn parse_pdf_length(dictionary: &[u8]) -> Option<usize> {
    let offset = find_subslice(dictionary, b"/Length")? + b"/Length".len();
    let mut cursor = offset;
    while dictionary
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        cursor += 1;
    }
    let start = cursor;
    while dictionary
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_digit())
    {
        cursor += 1;
    }
    if start == cursor {
        return None;
    }
    std::str::from_utf8(&dictionary[start..cursor])
        .ok()?
        .parse::<usize>()
        .ok()
}

fn inflate_pdf_stream(raw_stream: &[u8]) -> Result<Vec<u8>, ImportError> {
    if raw_stream.is_empty() {
        return Ok(Vec::new());
    }
    let decoder = ZlibDecoder::new(raw_stream);
    let mut output = Vec::new();
    decoder
        .take(MAX_PDF_STREAM_UNCOMPRESSED_BYTES.saturating_add(1))
        .read_to_end(&mut output)
        .map_err(|error| {
            parse_error(format!(
                "the PDF Flate stream could not be decoded: {error}"
            ))
        })?;
    if output.len() as u64 > MAX_PDF_STREAM_UNCOMPRESSED_BYTES {
        return Err(parse_error(
            "the PDF stream exceeds the decompressed size limit",
        ));
    }
    if output.len() as u64 > (raw_stream.len() as u64).saturating_mul(MAX_COMPRESSION_RATIO) {
        return Err(parse_error(
            "the PDF stream exceeds the compression ratio limit",
        ));
    }
    Ok(output)
}

fn extract_pdf_text_object(bytes: &[u8]) -> Result<String, ImportError> {
    let mut output = String::new();
    let mut cursor = 0usize;
    let mut in_text_object = false;
    while cursor < bytes.len() {
        if !in_text_object {
            if let Some(offset) = find_pdf_keyword(&bytes[cursor..], b"BT") {
                cursor += offset + 2;
                in_text_object = true;
            } else {
                break;
            }
            continue;
        }
        if let Some(offset) = pdf_keyword_at(bytes, cursor, b"ET") {
            cursor = offset + 2;
            in_text_object = false;
            continue;
        }
        match bytes[cursor] {
            b'(' => {
                let (end, value) = parse_pdf_literal_string(bytes, cursor)?;
                append_pdf_string(&mut output, &value)?;
                cursor = end;
            }
            b'<' if bytes.get(cursor + 1) != Some(&b'<') => {
                let (end, value) = parse_pdf_hex_string(bytes, cursor)?;
                append_pdf_string(&mut output, &value)?;
                cursor = end;
            }
            _ => cursor += 1,
        }
    }
    if in_text_object {
        return Err(parse_error("the PDF text object is not closed"));
    }
    Ok(normalize_extracted_text(output))
}

fn parse_pdf_literal_string(bytes: &[u8], start: usize) -> Result<(usize, Vec<u8>), ImportError> {
    let mut output = Vec::new();
    let mut cursor = start + 1;
    let mut depth = 1usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'(' => {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| parse_error("the PDF string nesting overflowed"))?;
                output.push(b'(');
                cursor += 1;
            }
            b')' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Ok((cursor, output));
                }
                output.push(b')');
            }
            b'\\' => {
                cursor += 1;
                let Some(&escaped) = bytes.get(cursor) else {
                    return Err(parse_error("the PDF string escape is truncated"));
                };
                match escaped {
                    b'n' => output.push(b'\n'),
                    b'r' => output.push(b'\r'),
                    b't' => output.push(b'\t'),
                    b'b' => output.push(8),
                    b'f' => output.push(12),
                    b'(' | b')' | b'\\' => output.push(escaped),
                    b'\n' => {}
                    b'\r' => {
                        if bytes.get(cursor + 1) == Some(&b'\n') {
                            cursor += 1;
                        }
                    }
                    digit @ b'0'..=b'7' => {
                        let mut value = u32::from(digit - b'0');
                        let mut digits = 1;
                        while digits < 3 {
                            let Some(next @ b'0'..=b'7') = bytes.get(cursor + 1) else {
                                break;
                            };
                            value = value * 8 + u32::from(*next - b'0');
                            cursor += 1;
                            digits += 1;
                        }
                        output.push(value as u8);
                    }
                    other => output.push(other),
                }
                cursor += 1;
            }
            byte => {
                output.push(byte);
                cursor += 1;
            }
        }
    }
    Err(parse_error("the PDF literal string is not closed"))
}

fn parse_pdf_hex_string(bytes: &[u8], start: usize) -> Result<(usize, Vec<u8>), ImportError> {
    let mut digits = Vec::new();
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'>' => {
                if digits.len() % 2 != 0 {
                    digits.push(0);
                }
                let value = digits
                    .chunks_exact(2)
                    .map(|pair| (pair[0] << 4) | pair[1])
                    .collect();
                return Ok((cursor + 1, value));
            }
            byte if byte.is_ascii_whitespace() => cursor += 1,
            byte => {
                let value = match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => return Err(parse_error("the PDF hex string contains invalid data")),
                };
                digits.push(value);
                cursor += 1;
            }
        }
    }
    Err(parse_error("the PDF hex string is not closed"))
}

fn append_pdf_string(output: &mut String, bytes: &[u8]) -> Result<(), ImportError> {
    let text = decode_pdf_text(bytes);
    if text.is_empty() {
        return Ok(());
    }
    if !output.is_empty() && !output.ends_with([' ', '\n', '\t']) {
        append_bounded_text(output, b" ")?;
    }
    append_bounded_text(output, text.as_bytes())
}

fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xfe, 0xff]) && bytes.len() % 2 == 0 {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        return String::from_utf16_lossy(&units.collect::<Vec<_>>());
    }
    if bytes.starts_with(&[0xff, 0xfe]) && bytes.len() % 2 == 0 {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        return String::from_utf16_lossy(&units.collect::<Vec<_>>());
    }
    String::from_utf8_lossy(bytes)
        .replace('\0', "")
        .trim()
        .to_owned()
}

fn skip_pdf_line_ending(bytes: &[u8], offset: usize) -> Option<usize> {
    match bytes.get(offset..offset.checked_add(2)?) {
        Some(b"\r\n") => Some(offset + 2),
        _ => match bytes.get(offset) {
            Some(b'\n' | b'\r') => Some(offset + 1),
            Some(_) => Some(offset),
            None => None,
        },
    }
}

fn skip_pdf_whitespace(bytes: &[u8], mut offset: usize) -> usize {
    while bytes
        .get(offset)
        .is_some_and(|byte| matches!(byte, b'\0' | b'\t' | b'\n' | b'\r' | b' ' | b'\x0c'))
    {
        offset += 1;
    }
    offset
}

fn find_pdf_keyword(bytes: &[u8], keyword: &[u8]) -> Option<usize> {
    (0..=bytes.len().saturating_sub(keyword.len()))
        .find(|&offset| pdf_keyword_at(bytes, offset, keyword).is_some())
}

fn pdf_keyword_at(bytes: &[u8], offset: usize, keyword: &[u8]) -> Option<usize> {
    let end = offset.checked_add(keyword.len())?;
    if bytes.get(offset..end)? != keyword {
        return None;
    }
    let before_is_token = offset
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| is_pdf_token_character(*byte));
    let after_is_token = bytes
        .get(end)
        .is_some_and(|byte| is_pdf_token_character(*byte));
    if before_is_token || after_is_token {
        None
    } else {
        Some(offset)
    }
}

fn is_pdf_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_last_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(haystack.len());
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

pub fn chunk_utf8_text(text: &str, max_chunk_bytes: usize) -> Result<Vec<TextChunk>, ImportError> {
    if max_chunk_bytes == 0 {
        return Err(ImportError::new(
            CODE_INVALID_CHUNK_CONFIG,
            "the chunk size must be greater than zero",
        ));
    }

    let mut chunks = Vec::new();
    let mut start_offset = 0usize;
    while start_offset < text.len() {
        let mut end_offset = text.len().min(start_offset.saturating_add(max_chunk_bytes));
        while end_offset > start_offset && !text.is_char_boundary(end_offset) {
            end_offset -= 1;
        }
        if end_offset == start_offset {
            end_offset = next_char_boundary(text, start_offset);
        }

        if end_offset < text.len() {
            if let Some(newline_offset) = text[start_offset..end_offset].rfind('\n') {
                let line_end = start_offset + newline_offset + 1;
                if line_end > start_offset {
                    end_offset = line_end;
                }
            }
        }

        let chunk_text = text[start_offset..end_offset].to_owned();
        chunks.push(TextChunk {
            chunk_index: chunks.len(),
            text: chunk_text,
            start_offset,
            end_offset,
            anchor: format!("bytes:{start_offset}-{end_offset}"),
        });
        start_offset = end_offset;
    }
    Ok(chunks)
}

fn next_char_boundary(text: &str, offset: usize) -> usize {
    text[offset..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| offset + index)
        .unwrap_or(text.len())
}

fn validate_source_path(path: &Path) -> Result<(), ImportError> {
    if path.as_os_str().is_empty() {
        return Err(ImportError::new(
            CODE_SOURCE_MISSING,
            "the source path is empty",
        ));
    }
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) => {}
                Prefix::UNC(_, _)
                | Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_) => {
                    return Err(ImportError::new(
                        CODE_UNSAFE_PATH,
                        "UNC, device and verbatim source paths are not supported",
                    ));
                }
            },
            Component::RootDir => {}
            Component::CurDir | Component::ParentDir => {
                return Err(ImportError::new(
                    CODE_UNSAFE_PATH,
                    "source paths cannot contain relative traversal components",
                ));
            }
            Component::Normal(value) => validate_path_component(value, "source path component")?,
        }
    }
    Ok(())
}

fn validate_path_component(value: impl AsRef<OsStr>, label: &str) -> Result<(), ImportError> {
    let value = value
        .as_ref()
        .to_str()
        .ok_or_else(|| ImportError::new(CODE_UNSAFE_PATH, format!("{label} is not valid UTF-8")))?;
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.chars().any(char::is_control)
        || value.chars().count() > 255
        || value.ends_with(' ')
        || value.ends_with('.')
        || value.chars().any(|character| {
            matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        })
        || is_windows_device_name(value)
    {
        return Err(ImportError::new(
            CODE_UNSAFE_PATH,
            format!("{label} contains an unsafe Windows filename or path component"),
        ));
    }
    Ok(())
}

fn is_windows_device_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

fn mime_type_for_filename(filename: &str) -> Result<String, ImportError> {
    let extension = Path::new(filename)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            ImportError::new(
                CODE_UNSUPPORTED_EXTENSION,
                "the document has no supported extension",
            )
        })?;
    let mime_type = match extension.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => {
            return Err(ImportError::new(
                CODE_UNSUPPORTED_EXTENSION,
                format!("the extension .{extension} is not supported"),
            ));
        }
    };
    Ok(mime_type.to_owned())
}

fn ensure_managed_directory(
    managed_root: &Path,
    components: &[&str],
) -> Result<PathBuf, ImportError> {
    if managed_root.as_os_str().is_empty() {
        return Err(ImportError::new(
            CODE_STORAGE_FAILED,
            "the managed knowledge root is empty",
        ));
    }
    fs::create_dir_all(managed_root).map_err(|error| {
        ImportError::new(
            CODE_STORAGE_FAILED,
            format!("the managed knowledge root could not be created: {error}"),
        )
    })?;
    ensure_directory_without_symlink(managed_root, "managed knowledge root")?;

    let mut current = managed_root.to_path_buf();
    for component in components {
        validate_path_component(component, "managed path component")?;
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ImportError::new(
                    CODE_UNSAFE_PATH,
                    format!("managed path component is a symbolic link: {component}"),
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(ImportError::new(
                    CODE_STORAGE_FAILED,
                    format!("managed path component is not a directory: {component}"),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    ImportError::new(
                        CODE_STORAGE_FAILED,
                        format!("managed directory could not be created: {error}"),
                    )
                })?;
            }
            Err(error) => {
                return Err(ImportError::new(
                    CODE_STORAGE_FAILED,
                    format!("managed directory could not be inspected: {error}"),
                ));
            }
        }
    }
    Ok(current)
}

fn ensure_directory_without_symlink(path: &Path, label: &str) -> Result<(), ImportError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ImportError::new(
            CODE_STORAGE_FAILED,
            format!("{label} could not be inspected: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ImportError::new(
            CODE_UNSAFE_PATH,
            format!("{label} must be a real directory"),
        ));
    }
    Ok(())
}

fn create_staging_directory(parent: &Path) -> Result<PathBuf, ImportError> {
    for _ in 0..16 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let name = format!("import-{}-{nanos}-{sequence}", std::process::id());
        let directory = parent.join(name);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ImportError::new(
                    CODE_STORAGE_FAILED,
                    format!("the staging directory could not be created: {error}"),
                ));
            }
        }
    }
    Err(ImportError::new(
        CODE_STORAGE_FAILED,
        "could not allocate a unique staging directory",
    ))
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn destination_conflicts(directory: &Path, display_name: &str) -> Result<bool, ImportError> {
    if path_exists(&directory.join(display_name)) {
        return Ok(true);
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        ImportError::new(
            CODE_STORAGE_FAILED,
            format!("the document directory could not be inspected: {error}"),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ImportError::new(
                CODE_STORAGE_FAILED,
                format!("the document directory entry could not be read: {error}"),
            )
        })?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(display_name))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn check_cancelled(options: &ImportOptions<'_>) -> Result<(), ImportError> {
    if options
        .cancellation
        .is_some_and(|cancellation| cancellation.is_cancelled())
    {
        return Err(ImportError::new(
            CODE_CANCELLED,
            "the document import was cancelled",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fox-import-{label}-{}",
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn imports_text_atomically_and_returns_hash_metadata() {
        let root = test_root("text");
        let source = root.join("source.md");
        let content = "# 标题\n第一段\n第二段\n";
        fs::write(&source, content.as_bytes()).unwrap();

        let document = import_document(&root, "kb-one", &source, ImportOptions::default()).unwrap();
        assert_eq!(document.display_name, "source.md");
        assert_eq!(document.relative_path, "source.md");
        assert_eq!(document.file_size, content.len() as u64);
        assert_eq!(document.mime_type, "text/markdown");
        assert_eq!(
            document.content_hash,
            "f97871a36f51d8c2c01b04c6c7a55e8e1d22471af77c6df1ad6d92ca17d3407e"
        );
        assert_eq!(fs::read(&document.stored_path).unwrap(), content.as_bytes());

        let parsed = parse_imported_document(&document, 10).unwrap();
        let ParsedDocument::Text { text, chunks } = parsed else {
            panic!("expected text parser result");
        };
        assert_eq!(text, content);
        assert!(!chunks.is_empty());
        assert_eq!(chunks.first().unwrap().start_offset, 0);
        assert_eq!(chunks.last().unwrap().end_offset, content.len());
        assert!(chunks
            .iter()
            .all(|chunk| chunk.anchor
                == format!("bytes:{}-{}", chunk.start_offset, chunk.end_offset)));
        assert!(fs::read_dir(root.join(STAGING_DIRECTORY))
            .unwrap()
            .next()
            .is_none());
        cleanup(&root);
    }

    #[test]
    fn reports_parse_failure_for_malformed_specialized_documents() {
        let root = test_root("binary");
        let source = root.join("report.PDF");
        fs::write(&source, [0xff, 0x00, 0xfe, 0x01]).unwrap();

        let document = import_document(&root, "kb-one", &source, ImportOptions::default()).unwrap();
        assert_eq!(document.mime_type, "application/pdf");
        assert_eq!(
            parse_imported_document(&document, DEFAULT_CHUNK_MAX_BYTES)
                .unwrap_err()
                .code,
            CODE_PARSE_FAILED
        );
        cleanup(&root);
    }

    #[test]
    fn reports_invalid_utf8_for_text_formats() {
        let root = test_root("utf8");
        let source = root.join("broken.txt");
        fs::write(&source, [0xff, 0xfe]).unwrap();
        let document = import_document(&root, "kb", &source, ImportOptions::default()).unwrap();
        assert_eq!(
            parse_imported_document(&document, DEFAULT_CHUNK_MAX_BYTES)
                .unwrap_err()
                .code,
            CODE_PARSE_FAILED
        );
        cleanup(&root);
    }

    #[test]
    fn rejects_missing_directories_unsupported_and_oversized_sources() {
        let root = test_root("reject");
        let missing = root.join("missing.txt");
        assert_eq!(
            import_document(&root, "kb", &missing, ImportOptions::default())
                .unwrap_err()
                .code,
            CODE_SOURCE_MISSING
        );

        let directory = root.join("directory.txt");
        fs::create_dir(&directory).unwrap();
        assert_eq!(
            import_document(&root, "kb", &directory, ImportOptions::default())
                .unwrap_err()
                .code,
            CODE_SOURCE_NOT_FILE
        );

        let unsupported = root.join("archive.zip");
        fs::write(&unsupported, b"not an archive").unwrap();
        assert_eq!(
            import_document(&root, "kb", &unsupported, ImportOptions::default())
                .unwrap_err()
                .code,
            CODE_UNSUPPORTED_EXTENSION
        );

        let oversized = root.join("large.txt");
        fs::write(&oversized, b"12345").unwrap();
        assert_eq!(
            import_document(
                &root,
                "kb",
                &oversized,
                ImportOptions::default().with_max_file_size(4)
            )
            .unwrap_err()
            .code,
            CODE_FILE_TOO_LARGE
        );
        cleanup(&root);
    }

    #[test]
    fn rejects_windows_dangerous_components_and_ads() {
        for value in [
            "../outside.txt",
            "CON.txt",
            "nul",
            "report.txt:secret",
            "trailing-space.txt ",
            "trailing-dot.txt.",
            "folder\\file.txt",
        ] {
            assert!(validate_path_component(value, "test").is_err(), "{value}");
        }
        for value in ["notes.txt", "报告.md", "COM10.txt", "LPT20.md"] {
            assert!(validate_path_component(value, "test").is_ok(), "{value}");
        }
        assert_eq!(
            validate_source_path(Path::new("../source.txt"))
                .unwrap_err()
                .code,
            CODE_UNSAFE_PATH
        );
    }

    #[test]
    fn cancellation_cleans_staging_and_never_commits() {
        let root = test_root("cancel");
        let source = root.join("large.txt");
        fs::write(&source, vec![b'x'; COPY_BUFFER_SIZE * 4]).unwrap();
        let checks = AtomicUsize::new(0);
        let callback = || checks.fetch_add(1, Ordering::Relaxed) >= 3;

        let error = import_document(
            &root,
            "kb",
            &source,
            ImportOptions::default().with_cancellation(&callback),
        )
        .unwrap_err();
        assert_eq!(error.code, CODE_CANCELLED);
        assert!(fs::read_dir(root.join(STAGING_DIRECTORY))
            .unwrap()
            .next()
            .is_none());
        assert!(!root
            .join(KNOWLEDGE_BASES_DIRECTORY)
            .join("kb")
            .join(DOCUMENTS_DIRECTORY)
            .join("large.txt")
            .exists());
        cleanup(&root);
    }

    #[test]
    fn token_cancels_before_any_filesystem_work() {
        let root = test_root("token");
        let source = root.join("source.txt");
        fs::write(&source, b"content").unwrap();
        let token = ImportCancellationToken::new();
        token.cancel();

        let error = import_document(
            &root,
            "kb",
            &source,
            ImportOptions::default().with_cancellation(&token),
        )
        .unwrap_err();
        assert_eq!(error.code, CODE_CANCELLED);
        cleanup(&root);
    }

    #[test]
    fn chunking_keeps_utf8_boundaries_and_offsets() {
        let text = "甲乙\n丙丁\n戊己";
        let chunks = chunk_utf8_text(text, 5).unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            text
        );
        assert_eq!(chunks.first().unwrap().start_offset, 0);
        assert_eq!(chunks.last().unwrap().end_offset, text.len());
        for chunk in &chunks {
            assert!(text.is_char_boundary(chunk.start_offset));
            assert!(text.is_char_boundary(chunk.end_offset));
            assert_eq!(chunk.text, &text[chunk.start_offset..chunk.end_offset]);
        }
        assert_eq!(
            chunk_utf8_text(text, 0).unwrap_err().code,
            CODE_INVALID_CHUNK_CONFIG
        );
    }

    #[test]
    fn parses_minimal_pdf_including_the_second_page() {
        let root = test_root("pdf-pages");
        let source = root.join("two-pages.pdf");
        fs::write(&source, minimal_pdf_fixture()).unwrap();
        let document = import_document(&root, "kb", &source, ImportOptions::default()).unwrap();
        let ParsedDocument::Text { text, chunks } = parse_imported_document(&document, 12).unwrap()
        else {
            panic!("expected parsed PDF text");
        };
        assert!(text.contains("PDF first page"), "{text}");
        assert!(text.contains("PDF second page"), "{text}");
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            text
        );
        cleanup(&root);
    }

    #[test]
    fn parses_minimal_docx_and_reuses_utf8_chunks() {
        let root = test_root("docx");
        let source = root.join("document.docx");
        fs::write(
            &source,
            stored_zip_fixture(&[(
                "word/document.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>DOCX 第一段 &amp; 文字</w:t></w:r></w:p><w:p><w:r><w:t>DOCX 第二段</w:t></w:r></w:p></w:body></w:document>"#.as_bytes(),
            )]),
        )
        .unwrap();
        let document = import_document(&root, "kb", &source, ImportOptions::default()).unwrap();
        let ParsedDocument::Text { text, chunks } = parse_imported_document(&document, 8).unwrap()
        else {
            panic!("expected parsed DOCX text");
        };
        assert_eq!(text, "DOCX 第一段 & 文字\nDOCX 第二段");
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            text
        );
        cleanup(&root);
    }

    #[test]
    fn parses_minimal_pptx_slides_in_numeric_order() {
        let root = test_root("pptx-slides");
        let source = root.join("slides.pptx");
        fs::write(
            &source,
            stored_zip_fixture(&[
                (
                    "ppt/slides/slide2.xml",
                    br#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:p><a:r><a:t>PPTX second slide</a:t></a:r></a:p></p:sld>"#,
                ),
                (
                    "ppt/slides/slide1.xml",
                    br#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:p><a:r><a:t>PPTX first slide</a:t></a:r></a:p></p:sld>"#,
                ),
            ]),
        )
        .unwrap();
        let document = import_document(&root, "kb", &source, ImportOptions::default()).unwrap();
        let ParsedDocument::Text { text, .. } =
            parse_imported_document(&document, DEFAULT_CHUNK_MAX_BYTES).unwrap()
        else {
            panic!("expected parsed PPTX text");
        };
        assert!(text.find("PPTX first slide").unwrap() < text.find("PPTX second slide").unwrap());
        cleanup(&root);
    }

    #[test]
    fn rejects_unsafe_zip_paths_and_excessive_uncompressed_sizes() {
        let unsafe_archive = stored_zip_fixture(&[(
            "../word/document.xml",
            b"<w:document xmlns:w=\"urn:test\"/>",
        )]);
        assert_eq!(
            parse_zip_based_text(&unsafe_archive, SpecializedDocumentKind::Docx)
                .unwrap_err()
                .code,
            CODE_UNSAFE_PATH
        );

        let mut oversized_archive =
            stored_zip_fixture(&[("word/document.xml", b"<w:document xmlns:w=\"urn:test\"/>")]);
        let central_offset = find_subslice(&oversized_archive, b"PK\x01\x02").unwrap();
        let declared_size = (MAX_ARCHIVE_ENTRY_UNCOMPRESSED_BYTES + 1) as u32;
        oversized_archive[central_offset + 24..central_offset + 28]
            .copy_from_slice(&declared_size.to_le_bytes());
        assert_eq!(
            parse_zip_based_text(&oversized_archive, SpecializedDocumentKind::Docx)
                .unwrap_err()
                .code,
            CODE_PARSE_FAILED
        );
    }

    fn stored_zip_fixture(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = Vec::new();
        let mut central_records = Vec::new();
        for (name, data) in entries {
            let local_offset = archive.len() as u32;
            let name_bytes = name.as_bytes();
            let size = data.len() as u32;
            let crc = crc32_ieee(data);
            archive.extend_from_slice(b"PK\x03\x04");
            archive.extend_from_slice(&20u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&crc.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(name_bytes);
            archive.extend_from_slice(data);
            central_records.push((name_bytes, crc, size, local_offset));
        }
        let central_offset = archive.len() as u32;
        for (name, crc, size, local_offset) in &central_records {
            archive.extend_from_slice(b"PK\x01\x02");
            archive.extend_from_slice(&20u16.to_le_bytes());
            archive.extend_from_slice(&20u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&crc.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&size.to_le_bytes());
            archive.extend_from_slice(&(name.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes());
            archive.extend_from_slice(&0u32.to_le_bytes());
            archive.extend_from_slice(&local_offset.to_le_bytes());
            archive.extend_from_slice(name);
        }
        let central_size = archive.len() as u32 - central_offset;
        archive.extend_from_slice(b"PK\x05\x06");
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&(central_records.len() as u16).to_le_bytes());
        archive.extend_from_slice(&(central_records.len() as u16).to_le_bytes());
        archive.extend_from_slice(&central_size.to_le_bytes());
        archive.extend_from_slice(&central_offset.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive
    }

    fn minimal_pdf_fixture() -> Vec<u8> {
        let page_one = b"BT /F1 12 Tf (PDF first page) Tj ET";
        let page_two = b"BT /F1 12 Tf (PDF second page) Tj ET";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                page_one.len(),
                String::from_utf8_lossy(page_one)
            )
            .into_bytes(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R >>".to_vec(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                page_two.len(),
                String::from_utf8_lossy(page_two)
            )
            .into_bytes(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0u32];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len() as u32);
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(object);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref_offset = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                offsets.len(),
                xref_offset
            )
            .as_bytes(),
        );
        pdf
    }
}

use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

#[cfg(feature = "zvec")]
use std::path::{Path, PathBuf};

pub const INVALID_REQUEST_CODE: &str = "vector_index.invalid_request";
pub const GENERATION_EXISTS_CODE: &str = "vector_index.generation_exists";
pub const GENERATION_NOT_FOUND_CODE: &str = "vector_index.generation_not_found";
pub const GENERATION_NOT_READY_CODE: &str = "vector_index.generation_not_ready";
pub const DIMENSION_MISMATCH_CODE: &str = "vector_index.dimension_mismatch";
pub const INVALID_VECTOR_CODE: &str = "vector_index.invalid_vector";
pub const OPERATION_FAILED_CODE: &str = "vector_index.operation_failed";
pub const INTEGRITY_FAILED_CODE: &str = "vector_index.integrity_failed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorIndexError {
    code: &'static str,
    message: String,
    retryable: bool,
}

impl VectorIndexError {
    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        debug_assert!(code.starts_with("vector_index."));
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST_CODE, message, false)
    }

    fn generation_exists(generation_id: &str) -> Self {
        Self::new(
            GENERATION_EXISTS_CODE,
            format!("generation '{generation_id}' already exists"),
            false,
        )
    }

    fn generation_not_found(generation_id: &str) -> Self {
        Self::new(
            GENERATION_NOT_FOUND_CODE,
            format!("generation '{generation_id}' was not found"),
            false,
        )
    }

    fn generation_not_ready(generation_id: &str, operation: &str) -> Self {
        Self::new(
            GENERATION_NOT_READY_CODE,
            format!("generation '{generation_id}' is not ready for {operation}"),
            false,
        )
    }

    fn dimension_mismatch(expected: usize, actual: usize) -> Self {
        Self::new(
            DIMENSION_MISMATCH_CODE,
            format!("vector dimension mismatch: expected {expected}, got {actual}"),
            false,
        )
    }

    fn invalid_vector(message: impl Into<String>) -> Self {
        Self::new(INVALID_VECTOR_CODE, message, false)
    }

    fn operation_failed(message: impl Into<String>) -> Self {
        Self::new(OPERATION_FAILED_CODE, message, true)
    }

    fn integrity_failed(message: impl Into<String>) -> Self {
        Self::new(INTEGRITY_FAILED_CODE, message, false)
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Display for VectorIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for VectorIndexError {}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
}

impl VectorRecord {
    pub fn new(id: impl Into<String>, vector: Vec<f32>) -> Self {
        Self {
            id: id.into(),
            vector,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchRequest {
    pub vector: Vec<f32>,
    pub top_k: usize,
}

impl SearchRequest {
    pub fn new(vector: Vec<f32>, top_k: usize) -> Self {
        Self { vector, top_k }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub generation_id: String,
    pub knowledge_base_id: String,
    pub vector_count: usize,
    pub dimension: Option<usize>,
    pub flushed: bool,
    pub verified: bool,
}

pub type VectorSearchRequest = SearchRequest;
pub type VectorHit = Hit;
pub type VectorStoreCheck = Check;

pub trait VectorStore: Send + Sync {
    fn create_generation(
        &self,
        knowledge_base_id: &str,
        generation_id: &str,
    ) -> Result<(), VectorIndexError>;

    fn upsert(&self, generation_id: &str, records: &[VectorRecord])
        -> Result<(), VectorIndexError>;

    fn search(
        &self,
        generation_id: &str,
        request: SearchRequest,
    ) -> Result<Vec<Hit>, VectorIndexError>;

    fn flush(&self, generation_id: &str) -> Result<(), VectorIndexError>;

    fn verify(&self, generation_id: &str) -> Result<Check, VectorIndexError>;

    fn delete_generation(&self, generation_id: &str) -> Result<(), VectorIndexError>;
}

#[cfg(feature = "zvec")]
use std::collections::BTreeSet;

#[cfg(feature = "zvec")]
use std::sync::OnceLock;

#[cfg(feature = "zvec")]
use zvec_rust::{
    Collection, CollectionSchema, DataType, Doc, FieldSchema, IndexParams, MetricType, SearchQuery,
};

#[cfg(all(feature = "zvec", target_os = "windows"))]
use std::os::windows::ffi::OsStrExt;

#[cfg(all(feature = "zvec", target_os = "windows"))]
use windows::{core::PCWSTR, Win32::System::LibraryLoader::LoadLibraryW};

#[cfg(feature = "zvec")]
static ZVEC_INITIALIZATION: OnceLock<Result<(), String>> = OnceLock::new();

#[cfg(feature = "zvec")]
struct ZvecGeneration {
    knowledge_base_id: String,
    path: PathBuf,
    collection: Option<Arc<Collection>>,
    dimension: Option<usize>,
    record_ids: BTreeSet<String>,
    status: GenerationStatus,
}

#[cfg(feature = "zvec")]
pub struct ZvecVectorStore {
    root: PathBuf,
    generations: Arc<Mutex<BTreeMap<String, ZvecGeneration>>>,
}

#[cfg(feature = "zvec")]
impl ZvecVectorStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, VectorIndexError> {
        let library_dir = resolve_zvec_library_dir(None)?;
        Self::new_with_library_dir(root, library_dir)
    }

    pub fn from_resource_dir(
        root: impl AsRef<Path>,
        resource_dir: impl AsRef<Path>,
    ) -> Result<Self, VectorIndexError> {
        let library_dir = resolve_zvec_library_dir(Some(resource_dir.as_ref()))?;
        Self::new_with_library_dir(root, library_dir)
    }

    pub fn new_with_library_dir(
        root: impl AsRef<Path>,
        library_dir: impl AsRef<Path>,
    ) -> Result<Self, VectorIndexError> {
        let root = root.as_ref().to_path_buf();
        validate_directory_path(&root, "vector store root")?;
        let library_dir = library_dir.as_ref().to_path_buf();
        ensure_zvec_library(&library_dir)?;
        initialize_zvec()?;
        std::fs::create_dir_all(&root).map_err(|error| {
            VectorIndexError::operation_failed(format!(
                "create vector store root '{}': {error}",
                root.display()
            ))
        })?;
        Ok(Self {
            root,
            generations: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    fn lock_generations(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ZvecGeneration>>, VectorIndexError> {
        self.generations
            .lock()
            .map_err(|_| VectorIndexError::operation_failed("vector store lock is poisoned"))
    }

    fn generation_path(&self, knowledge_base_id: &str, generation_id: &str) -> PathBuf {
        self.root
            .join(storage_component(knowledge_base_id))
            .join(storage_component(generation_id))
    }
}

#[cfg(feature = "zvec")]
impl VectorStore for ZvecVectorStore {
    fn create_generation(
        &self,
        knowledge_base_id: &str,
        generation_id: &str,
    ) -> Result<(), VectorIndexError> {
        validate_storage_identifier(knowledge_base_id, "knowledge_base_id")?;
        validate_storage_identifier(generation_id, "generation_id")?;
        let path = self.generation_path(knowledge_base_id, generation_id);
        let mut generations = self.lock_generations()?;
        if generations.contains_key(generation_id) || path.exists() {
            return Err(VectorIndexError::generation_exists(generation_id));
        }
        generations.insert(
            generation_id.to_owned(),
            ZvecGeneration {
                knowledge_base_id: knowledge_base_id.to_owned(),
                path,
                collection: None,
                dimension: None,
                record_ids: BTreeSet::new(),
                status: GenerationStatus::Building,
            },
        );
        Ok(())
    }

    fn upsert(
        &self,
        generation_id: &str,
        records: &[VectorRecord],
    ) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        if records.is_empty() {
            return Ok(());
        }
        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if generation.status != GenerationStatus::Building {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "upsert",
            ));
        }

        let batch_dimension = records[0].vector.len();
        for record in records {
            validate_record(record)?;
            if record.vector.len() != batch_dimension {
                return Err(VectorIndexError::dimension_mismatch(
                    batch_dimension,
                    record.vector.len(),
                ));
            }
            if let Some(expected_dimension) = generation.dimension {
                if record.vector.len() != expected_dimension {
                    return Err(VectorIndexError::dimension_mismatch(
                        expected_dimension,
                        record.vector.len(),
                    ));
                }
            }
        }

        if generation.collection.is_none() {
            let collection = create_zvec_collection(&generation.path, batch_dimension)?;
            generation.collection = Some(Arc::new(collection));
            generation.dimension = Some(batch_dimension);
        }

        let documents = records
            .iter()
            .map(|record| {
                let mut document = Doc::new().map_err(|error| {
                    VectorIndexError::operation_failed(format!("create zvec document: {error}"))
                })?;
                document.set_pk(&record.id);
                document.add_string("id", &record.id).map_err(|error| {
                    VectorIndexError::operation_failed(format!("set zvec document id: {error}"))
                })?;
                document
                    .add_vector_f32("embedding", &record.vector)
                    .map_err(|error| {
                        VectorIndexError::operation_failed(format!(
                            "set zvec document vector: {error}"
                        ))
                    })?;
                Ok(document)
            })
            .collect::<Result<Vec<_>, VectorIndexError>>()?;
        let document_refs = documents.iter().collect::<Vec<_>>();
        let write_result = generation
            .collection
            .as_ref()
            .expect("zvec collection is initialized before upsert")
            .upsert(&document_refs)
            .map_err(|error| VectorIndexError::operation_failed(format!("zvec upsert: {error}")))?;
        if write_result.error_count != 0 {
            return Err(VectorIndexError::operation_failed(format!(
                "zvec upsert partially failed: {} succeeded, {} failed",
                write_result.success_count, write_result.error_count
            )));
        }
        generation
            .record_ids
            .extend(records.iter().map(|record| record.id.clone()));
        Ok(())
    }

    fn search(
        &self,
        generation_id: &str,
        request: SearchRequest,
    ) -> Result<Vec<Hit>, VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        if request.top_k == 0 || request.top_k > i32::MAX as usize {
            return Err(VectorIndexError::invalid_request(
                "top_k must be between 1 and i32::MAX",
            ));
        }
        validate_vector(&request.vector, "query vector")?;
        let generations = self.lock_generations()?;
        let generation = generations
            .get(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if generation.status == GenerationStatus::Building {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "search",
            ));
        }
        let Some(collection) = generation.collection.as_ref() else {
            return Ok(Vec::new());
        };
        if let Some(expected_dimension) = generation.dimension {
            if request.vector.len() != expected_dimension {
                return Err(VectorIndexError::dimension_mismatch(
                    expected_dimension,
                    request.vector.len(),
                ));
            }
        }
        let query = SearchQuery::new("embedding", &request.vector, request.top_k as i32).map_err(
            |error| VectorIndexError::operation_failed(format!("create zvec query: {error}")),
        )?;
        let results = collection
            .query(&query)
            .map_err(|error| VectorIndexError::operation_failed(format!("zvec query: {error}")))?;
        results
            .into_iter()
            .map(|document| {
                let id = document
                    .get_pk()
                    .ok_or_else(|| {
                        VectorIndexError::integrity_failed("zvec result has no primary key")
                    })?
                    .to_owned();
                let score = document.get_score();
                if !score.is_finite() {
                    return Err(VectorIndexError::integrity_failed(
                        "zvec result has a non-finite score",
                    ));
                }
                Ok(Hit { id, score })
            })
            .collect()
    }

    fn flush(&self, generation_id: &str) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if let Some(collection) = generation.collection.as_ref() {
            collection.flush().map_err(|error| {
                VectorIndexError::operation_failed(format!("zvec flush: {error}"))
            })?;
        }
        if generation.status == GenerationStatus::Building {
            generation.status = GenerationStatus::Flushed;
        }
        Ok(())
    }

    fn verify(&self, generation_id: &str) -> Result<Check, VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if generation.status == GenerationStatus::Building {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "verify",
            ));
        }
        let vector_count = if let Some(collection) = generation.collection.as_ref() {
            let stats = collection.stats().map_err(|error| {
                VectorIndexError::operation_failed(format!("zvec stats: {error}"))
            })?;
            usize::try_from(stats.doc_count).map_err(|_| {
                VectorIndexError::integrity_failed("zvec document count exceeds platform limits")
            })?
        } else {
            0
        };
        if vector_count != generation.record_ids.len() {
            return Err(VectorIndexError::integrity_failed(format!(
                "zvec document count mismatch: expected {}, got {vector_count}",
                generation.record_ids.len()
            )));
        }
        generation.status = GenerationStatus::Verified;
        Ok(Check {
            generation_id: generation_id.to_owned(),
            knowledge_base_id: generation.knowledge_base_id.clone(),
            vector_count,
            dimension: generation.dimension,
            flushed: true,
            verified: true,
        })
    }

    fn delete_generation(&self, generation_id: &str) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        let mut generations = self.lock_generations()?;
        let generation = generations
            .remove(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        drop(generations);
        drop(generation.collection);
        if generation.path.exists() {
            std::fs::remove_dir_all(&generation.path).map_err(|error| {
                VectorIndexError::operation_failed(format!(
                    "delete zvec generation '{}': {error}",
                    generation.path.display()
                ))
            })?;
        }
        Ok(())
    }
}

#[cfg(feature = "zvec")]
fn create_zvec_collection(path: &Path, dimension: usize) -> Result<Collection, VectorIndexError> {
    if dimension == 0 {
        return Err(VectorIndexError::invalid_vector(
            "zvec vector dimension must be greater than zero",
        ));
    }
    if path.exists() {
        return Err(VectorIndexError::operation_failed(format!(
            "zvec generation path already exists: {}",
            path.display()
        )));
    }
    let dimension = u32::try_from(dimension)
        .map_err(|_| VectorIndexError::invalid_request("zvec vector dimension exceeds u32::MAX"))?;
    let schema = CollectionSchema::builder("fox_vector_generation")
        .add_field(
            FieldSchema::new("id", DataType::String, false, 0).map_err(|error| {
                VectorIndexError::operation_failed(format!("create zvec id field: {error}"))
            })?,
        )
        .add_vector_field(
            "embedding",
            DataType::VectorFp32,
            dimension,
            IndexParams::hnsw(MetricType::Cosine, 16, 200).map_err(|error| {
                VectorIndexError::operation_failed(format!("create zvec index params: {error}"))
            })?,
        )
        .build()
        .map_err(|error| {
            VectorIndexError::operation_failed(format!("build zvec schema: {error}"))
        })?;
    Collection::create_and_open(path.to_string_lossy().as_ref(), &schema, None).map_err(|error| {
        VectorIndexError::operation_failed(format!("create zvec collection: {error}"))
    })
}

#[cfg(feature = "zvec")]
fn initialize_zvec() -> Result<(), VectorIndexError> {
    match ZVEC_INITIALIZATION
        .get_or_init(|| zvec_rust::initialize(None).map_err(|error| error.to_string()))
    {
        Ok(()) => Ok(()),
        Err(error) => Err(VectorIndexError::operation_failed(format!(
            "initialize zvec: {error}"
        ))),
    }
}

#[cfg(feature = "zvec")]
fn resolve_zvec_library_dir(resource_dir: Option<&Path>) -> Result<PathBuf, VectorIndexError> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("ZVEC_LIB_DIR") {
        candidates.push(PathBuf::from(value));
    }
    if let Some(resource_dir) = resource_dir {
        candidates.push(resource_dir.to_path_buf());
    }
    candidates
        .into_iter()
        .find(|path| path.join(zvec_library_name()).is_file())
        .ok_or_else(|| {
            VectorIndexError::operation_failed(
                "Zvec is unavailable: set ZVEC_LIB_DIR or provide resources/vector/zvec_c_api.dll",
            )
        })
}

#[cfg(feature = "zvec")]
fn ensure_zvec_library(library_dir: &Path) -> Result<(), VectorIndexError> {
    let library_path = library_dir.join(zvec_library_name());
    if !library_path.is_file() {
        return Err(VectorIndexError::operation_failed(format!(
            "Zvec library is missing: {}",
            library_path.display()
        )));
    }
    #[cfg(target_os = "windows")]
    {
        let mut wide = library_path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.push(0);
        unsafe {
            LoadLibraryW(PCWSTR(wide.as_ptr())).map_err(|error| {
                VectorIndexError::operation_failed(format!(
                    "load Zvec library '{}': {error}",
                    library_path.display()
                ))
            })?;
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        return Err(VectorIndexError::operation_failed(
            "Zvec adapter currently supports Windows x64/MSVC only",
        ));
    }
    Ok(())
}

#[cfg(feature = "zvec")]
fn zvec_library_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "zvec_c_api.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libzvec_c_api.dylib"
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "libzvec_c_api.so"
    }
}

#[cfg(feature = "zvec")]
fn validate_storage_identifier(value: &str, field: &str) -> Result<(), VectorIndexError> {
    validate_identifier(value, field)?;
    if value.contains(['/', '\\', ':']) || value == "." || value == ".." {
        return Err(VectorIndexError::invalid_request(format!(
            "{field} must be a single path-safe component"
        )));
    }
    Ok(())
}

#[cfg(feature = "zvec")]
fn validate_directory_path(path: &Path, field: &str) -> Result<(), VectorIndexError> {
    if path.as_os_str().is_empty() {
        return Err(VectorIndexError::invalid_request(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(feature = "zvec")]
fn storage_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenerationStatus {
    Building,
    Flushed,
    Verified,
}

#[derive(Debug, Clone)]
struct MemoryGeneration {
    knowledge_base_id: String,
    dimension: Option<usize>,
    records: BTreeMap<String, VectorRecord>,
    status: GenerationStatus,
}

#[derive(Clone, Default)]
pub struct MemoryVectorStore {
    generations: Arc<Mutex<BTreeMap<String, MemoryGeneration>>>,
}

impl MemoryVectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock_generations(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, MemoryGeneration>>, VectorIndexError>
    {
        self.generations
            .lock()
            .map_err(|_| VectorIndexError::operation_failed("vector store lock is poisoned"))
    }
}

impl VectorStore for MemoryVectorStore {
    fn create_generation(
        &self,
        knowledge_base_id: &str,
        generation_id: &str,
    ) -> Result<(), VectorIndexError> {
        validate_identifier(knowledge_base_id, "knowledge_base_id")?;
        validate_identifier(generation_id, "generation_id")?;

        let mut generations = self.lock_generations()?;
        if generations.contains_key(generation_id) {
            return Err(VectorIndexError::generation_exists(generation_id));
        }

        generations.insert(
            generation_id.to_owned(),
            MemoryGeneration {
                knowledge_base_id: knowledge_base_id.to_owned(),
                dimension: None,
                records: BTreeMap::new(),
                status: GenerationStatus::Building,
            },
        );
        Ok(())
    }

    fn upsert(
        &self,
        generation_id: &str,
        records: &[VectorRecord],
    ) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;

        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;

        if generation.status != GenerationStatus::Building {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "upsert",
            ));
        }

        let expected_dimension = generation.dimension;
        let batch_dimension = records.first().map(|record| record.vector.len());
        for record in records {
            validate_record(record)?;
            if let Some(expected_dimension) = expected_dimension.or(batch_dimension) {
                if record.vector.len() != expected_dimension {
                    return Err(VectorIndexError::dimension_mismatch(
                        expected_dimension,
                        record.vector.len(),
                    ));
                }
            }
        }

        if let (Some(expected_dimension), Some(batch_dimension)) =
            (expected_dimension, batch_dimension)
        {
            if expected_dimension != batch_dimension {
                return Err(VectorIndexError::dimension_mismatch(
                    expected_dimension,
                    batch_dimension,
                ));
            }
        }

        if generation.dimension.is_none() {
            generation.dimension = batch_dimension;
        }
        for record in records {
            generation.records.insert(record.id.clone(), record.clone());
        }
        Ok(())
    }

    fn search(
        &self,
        generation_id: &str,
        request: SearchRequest,
    ) -> Result<Vec<Hit>, VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;
        if request.top_k == 0 {
            return Err(VectorIndexError::invalid_request(
                "top_k must be greater than zero",
            ));
        }
        validate_vector(&request.vector, "query vector")?;

        let generations = self.lock_generations()?;
        let generation = generations
            .get(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if matches!(generation.status, GenerationStatus::Building) {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "search",
            ));
        }

        let Some(expected_dimension) = generation.dimension else {
            return Ok(Vec::new());
        };
        if request.vector.len() != expected_dimension {
            return Err(VectorIndexError::dimension_mismatch(
                expected_dimension,
                request.vector.len(),
            ));
        }

        let mut hits = generation
            .records
            .values()
            .map(|record| {
                let score = cosine_similarity(&request.vector, &record.vector);
                Hit {
                    id: record.id.clone(),
                    score,
                }
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        hits.truncate(request.top_k);
        Ok(hits)
    }

    fn flush(&self, generation_id: &str) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;

        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if generation.status == GenerationStatus::Building {
            generation.status = GenerationStatus::Flushed;
        }
        Ok(())
    }

    fn verify(&self, generation_id: &str) -> Result<Check, VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;

        let mut generations = self.lock_generations()?;
        let generation = generations
            .get_mut(generation_id)
            .ok_or_else(|| VectorIndexError::generation_not_found(generation_id))?;
        if generation.status == GenerationStatus::Building {
            return Err(VectorIndexError::generation_not_ready(
                generation_id,
                "verify",
            ));
        }

        for (record_id, record) in &generation.records {
            if record_id != &record.id {
                return Err(VectorIndexError::integrity_failed(
                    "record key does not match record id",
                ));
            }
            validate_record(record).map_err(|error| {
                VectorIndexError::integrity_failed(format!("invalid record: {}", error.message()))
            })?;
            if let Some(expected_dimension) = generation.dimension {
                if record.vector.len() != expected_dimension {
                    return Err(VectorIndexError::integrity_failed(
                        "record dimension does not match generation dimension",
                    ));
                }
            }
        }

        generation.status = GenerationStatus::Verified;
        Ok(Check {
            generation_id: generation_id.to_owned(),
            knowledge_base_id: generation.knowledge_base_id.clone(),
            vector_count: generation.records.len(),
            dimension: generation.dimension,
            flushed: true,
            verified: true,
        })
    }

    fn delete_generation(&self, generation_id: &str) -> Result<(), VectorIndexError> {
        validate_identifier(generation_id, "generation_id")?;

        let mut generations = self.lock_generations()?;
        if generations.remove(generation_id).is_none() {
            return Err(VectorIndexError::generation_not_found(generation_id));
        }
        Ok(())
    }
}

fn validate_identifier(value: &str, field: &str) -> Result<(), VectorIndexError> {
    if value.trim().is_empty() {
        return Err(VectorIndexError::invalid_request(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_record(record: &VectorRecord) -> Result<(), VectorIndexError> {
    validate_identifier(&record.id, "record id")?;
    validate_vector(&record.vector, "record vector")
}

fn validate_vector(vector: &[f32], label: &str) -> Result<(), VectorIndexError> {
    if vector.is_empty() {
        return Err(VectorIndexError::invalid_vector(format!(
            "{label} must not be empty"
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(VectorIndexError::invalid_vector(format!(
            "{label} must contain only finite values"
        )));
    }

    let squared_norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>();
    if squared_norm == 0.0 {
        return Err(VectorIndexError::invalid_vector(format!(
            "{label} must not have zero magnitude"
        )));
    }
    Ok(())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let dot_product = left
        .iter()
        .zip(right)
        .map(|(left_value, right_value)| f64::from(*left_value) * f64::from(*right_value))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    (dot_product / (left_norm * right_norm)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, vector: &[f32]) -> VectorRecord {
        VectorRecord::new(id, vector.to_vec())
    }

    #[test]
    fn builds_flushes_verifies_and_searches_a_generation() {
        let store = MemoryVectorStore::new();
        store.create_generation("kb-1", "generation-1").unwrap();
        store
            .upsert(
                "generation-1",
                &[
                    record("chunk-b", &[0.0, 1.0]),
                    record("chunk-a", &[1.0, 0.0]),
                ],
            )
            .unwrap();

        let not_ready = store
            .search("generation-1", SearchRequest::new(vec![1.0, 0.0], 2))
            .unwrap_err();
        assert_eq!(not_ready.code(), GENERATION_NOT_READY_CODE);

        store.flush("generation-1").unwrap();
        let check = store.verify("generation-1").unwrap();
        assert_eq!(
            check,
            Check {
                generation_id: "generation-1".to_owned(),
                knowledge_base_id: "kb-1".to_owned(),
                vector_count: 2,
                dimension: Some(2),
                flushed: true,
                verified: true,
            }
        );

        let hits = store
            .search("generation-1", SearchRequest::new(vec![1.0, 0.0], 2))
            .unwrap();
        assert_eq!(
            hits[0],
            Hit {
                id: "chunk-a".to_owned(),
                score: 1.0
            }
        );
        assert_eq!(hits[1].id, "chunk-b");
        assert!(hits[1].score.abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_dimension_changes_atomically_and_after_flush() {
        let store = MemoryVectorStore::new();
        store.create_generation("kb-1", "generation-1").unwrap();

        let batch_error = store
            .upsert(
                "generation-1",
                &[
                    record("chunk-a", &[1.0, 0.0]),
                    record("chunk-b", &[1.0, 0.0, 0.0]),
                ],
            )
            .unwrap_err();
        assert_eq!(batch_error.code(), DIMENSION_MISMATCH_CODE);

        store
            .upsert("generation-1", &[record("chunk-a", &[1.0, 0.0])])
            .unwrap();

        let error = store
            .upsert("generation-1", &[record("chunk-b", &[1.0, 0.0, 0.0])])
            .unwrap_err();
        assert_eq!(error.code(), DIMENSION_MISMATCH_CODE);

        store.flush("generation-1").unwrap();
        let check = store.verify("generation-1").unwrap();
        assert_eq!(check.vector_count, 1);

        let immutable_error = store
            .upsert("generation-1", &[record("chunk-c", &[0.0, 1.0])])
            .unwrap_err();
        assert_eq!(immutable_error.code(), GENERATION_NOT_READY_CODE);
    }

    #[test]
    fn validates_generation_lifecycle_and_all_error_codes_are_stable() {
        let store = MemoryVectorStore::new();
        let missing = store.flush("missing").unwrap_err();
        assert_eq!(missing.code(), GENERATION_NOT_FOUND_CODE);
        assert!(missing.code().starts_with("vector_index."));

        store.create_generation("kb-1", "generation-1").unwrap();
        let duplicate = store.create_generation("kb-1", "generation-1").unwrap_err();
        assert_eq!(duplicate.code(), GENERATION_EXISTS_CODE);

        let empty_id = store.create_generation("", "generation-2").unwrap_err();
        assert_eq!(empty_id.code(), INVALID_REQUEST_CODE);

        let invalid = store
            .upsert("generation-1", &[record("bad", &[0.0, 0.0])])
            .unwrap_err();
        assert_eq!(invalid.code(), INVALID_VECTOR_CODE);

        let invalid_search = store
            .search("generation-1", SearchRequest::new(vec![1.0], 0))
            .unwrap_err();
        assert_eq!(invalid_search.code(), INVALID_REQUEST_CODE);

        store.delete_generation("generation-1").unwrap();
        let deleted = store.verify("generation-1").unwrap_err();
        assert_eq!(deleted.code(), GENERATION_NOT_FOUND_CODE);
    }

    #[test]
    fn returns_top_k_in_deterministic_cosine_order() {
        let store = MemoryVectorStore::new();
        store.create_generation("kb-1", "generation-1").unwrap();
        store
            .upsert(
                "generation-1",
                &[
                    record("same-b", &[2.0, 0.0]),
                    record("same-a", &[1.0, 0.0]),
                    record("opposite", &[-1.0, 0.0]),
                ],
            )
            .unwrap();
        store.flush("generation-1").unwrap();

        let hits = store
            .search("generation-1", SearchRequest::new(vec![1.0, 0.0], 2))
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "same-a");
        assert_eq!(hits[1].id, "same-b");
        assert!((hits[0].score - hits[1].score).abs() < f32::EPSILON);
    }

    #[cfg(all(feature = "zvec", target_os = "windows"))]
    #[test]
    fn zvec_vector_store_offline_smoke() {
        let library_dir = std::env::var_os("ZVEC_LIB_DIR")
            .map(PathBuf::from)
            .expect("ZVEC_LIB_DIR must point to a directory containing zvec_c_api.dll");
        let root =
            std::env::temp_dir().join(format!("fox-zvec-smoke-{}", uuid::Uuid::new_v4().simple()));
        let store = ZvecVectorStore::new_with_library_dir(&root, &library_dir)
            .expect("Zvec library must be available for the offline smoke");
        store
            .create_generation("kb-smoke", "generation-smoke")
            .unwrap();
        store
            .upsert(
                "generation-smoke",
                &[
                    record("chunk-a", &[1.0, 0.0, 0.0, 0.0]),
                    record("chunk-b", &[0.0, 1.0, 0.0, 0.0]),
                ],
            )
            .unwrap();
        store.flush("generation-smoke").unwrap();
        let hits = store
            .search(
                "generation-smoke",
                SearchRequest::new(vec![1.0, 0.0, 0.0, 0.0], 2),
            )
            .unwrap();
        assert_eq!(hits.first().map(|hit| hit.id.as_str()), Some("chunk-a"));
        let check = store.verify("generation-smoke").unwrap();
        assert_eq!(check.vector_count, 2);
        assert_eq!(check.dimension, Some(4));
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }
}

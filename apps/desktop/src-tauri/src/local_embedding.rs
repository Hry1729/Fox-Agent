#![cfg(feature = "local-embedding")]

use ort::{session::Session, value::Tensor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    fmt::{Display, Formatter},
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MANIFEST_FILE_NAME: &str = "manifest.json";
const MODEL_FILE_NAME: &str = "model.onnx";

#[derive(Debug)]
pub enum EmbeddingError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidManifest(String),
    InvalidInput(String),
    ModelHashMismatch { expected: String, actual: String },
    RuntimeNotFound(PathBuf),
    Runtime(String),
    Inference(String),
    DimensionMismatch { expected: usize, actual: usize },
}

impl Display for EmbeddingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Json(error) => write!(formatter, "manifest JSON error: {error}"),
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid model manifest: {message}")
            }
            Self::InvalidInput(message) => write!(formatter, "invalid embedding input: {message}"),
            Self::ModelHashMismatch { expected, actual } => {
                write!(
                    formatter,
                    "model hash mismatch: expected {expected}, got {actual}"
                )
            }
            Self::RuntimeNotFound(path) => write!(
                formatter,
                "ONNX Runtime library not found: {}",
                path.display()
            ),
            Self::Runtime(message) => {
                write!(formatter, "ONNX Runtime initialization failed: {message}")
            }
            Self::Inference(message) => write!(formatter, "embedding inference failed: {message}"),
            Self::DimensionMismatch { expected, actual } => {
                write!(
                    formatter,
                    "embedding dimension mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl Error for EmbeddingError {}

impl From<io::Error> for EmbeddingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for EmbeddingError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ort::Error> for EmbeddingError {
    fn from(error: ort::Error) -> Self {
        Self::Inference(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPackageManifest {
    pub version: String,
    pub dimension: usize,
    pub tokenizer: String,
    pub hash: String,
    pub license: String,
}

impl ModelPackageManifest {
    pub fn load(package_dir: &Path) -> Result<Self, EmbeddingError> {
        let manifest_path = package_dir.join(MANIFEST_FILE_NAME);
        let manifest: Self = serde_json::from_slice(&fs::read(manifest_path)?)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), EmbeddingError> {
        if self.version.trim().is_empty() || self.version.len() > 128 {
            return Err(EmbeddingError::InvalidManifest(
                "version must be 1-128 non-whitespace characters".to_owned(),
            ));
        }
        if self.dimension == 0 || self.dimension > 32_768 {
            return Err(EmbeddingError::InvalidManifest(
                "dimension must be between 1 and 32768".to_owned(),
            ));
        }
        if self.tokenizer.trim().is_empty() || self.tokenizer.len() > 512 {
            return Err(EmbeddingError::InvalidManifest(
                "tokenizer must be 1-512 non-whitespace characters".to_owned(),
            ));
        }
        if self.license.trim().is_empty() || self.license.len() > 256 {
            return Err(EmbeddingError::InvalidManifest(
                "license must be 1-256 non-whitespace characters".to_owned(),
            ));
        }
        let hash = normalize_sha256(&self.hash).ok_or_else(|| {
            EmbeddingError::InvalidManifest(
                "hash must be a 64-character SHA-256 hex digest, optionally prefixed with sha256:"
                    .to_owned(),
            )
        })?;
        if hash.len() != 64 {
            return Err(EmbeddingError::InvalidManifest(
                "hash must contain exactly 64 hexadecimal characters".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn normalized_hash(&self) -> Result<String, EmbeddingError> {
        self.validate()?;
        Ok(normalize_sha256(&self.hash).expect("validated SHA-256 hash"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizedInput {
    pub input_ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    pub token_type_ids: Option<Vec<i64>>,
}

impl TokenizedInput {
    pub fn smoke() -> Self {
        Self {
            input_ids: vec![101, 102],
            attention_mask: vec![1, 1],
            token_type_ids: None,
        }
    }

    fn validate(&self) -> Result<(), EmbeddingError> {
        if self.input_ids.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "input_ids cannot be empty".to_owned(),
            ));
        }
        if self.input_ids.len() != self.attention_mask.len() {
            return Err(EmbeddingError::InvalidInput(
                "input_ids and attention_mask must have the same length".to_owned(),
            ));
        }
        if let Some(token_type_ids) = &self.token_type_ids {
            if token_type_ids.len() != self.input_ids.len() {
                return Err(EmbeddingError::InvalidInput(
                    "token_type_ids must have the same length as input_ids".to_owned(),
                ));
            }
        }
        if self.input_ids.len() > 16_384 {
            return Err(EmbeddingError::InvalidInput(
                "token sequence exceeds the 16384 token limit".to_owned(),
            ));
        }
        if !self
            .attention_mask
            .iter()
            .all(|value| *value == 0 || *value == 1)
        {
            return Err(EmbeddingError::InvalidInput(
                "attention_mask values must be 0 or 1".to_owned(),
            ));
        }
        Ok(())
    }
}

pub trait EmbeddingProvider {
    fn dimension(&self) -> usize;
    fn embed(&mut self, input: &TokenizedInput) -> Result<Vec<f32>, EmbeddingError>;
}

pub struct OnnxEmbeddingProvider {
    manifest: ModelPackageManifest,
    session: Session,
}

impl OnnxEmbeddingProvider {
    pub fn from_package_dir(package_dir: &Path) -> Result<Self, EmbeddingError> {
        let package_dir = package_dir.canonicalize()?;
        let manifest = ModelPackageManifest::load(&package_dir)?;
        let model_path = package_dir.join(MODEL_FILE_NAME);
        if !model_path.is_file() {
            return Err(EmbeddingError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing {}", model_path.display()),
            )));
        }
        verify_model_hash(&manifest, &model_path)?;
        let runtime_path = resolve_runtime_path(&package_dir)?;
        initialize_runtime(&runtime_path)?;
        let session = Session::builder()
            .map_err(|error| EmbeddingError::Inference(error.to_string()))?
            .commit_from_file(&model_path)
            .map_err(|error| EmbeddingError::Inference(error.to_string()))?;
        Ok(Self { manifest, session })
    }

    pub fn manifest(&self) -> &ModelPackageManifest {
        &self.manifest
    }
}

impl EmbeddingProvider for OnnxEmbeddingProvider {
    fn dimension(&self) -> usize {
        self.manifest.dimension
    }

    fn embed(&mut self, input: &TokenizedInput) -> Result<Vec<f32>, EmbeddingError> {
        input.validate()?;
        let input_names: Vec<String> = self
            .session
            .inputs
            .iter()
            .map(|input| input.name.clone())
            .collect();
        if input_names.is_empty() {
            return Err(EmbeddingError::Inference(
                "model declares no inputs".to_owned(),
            ));
        }

        let input_ids_name = find_input_name(&input_names, &["input_ids", "inputIds"])
            .unwrap_or_else(|| input_names[0].clone());
        let mut inputs = ort::inputs![
            input_ids_name.clone() => Tensor::from_array((
                [1usize, input.input_ids.len()],
                input.input_ids.clone().into_boxed_slice()
            ))?
        ]?;

        if let Some(attention_name) =
            find_input_name(&input_names, &["attention_mask", "attentionMask"]).or_else(|| {
                input_names
                    .get(1)
                    .cloned()
                    .filter(|name| name != &input_ids_name)
            })
        {
            inputs.push((
                attention_name.into(),
                Tensor::from_array((
                    [1usize, input.attention_mask.len()],
                    input.attention_mask.clone().into_boxed_slice(),
                ))?
                .into(),
            ));
        }

        if let Some(token_type_ids) = &input.token_type_ids {
            if let Some(token_type_name) =
                find_input_name(&input_names, &["token_type_ids", "tokenTypeIds"])
                    .or_else(|| input_names.get(2).cloned())
            {
                inputs.push((
                    token_type_name.into(),
                    Tensor::from_array((
                        [1usize, token_type_ids.len()],
                        token_type_ids.clone().into_boxed_slice(),
                    ))?
                    .into(),
                ));
            }
        }

        let outputs = self
            .session
            .run(inputs)
            .map_err(|error| EmbeddingError::Inference(error.to_string()))?;
        if outputs.len() == 0 {
            return Err(EmbeddingError::Inference(
                "model returned no outputs".to_owned(),
            ));
        }
        let output = &outputs[0];
        let values = output
            .try_extract_tensor::<f32>()
            .map_err(|error| EmbeddingError::Inference(error.to_string()))?
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let dimension = self.manifest.dimension;
        if values.is_empty() || values.len() % dimension != 0 {
            return Err(EmbeddingError::DimensionMismatch {
                expected: dimension,
                actual: values.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(EmbeddingError::Inference(
                "model returned a non-finite embedding value".to_owned(),
            ));
        }

        let row_count = values.len() / dimension;
        let mut embedding = vec![0.0_f32; dimension];
        let mut selected_rows = 0usize;
        for (row_index, row) in values.chunks_exact(dimension).enumerate() {
            if row_index < input.attention_mask.len() && input.attention_mask[row_index] == 0 {
                continue;
            }
            for (output, value) in embedding.iter_mut().zip(row) {
                *output += *value;
            }
            selected_rows += 1;
        }
        if selected_rows == 0 {
            return Err(EmbeddingError::Inference(
                "attention_mask selected no output rows".to_owned(),
            ));
        }
        if row_count > 1 {
            let divisor = selected_rows as f32;
            for value in &mut embedding {
                *value /= divisor;
            }
        }
        Ok(embedding)
    }
}

fn normalize_sha256(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("sha256-"))
        .unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn verify_model_hash(
    manifest: &ModelPackageManifest,
    model_path: &Path,
) -> Result<(), EmbeddingError> {
    let expected = manifest.normalized_hash()?;
    let mut file = File::open(model_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected {
        return Err(EmbeddingError::ModelHashMismatch { expected, actual });
    }
    Ok(())
}

fn resolve_runtime_path(package_dir: &Path) -> Result<PathBuf, EmbeddingError> {
    if let Some(value) = env::var_os("ORT_DYLIB_PATH") {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            env::current_exe()?
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
                .join(path)
        };
        if path.is_file() {
            return Ok(path);
        }
        return Err(EmbeddingError::RuntimeNotFound(path));
    }

    let packaged_path = package_dir.join(runtime_library_name());
    if packaged_path.is_file() {
        return Ok(packaged_path);
    }
    Err(EmbeddingError::RuntimeNotFound(packaged_path))
}

fn runtime_library_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

fn initialize_runtime(runtime_path: &Path) -> Result<(), EmbeddingError> {
    static INITIALIZATION: OnceLock<Result<(), String>> = OnceLock::new();
    match INITIALIZATION.get_or_init(|| {
        ort::init_from(runtime_path.to_string_lossy())
            .commit()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(EmbeddingError::Runtime(error.clone())),
    }
}

fn find_input_name(input_names: &[String], preferred_names: &[&str]) -> Option<String> {
    input_names
        .iter()
        .find(|name| {
            preferred_names
                .iter()
                .any(|preferred| name.eq_ignore_ascii_case(preferred))
        })
        .cloned()
}

fn main() -> Result<(), EmbeddingError> {
    let package_dir = env::args().nth(1).ok_or_else(|| {
        EmbeddingError::InvalidInput(
            "usage: local-embedding-smoke <model-package-directory>".to_owned(),
        )
    })?;
    let mut provider = OnnxEmbeddingProvider::from_package_dir(Path::new(&package_dir))?;
    let embedding = provider.embed(&TokenizedInput::smoke())?;
    if embedding.len() != provider.dimension() {
        return Err(EmbeddingError::DimensionMismatch {
            expected: provider.dimension(),
            actual: embedding.len(),
        });
    }
    println!(
        "ONNX offline smoke passed: version={}, dimension={}, tokenizer={}, license={}",
        provider.manifest().version,
        embedding.len(),
        provider.manifest().tokenizer,
        provider.manifest().license
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{normalize_sha256, ModelPackageManifest};

    #[test]
    fn validates_model_manifest_and_normalizes_hash() {
        let manifest = ModelPackageManifest {
            version: "1.0.0".to_owned(),
            dimension: 384,
            tokenizer: "tokenizer.json".to_owned(),
            hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                .to_owned(),
            license: "Apache-2.0".to_owned(),
        };
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.normalized_hash().unwrap(), "a".repeat(64));
    }

    #[test]
    fn rejects_invalid_manifest_hash() {
        assert!(normalize_sha256("not-a-hash").is_none());
        let manifest = ModelPackageManifest {
            version: "1.0.0".to_owned(),
            dimension: 384,
            tokenizer: "tokenizer.json".to_owned(),
            hash: "bad".to_owned(),
            license: "Apache-2.0".to_owned(),
        };
        assert!(manifest.validate().is_err());
    }
}

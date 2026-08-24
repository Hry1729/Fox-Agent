use crate::local_knowledge::LocalKnowledgeStore;
use crate::model_service::ModelServiceClient;
use crate::{
    database::Database,
    runtime_host::RuntimeHost,
    yuxi::{runtime::YuxiRuntimeHost, YuxiClient},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::Mutex as AsyncMutex;

pub struct AppState {
    pub database: Database,
    pub runtime_host: RuntimeHost,
    pub yuxi_client: YuxiClient,
    pub yuxi_runtime: YuxiRuntimeHost,
    pub model_service_client: ModelServiceClient,
    pub local_knowledge: LocalKnowledgeStore,
    pub data_dir: PathBuf,
    pub skills_dir: PathBuf,
    knowledge_download_cancellations: Arc<Mutex<HashSet<String>>>,
    knowledge_downloaded_files: DownloadedFileRegistry,
    knowledge_preview_cancellations: KnowledgePreviewCancellationRegistry,
    knowledge_preview_locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
    externally_opened_preview_caches: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    pub fn new(
        database: Database,
        runtime_host: RuntimeHost,
        yuxi_client: YuxiClient,
        yuxi_runtime: YuxiRuntimeHost,
        model_service_client: ModelServiceClient,
        local_knowledge: LocalKnowledgeStore,
        data_dir: PathBuf,
        skills_dir: PathBuf,
    ) -> Self {
        Self {
            database,
            runtime_host,
            yuxi_client,
            yuxi_runtime,
            model_service_client,
            local_knowledge,
            data_dir,
            skills_dir,
            knowledge_download_cancellations: Arc::new(Mutex::new(HashSet::new())),
            knowledge_downloaded_files: DownloadedFileRegistry::default(),
            knowledge_preview_cancellations: KnowledgePreviewCancellationRegistry::default(),
            knowledge_preview_locks: Arc::new(Mutex::new(HashMap::new())),
            externally_opened_preview_caches: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn cancel_knowledge_download(&self, download_id: &str) -> Result<(), String> {
        self.knowledge_download_cancellations
            .lock()
            .map_err(|_| "download cancellation lock is poisoned".to_owned())?
            .insert(download_id.to_owned());
        Ok(())
    }

    pub fn knowledge_download_cancelled(&self, download_id: &str) -> Result<bool, String> {
        Ok(self
            .knowledge_download_cancellations
            .lock()
            .map_err(|_| "download cancellation lock is poisoned".to_owned())?
            .contains(download_id))
    }

    pub fn clear_knowledge_download_cancel(&self, download_id: &str) {
        if let Ok(mut cancellations) = self.knowledge_download_cancellations.lock() {
            cancellations.remove(download_id);
        }
    }

    pub fn register_downloaded_file(&self, path: &std::path::Path) -> Result<(), String> {
        self.knowledge_downloaded_files.register(path)
    }

    pub fn is_registered_downloaded_file(&self, path: &std::path::Path) -> Result<bool, String> {
        self.knowledge_downloaded_files.contains(path)
    }

    pub fn knowledge_preview_lock(&self, cache_key: &str) -> Result<Arc<AsyncMutex<()>>, String> {
        let mut locks = self
            .knowledge_preview_locks
            .lock()
            .map_err(|_| "preview cache lock registry is poisoned".to_owned())?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(cache_key).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(cache_key.to_owned(), Arc::downgrade(&lock));
        Ok(lock)
    }

    pub fn register_knowledge_preview_operation(&self, operation_id: &str) -> Result<bool, String> {
        self.knowledge_preview_cancellations.register(operation_id)
    }

    pub fn knowledge_preview_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<Arc<AtomicBool>>, String> {
        self.knowledge_preview_cancellations.operation(operation_id)
    }

    pub fn finish_knowledge_preview_operation(&self, operation_id: &str) -> Result<bool, String> {
        self.knowledge_preview_cancellations.finish(operation_id)
    }

    pub fn cancel_knowledge_preview(&self, operation_id: &str) -> Result<bool, String> {
        self.knowledge_preview_cancellations.cancel(operation_id)
    }

    pub fn register_externally_opened_preview_cache(
        &self,
        cache_key: &str,
    ) -> Result<bool, String> {
        Ok(self
            .externally_opened_preview_caches
            .lock()
            .map_err(|_| "preview external-open registry is poisoned".to_owned())?
            .insert(cache_key.to_owned()))
    }

    pub fn unregister_externally_opened_preview_cache(&self, cache_key: &str) {
        if let Ok(mut caches) = self.externally_opened_preview_caches.lock() {
            caches.remove(cache_key);
        }
    }
}

#[derive(Default)]
struct KnowledgePreviewCancellationRegistry {
    operations: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Default)]
struct DownloadedFileRegistry {
    files: Mutex<HashSet<PathBuf>>,
}

impl DownloadedFileRegistry {
    fn register(&self, path: &std::path::Path) -> Result<(), String> {
        let path = canonical_downloaded_file(path)?;
        let mut files = self
            .files
            .lock()
            .map_err(|_| "downloaded file registry lock is poisoned".to_owned())?;
        files.retain(|candidate| candidate.is_file());
        files.insert(path);
        Ok(())
    }

    fn contains(&self, path: &std::path::Path) -> Result<bool, String> {
        let path = canonical_downloaded_file(path)?;
        let mut files = self
            .files
            .lock()
            .map_err(|_| "downloaded file registry lock is poisoned".to_owned())?;
        files.retain(|candidate| candidate.is_file());
        Ok(files.contains(&path))
    }
}

fn canonical_downloaded_file(path: &std::path::Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("downloaded file cannot be resolved: {error}"))?;
    if !path.is_file() {
        return Err("downloaded path is not a file".to_owned());
    }
    Ok(path)
}

impl KnowledgePreviewCancellationRegistry {
    fn register(&self, operation_id: &str) -> Result<bool, String> {
        let mut operations = self
            .operations
            .lock()
            .map_err(|_| "preview cancellation lock is poisoned".to_owned())?;
        if operations.contains_key(operation_id) {
            return Ok(false);
        }
        operations.insert(operation_id.to_owned(), Arc::new(AtomicBool::new(false)));
        Ok(true)
    }

    fn operation(&self, operation_id: &str) -> Result<Option<Arc<AtomicBool>>, String> {
        Ok(self
            .operations
            .lock()
            .map_err(|_| "preview cancellation lock is poisoned".to_owned())?
            .get(operation_id)
            .cloned())
    }

    fn finish(&self, operation_id: &str) -> Result<bool, String> {
        Ok(self
            .operations
            .lock()
            .map_err(|_| "preview cancellation lock is poisoned".to_owned())?
            .remove(operation_id)
            .is_some())
    }

    fn cancel(&self, operation_id: &str) -> Result<bool, String> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| "preview cancellation lock is poisoned".to_owned())?;
        if let Some(cancelled) = operations.get(operation_id) {
            cancelled.store(true, Ordering::Release);
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadedFileRegistry, KnowledgePreviewCancellationRegistry};
    use std::fs;
    use std::sync::atomic::Ordering;
    use uuid::Uuid;

    #[test]
    fn preview_cancellation_registry_owns_the_complete_operation_lifecycle() {
        let registry = KnowledgePreviewCancellationRegistry::default();

        assert!(registry.register("preview-1").unwrap());
        assert!(!registry.register("preview-1").unwrap());

        let token = registry.operation("preview-1").unwrap().unwrap();
        assert!(!token.load(Ordering::Acquire));
        assert!(registry.cancel("preview-1").unwrap());
        assert!(token.load(Ordering::Acquire));

        assert!(registry.finish("preview-1").unwrap());
        assert!(registry.operation("preview-1").unwrap().is_none());
        assert!(!registry.cancel("preview-1").unwrap());
        assert!(!registry.finish("preview-1").unwrap());
    }

    #[test]
    fn downloaded_file_registry_only_allows_live_registered_files() {
        let registry = DownloadedFileRegistry::default();
        let root = std::env::temp_dir().join(format!("fox-download-registry-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let registered = root.join("manual.pdf");
        let unregistered = root.join("other.pdf");
        fs::write(&registered, b"pdf").unwrap();
        fs::write(&unregistered, b"other").unwrap();

        registry.register(&registered).unwrap();
        assert!(registry.contains(&registered).unwrap());
        assert!(!registry.contains(&unregistered).unwrap());

        fs::remove_file(&registered).unwrap();
        assert!(registry.contains(&registered).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}

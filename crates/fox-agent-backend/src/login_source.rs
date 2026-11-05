//! Host-owned source choices. Reuse stores file digests, never credentials.
use crate::{onboarding::{self, RealHome}, port::AgentBackend, statefile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    backend: AgentBackend,
    selected: Vec<String>,
    files: BTreeMap<String, String>,
    native: Option<crate::model_endpoint::NativeModel>,
}

#[derive(Default, Deserialize, Serialize)]
struct Choices {
    fox_homes: BTreeSet<String>,
    default_fox: bool,
    reuse: Option<Snapshot>,
}

type Sources = BTreeMap<String, Choices>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub choice_required: bool,
    pub native_available: bool,
    pub reusable: bool,
    pub source_changed: bool,
    pub fox_chosen: bool,
    pub current: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub model_endpoint: Option<String>,
}

pub fn real_home() -> Option<RealHome> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty())?;
    Some(RealHome { home: home.into(), codex_home: std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()).map(PathBuf::from) })
}

fn path(state: &Path) -> PathBuf { state.join("agent-login-sources.json") }

fn fingerprints(backend: AgentBackend, real: &RealHome, selected: &[String]) -> Result<BTreeMap<String, String>, String> {
    let candidates = onboarding::import_candidates(backend, real);
    let mut files = BTreeMap::new();
    for id in selected {
        let item = candidates.iter().find(|item| &item.id == id).ok_or("配置来源已变化，请重新预览")?;
        let meta = std::fs::symlink_metadata(item.source_path()).map_err(|_| "无法读取配置来源")?;
        if !meta.is_file() || meta.len() > onboarding::MAX_IMPORT_FILE_BYTES { return Err("配置文件无法导入".into()); }
        let bytes = std::fs::read(item.source_path()).map_err(|_| "无法读取配置来源")?;
        files.insert(item.destination.clone(), format!("{:x}", Sha256::digest(bytes)));
    }
    if files.is_empty() { return Err("请至少选择一项配置".into()); }
    Ok(files)
}

impl Snapshot {
    pub fn capture(backend: AgentBackend, real: &RealHome, selected: &[String]) -> Result<Self, String> {
        let files = fingerprints(backend, real, selected)?;
        let native = crate::model_endpoint::confirmed_import_model(backend, real, selected)?;
        if backend == AgentBackend::Pi && native.is_none() {
            return Err("要在新项目中复用，请同时选择原生模型配置和默认模型设置".into());
        }
        Ok(Self { backend, selected: selected.to_vec(), files, native })
    }
    pub fn matches_source(&self, real: &RealHome) -> bool {
        fingerprints(self.backend, real, &self.selected).is_ok_and(|files| files == self.files)
    }
    pub fn matches(&self, home: &Path) -> bool {
        self.matches_source(&RealHome { home: home.to_owned(), codex_home: None })
    }
    pub fn remember(&self, state: &Path) -> Result<(), String> {
        let file = path(state);
        let _lock = statefile::StateLock::acquire(&statefile::lock_path_for(&file)).map_err(|_| "无法锁定配置来源记录")?;
        let mut sources: Sources = statefile::read_json(&file)?;
        let choices = sources.entry(self.backend.as_str().into()).or_default();
        choices.default_fox = false;
        choices.fox_homes.clear();
        choices.reuse = Some(self.clone());
        statefile::write_json(&file, &sources)
    }
}

pub fn choose_fox(backend: AgentBackend, state: &Path, home: &Path, remember: bool) -> Result<(), String> {
    let file = path(state);
    let _lock = statefile::StateLock::acquire(&statefile::lock_path_for(&file)).map_err(|_| "无法锁定配置来源记录")?;
    let mut sources: Sources = statefile::read_json(&file)?;
    let choices = sources.entry(backend.as_str().into()).or_default();
    onboarding::archive_source(backend, home)?;
    choices.fox_homes.insert(home.to_string_lossy().into_owned());
    if remember { choices.default_fox = true; choices.reuse = None; }
    statefile::write_json(&file, &sources)
}

pub fn clear_choice(backend: AgentBackend, state: &Path) -> Result<(), String> {
    let file = path(state);
    let _lock = statefile::StateLock::acquire(&statefile::lock_path_for(&file)).map_err(|_| "无法锁定配置来源记录")?;
    let mut sources: Sources = statefile::read_json(&file)?;
    sources.remove(backend.as_str());
    statefile::write_json(&file, &sources)
}

pub fn status(backend: AgentBackend, state: &Path, home: &Path, real: Option<&RealHome>) -> Result<SourceStatus, String> {
    let mut sources: Sources = statefile::read_json(&path(state))?;
    let choices = sources.remove(backend.as_str()).unwrap_or_default();
    let native_available = real.is_some_and(|real| !onboarding::import_candidates(backend, real).is_empty());
    let own = crate::unified_login::user_login_present(backend, home, false);
    let fox = choices.fox_homes.contains(home.to_string_lossy().as_ref()) || choices.default_fox;
    let reusable = !own && !fox && choices.reuse.as_ref().is_some_and(|snapshot| real.is_some_and(|real| snapshot.matches_source(real)));
    let source_changed = !own && !fox && choices.reuse.is_some() && !reusable;
    let native = if own {
        crate::model_endpoint::resolve(backend, home, &home.join(".fox-no-project-settings")).ok()
    } else if reusable {
        choices.reuse.as_ref().and_then(|snapshot| snapshot.native.clone())
    } else if !fox {
        real.and_then(|real| crate::model_endpoint::resolve(backend, &real.home, &real.home.join(".fox-no-project-settings")).ok())
    } else { None };
    Ok(SourceStatus {
        choice_required: !own && !fox && !reusable && (native_available || source_changed),
        native_available, reusable, source_changed, fox_chosen: fox,
        current: if own { "native" } else if reusable { "reused_native" } else { "fox" }.into(),
        provider: native.as_ref().and_then(|native| native.provider.clone()),
        model: native.as_ref().and_then(|native| native.model.clone()),
        model_endpoint: native.as_ref().and_then(|native| native.endpoint.as_ref()).map(|endpoint| endpoint.entry.clone()),
    })
}

pub fn prepare(backend: AgentBackend, state: &Path, home: &Path, real: Option<&RealHome>) -> Result<(), String> {
    let source = status(backend, state, home, real)?;
    if source.choice_required { return Err("配置来源尚未确认或已经变化，请到「设置 → 模型与 Agent」重新选择或导入。".into()); }
    if !source.reusable { return Ok(()); }
    let mut sources: Sources = statefile::read_json(&path(state))?;
    let snapshot = sources.remove(backend.as_str()).and_then(|choices| choices.reuse).ok_or("配置来源已变化，请重试")?;
    let real = real.ok_or("找不到本机配置")?;
    if !snapshot.matches_source(real) { return Err("本机配置已变化，请重新预览".into()); }
    crate::unified_login::remove_managed(backend, home)?;
    let targets = [onboarding::ImportTarget { project: "reused".into(), label: "当前项目".into(), home: home.to_owned() }];
    onboarding::import_into(backend, real, &snapshot.selected, &targets, onboarding::ImportOptions { overwrite: false, backup_stamp: "reuse" })?;
    if !snapshot.matches_source(real) { return Err("本机配置已变化，请重新预览".into()); }
    if let Some(native) = snapshot.native {
        let copied = crate::model_endpoint::resolve(backend, home, &home.join(".fox-no-project-settings"))?;
        if copied.endpoint != native.endpoint || copied.provider != native.provider || copied.model != native.model {
            return Err("已保留的项目配置与导入来源不同，请检查后重试".into());
        }
        crate::model_endpoint::approve_import_snapshot(state, backend, home, &copied)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, RealHome, PathBuf, PathBuf, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let real = RealHome { home: dir.path().join("real"), codex_home: None };
        std::fs::create_dir_all(real.home.join(".pi/agent")).unwrap();
        std::fs::write(real.home.join(onboarding::PI_AUTH), r#"{"gateway":{"type":"api_key","key":"test-secret"}}"#).unwrap();
        std::fs::write(real.home.join(onboarding::PI_MODELS), r#"{"providers":{"gateway":{"baseUrl":"http://127.0.0.1:3425/v1","api":"openai-completions","models":[{"id":"model"}]}}}"#).unwrap();
        std::fs::write(real.home.join(onboarding::PI_SETTINGS), r#"{"defaultProvider":"gateway","defaultModel":"model"}"#).unwrap();
        let state = dir.path().join("state");
        let home = dir.path().join("project");
        let selected = [onboarding::PI_AUTH, onboarding::PI_MODELS, onboarding::PI_SETTINGS].iter().map(|p| format!("file:{p}")).collect();
        (dir, real, state, home, selected)
    }
    #[test]
    fn discover_requires_choice_then_reuses_confirmed_snapshot_and_exact_endpoint() {
        let (_dir, real, state, home, selected) = fixture();
        assert!(status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().choice_required);
        assert!(prepare(AgentBackend::Pi, &state, &home, Some(&real)).is_err());
        assert!(!home.exists());
        Snapshot::capture(AgentBackend::Pi, &real, &selected).unwrap().remember(&state).unwrap();
        assert!(status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().reusable);
        prepare(AgentBackend::Pi, &state, &home, Some(&real)).unwrap();
        assert_eq!(std::fs::read(home.join(onboarding::PI_AUTH)).unwrap(), std::fs::read(real.home.join(onboarding::PI_AUTH)).unwrap());
        let grants = std::fs::read_to_string(state.join("model-endpoint-grants.json")).unwrap();
        assert!(grants.contains("127.0.0.1:3425"));
        assert!(!std::fs::read_to_string(path(&state)).unwrap().contains("test-secret"));
    }
    #[test]
    fn changed_source_requires_confirmation_and_existing_project_is_preserved() {
        let (_dir, real, state, home, selected) = fixture();
        Snapshot::capture(AgentBackend::Pi, &real, &selected).unwrap().remember(&state).unwrap();
        prepare(AgentBackend::Pi, &state, &home, Some(&real)).unwrap();
        std::fs::write(real.home.join(onboarding::PI_SETTINGS), r#"{"defaultProvider":"other","defaultModel":"changed"}"#).unwrap();
        let fresh = home.with_file_name("new-project");
        assert!(status(AgentBackend::Pi, &state, &fresh, Some(&real)).unwrap().source_changed);
        assert!(prepare(AgentBackend::Pi, &state, &fresh, Some(&real)).is_err());
        prepare(AgentBackend::Pi, &state, &home, Some(&real)).unwrap();
        assert!(std::fs::read_to_string(home.join(onboarding::PI_SETTINGS)).unwrap().contains("gateway"));
    }
    #[test]
    fn fox_choice_is_scoped_unless_user_remembers_it() {
        let (_dir, real, state, home, _) = fixture();
        choose_fox(AgentBackend::Pi, &state, &home, false).unwrap();
        assert!(!status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().choice_required);
        let fresh = home.with_file_name("new-project");
        assert!(status(AgentBackend::Pi, &state, &fresh, Some(&real)).unwrap().choice_required);
        choose_fox(AgentBackend::Pi, &state, &home, true).unwrap();
        assert!(!status(AgentBackend::Pi, &state, &fresh, Some(&real)).unwrap().choice_required);
    }
    #[test]
    fn no_native_config_and_no_choice_reads_as_implicit_fox_until_chosen() {
        // 13.38（B3-4）：本机没有 Pi 配置、从没选过来源：current 是 fox，但 fox_chosen 为假（Host 据此写审计）。
        let dir = tempfile::tempdir().unwrap();
        let empty = RealHome { home: dir.path().join("empty-real"), codex_home: None };
        std::fs::create_dir_all(&empty.home).unwrap();
        let (state, home) = (dir.path().join("state"), dir.path().join("project"));
        let before = status(AgentBackend::Pi, &state, &home, Some(&empty)).unwrap();
        assert!(!before.choice_required);
        assert_eq!(before.current, "fox");
        assert!(!before.fox_chosen);
        choose_fox(AgentBackend::Pi, &state, &home, false).unwrap();
        let after = status(AgentBackend::Pi, &state, &home, Some(&empty)).unwrap();
        assert!(after.fox_chosen && after.current == "fox");
    }
    #[test]
    fn partial_selection_cannot_approve_reuse() {
        let (_dir, real, _, _, _) = fixture();
        assert!(Snapshot::capture(AgentBackend::Pi, &real, &[format!("file:{}", onboarding::PI_AUTH)]).is_err());
    }

    #[test]
    fn codex_and_dsh_reuse_are_scoped_and_changed_sources_require_preview() {
        for (backend, file, value) in [
            (AgentBackend::Codex, onboarding::CODEX_AUTH, r#"{"OPENAI_API_KEY":"fixture-key"}"#),
            (AgentBackend::Deepseek, onboarding::DSH_ENV, "DEEPSEEK_API_KEY=fixture-key\nUNRELATED=keep-out\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let real = RealHome { home: dir.path().join("real"), codex_home: None };
            let source = real.home.join(file);
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(&source, value).unwrap();
            let selected = onboarding::import_candidates(backend, &real).iter().map(|item| item.id.clone()).collect::<Vec<_>>();
            let (state, home) = (dir.path().join("state"), dir.path().join("project"));
            Snapshot::capture(backend, &real, &selected).unwrap().remember(&state).unwrap();
            prepare(backend, &state, &home, Some(&real)).unwrap();
            assert!(crate::unified_login::user_login_present(backend, &home, false));
            assert!(!std::fs::read_to_string(home.join(file)).unwrap().contains("UNRELATED"));
            assert!(!std::fs::read_to_string(path(&state)).unwrap().contains("fixture-key"));
            std::fs::write(&source, value.replace("fixture-key", "changed-key")).unwrap();
            let fresh = home.with_file_name("new-project");
            assert!(status(backend, &state, &fresh, Some(&real)).unwrap().source_changed);
            assert!(prepare(backend, &state, &fresh, Some(&real)).is_err());
            assert!(!fresh.exists());
            prepare(backend, &state, &home, Some(&real)).unwrap();
            assert!(std::fs::read_to_string(home.join(file)).unwrap().contains("fixture-key"));
        }
    }

    #[test]
    fn switching_imported_pi_to_fox_preserves_native_config_in_private_backup() {
        let (_dir, real, state, home, selected) = fixture();
        Snapshot::capture(AgentBackend::Pi, &real, &selected).unwrap().remember(&state).unwrap();
        prepare(AgentBackend::Pi, &state, &home, Some(&real)).unwrap();
        assert_eq!(status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().current, "native");
        choose_fox(AgentBackend::Pi, &state, &home, true).unwrap();
        let source = status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap();
        assert_eq!(source.current, "fox");
        assert!(source.fox_chosen);
        let backup = std::fs::read_dir(home.join(onboarding::BACKUP_DIR)).unwrap().next().unwrap().unwrap().path();
        for relative in [onboarding::PI_AUTH, onboarding::PI_MODELS, onboarding::PI_SETTINGS] {
            assert_eq!(std::fs::read(backup.join(relative)).unwrap(), std::fs::read(real.home.join(relative)).unwrap());
            assert!(!home.join(relative).exists());
        }
        let login = crate::unified_login::UnifiedLogin {
            provider_name: "Fox fixture".into(), base_url: "https://example.com/v1".into(),
            api_type: "openai-completions".into(), model: "fox-model".into(),
            api_key: crate::unified_login::Secret::new("fox-test-key"), responses_supported: None,
        };
        let report = crate::unified_login::apply(AgentBackend::Pi, &home, &login, false).unwrap();
        assert_eq!(report.outcome, crate::unified_login::Outcome::Generated);
        let native = crate::model_endpoint::resolve(AgentBackend::Pi, &home, &home.join("project")).unwrap();
        assert_eq!(native.provider.as_deref(), Some("fox"));
        assert_eq!(native.model.as_deref(), Some("fox-model"));
        assert_eq!(status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().current, "fox");
        onboarding::import_into(AgentBackend::Pi, &real, &selected,
            &[onboarding::ImportTarget { project: "project".into(), label: "project".into(), home: home.clone() }],
            onboarding::ImportOptions { overwrite: true, backup_stamp: "back-to-native" }).unwrap();
        assert_eq!(status(AgentBackend::Pi, &state, &home, Some(&real)).unwrap().current, "native");
    }
}

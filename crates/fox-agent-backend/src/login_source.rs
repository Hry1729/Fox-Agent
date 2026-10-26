//! Pi source choices are Host-owned. Reuse stores digests, never credentials.
use crate::{onboarding::{self, RealHome}, port::AgentBackend, statefile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}};

const FILES: &[&str] = &[onboarding::PI_AUTH, onboarding::PI_MODELS, onboarding::PI_SETTINGS];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    files: BTreeMap<String, String>,
    native: crate::model_endpoint::NativeModel,
}

#[derive(Default, Deserialize, Serialize)]
struct Choices {
    fox_homes: BTreeSet<String>,
    default_fox: bool,
    reuse: Option<Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub choice_required: bool,
    pub native_available: bool,
    pub reusable: bool,
    pub source_changed: bool,
    pub current: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub model_endpoint: Option<String>,
}

pub fn real_home() -> Option<RealHome> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty())?;
    Some(RealHome { home: home.into(), codex_home: None })
}

fn path(state: &Path) -> PathBuf { state.join("pi-login-sources.json") }

fn fingerprints(home: &Path, selected: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    for relative in FILES {
        if !selected.contains(&format!("file:{relative}")) { continue; }
        let file = onboarding::rel(home, relative);
        let meta = std::fs::metadata(&file).map_err(|_| format!("配置文件已变化：{relative}，请重新确认导入"))?;
        if !meta.is_file() || meta.len() > 1024 * 1024 { return Err(format!("无法复用配置文件：{relative}")); }
        let bytes = std::fs::read(file).map_err(|_| format!("无法读取配置文件：{relative}"))?;
        files.insert((*relative).into(), format!("{:x}", Sha256::digest(bytes)));
    }
    if files.is_empty() { return Err("请选择 Pi 配置文件".into()); }
    Ok(files)
}

impl Snapshot {
    pub fn capture(real: &RealHome, selected: &[String]) -> Result<Self, String> {
        let files = fingerprints(&real.home, selected)?;
        let native = crate::model_endpoint::confirmed_import_model(AgentBackend::Pi, real, selected)?
            .ok_or("要在新项目中复用，请同时选择原生模型配置和默认模型设置")?;
        Ok(Self { files, native })
    }
    fn selected(&self) -> Vec<String> { self.files.keys().map(|p| format!("file:{p}")).collect() }
    pub fn matches(&self, home: &Path) -> bool {
        fingerprints(home, &self.selected()).is_ok_and(|files| files == self.files)
    }
    pub fn remember(&self, state: &Path) -> Result<(), String> {
        let file = path(state);
        let _lock = statefile::StateLock::acquire(&statefile::lock_path_for(&file)).map_err(|_| "无法锁定配置来源记录")?;
        let mut choices: Choices = statefile::read_json(&file)?;
        choices.default_fox = false;
        choices.reuse = Some(self.clone());
        statefile::write_json(&file, &choices)
    }
}

pub fn choose_fox(state: &Path, home: &Path, remember: bool) -> Result<(), String> {
    let file = path(state);
    let _lock = statefile::StateLock::acquire(&statefile::lock_path_for(&file)).map_err(|_| "无法锁定配置来源记录")?;
    let mut choices: Choices = statefile::read_json(&file)?;
    choices.fox_homes.insert(home.to_string_lossy().into_owned());
    if remember { choices.default_fox = true; choices.reuse = None; }
    statefile::write_json(&file, &choices)
}

pub fn status(state: &Path, home: &Path, real: Option<&RealHome>) -> Result<SourceStatus, String> {
    let choices: Choices = statefile::read_json(&path(state))?;
    let native_available = real.is_some_and(|real| !onboarding::import_candidates(AgentBackend::Pi, real).is_empty());
    let own = crate::unified_login::user_login_present(AgentBackend::Pi, home, false);
    let fox = choices.fox_homes.contains(home.to_string_lossy().as_ref()) || choices.default_fox;
    let reusable = !own && !fox && choices.reuse.as_ref().is_some_and(|snapshot| real.is_some_and(|real| snapshot.matches(&real.home)));
    let source_changed = !own && !fox && choices.reuse.is_some() && !reusable;
    let native = if own {
        crate::model_endpoint::resolve(AgentBackend::Pi, home, &home.join(".fox-no-project-settings")).ok()
    } else if reusable {
        choices.reuse.as_ref().map(|snapshot| snapshot.native.clone())
    } else if !fox {
        real.and_then(|real| crate::model_endpoint::resolve(AgentBackend::Pi, &real.home, &real.home.join(".fox-no-project-settings")).ok())
    } else { None };
    Ok(SourceStatus {
        choice_required: !own && !fox && !reusable && (native_available || source_changed),
        native_available, reusable, source_changed,
        current: if own { "native" } else if reusable { "reused_native" } else { "fox" }.into(),
        provider: native.as_ref().and_then(|native| native.provider.clone()),
        model: native.as_ref().and_then(|native| native.model.clone()),
        model_endpoint: native.as_ref().and_then(|native| native.endpoint.as_ref()).map(|e| e.entry.clone()),
    })
}

pub fn prepare(state: &Path, home: &Path, real: Option<&RealHome>) -> Result<(), String> {
    let source = status(state, home, real)?;
    if source.choice_required { return Err(if source.source_changed { "原生 Pi 配置已变化，请在 Agent 栏重新确认配置来源" } else { "发现原生 Pi 配置，请在 Agent 栏选择导入或使用 Fox 模型服务" }.into()); }
    if !source.reusable { return Ok(()); }
    let choices: Choices = statefile::read_json(&path(state))?;
    let snapshot = choices.reuse.ok_or("配置来源已变化，请重试")?;
    let real = real.ok_or("找不到原生 Pi 配置")?;
    if !snapshot.matches(&real.home) { return Err("原生 Pi 配置已变化，请重新确认导入".into()); }
    crate::unified_login::remove_managed(AgentBackend::Pi, home)?;
    let targets = [onboarding::ImportTarget { project: "reused".into(), label: "当前项目".into(), home: home.to_owned() }];
    onboarding::import_into(AgentBackend::Pi, real, &snapshot.selected(), &targets, onboarding::ImportOptions { overwrite: false, backup_stamp: "reuse" })?;
    if !snapshot.matches(home) { return Err("复制后的 Pi 配置与已确认来源不一致，请重新导入".into()); }
    crate::model_endpoint::approve_import_snapshot(state, AgentBackend::Pi, home, &snapshot.native)
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
        let selected = FILES.iter().map(|p| format!("file:{p}")).collect();
        (dir, real, state, home, selected)
    }
    #[test]
    fn discover_requires_choice_then_reuses_confirmed_snapshot_and_exact_endpoint() {
        let (_dir, real, state, home, selected) = fixture();
        assert!(status(&state, &home, Some(&real)).unwrap().choice_required);
        assert!(prepare(&state, &home, Some(&real)).is_err());
        assert!(!home.exists());
        Snapshot::capture(&real, &selected).unwrap().remember(&state).unwrap();
        assert!(status(&state, &home, Some(&real)).unwrap().reusable);
        prepare(&state, &home, Some(&real)).unwrap();
        assert_eq!(std::fs::read(home.join(onboarding::PI_AUTH)).unwrap(), std::fs::read(real.home.join(onboarding::PI_AUTH)).unwrap());
        let grants = std::fs::read_to_string(state.join("model-endpoint-grants.json")).unwrap();
        assert!(grants.contains("127.0.0.1:3425"));
        assert!(!std::fs::read_to_string(path(&state)).unwrap().contains("test-secret"));
    }
    #[test]
    fn changed_source_requires_confirmation_and_existing_project_is_preserved() {
        let (_dir, real, state, home, selected) = fixture();
        Snapshot::capture(&real, &selected).unwrap().remember(&state).unwrap();
        prepare(&state, &home, Some(&real)).unwrap();
        std::fs::write(real.home.join(onboarding::PI_SETTINGS), r#"{"defaultProvider":"other","defaultModel":"changed"}"#).unwrap();
        let fresh = home.with_file_name("new-project");
        assert!(status(&state, &fresh, Some(&real)).unwrap().source_changed);
        assert!(prepare(&state, &fresh, Some(&real)).is_err());
        prepare(&state, &home, Some(&real)).unwrap();
        assert!(std::fs::read_to_string(home.join(onboarding::PI_SETTINGS)).unwrap().contains("gateway"));
    }
    #[test]
    fn fox_choice_is_scoped_unless_user_remembers_it() {
        let (_dir, real, state, home, _) = fixture();
        choose_fox(&state, &home, false).unwrap();
        assert!(!status(&state, &home, Some(&real)).unwrap().choice_required);
        let fresh = home.with_file_name("new-project");
        assert!(status(&state, &fresh, Some(&real)).unwrap().choice_required);
        choose_fox(&state, &home, true).unwrap();
        assert!(!status(&state, &fresh, Some(&real)).unwrap().choice_required);
    }
    #[test]
    fn partial_selection_cannot_approve_reuse() {
        let (_dir, real, _, _, _) = fixture();
        assert!(Snapshot::capture(&real, &[format!("file:{}", onboarding::PI_AUTH)]).is_err());
    }
}

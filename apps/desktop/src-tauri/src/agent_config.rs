use crate::{app_state::AppState, database::{ApiResponse, Database, ModelProviderRecord}, external_backend::ExternalBackendHost, external_model, model_service};
use fox_agent_backend::{agent_config::*, external_launch::HostLaunchConfig, login_source, onboarding::{self, ImportKind, ImportTarget, RealHome}, port::AgentBackend, provider_protocol::ProtocolProfile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, HashMap}, sync::{LazyLock, Mutex}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tauri::{Emitter, State};
use tokio::sync::watch;

static VERIFY_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Default)]
pub struct AgentConfigControl {
    running: Mutex<HashMap<AgentBackend, watch::Sender<bool>>>,
    previews: Mutex<HashMap<AgentBackend, BTreeMap<String, String>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigOverview { services: Vec<ServiceSummary>, agents: Vec<AgentConfigStatus> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceSummary { provider_id: String, name: String, is_default: bool, matches: Vec<ServiceMatch>, missing: Vec<MissingProtocol> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceMatch { agent: AgentBackend, endpoint_id: String, api_type: String, verified: bool, verified_at: Option<i64> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MissingProtocol { agent: AgentBackend, api_type: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Candidate { provider_id: String, endpoint_id: String, api_type: String, model_id: String, verified: bool }

#[derive(Serialize)]
struct NativeStatus { available: bool, changed: bool }

#[derive(Serialize)]
struct Diagnostic { code: String, detail: Option<String> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigStatus {
    agent: AgentBackend,
    enabled: bool,
    state: AgentConfigState,
    install: InstallStatus,
    native: NativeStatus,
    binding: Option<AgentBinding>,
    candidates: Vec<Candidate>,
    verification: Option<VerificationReport>,
    diagnostic: Option<Diagnostic>,
}

#[derive(Deserialize, Default)]
pub struct OverviewRequest { #[serde(default)] developer: bool }
#[derive(Deserialize)]
pub struct BackendRequest { backend: AgentBackend, #[serde(default)] developer: bool }
#[derive(Deserialize)]
pub struct EnableRequest { backend: AgentBackend, enabled: bool }
#[derive(Deserialize)]
pub struct SaveBindingRequest { backend: AgentBackend, binding: BindingSource, #[serde(default)] developer: bool }
#[derive(Deserialize)]
pub struct VerifyRequest { backend: AgentBackend, depth: VerifyDepth, #[serde(default)] developer: bool }

#[derive(Debug)]
struct ConfigError { code: &'static str, message: String }
impl ConfigError {
    fn new(code: &'static str, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
    fn read(message: String) -> Self { Self::new("agent_config.read_failed", message) }
}
fn response<T: Serialize>(result: Result<T, ConfigError>) -> ApiResponse<T> {
    match result { Ok(value) => ApiResponse::success(value), Err(error) => ApiResponse::failure(error.code, error.message, true) }
}
fn external_only(backend: AgentBackend) -> Result<(), ConfigError> {
    fox_agent_backend::host_routing::credential_entry_gate(backend, &fox_agent_backend::host_routing::BackendSwitch::from_env(), false)
        .map_err(|error| ConfigError::new(error.code(), error.message()))
}
fn now_ms() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64 }
fn detail(text: &str) -> String { crate::runtime_host::redact_execution_diagnostic(text, 4096) }

#[tauri::command]
pub fn agent_config_overview(state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: Option<OverviewRequest>) -> ApiResponse<AgentConfigOverview> {
    response(overview(&state.database, external.launch_config(), request.unwrap_or_default().developer))
}

fn overview(database: &Database, config: &HostLaunchConfig, developer: bool) -> Result<AgentConfigOverview, ConfigError> {
    let providers = database.list_model_providers().map_err(ConfigError::read)?;
    let agents: Vec<_> = AgentBackend::ALL.into_iter().map(|backend| {
        status(database, config, backend, &providers, developer).unwrap_or_else(|error| AgentConfigStatus {
            agent: backend, enabled: backend == AgentBackend::Fox, state: AgentConfigState::ReadFailed,
            install: config.install_status(backend), native: NativeStatus { available: false, changed: false }, binding: None,
            candidates: vec![], verification: None, diagnostic: developer.then(|| Diagnostic { code: error.code.into(), detail: Some(detail(&error.message)) }),
        })
    }).collect();
    let services = providers.iter().map(|provider| {
        let mut matches = vec![];
        let mut missing = vec![];
        for agent in &agents {
            let profiles = external_model::profiles(provider, agent.agent);
            if agent.enabled && profiles.is_empty() { missing.push(MissingProtocol { agent: agent.agent, api_type: external_model::required_api(agent.agent).into() }); }
            for profile in profiles {
                matches.push(ServiceMatch { agent: agent.agent, endpoint_id: profile.id.clone(), api_type: profile.api_type.clone(), verified: external_model::verified(database, provider, &profile), verified_at: external_model::verified_at(database, provider, &profile) });
            }
        }
        ServiceSummary { provider_id: provider.id.clone(), name: provider.name.clone(), is_default: provider.is_default, matches, missing }
    }).collect();
    Ok(AgentConfigOverview { services, agents })
}

fn status(database: &Database, config: &HostLaunchConfig, backend: AgentBackend, providers: &[ModelProviderRecord], developer: bool) -> Result<AgentConfigStatus, ConfigError> {
    let record = database.agent_binding(backend).map_err(ConfigError::read)?;
    let binding = record.as_ref().and_then(|record| record.binding.clone());
    let install = config.install_status(backend);
    let mut native = NativeStatus { available: false, changed: false };
    if backend != AgentBackend::Fox {
        let home = config.private_home_path(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID, backend).map_err(|e| ConfigError::read(e.message))?;
        let source = login_source::status(backend, &config.state_root, &home, config.native_login_home.as_ref()).map_err(ConfigError::read)?;
        native = NativeStatus { available: source.native_available, changed: source.source_changed };
    }
    let identity = verification_identity(database, config, backend)?;
    let mut verification = database.agent_verification(backend, &identity).map_err(ConfigError::read)?;
    if !developer { if let Some(report) = verification.as_mut() { for layer in &mut report.layers { layer.code = None; layer.detail = None; } } }
    let configured = configured_connection(database, config, backend);
    let binding_valid = configured.is_ok() && !native.changed;
    let model_verified = configured.as_ref().ok().and_then(Option::as_ref).is_some_and(|(provider, profile)| external_model::verified(database, provider, profile))
        || verification.as_ref().is_some_and(|report| report.layers.iter().any(|layer| matches!(layer.layer, VerificationLayer::ModelRequest | VerificationLayer::AgentOutput) && layer.status == VerificationStatus::Passed));
    let state_binding = if backend == AgentBackend::Fox && providers.iter().any(|provider| provider.is_default) { Some(AgentBinding { source: BindingSource::Native, updated_at: 0 }) } else { binding.clone() };
    let state = derive_state(&StateInputs { install: Ok(install.clone()), native_available: native.available, binding: state_binding, binding_valid, model_verified });
    let diagnostic = developer.then(|| configured.err().map(|e| Diagnostic { code: e.code.into(), detail: Some(detail(&e.message)) })).flatten();
    let candidates = providers.iter().filter(|provider| provider.enabled).flat_map(|provider| external_model::profiles(provider, backend).into_iter().map(|profile| Candidate {
        provider_id: provider.id.clone(), endpoint_id: profile.id.clone(), api_type: profile.api_type.clone(), model_id: profile.model_id.clone(), verified: external_model::verified(database, provider, &profile),
    })).collect();
    Ok(AgentConfigStatus { agent: backend, enabled: backend == AgentBackend::Fox || record.is_some_and(|record| record.enabled), state, install, native, binding, candidates, verification, diagnostic })
}

fn refreshed(database: &Database, config: &HostLaunchConfig, backend: AgentBackend, developer: bool) -> Result<AgentConfigStatus, ConfigError> {
    status(database, config, backend, &database.list_model_providers().map_err(ConfigError::read)?, developer)
}

#[tauri::command]
pub fn agent_enable_set(state: State<'_, AppState>, request: EnableRequest) -> ApiResponse<()> {
    response(external_only(request.backend).and_then(|_| state.database.set_agent_enabled(request.backend, request.enabled).map_err(ConfigError::read)))
}

#[tauri::command]
pub fn agent_binding_save(state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: SaveBindingRequest) -> ApiResponse<AgentConfigStatus> {
    response((|| {
        external_only(request.backend)?;
        validate_binding(&state.database, external.launch_config(), request.backend, &request.binding)?;
        state.database.save_agent_binding(request.backend, &request.binding).map_err(ConfigError::read)?;
        let errors = external.clear_managed_model_files(request.backend);
        if !errors.is_empty() { external.audit_warning("agent_binding.cleanup_failed", &serde_json::json!({"backend":request.backend,"failures":errors.len()})); }
        refreshed(&state.database, external.launch_config(), request.backend, request.developer)
    })())
}

#[tauri::command]
pub fn agent_binding_clear(state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: BackendRequest) -> ApiResponse<AgentConfigStatus> {
    response((|| {
        external_only(request.backend)?;
        state.database.clear_agent_binding(request.backend).map_err(ConfigError::read)?;
        let errors = external.clear_managed_model_files(request.backend);
        login_source::clear_choice(request.backend, &external.launch_config().state_root).map_err(ConfigError::read)?;
        if !errors.is_empty() { external.audit_warning("agent_binding.cleanup_failed", &serde_json::json!({"backend":request.backend,"failures":errors.len()})); }
        refreshed(&state.database, external.launch_config(), request.backend, request.developer)
    })())
}

fn service_connection(database: &Database, backend: AgentBackend, source: &BindingSource) -> Result<(ModelProviderRecord, ProtocolProfile), ConfigError> {
    let BindingSource::FoxService { provider_id, endpoint_id, model_id } = source else { unreachable!("native bindings use native configuration") };
    let provider = database.list_model_providers().map_err(ConfigError::read)?.into_iter()
        .find(|provider| Some(&provider.id) == provider_id.as_ref() && provider.enabled)
        .ok_or_else(|| ConfigError::new("agent_binding.provider_not_found", "选择的服务已被删除或停用，请重新选择。"))?;
    let mut profile = external_model::profiles(&provider, backend).into_iter().find(|profile| &profile.id == endpoint_id)
        .ok_or_else(|| ConfigError::new("agent_binding.endpoint_not_found", "这个服务没有该 Agent 能用的入口。"))?;
    if model_id.trim().is_empty() { return Err(ConfigError::new("agent_binding.model_required", "请选择一个模型。")); }
    profile.model_id = model_id.clone();
    Ok((provider, profile))
}

fn validate_binding(database: &Database, config: &HostLaunchConfig, backend: AgentBackend, source: &BindingSource) -> Result<(), ConfigError> {
    match source {
        BindingSource::FoxService { .. } => { service_connection(database, backend, source)?; }
        BindingSource::Native => { native_configured(database, config, backend)?; }
    }
    Ok(())
}

fn native_configured(database: &Database, config: &HostLaunchConfig, backend: AgentBackend) -> Result<(), ConfigError> {
    let projects = crate::external_backend::import_home_targets(true, None, &database.known_projects().map_err(ConfigError::read)?);
    for (project, _) in projects {
        let home = config.private_home_path(&project, backend).map_err(|e| ConfigError::read(e.message))?;
        let source = login_source::status(backend, &config.state_root, &home, config.native_login_home.as_ref()).map_err(ConfigError::read)?;
        if source.current == "native" || source.reusable { return Ok(()); }
    }
    Err(ConfigError::new("agent_binding.native_required", "请先导入电脑上已有的登录配置。"))
}

fn configured_connection(database: &Database, config: &HostLaunchConfig, backend: AgentBackend) -> Result<Option<(ModelProviderRecord, ProtocolProfile)>, ConfigError> {
    if backend != AgentBackend::Fox {
        let binding = database.agent_binding(backend).map_err(ConfigError::read)?.and_then(|record| record.binding)
            .ok_or_else(|| ConfigError::new("model.connection_required", "请先选择模型服务或导入登录配置。"))?;
        if binding.source == BindingSource::Native { native_configured(database, config, backend)?; return Ok(None); }
        let (provider, profile) = service_connection(database, backend, &binding.source)?;
        if external_model::login(&provider, &profile).is_none() { return Err(ConfigError::new("agent_binding.missing_key", "这个服务还没有填写 API Key。")); }
        return Ok(Some((provider, profile)));
    }
    let (provider, profile) = external_model::bound_connection(database, backend).ok_or_else(|| ConfigError::new("model.connection_required", "请先设置默认模型服务。"))?;
    if external_model::login(&provider, &profile).is_none() { return Err(ConfigError::new("agent_binding.missing_key", "这个服务还没有填写 API Key。")); }
    Ok(Some((provider, profile)))
}

fn source_digests(backend: AgentBackend, real: &RealHome) -> Result<BTreeMap<String, String>, ConfigError> {
    onboarding::import_candidates(backend, real).into_iter().map(|item| {
        let metadata = std::fs::symlink_metadata(item.source_path()).map_err(|e| ConfigError::read(e.to_string()))?;
        if !metadata.is_file() || metadata.len() > onboarding::MAX_IMPORT_FILE_BYTES { return Err(ConfigError::new("agent_backend_source_changed", "配置文件无法读取，请重新预览。")); }
        let bytes = std::fs::read(item.source_path()).map_err(|e| ConfigError::read(e.to_string()))?;
        Ok((item.id, format!("{:x}", Sha256::digest(bytes))))
    }).collect()
}

fn verification_identity(database: &Database, config: &HostLaunchConfig, backend: AgentBackend) -> Result<String, ConfigError> {
    let binding = database.agent_binding(backend).map_err(ConfigError::read)?.and_then(|record| record.binding);
    let connection = external_model::bound_connection(database, backend).map(|(provider, profile)| {
        let credential = external_model::credential_ref(&provider, &profile);
        serde_json::json!({"providerId":provider.id,"api":profile.api_type,"url":profile.base_url,"model":profile.model_id,"credential":credential,"revision":external_model::credential_revision(database,&credential)})
    });
    let native = if binding.as_ref().is_some_and(|binding| binding.source == BindingSource::Native) {
        let mut fingerprints = BTreeMap::new();
        if let Some(real) = &config.native_login_home { fingerprints.insert("source".into(), source_digests(backend, real)?); }
        for (project, _) in crate::external_backend::import_home_targets(true, None, &database.known_projects().map_err(ConfigError::read)?) {
            let home = config.private_home_path(&project, backend).map_err(|e| ConfigError::read(e.message))?;
            fingerprints.insert(project, source_digests(backend, &RealHome { home, codex_home: None })?);
        }
        Some(fingerprints)
    } else { None };
    let program = config.managed_program(backend).and_then(|path| std::fs::metadata(&path).ok().map(|meta| (path.to_string_lossy().into_owned(), meta.len(), meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map(|time| time.as_nanos().to_string()))));
    let identity = serde_json::json!({"backend":backend,"binding":binding.map(|binding|binding.source),"connection":connection,"native":native,"runtime":config.runtime_root,"program":program});
    Ok(format!("{:x}", Sha256::digest(crate::canonical_json::to_string(&identity).map_err(|e| ConfigError::read(e.to_string()))?)))
}

#[tauri::command]
pub async fn agent_config_verify(app: tauri::AppHandle, state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: VerifyRequest) -> Result<ApiResponse<VerificationReport>, ()> {
    let (sender, mut cancel) = watch::channel(false);
    {
        let mut running = state.agent_configuration.running.lock().unwrap();
        if running.contains_key(&request.backend) { return Ok(ApiResponse::failure("agent_verify.busy", "正在验证，请稍候。", true)); }
        running.insert(request.backend, sender);
    }
    let result = verify_all(&state.database, external.launch_config(), &state.model_service_client, &request, &mut cancel, |report| {
        let mut public = report.clone();
        if !request.developer { for layer in &mut public.layers { layer.code = None; layer.detail = None; } }
        let _ = app.emit("agent-config-verification", serde_json::json!({"backend":request.backend,"report":public}));
    }).await;
    state.agent_configuration.running.lock().unwrap().remove(&request.backend);
    Ok(response(result))
}

#[tauri::command]
pub fn agent_config_verify_cancel(state: State<'_, AppState>, request: BackendRequest) -> ApiResponse<bool> {
    let running = state.agent_configuration.running.lock().unwrap();
    ApiResponse::success(running.get(&request.backend).is_some_and(|sender| sender.send(true).is_ok()))
}

async fn verify_model(database: &Database, config: &HostLaunchConfig, client: &model_service::ModelServiceClient, request: &VerifyRequest, cancel: &mut watch::Receiver<bool>) -> Result<VerificationReport, ConfigError> {
    let identity = verification_identity(database, config, request.backend)?;
    let mut report = VerificationReport { identity, checked_at: now_ms(), layers: VerificationLayer::ALL.into_iter().map(|layer| LayerResult::new(layer, VerificationStatus::Skipped, None, None)).collect() };
    let connection = match configured_connection(database, config, request.backend) {
        Ok(connection) => { report.layers[0] = LayerResult::new(VerificationLayer::ConfigSaved, VerificationStatus::Passed, None, None); connection }
        Err(error) => {
            report.layers[0] = LayerResult::new(VerificationLayer::ConfigSaved, VerificationStatus::Failed, Some(error.code), None);
            return persist_report(database, request, report);
        }
    };
    if let Some((provider, profile)) = connection {
        let login = external_model::login(&provider, &profile).ok_or_else(|| ConfigError::new("agent_binding.missing_key", "这个服务还没有填写 API Key。"))?;
        let start = Instant::now();
        let work = async {
            let _guard = VERIFY_LOCK.lock().await;
            client.test_output(&profile.base_url, Some(login.api_key.expose()), &profile.api_type, &profile.model_id).await
        };
        let outcome = tokio::select! {
            _ = cancel.changed() => Err(("agent_verify.cancelled", String::new())),
            _ = tokio::time::sleep(Duration::from_secs(120)) => Err(("agent_verify.timeout", String::new())),
            result = work => result.map_err(|message| ("agent_verify.model_failed", message)),
        };
        let outcome = outcome.and_then(|_| {
            let unchanged = database.list_model_providers().ok().is_some_and(|providers| providers.iter().any(|current| current.id == provider.id && current.updated_at == provider.updated_at))
                && verification_identity(database, config, request.backend).is_ok_and(|identity| identity == report.identity);
            if !unchanged { return Err(("agent_verify.config_changed", "配置已变化，请重新验证。".into())); }
            external_model::record_verified_connection(database, &profile.api_type, &profile.base_url, &profile.model_id, &external_model::credential_ref(&provider, &profile))
                .map_err(|error| ("agent_config.read_failed", error))
        });
        report.layers[1] = match outcome {
            Ok(()) => LayerResult::new(VerificationLayer::ModelRequest, VerificationStatus::Passed, None, Some(start.elapsed().as_millis() as i64)),
            Err((code, message)) => {
                if code == "agent_verify.model_failed" { external_model::forget_verified_connection(database, &provider, &profile).map_err(ConfigError::read)?; }
                let mut layer = LayerResult::new(VerificationLayer::ModelRequest, VerificationStatus::Failed, Some(code), Some(start.elapsed().as_millis() as i64));
                if request.developer && !message.is_empty() { layer.detail = Some(detail(&message)); }
                layer
            }
        };
    } else {
        report.layers[1].message = "原生登录需通过完整 Agent 验证。".into();
    }
    if request.backend == AgentBackend::Fox {
        for layer in &mut report.layers[2..] { layer.message = "Fox 内置 Agent 不需要单独验证".into(); }
    }
    persist_report(database, request, report)
}

async fn verify_all(database: &Database, config: &HostLaunchConfig, client: &model_service::ModelServiceClient, request: &VerifyRequest, cancel: &mut watch::Receiver<bool>, mut progress: impl FnMut(&VerificationReport)) -> Result<VerificationReport, ConfigError> {
    let mut report = verify_model(database, config, client, request, cancel).await?;
    progress(&report);
    if request.backend == AgentBackend::Fox || matches!(request.depth, VerifyDepth::Model) || report.layers[..2].iter().any(|layer| layer.status == VerificationStatus::Failed) { return Ok(report); }
    let connection = configured_connection(database, config, request.backend)?;
    let login = connection.as_ref().and_then(|(provider, profile)| external_model::login(provider, profile));
    let native_home = if connection.is_none() {
        let projects = crate::external_backend::import_home_targets(true, None, &database.known_projects().map_err(ConfigError::read)?);
        projects.into_iter().find_map(|(id, _)| {
            let home = config.private_home_path(&id, request.backend).ok()?;
            fox_agent_backend::unified_login::user_login_present(request.backend, &home, false).then_some(home)
        })
    } else { None };
    if connection.is_none() && native_home.is_none() { return Err(ConfigError::new("agent_binding.native_required", "请先导入本机配置，再进行完整验证。")); }
    let input = fox_agent_backend::verify_session::SessionVerification { config: config.clone(), backend: request.backend, login, native_home, with_tool: matches!(request.depth, VerifyDepth::Tool) };
    let layers = fox_agent_backend::verify_session::verify_session(input, cancel, |layer| {
        let at = if layer.layer == VerificationLayer::AgentOutput { 2 } else { 3 };
        report.layers[at] = layer.clone();
        progress(&report);
    }).await;
    report.layers[2] = layers[0].clone();
    report.layers[3] = layers[1].clone();
    if verification_identity(database, config, request.backend)? != report.identity {
        report.layers[2] = LayerResult::new(VerificationLayer::AgentOutput, VerificationStatus::Failed, Some("agent_verify.config_changed"), None);
        report.layers[3] = LayerResult::new(VerificationLayer::ToolCall, VerificationStatus::Skipped, None, None);
    } else if connection.is_none() && report.layers[2].status == VerificationStatus::Passed {
        report.layers[1] = LayerResult::new(VerificationLayer::ModelRequest, VerificationStatus::Passed, None, report.layers[2].latency_ms);
        report.layers[1].message = "原生登录模型请求通过（随 Agent 验证）".into();
    }
    let report = persist_report(database, request, report)?;
    progress(&report);
    Ok(report)
}

fn persist_report(database: &Database, request: &VerifyRequest, mut report: VerificationReport) -> Result<VerificationReport, ConfigError> {
    report.checked_at = now_ms();
    database.save_agent_verification(request.backend, &report).map_err(ConfigError::read)?;
    if !request.developer { for layer in &mut report.layers { layer.code = None; layer.detail = None; } }
    Ok(report)
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportScope { AllAndNew, AllExisting, DefaultHome }
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictMode { Keep, OverwriteWithBackup }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportApplyRequest { backend: AgentBackend, selected: Vec<String>, scope: ImportScope, on_conflict: ConflictMode, #[serde(default)] developer: bool }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewItem { id: String, label: String, kind: ImportKind, default_selected: bool, dev_path: Option<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeCount { id: ImportScope, project_count: usize }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportConflict { project_id: String, project_name: String, items: Vec<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview { backend: AgentBackend, items: Vec<PreviewItem>, scopes: Vec<ScopeCount>, conflicts: Vec<ImportConflict>, source_changed: bool }
#[derive(Serialize)]
pub struct ImportApplied { written: usize, kept: usize, backups: usize, status: AgentConfigStatus }

fn import_projects(database: &Database, scope: ImportScope) -> Result<Vec<(String, String)>, ConfigError> {
    let known = database.known_projects().map_err(ConfigError::read)?;
    Ok(crate::external_backend::import_home_targets(!matches!(scope, ImportScope::DefaultHome), None, &known))
}

#[tauri::command]
pub fn agent_import_preview(state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: BackendRequest) -> ApiResponse<ImportPreview> {
    response((|| {
        external_only(request.backend)?;
        let config = external.launch_config();
        let real = config.native_login_home.as_ref().ok_or_else(|| ConfigError::new("agent_config.read_failed", "找不到电脑上的登录配置。"))?;
        let preview = import_preview(&state.database, config, request.backend, request.developer)?;
        state.agent_configuration.previews.lock().unwrap().insert(request.backend, source_digests(request.backend, real)?);
        Ok(preview)
    })())
}

fn import_preview(database: &Database, config: &HostLaunchConfig, backend: AgentBackend, developer: bool) -> Result<ImportPreview, ConfigError> {
    let real = config.native_login_home.as_ref().ok_or_else(|| ConfigError::new("agent_config.read_failed", "找不到电脑上的登录配置。"))?;
    let candidates = onboarding::import_candidates(backend, real);
    let selected: Vec<_> = candidates.iter().map(|item| item.id.clone()).collect();
    let projects = import_projects(database, ImportScope::AllAndNew)?;
    let targets = projects.iter().map(|(project, label)| config.private_home_path(project, backend)
        .map(|home| ImportTarget { project: project.clone(), label: label.clone(), home }).map_err(|e| ConfigError::read(e.message))).collect::<Result<Vec<_>, _>>()?;
    let plan = onboarding::import_plan(backend, real, &selected, &targets).map_err(ConfigError::read)?;
    let home = config.private_home_path(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID, backend).map_err(|e| ConfigError::read(e.message))?;
    let source_changed = login_source::status(backend, &config.state_root, &home, Some(real)).map_err(ConfigError::read)?.source_changed;
    let conflicts = plan.skipped_existing.into_iter().map(|conflict| ImportConflict {
        project_id: conflict.project, project_name: conflict.label,
        items: conflict.items.iter().map(|name| candidates.iter().find(|item| &item.destination == name || &item.name == name).map(|item| item.label()).unwrap_or("已有登录配置").to_owned()).collect(),
    }).collect();
    let items = candidates.into_iter().map(|item| PreviewItem { label: item.label().into(), dev_path: developer.then(|| item.source_path().to_string_lossy().into_owned()), id: item.id, kind: item.kind, default_selected: true }).collect();
    Ok(ImportPreview { backend, items, scopes: vec![ScopeCount { id: ImportScope::AllAndNew, project_count: projects.len() }, ScopeCount { id: ImportScope::AllExisting, project_count: projects.len() }, ScopeCount { id: ImportScope::DefaultHome, project_count: 1 }], conflicts, source_changed })
}

#[tauri::command]
pub fn agent_import_apply(state: State<'_, AppState>, external: State<'_, ExternalBackendHost>, request: ImportApplyRequest) -> ApiResponse<ImportApplied> {
    response((|| {
        external_only(request.backend)?;
        if request.selected.is_empty() { return Err(ConfigError::new("agent_import.nothing_selected", "请至少选择一项。")); }
        let config = external.launch_config();
        let real = config.native_login_home.as_ref().ok_or_else(|| ConfigError::new("agent_config.read_failed", "找不到电脑上的登录配置。"))?;
        let preview = state.agent_configuration.previews.lock().unwrap().get(&request.backend).cloned();
        if preview.as_ref() != Some(&source_digests(request.backend, real)?) { return Err(ConfigError::new("agent_backend_source_changed", "电脑上的配置刚刚变了，请重新预览。")); }
        let available = onboarding::import_candidates(request.backend, real);
        if request.selected.iter().any(|id| !available.iter().any(|item| &item.id == id)) { return Err(ConfigError::new("agent_backend_source_changed", "电脑上的配置刚刚变了，请重新预览。")); }
        let snapshot = if matches!(request.scope, ImportScope::AllAndNew) { Some(login_source::Snapshot::capture(request.backend, real, &request.selected).map_err(ConfigError::read)?) } else { None };
        let projects = import_projects(&state.database, request.scope)?;
        let report = config.import_credentials(request.backend, &projects, real, &request.selected, matches!(request.on_conflict, ConflictMode::OverwriteWithBackup)).map_err(|e| ConfigError::new("agent_import.failed", e.message))?;
        if preview.as_ref() != Some(&source_digests(request.backend, real)?) { return Err(ConfigError::new("agent_backend_source_changed", "电脑上的配置刚刚变了，请重新预览。")); }
        if let Some(snapshot) = snapshot { snapshot.remember(&config.state_root).map_err(ConfigError::read)?; }
        else { login_source::clear_choice(request.backend, &config.state_root).map_err(ConfigError::read)?; }
        state.database.save_agent_binding(request.backend, &BindingSource::Native).map_err(ConfigError::read)?;
        external.audit_host("agent_backend.credentials_imported", &report.audit_event());
        Ok(ImportApplied { written: report.homes_written, kept: report.skipped_existing.len(), backups: report.backups.len(), status: refreshed(&state.database, config, request.backend, request.developer)? })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}};

    struct Fixture { database: Database, config: HostLaunchConfig, root: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fox-config-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let mut config = HostLaunchConfig::from_env(&root);
            config.fox_data = root.clone();
            config.state_root = root.join("agent-state");
            config.runtime_root = root.join("runtime");
            config.native_login_home = Some(RealHome { home: root.join("real"), codex_home: None });
            Self { database: Database::open(":memory:".into()).unwrap(), config, root }
        }
        fn provider(&self, url: &str) -> ModelProviderRecord {
            let provider = ModelProviderRecord {
                id: uuid::Uuid::new_v4().to_string(), name: "配置测试".into(), icon: None, base_url: url.into(), api_type: "openai-completions".into(), enabled: true, is_default: true, credential_configured: false, connection_type: "remote".into(), last_status: "unknown".into(), last_latency_ms: None, last_checked_at: None, created_at: 0, updated_at: 0,
                models: vec![crate::database::ProviderModelRecord { id: uuid::Uuid::new_v4().to_string(), model_id: "fixture-model".into(), display_name: "Fixture".into(), context_window: 128000, max_output_tokens: 8192, supports_image_input: false, is_default: true }], protocol_profiles: vec![],
            };
            self.database.save_model_provider(&provider).unwrap()
        }
        fn bind_pi(&self, provider: &ModelProviderRecord) {
            self.database.save_agent_binding(AgentBackend::Pi, &BindingSource::FoxService { provider_id: Some(provider.id.clone()), endpoint_id: "primary".into(), model_id: "fixture-model".into() }).unwrap();
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

    fn tree(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut result = BTreeMap::new();
        if !path.exists() { return result; }
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                for (child, value) in tree(&entry.path()) { result.insert(format!("{}/{child}", entry.file_name().to_string_lossy()), value); }
            } else { result.insert(entry.file_name().to_string_lossy().into_owned(), std::fs::read(entry.path()).unwrap()); }
        }
        result
    }

    struct HttpFixture { url: String, hits: Arc<AtomicUsize>, stop: Arc<AtomicBool>, thread: Option<std::thread::JoinHandle<()>> }
    impl HttpFixture {
        fn new(status: u16, delayed: bool) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/v1", listener.local_addr().unwrap());
            let hits = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (count, closed) = (hits.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                while !closed.load(Ordering::SeqCst) {
                    if let Ok((mut stream, _)) = listener.accept() {
                        count.fetch_add(1, Ordering::SeqCst);
                        stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                        let _ = stream.read(&mut [0; 8192]);
                        if delayed { while !closed.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(5)); } }
                        let body = r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}]}"#;
                        let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    } else { std::thread::sleep(Duration::from_millis(5)); }
                }
            });
            Self { url, hits, stop, thread: Some(thread) }
        }
    }
    impl Drop for HttpFixture { fn drop(&mut self) { self.stop.store(true, Ordering::SeqCst); self.thread.take().unwrap().join().unwrap(); let _ = model_service::clear_api_key(&self.url); } }

    #[test]
    fn overview_and_preview_are_read_only_in_database_files_and_network() {
        let fixture = Fixture::new();
        let server = HttpFixture::new(200, false);
        fixture.provider(&server.url);
        let real = fixture.config.native_login_home.as_ref().unwrap();
        std::fs::create_dir_all(real.home.join(".codex")).unwrap();
        std::fs::write(real.home.join(".codex/auth.json"), r#"{"OPENAI_API_KEY":"fixture-secret"}"#).unwrap();
        let before = tree(&fixture.root);
        let changes = || fixture.database.with_connection(|connection| Ok(connection.total_changes())).unwrap();
        let changed_before = changes();
        let providers_before = serde_json::to_value(fixture.database.list_model_providers().unwrap()).unwrap();
        for _ in 0..2 {
            let value = overview(&fixture.database, &fixture.config, false).unwrap();
            assert_eq!(value.agents.len(), 4);
            let preview = import_preview(&fixture.database, &fixture.config, AgentBackend::Codex, false).unwrap();
            assert_eq!(preview.items.len(), 1);
            assert!(preview.items[0].dev_path.is_none());
            let text = crate::canonical_json::to_string(&preview).unwrap();
            assert!(!text.contains("fixture-secret"));
            assert!(!text.contains(&real.home.to_string_lossy().replace('\\', "\\\\")));
        }
        assert_eq!(changes(), changed_before);
        assert_eq!(serde_json::to_value(fixture.database.list_model_providers().unwrap()).unwrap(), providers_before);
        assert_eq!(tree(&fixture.root), before);
        assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn identity_tracks_bound_service_model_and_key_revision_and_clear_keeps_enabled() {
        let fixture = Fixture::new();
        let provider = fixture.provider("http://127.0.0.1:55991/v1");
        fixture.bind_pi(&provider);
        fixture.database.set_agent_enabled(AgentBackend::Pi, true).unwrap();
        let identity = verification_identity(&fixture.database, &fixture.config, AgentBackend::Pi).unwrap();
        external_model::bump_credential_revision(&fixture.database, &provider.base_url).unwrap();
        assert_ne!(verification_identity(&fixture.database, &fixture.config, AgentBackend::Pi).unwrap(), identity);
        let report = VerificationReport { identity, checked_at: now_ms(), layers: vec![LayerResult::new(VerificationLayer::ModelRequest, VerificationStatus::Passed, None, Some(2))] };
        fixture.database.save_agent_verification(AgentBackend::Pi, &report).unwrap();
        assert!(fixture.database.agent_verification(AgentBackend::Pi, &report.identity).unwrap().is_some());
        fixture.database.clear_agent_binding(AgentBackend::Pi).unwrap();
        let cleared = fixture.database.agent_binding(AgentBackend::Pi).unwrap().unwrap();
        assert!(cleared.enabled && cleared.binding.is_none());
        assert!(fixture.database.agent_verification(AgentBackend::Pi, &report.identity).unwrap().is_none());
    }

    #[tokio::test]
    async fn model_verification_uses_http_and_records_only_success() {
        for status in [200, 401] {
            let fixture = Fixture::new();
            let server = HttpFixture::new(status, false);
            let provider = fixture.provider(&server.url);
            model_service::set_api_key(&server.url, "fixture-test-key").unwrap();
            fixture.bind_pi(&provider);
            if status == 401 { external_model::record_verified_connection(&fixture.database, &provider.api_type, &provider.base_url, "fixture-model", &provider.base_url).unwrap(); }
            let (_sender, mut cancel) = watch::channel(false);
            let report = verify_model(&fixture.database, &fixture.config, &model_service::ModelServiceClient::new().unwrap(), &VerifyRequest { backend: AgentBackend::Pi, depth: VerifyDepth::Model, developer: false }, &mut cancel).await.unwrap();
            assert_eq!(report.layers[0].status, VerificationStatus::Passed);
            assert_eq!(report.layers[1].status, if status == 200 { VerificationStatus::Passed } else { VerificationStatus::Failed });
            assert_eq!(server.hits.load(Ordering::SeqCst), 1);
            assert!(report.layers.iter().all(|layer| layer.code.is_none() && layer.detail.is_none()));
            let (provider, profile) = external_model::bound_connection(&fixture.database, AgentBackend::Pi).unwrap();
            assert_eq!(external_model::verified(&fixture.database, &provider, &profile), status == 200);
            assert!(!fixture.root.join("agent-homes").exists());
        }
    }

    #[tokio::test]
    async fn cancellation_stops_a_pending_request_without_marking_it_verified() {
        let fixture = Fixture::new();
        let server = HttpFixture::new(200, true);
        let provider = fixture.provider(&server.url);
        model_service::set_api_key(&server.url, "fixture-test-key").unwrap();
        fixture.bind_pi(&provider);
        let (sender, mut cancel) = watch::channel(false);
        let cancel_when_requested = async {
            while server.hits.load(Ordering::SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(10)).await; }
            sender.send(true).unwrap();
        };
        let request = VerifyRequest { backend: AgentBackend::Pi, depth: VerifyDepth::Model, developer: true };
        let client = model_service::ModelServiceClient::new().unwrap();
        let (result, _) = tokio::join!(verify_model(&fixture.database, &fixture.config, &client, &request, &mut cancel), cancel_when_requested);
        assert_eq!(result.unwrap().layers[1].code.as_deref(), Some("agent_verify.cancelled"));
        let (provider, profile) = external_model::bound_connection(&fixture.database, AgentBackend::Pi).unwrap();
        assert!(!external_model::verified(&fixture.database, &provider, &profile));
    }
}

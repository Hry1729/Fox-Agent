use crate::{app_state::AppState, database::{ApiResponse, Database, ModelProviderRecord}, model_service};
use fox_agent_backend::{port::AgentBackend, provider_protocol::{self, ProtocolProfile}, unified_login::{Secret, UnifiedLogin}};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    pub status: String,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub api_type: String,
    pub candidates: Vec<ProtocolProfile>,
    pub message: Option<String>,
}

pub fn required_api(backend: AgentBackend) -> &'static str {
    match backend { AgentBackend::Codex => "openai-responses", AgentBackend::Deepseek => "anthropic-messages", _ => "openai-completions" }
}

pub fn trusted(profile: &ProtocolProfile) -> bool {
    matches!((profile.preset_id.as_deref(), profile.api_type.as_str(), profile.base_url.trim_end_matches('/')),
        (Some("deepseek-official"), "anthropic-messages", "https://api.deepseek.com/anthropic") |
        (Some("deepseek-official"), "openai-responses" | "openai-completions", "https://api.deepseek.com") |
        (Some("openai"), "openai-responses" | "openai-completions", "https://api.openai.com/v1"))
}

fn profiles(provider: &ModelProviderRecord, backend: AgentBackend) -> Vec<ProtocolProfile> {
    let model = provider.models.iter().find(|m| m.is_default).or(provider.models.first()).map(|m| m.model_id.as_str()).unwrap_or_default();
    let mut profiles = provider.protocol_profiles.clone();
    if !profiles.iter().any(|p| p.api_type == provider.api_type) {
        profiles.push(ProtocolProfile { preset_id: None, id: "primary".into(), api_type: provider.api_type.clone(), base_url: provider.base_url.clone(), model_id: model.into(), separate_credential: false, backends: vec![], credential_configured: false });
    }
    for p in &mut profiles { if p.model_id.trim().is_empty() { p.model_id = model.into(); } }
    profiles.retain(|p| provider_protocol::supports(&p.api_type, backend.as_str()));
    let bound: Vec<_> = profiles.iter().filter(|p| p.backends.iter().any(|b| b == backend.as_str())).cloned().collect();
    if !bound.is_empty() { return bound; }
    // pi accepts all three protocols. Prefer the supplier's main API, then Messages.
    if backend == AgentBackend::Pi {
        let api = if profiles.iter().any(|p| p.api_type == provider.api_type) { provider.api_type.as_str() }
            else if profiles.iter().any(|p| p.api_type == "anthropic-messages") { "anthropic-messages" } else { "openai-responses" };
        profiles.retain(|p| p.api_type == api);
    }
    profiles
}

fn current(database: &Database) -> Result<Option<ModelProviderRecord>, String> {
    Ok(database.list_model_providers()?.into_iter().find(|p| p.enabled && p.is_default))
}

fn cache_key(provider: &ModelProviderRecord, profile: &ProtocolProfile) -> String {
    format!("model.protocol.{}.{}.{}", provider.id, profile.api_type, profile.id)
}

fn cache_value(provider: &ModelProviderRecord, profile: &ProtocolProfile) -> String {
    // The revision invalidates validation on credential edits as well as address/model edits.
    crate::canonical_json::to_string(&(profile.base_url.as_str(), profile.model_id.as_str(), provider.updated_at)).expect("connection identity serializes")
}

fn verified(database: &Database, provider: &ModelProviderRecord, profile: &ProtocolProfile) -> bool {
    trusted(profile) || database.app_setting(&cache_key(provider, profile)).ok().flatten().as_deref() == Some(cache_value(provider, profile).as_str())
}

fn login(provider: &ModelProviderRecord, profile: &ProtocolProfile) -> Option<UnifiedLogin> {
    let credential = if profile.separate_credential { provider_protocol::credential_id(&provider.id, &profile.id) } else { provider.base_url.clone() };
    Some(UnifiedLogin { provider_name: provider.name.clone(), base_url: profile.base_url.clone(), api_type: profile.api_type.clone(), model: profile.model_id.clone(), api_key: Secret::new(model_service::get_api_key(&credential)?), responses_supported: None })
}

pub fn external_login(database: &Database, backend: AgentBackend) -> Option<UnifiedLogin> {
    let provider = current(database).ok()??;
    let profiles = profiles(&provider, backend);
    if profiles.len() != 1 || !verified(database, &provider, &profiles[0]) { return None; }
    login(&provider, &profiles[0])
}

/// 13.19 fix ④: the provider address an Agent's native permission list is
/// reports (it used to decide Codex「帮我批准」; plan C2 never offers that
/// mode). Unlike [`external_login`] it
/// needs no key and no verification, so a new conversation's list is right
/// before the first login: the protocol address that serves `backend`, else
/// (no such address bound) the default provider's own base URL. `None` when
/// there is no default provider or it has no address at all.
pub fn catalog_provider_base_url(database: &Database, backend: AgentBackend) -> Option<String> {
    current(database).ok().flatten().and_then(|provider| provider_catalog_base_url(&provider, backend))
}

fn provider_catalog_base_url(provider: &ModelProviderRecord, backend: AgentBackend) -> Option<String> {
    let candidates: Vec<String> = profiles(provider, backend).into_iter().map(|p| p.base_url.trim().to_owned()).filter(|url| !url.is_empty()).collect();
    // Several unbound candidates: any non-OpenAI one decides (fail-closed).
    if let Some(url) = candidates.iter().find(|url| !fox_agent_backend::native_permission::provider_is_openai(url)).or(candidates.first()) {
        return Some(url.clone());
    }
    let own = provider.base_url.trim();
    (!own.is_empty()).then(|| own.to_owned())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRequest {
    pub target: crate::external_backend::AgentBackendTarget,
    pub provider_id: Option<String>,
    pub profile_id: Option<String>,
    pub base_url: Option<String>,
}

async fn resolve(state: &AppState, external: &crate::external_backend::ExternalBackendHost, request: ConnectionRequest) -> Result<ConnectionState, String> {
    let (backend, own_login) = external.model_login_context(&request.target)?;
    let mut view = ConnectionState { status: "missing_provider".into(), provider_id: None, provider_name: None, api_type: required_api(backend).into(), candidates: vec![], message: None };
    if own_login { view.status = "existing_login".into(); return Ok(view); }
    let Some(mut provider) = current(&state.database)? else { return Ok(view); };
    view.provider_id = Some(provider.id.clone());
    view.provider_name = Some(provider.name.clone());
    if request.base_url.is_some() || request.profile_id.is_some() {
        if request.provider_id.as_deref() != Some(provider.id.as_str()) { return Err("当前供应商已改变，请重试".into()); }
        if let Some(id) = request.profile_id {
            let choices = profiles(&provider, backend);
            let mut selected = choices.into_iter().find(|p| p.id == id).ok_or("入口已改变，请重新选择")?;
            for p in &mut provider.protocol_profiles { p.backends.retain(|b| b != backend.as_str()); }
            if let Some(p) = provider.protocol_profiles.iter_mut().find(|p| p.id == id) { p.backends.push(backend.as_str().into()); }
            else { selected.backends = vec![backend.as_str().into()]; provider.protocol_profiles.push(selected); }
        } else if let Some(base_url) = request.base_url {
            if !profiles(&provider, backend).is_empty() { return Err("已有可用入口，请刷新后选择".into()); }
            provider.protocol_profiles.push(ProtocolProfile { preset_id: None, id: uuid::Uuid::new_v4().to_string(), api_type: required_api(backend).into(), base_url: model_service::normalize_model_base_url(&base_url)?, model_id: String::new(), separate_credential: false, backends: vec![backend.as_str().into()], credential_configured: false });
        }
        provider = state.database.save_model_provider(&provider)?;
    }
    view.candidates = profiles(&provider, backend);
    if view.candidates.is_empty() { view.status = "missing_address".into(); return Ok(view); }
    if view.candidates.len() > 1 { view.status = "choose_address".into(); return Ok(view); }
    let profile = &view.candidates[0];
    let Some(login) = login(&provider, profile) else { view.status = "missing_key".into(); return Ok(view); };
    static VERIFY_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _verification_guard = VERIFY_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
    if !verified(&state.database, &provider, profile) {
        if let Err(error) = state.model_service_client.test_output(&login.base_url, Some(login.api_key.expose()), &login.api_type, &login.model).await {
            view.status = "verification_failed".into(); view.message = Some(error); return Ok(view);
        }
        // A late probe cannot validate a supplier that changed while the request was running.
        let latest = current(&state.database)?.ok_or("当前供应商已清空")?;
        if latest.id != provider.id || latest.updated_at != provider.updated_at { return Err("供应商配置已改变，请重试".into()); }
        state.database.set_app_setting(&cache_key(&provider, profile), &cache_value(&provider, profile))?;
    }
    external.prepare_model_login(&request.target)?;
    view.status = "ready".into();
    Ok(view)
}

#[tauri::command]
pub async fn agent_backend_model_connection(state: tauri::State<'_, AppState>, external: tauri::State<'_, crate::external_backend::ExternalBackendHost>, request: ConnectionRequest) -> Result<ApiResponse<ConnectionState>, ()> {
    Ok(match resolve(&state, &external, request).await {
        Ok(view) => ApiResponse::success(view),
        Err(error) => ApiResponse::failure("model.connection_failed", error, true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn provider() -> ModelProviderRecord {
        ModelProviderRecord {
            id: "supplier".into(), name: "Supplier".into(), icon: None, base_url: "https://example.com/v1".into(), api_type: "openai-completions".into(), enabled: true, is_default: true, credential_configured: false, connection_type: "remote".into(), last_status: "unknown".into(), last_latency_ms: None, last_checked_at: None, created_at: 0, updated_at: 1,
            models: vec![crate::database::ProviderModelRecord { id: "model".into(), model_id: "default-model".into(), display_name: "Default".into(), context_window: 128000, max_output_tokens: 8192, supports_image_input: false, is_default: true }], protocol_profiles: vec![],
        }
    }
    fn endpoint(id: &str, api: &str) -> ProtocolProfile {
        ProtocolProfile { preset_id: None, id: id.into(), api_type: api.into(), base_url: "https://example.com/v1".into(), model_id: String::new(), separate_credential: false, backends: vec![], credential_configured: false }
    }
    #[test]
    fn automatically_matches_and_inherits_current_supplier_model() {
        let mut p = provider();
        assert!(profiles(&p, AgentBackend::Codex).is_empty());
        assert!(profiles(&p, AgentBackend::Deepseek).is_empty());
        p.protocol_profiles = vec![endpoint("responses", "openai-responses"), endpoint("messages", "anthropic-messages")];
        assert_eq!(profiles(&p, AgentBackend::Codex)[0].id, "responses");
        assert_eq!(profiles(&p, AgentBackend::Deepseek)[0].model_id, "default-model");
        assert_eq!(profiles(&p, AgentBackend::Pi)[0].api_type, p.api_type);
        assert_eq!(p.models[0].model_id, "default-model");
    }
    #[test]
    fn ambiguity_requires_choice_and_explicit_binding_remembers_it() {
        let mut p = provider();
        p.protocol_profiles = vec![endpoint("one", "openai-responses"), endpoint("two", "openai-responses")];
        assert_eq!(profiles(&p, AgentBackend::Codex).len(), 2);
        p.protocol_profiles[1].backends = vec!["codex".into()];
        assert_eq!(profiles(&p, AgentBackend::Codex)[0].id, "two");
        assert_eq!(profiles(&p, AgentBackend::Codex).len(), 1);
    }
    #[test]
    fn permission_catalog_falls_back_to_the_provider_address_without_a_login() {
        use fox_agent_backend::native_permission::{catalog, CatalogContext, CODEX_NO_AUTO_TIER_REASON};
        // Plan C2: Codex's 自动 tier is greyed whatever the provider.
        let auto_review = |url: Option<String>| catalog(AgentBackend::Codex, &CatalogContext { provider_base_url: url, full_access_forbidden: false })
            .options.into_iter().find(|option| option.id == "agent").unwrap().disabled_reason;
        // No protocol address for Codex (Responses): the provider's own URL.
        let mut p = provider();
        p.base_url = "https://api.deepseek.com".into();
        assert!(profiles(&p, AgentBackend::Codex).is_empty());
        assert_eq!(provider_catalog_base_url(&p, AgentBackend::Codex).as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(auto_review(provider_catalog_base_url(&p, AgentBackend::Codex)).as_deref(), Some(CODEX_NO_AUTO_TIER_REASON));
        // A bound Responses address wins over the provider URL.
        let mut openai = endpoint("responses", "openai-responses");
        openai.base_url = "https://api.openai.com/v1".into();
        p.protocol_profiles = vec![openai];
        assert_eq!(provider_catalog_base_url(&p, AgentBackend::Codex).as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(auto_review(provider_catalog_base_url(&p, AgentBackend::Codex)).as_deref(), Some(CODEX_NO_AUTO_TIER_REASON));
        // Two candidates, one not OpenAI: the non-OpenAI one is reported.
        p.protocol_profiles.push(endpoint("other", "openai-responses"));
        assert_eq!(provider_catalog_base_url(&p, AgentBackend::Codex).as_deref(), Some("https://example.com/v1"));
        // Nothing to judge by: None.
        p.protocol_profiles.clear();
        p.base_url = "  ".into();
        assert_eq!(provider_catalog_base_url(&p, AgentBackend::Codex), None);
        assert_eq!(auto_review(None).as_deref(), Some(CODEX_NO_AUTO_TIER_REASON));
    }
    #[test]
    fn only_catalog_provenance_and_exact_endpoint_are_trusted() {
        let mut p = endpoint("responses", "openai-responses");
        p.base_url = "https://api.deepseek.com".into();
        assert!(!trusted(&p));
        p.preset_id = Some("deepseek-official".into());
        assert!(trusted(&p));
        p.base_url = "https://api.deepseek.com.example.org".into();
        assert!(!trusted(&p));
    }
    #[test]
    fn validation_cache_tracks_model_address_and_supplier_revision() {
        let path = std::env::temp_dir().join(format!("fox-protocol-cache-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let mut p = provider();
        let mut profile = endpoint("responses", "openai-responses");
        profile.model_id = "one".into();
        assert!(!verified(&db, &p, &profile));
        db.set_app_setting(&cache_key(&p, &profile), &cache_value(&p, &profile)).unwrap();
        assert!(verified(&db, &p, &profile));
        profile.model_id = "two".into(); assert!(!verified(&db, &p, &profile));
        profile.model_id = "one".into(); profile.base_url.push_str("/changed"); assert!(!verified(&db, &p, &profile));
        profile.base_url = "https://example.com/v1".into(); p.updated_at += 1; assert!(!verified(&db, &p, &profile));
        drop(db); let _ = std::fs::remove_file(path);
    }
    #[test]
    fn does_not_fall_back_to_another_supplier() {
        let path = std::env::temp_dir().join(format!("fox-protocol-current-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let mut other = provider(); other.id = "other".into(); other.base_url = "https://other.example.com/v1".into(); other.is_default = false; other.models[0].id = "other-model".into(); other.protocol_profiles = vec![endpoint("responses", "openai-responses")];
        db.save_model_provider(&other).unwrap();
        let chosen = db.save_model_provider(&provider()).unwrap();
        assert_eq!(current(&db).unwrap().unwrap().id, chosen.id);
        assert!(profiles(&current(&db).unwrap().unwrap(), AgentBackend::Codex).is_empty());
        db.delete_model_provider(&chosen.id).unwrap();
        assert!(current(&db).unwrap().is_none());
        drop(db); let _ = std::fs::remove_file(path);
    }
}

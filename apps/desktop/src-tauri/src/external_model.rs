use crate::{database::{Database, ModelProviderRecord}, model_service};
use fox_agent_backend::{agent_config::BindingSource, port::AgentBackend, provider_protocol::{self, ProtocolProfile}, unified_login::{Secret, UnifiedLogin}};

pub fn required_api(backend: AgentBackend) -> &'static str {
    match backend { AgentBackend::Codex => "openai-responses", AgentBackend::Deepseek => "anthropic-messages", _ => "openai-completions" }
}

pub fn trusted(profile: &ProtocolProfile) -> bool {
    matches!((profile.preset_id.as_deref(), profile.api_type.as_str(), profile.base_url.trim_end_matches('/')),
        (Some("deepseek-official"), "anthropic-messages", "https://api.deepseek.com/anthropic") |
        (Some("deepseek-official"), "openai-responses" | "openai-completions", "https://api.deepseek.com") |
        (Some("openai"), "openai-responses" | "openai-completions", "https://api.openai.com/v1"))
}

pub fn profiles(provider: &ModelProviderRecord, backend: AgentBackend) -> Vec<ProtocolProfile> {
    let model = provider.models.iter().find(|model| model.is_default).or(provider.models.first()).map(|model| model.model_id.as_str()).unwrap_or_default();
    let primary = ProtocolProfile { preset_id: None, id: "primary".into(), api_type: provider.api_type.clone(), base_url: provider.base_url.clone(), model_id: model.into(), separate_credential: false, credential_configured: false };
    let mut profiles = vec![primary];
    if backend != AgentBackend::Fox { profiles.extend(provider.protocol_profiles.iter().filter(|profile| profile.id != "primary").cloned()); }
    for profile in &mut profiles { if profile.model_id.trim().is_empty() { profile.model_id = model.into(); } }
    profiles.retain(|profile| provider_protocol::supports(&profile.api_type, backend.as_str()));
    profiles
}

pub fn current(database: &Database) -> Result<Option<ModelProviderRecord>, String> {
    Ok(database.list_model_providers()?.into_iter().find(|provider| provider.enabled && provider.is_default))
}

pub fn credential_ref(provider: &ModelProviderRecord, profile: &ProtocolProfile) -> String {
    if profile.separate_credential { provider_protocol::credential_id(&provider.id, &profile.id) } else { provider.base_url.clone() }
}

fn credential_revision_key(credential: &str) -> String { format!("model.credential_revision.{credential}") }

pub fn credential_revision(database: &Database, credential: &str) -> String {
    database.app_setting(&credential_revision_key(credential)).ok().flatten().unwrap_or_else(|| "0".to_owned())
}

pub fn bump_credential_revision(database: &Database, credential: &str) -> Result<(), String> {
    database.set_app_setting(&credential_revision_key(credential), &uuid::Uuid::new_v4().to_string())
}

fn verification_key(api_type: &str, base_url: &str, model_id: &str, credential: &str) -> String {
    format!("model.verified.{}", crate::canonical_json::to_string(&(api_type, base_url, model_id, credential)).expect("connection identity serializes"))
}

pub fn record_verified_connection(database: &Database, api_type: &str, base_url: &str, model_id: &str, credential: &str) -> Result<(), String> {
    database.set_app_setting(&verification_key(api_type, base_url, model_id, credential), &credential_revision(database, credential))
}

pub fn forget_verified_connection(database: &Database, provider: &ModelProviderRecord, profile: &ProtocolProfile) -> Result<(), String> {
    let key = verification_key(&profile.api_type, &profile.base_url, &profile.model_id, &credential_ref(provider, profile));
    database.with_connection(|connection| { connection.execute("DELETE FROM app_settings WHERE key=?1", [key])?; Ok(()) })
}

pub fn verified(database: &Database, provider: &ModelProviderRecord, profile: &ProtocolProfile) -> bool {
    let credential = credential_ref(provider, profile);
    database.app_setting(&verification_key(&profile.api_type, &profile.base_url, &profile.model_id, &credential)).ok().flatten().as_deref() == Some(credential_revision(database, &credential).as_str())
}

pub fn verified_at(database: &Database, provider: &ModelProviderRecord, profile: &ProtocolProfile) -> Option<i64> {
    if !verified(database, provider, profile) { return None; }
    database.app_setting_updated_at(&verification_key(&profile.api_type, &profile.base_url, &profile.model_id, &credential_ref(provider, profile))).ok().flatten()
}

pub fn record_connection_test(database: &Database, api_type: &str, base_url: &str, model_id: &str, credential: Option<&str>) -> Result<(), String> {
    let Some(credential) = credential else { return Ok(()) };
    let model_id = crate::model_service::normalize_model_id(base_url, model_id);
    if base_url.trim().is_empty() || model_id.trim().is_empty() { return Ok(()); }
    record_verified_connection(database, api_type, base_url, &model_id, credential)
}

pub fn login(provider: &ModelProviderRecord, profile: &ProtocolProfile) -> Option<UnifiedLogin> {
    Some(UnifiedLogin { provider_name: provider.name.clone(), base_url: profile.base_url.clone(), api_type: profile.api_type.clone(), model: profile.model_id.clone(), api_key: Secret::new(model_service::get_api_key(&credential_ref(provider, profile))?), responses_supported: None })
}

pub fn bound_connection(database: &Database, backend: AgentBackend) -> Option<(ModelProviderRecord, ProtocolProfile)> {
    if backend == AgentBackend::Fox {
        let provider = current(database).ok()??;
        let profile = profiles(&provider, backend).into_iter().next()?;
        return Some((provider, profile));
    }
    let binding = database.agent_binding(backend).ok()??.binding?;
    let BindingSource::FoxService { provider_id: Some(provider_id), endpoint_id, model_id } = binding.source else { return None };
    let provider = database.list_model_providers().ok()?.into_iter().find(|provider| provider.id == provider_id && provider.enabled)?;
    let mut profile = profiles(&provider, backend).into_iter().find(|profile| profile.id == endpoint_id)?;
    profile.model_id = model_id;
    Some((provider, profile))
}

pub fn external_login(database: &Database, backend: AgentBackend) -> Option<UnifiedLogin> {
    let (provider, profile) = bound_connection(database, backend)?;
    verified(database, &provider, &profile).then(|| login(&provider, &profile)).flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginGap { Unbound, Missing, Unverified }

pub fn external_login_gap(database: &Database, backend: AgentBackend) -> Option<LoginGap> {
    let Some(record) = database.agent_binding(backend).ok().flatten() else { return Some(LoginGap::Unbound) };
    if record.binding.is_none() { return Some(LoginGap::Unbound); }
    let Some((provider, profile)) = bound_connection(database, backend) else { return Some(LoginGap::Missing) };
    if !verified(database, &provider, &profile) { return Some(LoginGap::Unverified); }
    login(&provider, &profile).is_none().then_some(LoginGap::Missing)
}

pub fn catalog_provider_base_url(database: &Database, backend: AgentBackend) -> Option<String> {
    bound_connection(database, backend).map(|(_, profile)| profile.base_url)
        .or_else(|| current(database).ok().flatten().map(|provider| provider.base_url))
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
        ProtocolProfile { preset_id: None, id: id.into(), api_type: api.into(), base_url: "https://example.com/v1".into(), model_id: String::new(), separate_credential: false, credential_configured: false }
    }
    #[test]
    fn a_passing_test_of_the_saved_connection_survives_a_resave_but_not_a_key_change() {
        // 13.37 B3-2: 「验证此协议」 passed, the user saved the provider again,
        // and the next dsh send still said the connection needed a test.
        let path = std::env::temp_dir().join(format!("fox-protocol-verified-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let mut p = provider();
        p.protocol_profiles = vec![endpoint("messages", "anthropic-messages")];
        let saved = db.save_model_provider(&p).unwrap();
        let profile = profiles(&saved, AgentBackend::Deepseek).remove(0);
        assert_eq!(profile.model_id, "default-model");
        assert!(!verified(&db, &saved, &profile));
        // The protocol test used the provider's saved Key (credential = its base URL).
        record_connection_test(&db, "anthropic-messages", "https://example.com/v1", "default-model", Some(saved.base_url.as_str())).unwrap();
        assert!(verified(&db, &saved, &profile));
        // Saving again (no Key, address or model change) keeps it.
        let resaved = db.save_model_provider(&saved).unwrap();
        let profile = profiles(&resaved, AgentBackend::Deepseek).remove(0);
        assert!(verified(&db, &resaved, &profile));
        // Another model, another address: not this test.
        let mut other = profile.clone(); other.model_id = "other-model".into(); assert!(!verified(&db, &resaved, &other));
        let mut moved = profile.clone(); moved.base_url = "https://example.com/v2".into(); assert!(!verified(&db, &resaved, &moved));
        // A new Key for the provider: the old test no longer counts.
        bump_credential_revision(&db, &resaved.base_url).unwrap();
        assert!(!verified(&db, &resaved, &profile));
        // A typed Key that is not saved proves nothing.
        record_connection_test(&db, "anthropic-messages", "https://example.com/v1", "default-model", None).unwrap();
        assert!(!verified(&db, &resaved, &profile));
        // A separate protocol Key has its own revision.
        let mut separate = profile.clone(); separate.separate_credential = true;
        let credential = provider_protocol::credential_id(&resaved.id, &separate.id);
        record_connection_test(&db, "anthropic-messages", "https://example.com/v1", "default-model", Some(credential.as_str())).unwrap();
        assert!(verified(&db, &resaved, &separate));
        assert!(!verified(&db, &resaved, &profile));
        bump_credential_revision(&db, &credential).unwrap();
        assert!(!verified(&db, &resaved, &separate));
        drop(db); let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bindings_select_a_non_default_service_and_keep_missing_provider_visible() {
        let db = Database::open(":memory:".into()).unwrap();
        let first = db.save_model_provider(&provider()).unwrap();
        assert!(external_login(&db, AgentBackend::Pi).is_none());
        assert_eq!(external_login_gap(&db, AgentBackend::Pi), Some(LoginGap::Unbound));
        let mut second = provider(); second.id = "second".into(); second.base_url = "https://second.example/v1".into(); second.is_default = false; second.models[0].id = "second-model".into();
        db.save_model_provider(&second).unwrap();
        db.set_agent_enabled(AgentBackend::Pi, true).unwrap();
        db.save_agent_binding(AgentBackend::Pi, &BindingSource::FoxService { provider_id: Some(second.id.clone()), endpoint_id: "primary".into(), model_id: "selected-model".into() }).unwrap();
        let (bound, profile) = bound_connection(&db, AgentBackend::Pi).unwrap();
        assert_eq!(bound.id, second.id); assert_eq!(profile.model_id, "selected-model");
        assert_eq!(current(&db).unwrap().unwrap().id, first.id);
        assert_eq!(catalog_provider_base_url(&db, AgentBackend::Pi), Some(second.base_url.clone()));
        assert_eq!(external_login_gap(&db, AgentBackend::Pi), Some(LoginGap::Unverified));
        record_verified_connection(&db, &profile.api_type, &profile.base_url, &profile.model_id, &second.base_url).unwrap();
        assert!(verified(&db,&bound,&profile));
        bump_credential_revision(&db,&second.base_url).unwrap();
        assert!(!verified(&db,&bound,&profile));
        db.delete_model_provider(&second.id).unwrap();
        assert!(bound_connection(&db, AgentBackend::Pi).is_none());
        assert_eq!(external_login_gap(&db,AgentBackend::Pi),Some(LoginGap::Missing));
        assert!(db.agent_binding(AgentBackend::Pi).unwrap().unwrap().enabled);
    }

    #[test]
    fn primary_and_extra_endpoints_remain_explicit_and_trust_is_not_a_test() {
        let db = Database::open(":memory:".into()).unwrap();
        let mut provider = provider();
        let mut profile = endpoint("official", "openai-completions");
        profile.preset_id = Some("openai".into()); profile.base_url = "https://api.openai.com/v1".into();
        assert!(trusted(&profile)); assert!(!verified(&db,&provider,&profile));
        provider.protocol_profiles = vec![profile];
        assert_eq!(profiles(&provider, AgentBackend::Pi).len(), 2);
        assert_eq!(profiles(&provider, AgentBackend::Fox).len(), 1);
        assert_eq!(profiles(&provider, AgentBackend::Fox)[0].id, "primary");
    }
}

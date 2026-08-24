use crate::{
    app_state::AppState,
    database::{
        ApiResponse, CreateDigitalColleagueInput, DigitalColleagueAuditRecord,
        DigitalColleagueChannelRecord, DigitalColleagueRecord, DigitalColleagueScheduleRecord,
        DigitalColleagueTriggerAcceptance, DigitalColleagueTriggerRecord, KnowledgeReference,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tauri::State;
use uuid::Uuid;

const CHANNEL_KEYRING_SERVICE: &str = "FoxDigitalColleagueChannel";
const MAX_TRIGGER_BYTES: usize = 64 * 1024;
const SIGNATURE_WINDOW_SECONDS: i64 = 5 * 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueCreateRequest {
    name: String,
    expert_id: String,
    objective: String,
    project_id: Option<String>,
    project_root: Option<String>,
    #[serde(default)]
    knowledge_references: Vec<KnowledgeReference>,
    #[serde(default = "default_runs_per_day")]
    max_runs_per_day: i64,
    #[serde(default = "default_tokens_per_day")]
    max_tokens_per_day: i64,
    #[serde(default = "default_duration_ms")]
    max_duration_ms: i64,
    #[serde(default = "default_output_tokens")]
    max_output_tokens: i64,
    #[serde(default = "default_tool_calls")]
    max_tool_calls: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueUpdateRequest {
    colleague_id: String,
    name: String,
    objective: String,
    max_runs_per_day: i64,
    max_tokens_per_day: i64,
    max_duration_ms: i64,
    max_output_tokens: i64,
    max_tool_calls: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueIdRequest {
    colleague_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleaguePauseRequest {
    colleague_id: String,
    paused: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueRevokeRequest {
    colleague_id: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueScheduleSaveRequest {
    schedule_id: Option<String>,
    colleague_id: String,
    name: String,
    interval_seconds: i64,
    #[serde(default = "default_catchup_seconds")]
    catchup_window_seconds: i64,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueChannelCreateRequest {
    colleague_id: String,
    name: String,
    channel_kind: String,
    external_identity: String,
    #[serde(default = "default_channel_rate")]
    rate_limit_per_minute: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueChannelIdRequest {
    channel_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueChannelCreated {
    channel: DigitalColleagueChannelRecord,
    secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueManualTriggerRequest {
    colleague_id: String,
    idempotency_key: Option<String>,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueChannelTriggerRequest {
    channel_id: String,
    sender_id: String,
    timestamp: i64,
    idempotency_key: String,
    signature: String,
    #[serde(default)]
    payload: Value,
}

fn default_runs_per_day() -> i64 {
    8
}
fn default_tokens_per_day() -> i64 {
    128_000
}
fn default_duration_ms() -> i64 {
    300_000
}
fn default_output_tokens() -> i64 {
    4_096
}
fn default_tool_calls() -> i64 {
    16
}
fn default_catchup_seconds() -> i64 {
    3_600
}
fn default_channel_rate() -> i64 {
    10
}
fn default_true() -> bool {
    true
}

fn response<T: Serialize>(result: Result<T, String>) -> ApiResponse<T> {
    result.map(ApiResponse::success).unwrap_or_else(|message| {
        let code = if message.starts_with("digital_colleague.") {
            message.as_str()
        } else {
            "digital_colleague.failed"
        };
        ApiResponse::failure(code, message.clone(), false)
    })
}

fn validate_create(request: &DigitalColleagueCreateRequest) -> Result<(), String> {
    validate_text("name", &request.name, 1, 120)?;
    validate_id("expertId", &request.expert_id, 160)?;
    validate_text("objective", &request.objective, 1, 8_000)?;
    if let Some(root) = &request.project_root {
        validate_text("projectRoot", root, 1, 1_024)?;
    }
    for reference in &request.knowledge_references {
        reference.validate()?;
    }
    validate_quotas(
        request.max_runs_per_day,
        request.max_tokens_per_day,
        request.max_duration_ms,
        request.max_output_tokens,
        request.max_tool_calls,
    )
}

fn validate_quotas(
    runs: i64,
    tokens: i64,
    duration: i64,
    output: i64,
    tools: i64,
) -> Result<(), String> {
    if !(1..=100).contains(&runs)
        || !(256..=2_000_000).contains(&tokens)
        || !(1_000..=900_000).contains(&duration)
        || !(64..=32_768).contains(&output)
        || output > tokens.min(200_000)
        || !(0..=100).contains(&tools)
    {
        return Err("digital_colleague.quota_invalid".to_owned());
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, min: usize, max: usize) -> Result<(), String> {
    let length = value.trim().chars().count();
    if length < min || length > max || value.chars().any(char::is_control) {
        return Err(format!("digital_colleague.{field}_invalid"));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(format!("digital_colleague.{field}_invalid"));
    }
    Ok(())
}

fn validate_payload(payload: &Value) -> Result<(), String> {
    let size = serde_json::to_vec(payload)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_TRIGGER_BYTES {
        return Err("digital_colleague.payload_too_large".to_owned());
    }
    Ok(())
}

fn dispatch_acceptance(
    state: &AppState,
    acceptance: DigitalColleagueTriggerAcceptance,
) -> Result<DigitalColleagueTriggerRecord, String> {
    if let Some(prepared) = acceptance.prepared {
        let run_id = prepared.started.run.id.clone();
        if let Err(error) = state
            .runtime_host
            .dispatch_digital_colleague_trigger(prepared)
        {
            let _ = state.database.mark_run_failed(
                &run_id,
                "digital_colleague.dispatch_failed",
                &error,
            );
            return Err(format!("digital_colleague.dispatch_failed: {error}"));
        }
    }
    Ok(acceptance.trigger)
}

#[tauri::command]
pub fn digital_colleagues_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<DigitalColleagueRecord>> {
    response(state.database.list_digital_colleagues())
}

#[tauri::command]
pub fn digital_colleague_create(
    state: State<'_, AppState>,
    request: DigitalColleagueCreateRequest,
) -> ApiResponse<DigitalColleagueRecord> {
    response((|| {
        validate_create(&request)?;
        state
            .database
            .create_digital_colleague(&CreateDigitalColleagueInput {
                name: request.name,
                expert_id: request.expert_id,
                objective: request.objective,
                project_id: request.project_id,
                project_root: request.project_root,
                knowledge_references: request.knowledge_references,
                max_runs_per_day: request.max_runs_per_day,
                max_tokens_per_day: request.max_tokens_per_day,
                max_duration_ms: request.max_duration_ms,
                max_output_tokens: request.max_output_tokens,
                max_tool_calls: request.max_tool_calls,
            })
    })())
}

#[tauri::command]
pub fn digital_colleague_update(
    state: State<'_, AppState>,
    request: DigitalColleagueUpdateRequest,
) -> ApiResponse<DigitalColleagueRecord> {
    response((|| {
        validate_text("name", &request.name, 1, 120)?;
        validate_text("objective", &request.objective, 1, 8_000)?;
        validate_quotas(
            request.max_runs_per_day,
            request.max_tokens_per_day,
            request.max_duration_ms,
            request.max_output_tokens,
            request.max_tool_calls,
        )?;
        state.database.update_digital_colleague(
            &request.colleague_id,
            &request.name,
            &request.objective,
            request.max_runs_per_day,
            request.max_tokens_per_day,
            request.max_duration_ms,
            request.max_output_tokens,
            request.max_tool_calls,
        )
    })())
}

#[tauri::command]
pub fn digital_colleague_set_paused(
    state: State<'_, AppState>,
    request: DigitalColleaguePauseRequest,
) -> ApiResponse<DigitalColleagueRecord> {
    response(
        state
            .database
            .set_digital_colleague_paused(&request.colleague_id, request.paused),
    )
}

#[tauri::command]
pub fn digital_colleague_revoke(
    state: State<'_, AppState>,
    request: DigitalColleagueRevokeRequest,
) -> ApiResponse<DigitalColleagueRecord> {
    response((|| {
        let credentials = state
            .database
            .digital_colleague_channel_credentials(&request.colleague_id)?;
        let record = state.database.revoke_digital_colleague(
            &request.colleague_id,
            if request.reason.trim().is_empty() {
                "revoked by user"
            } else {
                request.reason.trim()
            },
        )?;
        for (_, credential_ref) in credentials {
            let _ = channel_credential(&credential_ref)?.delete_credential();
        }
        if let Some(active_run) = state
            .database
            .list_digital_colleague_triggers(&request.colleague_id, 20)?
            .into_iter()
            .find(|trigger| matches!(trigger.status.as_str(), "queued" | "running"))
            .and_then(|trigger| trigger.run_id)
        {
            let _ = state.runtime_host.cancel_managed_run(&active_run);
        }
        Ok(record)
    })())
}

#[tauri::command]
pub fn digital_colleague_schedules_list(
    state: State<'_, AppState>,
    request: DigitalColleagueIdRequest,
) -> ApiResponse<Vec<DigitalColleagueScheduleRecord>> {
    response(
        state
            .database
            .list_digital_colleague_schedules(&request.colleague_id),
    )
}

#[tauri::command]
pub fn digital_colleague_schedule_save(
    state: State<'_, AppState>,
    request: DigitalColleagueScheduleSaveRequest,
) -> ApiResponse<DigitalColleagueScheduleRecord> {
    response((|| {
        validate_text("schedule_name", &request.name, 1, 120)?;
        if !(60..=2_592_000).contains(&request.interval_seconds)
            || !(60..=86_400).contains(&request.catchup_window_seconds)
        {
            return Err("digital_colleague.schedule_invalid".to_owned());
        }
        state.database.save_digital_colleague_schedule(
            request.schedule_id.as_deref(),
            &request.colleague_id,
            &request.name,
            request.interval_seconds,
            request.catchup_window_seconds,
            request.enabled,
        )
    })())
}

#[tauri::command]
pub fn digital_colleague_channels_list(
    state: State<'_, AppState>,
    request: DigitalColleagueIdRequest,
) -> ApiResponse<Vec<DigitalColleagueChannelRecord>> {
    response(
        state
            .database
            .list_digital_colleague_channels(&request.colleague_id),
    )
}

#[tauri::command]
pub fn digital_colleague_channel_create(
    state: State<'_, AppState>,
    request: DigitalColleagueChannelCreateRequest,
) -> ApiResponse<DigitalColleagueChannelCreated> {
    response((|| {
        validate_text("channel_name", &request.name, 1, 120)?;
        validate_id("external_identity", &request.external_identity, 240)?;
        if !matches!(request.channel_kind.as_str(), "webhook" | "im_bridge")
            || !(1..=60).contains(&request.rate_limit_per_minute)
        {
            return Err("digital_colleague.channel_invalid".to_owned());
        }
        let credential_ref = format!("channel-{}", Uuid::new_v4());
        let secret = format!(
            "foxdc_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        channel_credential(&credential_ref)?
            .set_password(&secret)
            .map_err(|error| format!("digital_colleague.credential_store_failed: {error}"))?;
        let result = state.database.insert_digital_colleague_channel(
            &request.colleague_id,
            &request.name,
            &request.channel_kind,
            &request.external_identity,
            &credential_ref,
            &secret[..12],
            request.rate_limit_per_minute,
        );
        match result {
            Ok(channel) => Ok(DigitalColleagueChannelCreated { channel, secret }),
            Err(error) => {
                let _ = channel_credential(&credential_ref)?.delete_credential();
                Err(error)
            }
        }
    })())
}

#[tauri::command]
pub fn digital_colleague_channel_revoke(
    state: State<'_, AppState>,
    request: DigitalColleagueChannelIdRequest,
) -> ApiResponse<DigitalColleagueChannelRecord> {
    response((|| {
        let (channel, credential_ref) = state
            .database
            .revoke_digital_colleague_channel(&request.channel_id)?
            .ok_or_else(|| "digital_colleague.channel_not_found".to_owned())?;
        let _ = channel_credential(&credential_ref)?.delete_credential();
        Ok(channel)
    })())
}

#[tauri::command]
pub fn digital_colleague_trigger_manual(
    state: State<'_, AppState>,
    request: DigitalColleagueManualTriggerRequest,
) -> ApiResponse<DigitalColleagueTriggerRecord> {
    response((|| {
        validate_payload(&request.payload)?;
        let key = request
            .idempotency_key
            .unwrap_or_else(|| format!("manual:{}", Uuid::new_v4()));
        validate_id("idempotency_key", &key, 240)?;
        let acceptance = state.database.accept_digital_colleague_trigger(
            &request.colleague_id,
            "manual",
            None,
            &key,
            &request.payload,
            None,
            "user",
        )?;
        dispatch_acceptance(&state, acceptance)
    })())
}

#[tauri::command]
pub fn digital_colleague_trigger_channel(
    state: State<'_, AppState>,
    request: DigitalColleagueChannelTriggerRequest,
) -> ApiResponse<DigitalColleagueTriggerRecord> {
    response((|| {
        validate_payload(&request.payload)?;
        validate_id("sender_id", &request.sender_id, 240)?;
        validate_id("idempotency_key", &request.idempotency_key, 240)?;
        let (channel, credential_ref) = state
            .database
            .get_digital_colleague_channel(&request.channel_id)?
            .ok_or_else(|| "digital_colleague.channel_not_found".to_owned())?;
        let now = crate::database::now_ms();
        let actor = format!("channel:{}", request.sender_id);
        let reject = |code: &str| {
            let _ = state.database.record_digital_colleague_audit(
                Some(&channel.colleague_id),
                Some(&channel.id),
                None,
                "channel.trigger_received",
                "rejected",
                &actor,
                serde_json::json!({ "code": code }),
            );
            Err(code.to_owned())
        };
        if channel.status != "active" {
            return reject("digital_colleague.channel_revoked");
        }
        if channel.external_identity != request.sender_id {
            return reject("digital_colleague.channel_identity_mismatch");
        }
        if (now / 1_000).abs_diff(request.timestamp) > SIGNATURE_WINDOW_SECONDS as u64 {
            return reject("digital_colleague.channel_timestamp_invalid");
        }
        let attempts = state
            .database
            .digital_colleague_channel_attempts_since(&channel.id, now.saturating_sub(60_000))?;
        if attempts >= channel.rate_limit_per_minute {
            return reject("digital_colleague.channel_rate_limited");
        }
        let secret = channel_credential(&credential_ref)?
            .get_password()
            .map_err(|error| format!("digital_colleague.credential_load_failed: {error}"))?;
        let signing_input = channel_signing_input(
            request.timestamp,
            &request.idempotency_key,
            &request.payload,
        );
        let expected = format!(
            "v1={}",
            hex::encode(hmac_sha256(secret.as_bytes(), signing_input.as_bytes()))
        );
        if !constant_time_equal(expected.as_bytes(), request.signature.as_bytes()) {
            return reject("digital_colleague.channel_signature_invalid");
        }
        state.database.record_digital_colleague_audit(
            Some(&channel.colleague_id),
            Some(&channel.id),
            None,
            "channel.trigger_received",
            "accepted",
            &actor,
            serde_json::json!({ "idempotencyKey": request.idempotency_key }),
        )?;
        let acceptance = state.database.accept_digital_colleague_trigger(
            &channel.colleague_id,
            "channel",
            Some(&channel.id),
            &request.idempotency_key,
            &request.payload,
            None,
            &actor,
        )?;
        dispatch_acceptance(&state, acceptance)
    })())
}

#[tauri::command]
pub fn digital_colleague_triggers_list(
    state: State<'_, AppState>,
    request: DigitalColleagueIdRequest,
) -> ApiResponse<Vec<DigitalColleagueTriggerRecord>> {
    response(
        state
            .database
            .list_digital_colleague_triggers(&request.colleague_id, 100),
    )
}

#[tauri::command]
pub fn digital_colleague_audit_list(
    state: State<'_, AppState>,
    request: DigitalColleagueIdRequest,
) -> ApiResponse<Vec<DigitalColleagueAuditRecord>> {
    response(
        state
            .database
            .list_digital_colleague_audit(&request.colleague_id, 200),
    )
}

fn channel_credential(credential_ref: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(CHANNEL_KEYRING_SERVICE, credential_ref)
        .map_err(|error| format!("digital_colleague.credential_entry_failed: {error}"))
}

fn channel_signing_input(timestamp: i64, idempotency_key: &str, payload: &Value) -> String {
    format!(
        "v1:{timestamp}:{idempotency_key}:{}",
        canonical_json(payload)
    )
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("serialize JSON string"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("serialize JSON key"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let maximum = left.len().max(right.len());
    for index in 0..maximum {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hmac_matches_rfc_4231_and_comparison_is_constant_shape() {
        let key = [0x0bu8; 20];
        assert_eq!(
            hex::encode(hmac_sha256(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert!(constant_time_equal(b"same", b"same"));
        assert!(!constant_time_equal(b"same", b"different"));
    }

    #[test]
    fn signature_input_is_canonical_and_binds_timestamp_and_idempotency() {
        let left = channel_signing_input(42, "event-1", &json!({"b": 2, "a": 1}));
        let right = channel_signing_input(42, "event-1", &json!({"a": 1, "b": 2}));
        assert_eq!(left, right);
        assert_ne!(
            left,
            channel_signing_input(43, "event-1", &json!({"a": 1, "b": 2}))
        );
        assert_ne!(
            left,
            channel_signing_input(42, "event-2", &json!({"a": 1, "b": 2}))
        );
    }
}

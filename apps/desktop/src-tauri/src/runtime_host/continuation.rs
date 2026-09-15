use super::protocol::RuntimeCapabilityManifest;
use crate::database::{
    AppendContinuationDecisionInput, ContinuationDecisionKind, ContinuationDecisionRecord,
    ContinuationReasonCode, ContinuationRetryClass, ContinuationValidationOutcome, Database,
    EvidenceValidityStatus, GoalStatus, RecordContinuationIngestDiagnosticInput,
    TaskLedgerCompletedOutcome, TaskLedgerPendingActionKind, TaskLedgerProjection, WorkTaskStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

pub const EXECUTION_PROFILE_SETTING: &str = "agent.execution_profile_v1";
pub const CONTINUATION_PROPOSAL_EVENT_TYPE: &str = "run.continuation_proposed";

const CONTINUATION_PROPOSAL_TOOL_NAME: &str = "continuation_propose";
const GRAPH_READONLY_TOOL_NAME: &str = "graph_readonly_run";
const DURABLE_V2_PERSISTENT_GRAPH_TOOLS: &[&str] = &[
    "graph_readonly_activate",
    "graph_readonly_snapshot_get",
    "graph_readonly_node_start",
    "graph_readonly_node_finish",
    "graph_readonly_node_cancel",
    "graph_readonly_node_review",
    "graph_readonly_accept",
];
const MAX_IDENTIFIER_CHARS: usize = 200;
const MAX_IDENTIFIER_ITEMS: usize = 100;
const MAX_NEXT_ACTION_CHARS: usize = 2_000;
const MAX_DIAGNOSTIC_CODE_CHARS: usize = 200;
const MAX_DIAGNOSTIC_MESSAGE_CHARS: usize = 4_000;
const MAX_PROJECTION_RETRIES: usize = 2;
const PROJECTION_HASH_MISMATCH_PREFIX: &str = "[continuation.projection_hash_mismatch]";
const CONTINUATION_DECISIONS: &[&str] =
    &["continue", "repair", "wait_approval", "complete", "blocked"];
const CONTINUATION_REASON_CODES: &[&str] = &[
    "work_remaining",
    "validation_failed",
    "approval_pending",
    "external_dependency_unavailable",
    "budget_exhausted",
    "user_input_required",
    "retry_available",
    "retry_exhausted",
    "acceptance_missing",
    "acceptance_candidate",
    "acceptance_passed",
    "host_audit_required",
    "tool_failed",
    "task_interrupted",
];
const CONTINUATION_RETRY_CLASSES: &[&str] = &[
    "none",
    "recoverable",
    "retry_limited",
    "non_retryable",
    "host_decides",
];

const READ_ONLY_PROFILE_TOOLS: &[&str] = &[
    "read",
    "ls",
    "find",
    "grep",
    "read_attachment",
    // Ranges over results Host already stored for this conversation. Nothing is
    // executed, so it is allowed wherever reading your own context is allowed.
    "read_tool_result",
    "sqlite_read",
    "structured_data",
    "tabular_data",
    "work_snapshot_get",
    "workflow_snapshot_get",
    "team_snapshot_get",
    "child_agent_list",
    "memory_search",
    "list_knowledge_bases",
    "search_knowledge",
    "read_knowledge_document",
    "query_knowledge_graph",
    "list_mcp_tools",
    "continuation_propose",
];

const DURABLE_UNINTEGRATED_WORKFLOW_MUTATORS: &[&str] = &[
    "workflow_start",
    "workflow_stage_start",
    "workflow_stage_complete",
    "workflow_stage_fail",
    "workflow_cancel",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProfileSelection {
    id: &'static str,
    completion_audit: &'static str,
    validation_policy: &'static str,
    prompt_policy: &'static str,
    continuation_mode: &'static str,
    tool_policy: &'static str,
    graph_mode: &'static str,
    graph_max_nodes: u64,
    graph_max_depth: u64,
    graph_writers_allowed: bool,
    shadow: bool,
}

impl ExecutionProfileSelection {
    pub fn load(database: &Database) -> Result<Self, String> {
        let configured = database
            .app_setting(EXECUTION_PROFILE_SETTING)?
            .unwrap_or_else(|| "legacy".to_owned());
        Self::resolve(configured.trim())
    }

    pub fn resolve(id: &str) -> Result<Self, String> {
        let selection = match id {
            "legacy" => Self {
                id: "legacy",
                completion_audit: "legacy",
                validation_policy: "legacy",
                prompt_policy: "stable_v1",
                continuation_mode: "disabled",
                tool_policy: "host_guarded",
                graph_mode: "disabled",
                graph_max_nodes: 0,
                graph_max_depth: 0,
                graph_writers_allowed: false,
                shadow: false,
            },
            "durable_v2_shadow" => Self {
                id: "durable_v2_shadow",
                completion_audit: "strict_v2",
                validation_policy: "risk_v1",
                prompt_policy: "stable_v1",
                continuation_mode: "shadow",
                tool_policy: "read_only_shadow",
                graph_mode: "disabled",
                graph_max_nodes: 0,
                graph_max_depth: 0,
                graph_writers_allowed: false,
                shadow: true,
            },
            "durable_v2" => Self {
                id: "durable_v2",
                completion_audit: "strict_v2",
                validation_policy: "risk_v1",
                prompt_policy: "stable_v1",
                continuation_mode: "propose",
                tool_policy: "host_guarded",
                graph_mode: "disabled",
                graph_max_nodes: 0,
                graph_max_depth: 0,
                graph_writers_allowed: false,
                shadow: false,
            },
            "graph_readonly_preview" => Self {
                id: "graph_readonly_preview",
                completion_audit: "strict_v2",
                validation_policy: "risk_v1",
                prompt_policy: "stable_v1",
                continuation_mode: "propose",
                tool_policy: "read_only_preview",
                graph_mode: "read_only_preview",
                graph_max_nodes: 3,
                graph_max_depth: 1,
                graph_writers_allowed: false,
                shadow: false,
            },
            "graph_reviewer_v1" => Self {
                id: "graph_reviewer_v1",
                completion_audit: "strict_v2",
                validation_policy: "high_risk_v1",
                prompt_policy: "graph_reviewer_v1",
                continuation_mode: "disabled",
                tool_policy: "graph_reviewer_read_only",
                graph_mode: "disabled",
                graph_max_nodes: 0,
                graph_max_depth: 0,
                graph_writers_allowed: false,
                shadow: false,
            },
            other => {
                return Err(format!(
                    "[runtime.execution_profile.invalid] unsupported execution profile '{other}'"
                ))
            }
        };
        Ok(selection)
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    pub fn is_shadow(&self) -> bool {
        self.shadow
    }

    pub fn treats_proposal_error_as_observation(&self, payload: &Value) -> bool {
        self.shadow
            && payload.get("type").and_then(Value::as_str) == Some(CONTINUATION_PROPOSAL_EVENT_TYPE)
    }

    pub fn initialize_fields(&self) -> Value {
        json!({
            "executionProfile": self.id,
            "executionStrategy": {
                "completionAudit": self.completion_audit,
                "validationPolicy": self.validation_policy,
                "promptPolicy": self.prompt_policy,
            },
        })
    }

    pub fn allows_host_tool(&self, tool: &str) -> bool {
        if self.id == "graph_reviewer_v1" {
            return matches!(tool, "read" | "ls" | "find" | "grep");
        }
        if tool == GRAPH_READONLY_TOOL_NAME {
            return self.id == "graph_readonly_preview";
        }
        if DURABLE_V2_PERSISTENT_GRAPH_TOOLS.contains(&tool) {
            return self.id == "durable_v2";
        }
        if tool == "task_repair_escalate_start" {
            return self.id == "durable_v2";
        }
        if self.id == "durable_v2" && DURABLE_UNINTEGRATED_WORKFLOW_MUTATORS.contains(&tool) {
            return false;
        }
        self.tool_policy == "host_guarded" || READ_ONLY_PROFILE_TOOLS.contains(&tool)
    }

    pub fn validate_capability_manifest(
        &self,
        manifest: &RuntimeCapabilityManifest,
    ) -> Result<(), String> {
        if let Some(tool) = manifest
            .tools
            .iter()
            .find(|tool| !self.allows_host_tool(&tool.name))
        {
            return Err(format!(
                "runtime capability tool '{}' is outside execution profile '{}'",
                tool.name, self.id
            ));
        }
        Ok(())
    }

    pub fn validate_ready_payload(&self, payload: &Value) -> Result<(), String> {
        let profile = payload
            .get("executionProfile")
            .and_then(Value::as_object)
            .ok_or_else(|| "runtime handshake omitted executionProfile".to_owned())?;
        require_u64(
            profile.get("schemaVersion"),
            1,
            "executionProfile.schemaVersion",
        )?;
        require_str(profile.get("id"), self.id, "executionProfile.id")?;
        require_str(
            profile.get("toolPolicy"),
            self.tool_policy,
            "executionProfile.toolPolicy",
        )?;
        require_bool(
            profile.get("shadow"),
            self.shadow,
            "executionProfile.shadow",
        )?;
        require_bool(
            profile.get("sideEffectsAllowed"),
            self.tool_policy == "host_guarded",
            "executionProfile.sideEffectsAllowed",
        )?;
        let profile_continuation = profile
            .get("continuation")
            .and_then(Value::as_object)
            .ok_or_else(|| "runtime handshake omitted executionProfile.continuation".to_owned())?;
        require_u64(
            profile_continuation.get("schemaVersion"),
            1,
            "executionProfile.continuation.schemaVersion",
        )?;
        require_str(
            profile_continuation.get("mode"),
            self.continuation_mode,
            "executionProfile.continuation.mode",
        )?;
        require_bool(
            profile_continuation.get("proposalOnly"),
            self.continuation_mode != "disabled",
            "executionProfile.continuation.proposalOnly",
        )?;
        require_bool(
            profile_continuation.get("hostValidationRequired"),
            self.continuation_mode != "disabled",
            "executionProfile.continuation.hostValidationRequired",
        )?;
        let graph = profile
            .get("graph")
            .and_then(Value::as_object)
            .ok_or_else(|| "runtime handshake omitted executionProfile.graph".to_owned())?;
        require_str(
            graph.get("mode"),
            self.graph_mode,
            "executionProfile.graph.mode",
        )?;
        require_u64(
            graph.get("maxNodes"),
            self.graph_max_nodes,
            "executionProfile.graph.maxNodes",
        )?;
        require_u64(
            graph.get("maxDepth"),
            self.graph_max_depth,
            "executionProfile.graph.maxDepth",
        )?;
        require_bool(
            graph.get("writersAllowed"),
            self.graph_writers_allowed,
            "executionProfile.graph.writersAllowed",
        )?;
        let strategies = profile
            .get("strategies")
            .and_then(Value::as_object)
            .ok_or_else(|| "runtime handshake omitted executionProfile.strategies".to_owned())?;
        require_str(
            strategies.get("completionAudit"),
            self.completion_audit,
            "executionProfile.strategies.completionAudit",
        )?;
        require_str(
            strategies.get("validationPolicy"),
            self.validation_policy,
            "executionProfile.strategies.validationPolicy",
        )?;
        require_str(
            strategies.get("promptPolicy"),
            self.prompt_policy,
            "executionProfile.strategies.promptPolicy",
        )?;

        let contract = payload
            .get("continuationDecisionContract")
            .and_then(Value::as_object)
            .ok_or_else(|| "runtime handshake omitted continuationDecisionContract".to_owned())?;
        require_u64(
            contract.get("schemaVersion"),
            1,
            "continuationDecisionContract.schemaVersion",
        )?;
        require_str(
            contract.get("mode"),
            self.continuation_mode,
            "continuationDecisionContract.mode",
        )?;
        require_bool(
            contract.get("enabled"),
            self.continuation_mode != "disabled",
            "continuationDecisionContract.enabled",
        )?;
        require_bool(
            contract.get("proposalOnly"),
            true,
            "continuationDecisionContract.proposalOnly",
        )?;
        require_bool(
            contract.get("hostValidationRequired"),
            true,
            "continuationDecisionContract.hostValidationRequired",
        )?;
        require_bool(
            contract.get("runtimeMayPersistOrApply"),
            false,
            "continuationDecisionContract.runtimeMayPersistOrApply",
        )?;
        require_bool(
            contract.get("shadow"),
            self.shadow,
            "continuationDecisionContract.shadow",
        )?;
        require_str(
            contract.get("toolName"),
            CONTINUATION_PROPOSAL_TOOL_NAME,
            "continuationDecisionContract.toolName",
        )?;
        require_str(
            contract.get("proposalEventType"),
            CONTINUATION_PROPOSAL_EVENT_TYPE,
            "continuationDecisionContract.proposalEventType",
        )?;
        require_bool(
            contract.get("shadowSideEffectsAllowed"),
            false,
            "continuationDecisionContract.shadowSideEffectsAllowed",
        )?;
        require_string_array(
            contract.get("decisions"),
            CONTINUATION_DECISIONS,
            "continuationDecisionContract.decisions",
        )?;
        require_string_array(
            contract.get("reasonCodes"),
            CONTINUATION_REASON_CODES,
            "continuationDecisionContract.reasonCodes",
        )?;
        require_string_array(
            contract.get("retryClasses"),
            CONTINUATION_RETRY_CLASSES,
            "continuationDecisionContract.retryClasses",
        )?;
        Ok(())
    }
}

fn require_str(value: Option<&Value>, expected: &str, field: &str) -> Result<(), String> {
    let actual = value.and_then(Value::as_str).unwrap_or_default();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "runtime handshake {field} mismatch: expected '{expected}', got '{actual}'"
        ))
    }
}

fn require_u64(value: Option<&Value>, expected: u64, field: &str) -> Result<(), String> {
    let actual = value.and_then(Value::as_u64);
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "runtime handshake {field} mismatch: expected {expected}, got {actual:?}"
        ))
    }
}

fn require_bool(value: Option<&Value>, expected: bool, field: &str) -> Result<(), String> {
    let actual = value.and_then(Value::as_bool);
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "runtime handshake {field} mismatch: expected {expected}, got {actual:?}"
        ))
    }
}

fn require_string_array(
    value: Option<&Value>,
    expected: &[&str],
    field: &str,
) -> Result<(), String> {
    let actual = value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    if actual.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "runtime handshake {field} mismatch: expected {expected:?}, got {actual:?}"
        ))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeContinuationProposal {
    schema_version: u32,
    decision_id: String,
    run_id: String,
    event_cursor: i64,
    decision: ContinuationDecisionKind,
    reason_code: ContinuationReasonCode,
    #[serde(default)]
    active_task_ids: Vec<String>,
    #[serde(default)]
    evidence_ids: Vec<String>,
    #[serde(default)]
    missing_acceptance: Vec<String>,
    next_action: String,
    retry_class: ContinuationRetryClass,
    #[serde(default)]
    blocked_dependency_refs: Vec<String>,
    proposal_kind: String,
    proposal_only: bool,
    host_validation_required: bool,
    shadow: bool,
    side_effects_applied: bool,
    execution_profile_id: String,
}

#[derive(Debug, Default)]
pub struct ContinuationReconciliationReport {
    pub scanned: usize,
    pub decisions: Vec<ContinuationDecisionRecord>,
    pub diagnostics: Vec<ContinuationReconciliationDiagnostic>,
}

#[derive(Debug, Default)]
pub struct ContinuationReconciliationSweep {
    pub report: ContinuationReconciliationReport,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationReconciliationDiagnostic {
    pub run_id: String,
    pub seq: i64,
    pub execution_profile_id: String,
    pub shadow: bool,
    pub code: String,
    pub error: String,
}

pub fn reconcile_unprojected_proposals(
    database: &Database,
    limit: usize,
) -> Result<ContinuationReconciliationReport, String> {
    let proposals = database
        .unprojected_continuation_proposals(limit)
        .map_err(|error| error.to_string())?;
    let mut report = ContinuationReconciliationReport {
        scanned: proposals.len(),
        ..ContinuationReconciliationReport::default()
    };
    for event in proposals {
        let profile_id = event
            .execution_profile_id
            .clone()
            .unwrap_or_else(|| "<missing>".to_owned());
        let claimed_profile_id = proposal_execution_profile_id(&event.payload);
        let reconciled = if event.execution_profile_id.is_none() {
            Err("continuation proposal has no authoritative execution profile in its prior event chain".to_owned())
        } else if claimed_profile_id.as_deref() != Some(profile_id.as_str()) {
            Err(format!(
                "continuation proposal execution profile claim {:?} does not match authoritative profile '{}'",
                claimed_profile_id, profile_id
            ))
        } else {
            ExecutionProfileSelection::resolve(&profile_id).and_then(|profile| {
                persist_runtime_proposal(
                    database,
                    &event.run_id,
                    event.seq,
                    &profile,
                    &event.payload,
                )
            })
        };
        match reconciled {
            Ok(decision) => report.decisions.push(decision),
            Err(error) => {
                let shadow = profile_id == "durable_v2_shadow";
                let code = "runtime.continuation.reconciliation_failed";
                let error = match record_ingest_diagnostic(
                    database,
                    &event.run_id,
                    event.seq,
                    event.execution_profile_id.as_deref(),
                    shadow,
                    code,
                    &error,
                ) {
                    Ok(()) => error,
                    Err(diagnostic_error) => format!(
                        "{error}; continuation ingest diagnostic persistence failed: {diagnostic_error}"
                    ),
                };
                report
                    .diagnostics
                    .push(ContinuationReconciliationDiagnostic {
                        run_id: event.run_id,
                        seq: event.seq,
                        execution_profile_id: profile_id.clone(),
                        shadow,
                        code: code.to_owned(),
                        error,
                    });
            }
        }
    }
    Ok(report)
}

pub fn reconcile_unprojected_proposals_until_budget(
    database: &Database,
    batch_limit: usize,
    max_items: usize,
    max_duration: Duration,
) -> Result<ContinuationReconciliationSweep, String> {
    if batch_limit == 0 || max_items == 0 || max_duration.is_zero() {
        return Err("continuation reconciliation budgets must be positive".to_owned());
    }
    let started_at = Instant::now();
    let mut sweep = ContinuationReconciliationSweep::default();
    loop {
        let remaining = max_items.saturating_sub(sweep.report.scanned);
        if remaining == 0 {
            sweep.has_more = true;
            return Ok(sweep);
        }
        let requested = batch_limit.min(remaining);
        let batch = reconcile_unprojected_proposals(database, requested)?;
        let scanned = batch.scanned;
        sweep.report.scanned += scanned;
        sweep.report.decisions.extend(batch.decisions);
        sweep.report.diagnostics.extend(batch.diagnostics);
        if scanned < requested {
            return Ok(sweep);
        }
        if sweep.report.scanned >= max_items || started_at.elapsed() >= max_duration {
            sweep.has_more = true;
            return Ok(sweep);
        }
    }
}

pub fn record_ingest_diagnostic(
    database: &Database,
    run_id: &str,
    event_seq: i64,
    execution_profile_id: Option<&str>,
    shadow_mode: bool,
    error_code: &str,
    error_message: &str,
) -> Result<(), String> {
    let error_code = truncate_chars(error_code.trim(), MAX_DIAGNOSTIC_CODE_CHARS);
    let error_code = if error_code.is_empty() {
        "runtime.continuation.ingest_failed".to_owned()
    } else {
        error_code
    };
    let error_message = truncate_chars(error_message.trim(), MAX_DIAGNOSTIC_MESSAGE_CHARS);
    let error_message = if error_message.is_empty() {
        "continuation proposal rejected".to_owned()
    } else {
        error_message
    };
    database
        .record_continuation_ingest_diagnostic(&RecordContinuationIngestDiagnosticInput {
            run_id: run_id.to_owned(),
            event_seq,
            execution_profile_id: execution_profile_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| truncate_chars(value, MAX_IDENTIFIER_CHARS)),
            shadow_mode,
            error_code,
            error_message,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn proposal_execution_profile_id(payload: &Value) -> Option<String> {
    payload
        .get("proposal")
        .and_then(|proposal| proposal.get("executionProfileId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn persist_runtime_proposal(
    database: &Database,
    envelope_run_id: &str,
    proposal_event_seq: i64,
    profile: &ExecutionProfileSelection,
    payload: &Value,
) -> Result<ContinuationDecisionRecord, String> {
    let proposal_value = payload
        .get("proposal")
        .cloned()
        .ok_or_else(|| "continuation proposal event omitted proposal".to_owned())?;
    let proposal: RuntimeContinuationProposal = serde_json::from_value(proposal_value)
        .map_err(|error| format!("invalid ContinuationDecision proposal: {error}"))?;
    validate_proposal_metadata(&proposal, envelope_run_id, proposal_event_seq, profile)?;
    if let Some(existing) = database
        .continuation_decision_by_id(envelope_run_id, &proposal.decision_id)
        .map_err(|error| error.to_string())?
    {
        if persisted_decision_matches_proposal(&existing, &proposal) {
            return Ok(existing);
        }
        return Err(format!(
            "continuation decision '{}' conflicts with the persisted proposal core",
            proposal.decision_id
        ));
    }
    let conversation_id = database
        .authoritative_run_conversation_id(envelope_run_id)
        .map_err(|error| error.to_string())?;

    for attempt in 0..=MAX_PROJECTION_RETRIES {
        let ledger = database
            .task_ledger_projection(&conversation_id)
            .map_err(|error| error.to_string())?;
        let semantic_error = validate_semantics(&proposal, &ledger, profile.id == "legacy");
        let validation_error = if profile.shadow {
            Some(match semantic_error {
                Some(error) => format!("runtime.continuation.shadow_invalid:{error}"),
                None => "runtime.continuation.shadow_valid:observation_only".to_owned(),
            })
        } else {
            semantic_error
        };
        let host_validation_outcome = if validation_error.is_some() {
            ContinuationValidationOutcome::Rejected
        } else {
            ContinuationValidationOutcome::Accepted
        };
        let input = AppendContinuationDecisionInput {
            schema_version: proposal.schema_version,
            decision_id: proposal.decision_id.clone(),
            run_id: proposal.run_id.clone(),
            event_cursor: proposal.event_cursor,
            decision: proposal.decision.clone(),
            reason_code: proposal.reason_code.clone(),
            active_task_ids: proposal.active_task_ids.clone(),
            evidence_ids: proposal.evidence_ids.clone(),
            missing_acceptance: proposal.missing_acceptance.clone(),
            next_action: (!proposal.next_action.trim().is_empty())
                .then_some(proposal.next_action.clone()),
            retry_class: Some(proposal.retry_class.clone()),
            blocked_dependency_refs: proposal.blocked_dependency_refs.clone(),
            host_validation_outcome,
            host_validation_error: validation_error,
            expected_projection_hash: Some(ledger.projection_meta.projection_hash),
        };
        match database.append_continuation_decision(input) {
            Ok(record) => return Ok(record),
            Err(error)
                if attempt < MAX_PROJECTION_RETRIES
                    && error
                        .to_string()
                        .starts_with(PROJECTION_HASH_MISMATCH_PREFIX) =>
            {
                continue;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("bounded projection retry loop must return")
}

fn persisted_decision_matches_proposal(
    existing: &ContinuationDecisionRecord,
    proposal: &RuntimeContinuationProposal,
) -> bool {
    existing.schema_version == proposal.schema_version
        && existing.decision_id == proposal.decision_id
        && existing.run_id == proposal.run_id
        && existing.event_cursor == proposal.event_cursor
        && existing.decision == proposal.decision
        && existing.reason_code == proposal.reason_code
        && existing.active_task_ids == proposal.active_task_ids
        && existing.evidence_ids == proposal.evidence_ids
        && existing.missing_acceptance == proposal.missing_acceptance
        && existing.next_action.as_deref()
            == (!proposal.next_action.trim().is_empty()).then_some(proposal.next_action.as_str())
        && existing.retry_class.as_ref() == Some(&proposal.retry_class)
        && existing.blocked_dependency_refs == proposal.blocked_dependency_refs
}

fn validate_proposal_metadata(
    proposal: &RuntimeContinuationProposal,
    envelope_run_id: &str,
    proposal_event_seq: i64,
    profile: &ExecutionProfileSelection,
) -> Result<(), String> {
    if proposal.schema_version != 1 {
        return Err(format!(
            "unsupported ContinuationDecision schema version {}",
            proposal.schema_version
        ));
    }
    validate_bounded_identifier("decisionId", &proposal.decision_id)?;
    validate_bounded_identifier("runId", &proposal.run_id)?;
    validate_identifier_list("activeTaskIds", &proposal.active_task_ids)?;
    validate_identifier_list("evidenceIds", &proposal.evidence_ids)?;
    validate_identifier_list("missingAcceptance", &proposal.missing_acceptance)?;
    validate_identifier_list("blockedDependencyRefs", &proposal.blocked_dependency_refs)?;
    if proposal.next_action.chars().count() > MAX_NEXT_ACTION_CHARS {
        return Err(format!(
            "ContinuationDecision nextAction exceeds {MAX_NEXT_ACTION_CHARS} characters"
        ));
    }
    if proposal.decision_id.trim().is_empty() || proposal.run_id != envelope_run_id {
        return Err("continuation proposal identity does not match the active run".to_owned());
    }
    if proposal_event_seq <= 0 || proposal.event_cursor != proposal_event_seq - 1 {
        return Err(format!(
            "continuation proposal event cursor mismatch: expected {}, got {}",
            proposal_event_seq.saturating_sub(1),
            proposal.event_cursor
        ));
    }
    if proposal.proposal_kind != "runtime_continuation_decision"
        || !proposal.proposal_only
        || !proposal.host_validation_required
        || proposal.side_effects_applied
    {
        return Err(
            "continuation proposal metadata violates the proposal-only contract".to_owned(),
        );
    }
    if profile.continuation_mode == "disabled" {
        return Err("legacy execution profile cannot emit ContinuationDecision".to_owned());
    }
    if proposal.execution_profile_id != profile.id || proposal.shadow != profile.shadow {
        return Err("continuation proposal execution profile does not match Host state".to_owned());
    }
    Ok(())
}

fn validate_bounded_identifier(field: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if value.trim().is_empty() || length > MAX_IDENTIFIER_CHARS {
        return Err(format!(
            "ContinuationDecision {field} must contain 1..={MAX_IDENTIFIER_CHARS} characters"
        ));
    }
    Ok(())
}

fn validate_identifier_list(field: &str, values: &[String]) -> Result<(), String> {
    if values.len() > MAX_IDENTIFIER_ITEMS {
        return Err(format!(
            "ContinuationDecision {field} exceeds {MAX_IDENTIFIER_ITEMS} items"
        ));
    }
    let mut unique = HashSet::new();
    for value in values {
        validate_bounded_identifier(field, value)?;
        if !unique.insert(value.as_str()) {
            return Err(format!(
                "ContinuationDecision {field} contains duplicate value '{value}'"
            ));
        }
    }
    Ok(())
}

fn validate_semantics(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
    allow_legacy_skipped: bool,
) -> Option<String> {
    if !decision_reason_allowed(&proposal.decision, &proposal.reason_code) {
        return Some("runtime.continuation.reason_mismatch".to_owned());
    }
    if !retry_class_allowed(&proposal.decision, &proposal.retry_class) {
        return Some("runtime.continuation.retry_class_mismatch".to_owned());
    }
    let active_task_ids = ledger
        .active_tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .collect::<HashSet<_>>();
    if proposal
        .active_task_ids
        .iter()
        .any(|task_id| !active_task_ids.contains(task_id.as_str()))
    {
        return Some("runtime.continuation.active_task_unknown".to_owned());
    }
    let evidence_ids = ledger
        .active_tasks
        .iter()
        .flat_map(|task| {
            task.latest_evidence
                .iter()
                .filter(|evidence| {
                    matches!(evidence.validity_status, EvidenceValidityStatus::Valid)
                })
                .map(|evidence| evidence.id.as_str())
        })
        .chain(
            ledger
                .completed_summary
                .iter()
                .flat_map(|task| task.evidence_ids.iter().map(String::as_str)),
        )
        .collect::<HashSet<_>>();
    if proposal
        .evidence_ids
        .iter()
        .any(|evidence_id| !evidence_ids.contains(evidence_id.as_str()))
    {
        return Some("runtime.continuation.evidence_unknown".to_owned());
    }
    if matches!(
        proposal.reason_code,
        ContinuationReasonCode::AcceptanceMissing
    ) && proposal.missing_acceptance.is_empty()
    {
        return Some("runtime.continuation.missing_acceptance_required".to_owned());
    }

    match proposal.decision {
        ContinuationDecisionKind::Continue => validate_continue(proposal, ledger),
        ContinuationDecisionKind::Repair => validate_repair(proposal, ledger),
        ContinuationDecisionKind::WaitApproval => validate_wait_approval(proposal, ledger),
        ContinuationDecisionKind::Complete => {
            validate_complete(proposal, ledger, allow_legacy_skipped)
        }
        ContinuationDecisionKind::Blocked => validate_blocked(proposal, ledger),
    }
}

fn validate_continue(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
) -> Option<String> {
    if ledger.active_tasks.is_empty() && proposal.next_action.trim().is_empty() {
        return Some("runtime.continuation.next_action_required".to_owned());
    }
    None
}

fn validate_repair(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
) -> Option<String> {
    let has_review_or_repair = ledger.pending_actions.iter().any(|action| {
        matches!(
            action.kind,
            TaskLedgerPendingActionKind::Review | TaskLedgerPendingActionKind::Repair
        )
    });
    if !has_review_or_repair && proposal.missing_acceptance.is_empty() {
        return Some("runtime.continuation.repair_fact_missing".to_owned());
    }
    if proposal.next_action.trim().is_empty() {
        return Some("runtime.continuation.next_action_required".to_owned());
    }
    None
}

fn validate_wait_approval(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
) -> Option<String> {
    let approval_ids = ledger
        .pending_actions
        .iter()
        .filter(|action| {
            matches!(action.kind, TaskLedgerPendingActionKind::Approval)
                && action.run_id.as_deref() == Some(proposal.run_id.as_str())
                && action.tool_call_id.is_some()
        })
        .map(|action| action.reference_id.as_str())
        .collect::<HashSet<_>>();
    if approval_ids.is_empty() {
        return Some("runtime.continuation.approval_not_pending".to_owned());
    }
    if proposal.blocked_dependency_refs.is_empty() {
        return Some("runtime.continuation.approval_reference_required".to_owned());
    }
    if proposal
        .blocked_dependency_refs
        .iter()
        .any(|reference| !approval_ids.contains(reference.as_str()))
    {
        return Some("runtime.continuation.approval_reference_unknown".to_owned());
    }
    None
}

fn validate_complete(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
    allow_legacy_skipped: bool,
) -> Option<String> {
    if !proposal.active_task_ids.is_empty() || !ledger.active_tasks.is_empty() {
        return Some("runtime.continuation.active_work_remaining".to_owned());
    }
    if !proposal.missing_acceptance.is_empty() {
        return Some("runtime.continuation.acceptance_missing".to_owned());
    }
    if !ledger.pending_actions.is_empty() {
        return Some("runtime.continuation.pending_action_remaining".to_owned());
    }
    if !allow_legacy_skipped
        && ledger
            .completed_summary
            .iter()
            .any(|task| matches!(task.outcome, TaskLedgerCompletedOutcome::Skipped))
    {
        return Some("runtime.continuation.skipped_task_not_verified".to_owned());
    }
    if let Some(goal) = ledger.goal.as_ref() {
        if !matches!(goal.status, GoalStatus::Completed) {
            return Some("runtime.continuation.goal_not_completed".to_owned());
        }
        if ledger
            .completed_summary
            .iter()
            .any(|task| matches!(task.outcome, TaskLedgerCompletedOutcome::Completed))
        {
            return Some("runtime.continuation.acceptance_not_persisted".to_owned());
        }
        let accepted_work_exists = ledger
            .completed_summary
            .iter()
            .any(|task| matches!(task.outcome, TaskLedgerCompletedOutcome::Accepted));
        if accepted_work_exists && proposal.evidence_ids.is_empty() {
            return Some("runtime.continuation.evidence_claim_required".to_owned());
        }
    } else if matches!(
        proposal.reason_code,
        ContinuationReasonCode::AcceptancePassed
    ) {
        return Some("runtime.continuation.acceptance_fact_missing".to_owned());
    }
    None
}

fn validate_blocked(
    proposal: &RuntimeContinuationProposal,
    ledger: &TaskLedgerProjection,
) -> Option<String> {
    let goal_blocked = ledger
        .goal
        .as_ref()
        .is_some_and(|goal| matches!(goal.status, GoalStatus::Blocked));
    let task_blocked = ledger
        .active_tasks
        .iter()
        .any(|task| matches!(task.status, WorkTaskStatus::Blocked));
    let external_wait = ledger
        .pending_actions
        .iter()
        .any(|action| matches!(action.kind, TaskLedgerPendingActionKind::ExternalWait));
    if !goal_blocked && !task_blocked && !external_wait {
        return Some("runtime.continuation.blocked_fact_missing".to_owned());
    }
    if matches!(
        proposal.reason_code,
        ContinuationReasonCode::ExternalDependencyUnavailable
    ) && proposal.blocked_dependency_refs.is_empty()
    {
        return Some("runtime.continuation.blocked_dependency_required".to_owned());
    }
    None
}

fn decision_reason_allowed(
    decision: &ContinuationDecisionKind,
    reason: &ContinuationReasonCode,
) -> bool {
    match decision {
        ContinuationDecisionKind::Continue => matches!(
            reason,
            ContinuationReasonCode::WorkRemaining
                | ContinuationReasonCode::RetryAvailable
                | ContinuationReasonCode::AcceptanceMissing
                | ContinuationReasonCode::HostAuditRequired
                | ContinuationReasonCode::ToolFailed
                | ContinuationReasonCode::TaskInterrupted
        ),
        ContinuationDecisionKind::Repair => matches!(
            reason,
            ContinuationReasonCode::ValidationFailed
                | ContinuationReasonCode::AcceptanceMissing
                | ContinuationReasonCode::RetryAvailable
                | ContinuationReasonCode::ToolFailed
        ),
        ContinuationDecisionKind::WaitApproval => {
            matches!(reason, ContinuationReasonCode::ApprovalPending)
        }
        ContinuationDecisionKind::Complete => matches!(
            reason,
            ContinuationReasonCode::AcceptanceCandidate
                | ContinuationReasonCode::AcceptancePassed
                | ContinuationReasonCode::HostAuditRequired
        ),
        ContinuationDecisionKind::Blocked => matches!(
            reason,
            ContinuationReasonCode::ExternalDependencyUnavailable
                | ContinuationReasonCode::BudgetExhausted
                | ContinuationReasonCode::UserInputRequired
                | ContinuationReasonCode::RetryExhausted
                | ContinuationReasonCode::ToolFailed
                | ContinuationReasonCode::TaskInterrupted
        ),
    }
}

fn retry_class_allowed(
    decision: &ContinuationDecisionKind,
    retry: &ContinuationRetryClass,
) -> bool {
    match decision {
        ContinuationDecisionKind::Continue | ContinuationDecisionKind::Repair => !matches!(
            retry,
            ContinuationRetryClass::None | ContinuationRetryClass::NonRetryable
        ),
        ContinuationDecisionKind::WaitApproval => matches!(
            retry,
            ContinuationRetryClass::None | ContinuationRetryClass::HostDecides
        ),
        ContinuationDecisionKind::Complete => matches!(retry, ContinuationRetryClass::None),
        ContinuationDecisionKind::Blocked => matches!(
            retry,
            ContinuationRetryClass::None
                | ContinuationRetryClass::NonRetryable
                | ContinuationRetryClass::HostDecides
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        persist_runtime_proposal, reconcile_unprojected_proposals,
        reconcile_unprojected_proposals_until_budget, record_ingest_diagnostic,
        validate_proposal_metadata, validate_semantics, ExecutionProfileSelection,
        RuntimeContinuationProposal,
    };
    use crate::database::{
        Database, GoalStatus, RecordContinuationIngestDiagnosticInput, TaskLedgerCompletedOutcome,
        TaskLedgerCompletedSummary, TaskLedgerGoalProjection, TaskLedgerProjection,
    };
    use crate::runtime_host::protocol::RuntimeCapabilityManifest;
    use serde_json::json;
    use std::time::Duration;
    use uuid::Uuid;

    fn continuation_payload(profile_id: &str) -> serde_json::Value {
        json!({
            "type": "run.continuation_proposed",
            "proposal": {
                "schemaVersion": 1,
                "decisionId": "decision-1",
                "runId": "placeholder",
                "eventCursor": 1,
                "decision": "continue",
                "reasonCode": "work_remaining",
                "activeTaskIds": [],
                "evidenceIds": [],
                "missingAcceptance": [],
                "nextAction": "continue",
                "retryClass": "recoverable",
                "blockedDependencyRefs": [],
                "proposalKind": "runtime_continuation_decision",
                "proposalOnly": true,
                "hostValidationRequired": true,
                "shadow": profile_id == "durable_v2_shadow",
                "sideEffectsApplied": false,
                "executionProfileId": profile_id
            }
        })
    }

    fn ledger_with(
        active_tasks: serde_json::Value,
        pending_actions: serde_json::Value,
    ) -> TaskLedgerProjection {
        serde_json::from_value(json!({
            "schemaVersion": 1,
            "conversationId": "conversation-1",
            "goal": null,
            "activeTasks": active_tasks,
            "pendingActions": pending_actions,
            "completedSummary": [],
            "executionCursor": {
                "currentPhase": "execute",
                "runId": "run-new",
                "eventCursor": 1,
                "lastEventId": null,
                "resumableFrom": null
            },
            "projectionMeta": {
                "schemaVersion": 1,
                "projectionHash": "hash-1",
                "sourceHighWatermark": {
                    "runCursors": [{ "runId": "run-new", "eventCursor": 1 }],
                    "workUpdatedAt": null,
                    "acceptanceCreatedAt": null
                },
                "generatedAt": 1,
                "derivedGoalIds": [],
                "derivedTaskIds": [],
                "derivedRunIds": ["run-new"],
                "eventRange": null
            },
            "latestHostAcceptedDecision": null
        }))
        .unwrap()
    }

    #[test]
    fn execution_profiles_are_finite_and_reject_mixed_ready_contracts() {
        let profile = ExecutionProfileSelection::resolve("durable_v2").unwrap();
        assert!(profile.allows_host_tool("write_file"));
        assert!(profile.allows_host_tool("task_repair_escalate_start"));
        assert!(profile.allows_host_tool("graph_readonly_activate"));
        assert!(profile.allows_host_tool("graph_readonly_snapshot_get"));
        assert!(profile.allows_host_tool("graph_readonly_node_start"));
        assert!(profile.allows_host_tool("workflow_snapshot_get"));
        assert!(!profile.allows_host_tool("workflow_start"));
        assert!(ExecutionProfileSelection::resolve("legacy")
            .unwrap()
            .allows_host_tool("workflow_start"));
        assert!(!ExecutionProfileSelection::resolve("legacy")
            .unwrap()
            .allows_host_tool("task_repair_escalate_start"));
        assert!(ExecutionProfileSelection::resolve("durable_v2_shadow")
            .unwrap()
            .allows_host_tool("read"));
        assert!(!ExecutionProfileSelection::resolve("durable_v2_shadow")
            .unwrap()
            .allows_host_tool("write_file"));
        assert!(!ExecutionProfileSelection::resolve("durable_v2_shadow")
            .unwrap()
            .allows_host_tool("task_repair_escalate_start"));
        for profile_id in ["legacy", "durable_v2_shadow", "durable_v2"] {
            assert!(!ExecutionProfileSelection::resolve(profile_id)
                .unwrap()
                .allows_host_tool("graph_readonly_run"));
        }
        assert!(ExecutionProfileSelection::resolve("graph_readonly_preview")
            .unwrap()
            .allows_host_tool("graph_readonly_run"));
        let reviewer = ExecutionProfileSelection::resolve("graph_reviewer_v1").unwrap();
        for tool in ["read", "ls", "find", "grep"] {
            assert!(reviewer.allows_host_tool(tool));
        }
        for tool in [
            "write_file",
            "test_run",
            "continuation_propose",
            "child_run_start",
            "graph_readonly_node_review",
            "graph_readonly_accept",
        ] {
            assert!(!reviewer.allows_host_tool(tool));
        }
        for profile_id in [
            "legacy",
            "durable_v2_shadow",
            "graph_readonly_preview",
            "graph_reviewer_v1",
        ] {
            let profile = ExecutionProfileSelection::resolve(profile_id).unwrap();
            assert!(!profile.allows_host_tool("graph_readonly_activate"));
            assert!(!profile.allows_host_tool("graph_readonly_snapshot_get"));
            assert!(!profile.allows_host_tool("graph_readonly_node_start"));
        }
        assert!(ExecutionProfileSelection::resolve("invented").is_err());

        let valid = json!({
            "executionProfile": {
                "schemaVersion": 1,
                "id": "durable_v2",
                "strategies": {
                    "completionAudit": "strict_v2",
                    "validationPolicy": "risk_v1",
                    "promptPolicy": "stable_v1"
                },
                "toolPolicy": "host_guarded",
                "shadow": false,
                "sideEffectsAllowed": true,
                "continuation": {
                    "schemaVersion": 1,
                    "mode": "propose",
                    "proposalOnly": true,
                    "hostValidationRequired": true
                },
                "graph": {
                    "mode": "disabled",
                    "maxNodes": 0,
                    "maxDepth": 0,
                    "writersAllowed": false
                }
            },
            "continuationDecisionContract": {
                "schemaVersion": 1,
                "enabled": true,
                "mode": "propose",
                "proposalOnly": true,
                "hostValidationRequired": true,
                "runtimeMayPersistOrApply": false,
                "shadow": false,
                "shadowSideEffectsAllowed": false,
                "toolName": "continuation_propose",
                "proposalEventType": "run.continuation_proposed",
                "decisions": ["continue", "repair", "wait_approval", "complete", "blocked"],
                "reasonCodes": [
                    "work_remaining", "validation_failed", "approval_pending",
                    "external_dependency_unavailable", "budget_exhausted",
                    "user_input_required", "retry_available", "retry_exhausted",
                    "acceptance_missing", "acceptance_candidate", "acceptance_passed",
                    "host_audit_required", "tool_failed", "task_interrupted"
                ],
                "retryClasses": [
                    "none", "recoverable", "retry_limited", "non_retryable", "host_decides"
                ]
            }
        });
        profile.validate_ready_payload(&valid).unwrap();
        let mut invalid = valid.clone();
        invalid["executionProfile"]["strategies"]["validationPolicy"] = json!("legacy");
        assert!(profile.validate_ready_payload(&invalid).is_err());

        let mut graph_mismatch = valid.clone();
        graph_mismatch["executionProfile"]["graph"]["writersAllowed"] = json!(true);
        assert!(profile.validate_ready_payload(&graph_mismatch).is_err());

        let mut reviewer_ready = valid.clone();
        reviewer_ready["executionProfile"]["id"] = json!("graph_reviewer_v1");
        reviewer_ready["executionProfile"]["strategies"]["validationPolicy"] =
            json!("high_risk_v1");
        reviewer_ready["executionProfile"]["strategies"]["promptPolicy"] =
            json!("graph_reviewer_v1");
        reviewer_ready["executionProfile"]["toolPolicy"] = json!("graph_reviewer_read_only");
        reviewer_ready["executionProfile"]["sideEffectsAllowed"] = json!(false);
        reviewer_ready["executionProfile"]["continuation"]["mode"] = json!("disabled");
        reviewer_ready["executionProfile"]["continuation"]["proposalOnly"] = json!(false);
        reviewer_ready["executionProfile"]["continuation"]["hostValidationRequired"] = json!(false);
        reviewer_ready["continuationDecisionContract"]["enabled"] = json!(false);
        reviewer_ready["continuationDecisionContract"]["mode"] = json!("disabled");
        reviewer
            .validate_ready_payload(&reviewer_ready)
            .expect("Rust and Runtime must freeze the same graph_reviewer_v1 snapshot");
        let mut reviewer_profile_spoof = reviewer_ready;
        reviewer_profile_spoof["executionProfile"]["toolPolicy"] = json!("host_guarded");
        assert!(reviewer
            .validate_ready_payload(&reviewer_profile_spoof)
            .is_err());

        let mut enum_mismatch = valid;
        enum_mismatch["continuationDecisionContract"]["reasonCodes"] = json!(["work_remaining"]);
        assert!(profile.validate_ready_payload(&enum_mismatch).is_err());
    }

    #[test]
    fn shadow_ready_accepts_internal_proposal_tool_but_rejects_write_capabilities() {
        let profile = ExecutionProfileSelection::resolve("durable_v2_shadow").unwrap();
        profile
            .validate_ready_payload(&json!({
                "executionProfile": {
                    "schemaVersion": 1,
                    "id": "durable_v2_shadow",
                    "strategies": {
                        "completionAudit": "strict_v2",
                        "validationPolicy": "risk_v1",
                        "promptPolicy": "stable_v1"
                    },
                    "toolPolicy": "read_only_shadow",
                    "shadow": true,
                    "sideEffectsAllowed": false,
                    "continuation": {
                        "schemaVersion": 1,
                        "mode": "shadow",
                        "proposalOnly": true,
                        "hostValidationRequired": true
                    },
                    "graph": {
                        "mode": "disabled",
                        "maxNodes": 0,
                        "maxDepth": 0,
                        "writersAllowed": false
                    }
                },
                "continuationDecisionContract": {
                    "schemaVersion": 1,
                    "enabled": true,
                    "mode": "shadow",
                    "toolName": "continuation_propose",
                    "proposalEventType": "run.continuation_proposed",
                    "decisions": ["continue", "repair", "wait_approval", "complete", "blocked"],
                    "reasonCodes": [
                        "work_remaining", "validation_failed", "approval_pending",
                        "external_dependency_unavailable", "budget_exhausted",
                        "user_input_required", "retry_available", "retry_exhausted",
                        "acceptance_missing", "acceptance_candidate", "acceptance_passed",
                        "host_audit_required", "tool_failed", "task_interrupted"
                    ],
                    "retryClasses": [
                        "none", "recoverable", "retry_limited", "non_retryable", "host_decides"
                    ],
                    "proposalOnly": true,
                    "hostValidationRequired": true,
                    "runtimeMayPersistOrApply": false,
                    "shadow": true,
                    "shadowSideEffectsAllowed": false
                }
            }))
            .unwrap();
        let manifest = |tool: &str, execution: &str| {
            serde_json::from_value::<RuntimeCapabilityManifest>(json!({
                "manifestVersion": 2,
                "streamingText": true,
                "cancellation": true,
                "reasoning": true,
                "sessionResume": true,
                "toolApproval": true,
                "imageInput": false,
                "steering": true,
                "contextCompaction": true,
                "dynamicModelSwitch": true,
                "workLoop": true,
                "tools": [{
                    "name": tool,
                    "category": "work",
                    "execution": execution,
                    "approval": "none"
                }]
            }))
            .unwrap()
        };
        profile
            .validate_capability_manifest(&manifest("continuation_propose", "runtime"))
            .unwrap();
        assert!(profile
            .validate_capability_manifest(&manifest("write_file", "host"))
            .is_err());
        assert!(profile
            .validate_capability_manifest(&manifest("git_read", "host"))
            .is_err());
    }

    #[test]
    fn proposal_schema_limits_and_shadow_error_policy_are_host_enforced() {
        let profile = ExecutionProfileSelection::resolve("durable_v2").unwrap();
        let candidate = json!({
            "schemaVersion": 1,
            "decisionId": "decision-1",
            "runId": "run-1",
            "eventCursor": 1,
            "decision": "continue",
            "reasonCode": "work_remaining",
            "activeTaskIds": [],
            "evidenceIds": [],
            "missingAcceptance": [],
            "nextAction": "continue",
            "retryClass": "recoverable",
            "blockedDependencyRefs": [],
            "proposalKind": "runtime_continuation_decision",
            "proposalOnly": true,
            "hostValidationRequired": true,
            "shadow": false,
            "sideEffectsApplied": false,
            "executionProfileId": "durable_v2"
        });
        let proposal: RuntimeContinuationProposal =
            serde_json::from_value(candidate.clone()).unwrap();
        validate_proposal_metadata(&proposal, "run-1", 2, &profile).unwrap();

        let mut oversized_id = candidate.clone();
        oversized_id["decisionId"] = json!("x".repeat(201));
        let proposal: RuntimeContinuationProposal = serde_json::from_value(oversized_id).unwrap();
        assert!(validate_proposal_metadata(&proposal, "run-1", 2, &profile).is_err());

        let mut oversized_array = candidate.clone();
        oversized_array["activeTaskIds"] = json!((0..101)
            .map(|index| format!("task-{index}"))
            .collect::<Vec<_>>());
        let proposal: RuntimeContinuationProposal =
            serde_json::from_value(oversized_array).unwrap();
        assert!(validate_proposal_metadata(&proposal, "run-1", 2, &profile).is_err());

        let mut oversized_action = candidate;
        oversized_action["nextAction"] = json!("x".repeat(2_001));
        let proposal: RuntimeContinuationProposal =
            serde_json::from_value(oversized_action).unwrap();
        assert!(validate_proposal_metadata(&proposal, "run-1", 2, &profile).is_err());

        let shadow = ExecutionProfileSelection::resolve("durable_v2_shadow").unwrap();
        assert!(shadow
            .treats_proposal_error_as_observation(&json!({ "type": "run.continuation_proposed" })));
        assert!(!shadow.treats_proposal_error_as_observation(&json!({ "type": "run.completed" })));
    }

    #[test]
    fn reconciliation_uses_authoritative_profile_and_is_idempotent() {
        let path = std::env::temp_dir().join(format!("fox-continuation-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("reconcile"), None, None)
            .unwrap();
        let started = database
            .create_run(&conversation.id, "continue", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({
                    "type": "run.started",
                    "executionProfileId": "durable_v2"
                }),
            )
            .unwrap();
        let mut payload = continuation_payload("durable_v2");
        payload["proposal"]["runId"] = json!(started.run.id);
        database
            .apply_runtime_event(&started.run.id, 2, &payload)
            .unwrap();

        let first = reconcile_unprojected_proposals(&database, 10).unwrap();
        assert_eq!(first.decisions.len(), 1);
        assert!(first.diagnostics.is_empty());
        database
            .apply_runtime_event(
                &started.run.id,
                3,
                &json!({ "type": "run.phase", "phase": "later-facts" }),
            )
            .unwrap();
        let replayed = persist_runtime_proposal(
            &database,
            &started.run.id,
            2,
            &ExecutionProfileSelection::resolve("durable_v2").unwrap(),
            &payload,
        )
        .unwrap();
        assert_eq!(replayed, first.decisions[0]);
        let second = reconcile_unprojected_proposals(&database, 10).unwrap();
        assert!(second.decisions.is_empty());
        assert!(second.diagnostics.is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reconciliation_cannot_self_declare_shadow_and_malformed_shadow_is_diagnostic_only() {
        let path = std::env::temp_dir().join(format!("fox-continuation-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("reconcile"), None, None)
            .unwrap();

        let durable = database
            .create_run(&conversation.id, "durable", None)
            .unwrap();
        database
            .apply_runtime_event(
                &durable.run.id,
                1,
                &json!({ "type": "run.started", "executionProfileId": "durable_v2" }),
            )
            .unwrap();
        let mut spoofed = continuation_payload("durable_v2_shadow");
        spoofed["proposal"]["runId"] = json!(durable.run.id);
        database
            .apply_runtime_event(&durable.run.id, 2, &spoofed)
            .unwrap();

        let shadow_conversation = database
            .create_conversation(database.default_agent_id(), Some("shadow"), None, None)
            .unwrap();
        let shadow = database
            .create_run(&shadow_conversation.id, "shadow", None)
            .unwrap();
        database
            .apply_runtime_event(
                &shadow.run.id,
                1,
                &json!({ "type": "run.started", "executionProfileId": "durable_v2_shadow" }),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &shadow.run.id,
                2,
                &json!({ "type": "run.continuation_proposed", "proposal": {} }),
            )
            .unwrap();

        let report = reconcile_unprojected_proposals(&database, 10).unwrap();
        assert!(report.decisions.is_empty());
        assert_eq!(report.diagnostics.len(), 2);
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.run_id == durable.run.id && !diagnostic.shadow));
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.run_id == shadow.run.id && diagnostic.shadow));
        assert!(report
            .diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.code == "runtime.continuation.reconciliation_failed" }));

        let replay = reconcile_unprojected_proposals(&database, 10).unwrap();
        assert!(replay.decisions.is_empty());
        assert!(replay.diagnostics.is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reconciliation_drains_more_than_one_batch_with_malformed_events_and_is_idempotent() {
        let path = std::env::temp_dir().join(format!("fox-continuation-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                Some("large reconcile"),
                None,
                None,
            )
            .unwrap();
        let started = database
            .create_run(&conversation.id, "large reconcile", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.started", "executionProfileId": "durable_v2" }),
            )
            .unwrap();
        for index in 0..257_i64 {
            let seq = index + 2;
            let payload = if index == 128 {
                json!({ "type": "run.continuation_proposed", "proposal": {} })
            } else {
                let mut payload = continuation_payload("durable_v2");
                payload["proposal"]["runId"] = json!(started.run.id);
                payload["proposal"]["decisionId"] = json!(format!("decision-{index}"));
                payload["proposal"]["eventCursor"] = json!(seq - 1);
                payload
            };
            database
                .apply_runtime_event(&started.run.id, seq, &payload)
                .unwrap();
        }

        let mut scanned = 0;
        let mut decisions = 0;
        let mut diagnostics = 0;
        let mut passes = 0;
        loop {
            let sweep = reconcile_unprojected_proposals_until_budget(
                &database,
                256,
                128,
                Duration::from_secs(30),
            )
            .unwrap();
            passes += 1;
            scanned += sweep.report.scanned;
            decisions += sweep.report.decisions.len();
            diagnostics += sweep.report.diagnostics.len();
            if !sweep.has_more {
                break;
            }
        }
        assert_eq!(passes, 3);
        assert_eq!(scanned, 257);
        assert_eq!(decisions, 256);
        assert_eq!(diagnostics, 1);

        let replay = reconcile_unprojected_proposals_until_budget(
            &database,
            256,
            1_024,
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(replay.report.scanned, 0);
        assert!(replay.report.decisions.is_empty());
        assert!(replay.report.diagnostics.is_empty());
        assert!(!replay.has_more);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ingest_diagnostics_are_unicode_safely_bounded_and_idempotent() {
        let path = std::env::temp_dir().join(format!("fox-continuation-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("diagnostic"), None, None)
            .unwrap();
        let started = database
            .create_run(&conversation.id, "diagnostic", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.continuation_proposed", "proposal": {} }),
            )
            .unwrap();

        let long_profile = "p".repeat(250);
        let long_code = "c".repeat(250);
        let long_message = "错".repeat(5_000);
        record_ingest_diagnostic(
            &database,
            &started.run.id,
            1,
            Some(&long_profile),
            true,
            &long_code,
            &long_message,
        )
        .unwrap();
        let record = database
            .record_continuation_ingest_diagnostic(&RecordContinuationIngestDiagnosticInput {
                run_id: started.run.id.clone(),
                event_seq: 1,
                execution_profile_id: Some("p".repeat(200)),
                shadow_mode: true,
                error_code: "c".repeat(200),
                error_message: "错".repeat(4_000),
            })
            .unwrap();
        assert_eq!(record.error_message.chars().count(), 4_000);
        assert_eq!(record.error_code.chars().count(), 200);
        assert_eq!(
            record
                .execution_profile_id
                .as_deref()
                .unwrap()
                .chars()
                .count(),
            200
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn semantics_reject_invalid_evidence_and_cross_run_or_unreferenced_approval() {
        let ledger = ledger_with(
            json!([{
                "taskId": "task-1",
                "title": "validate",
                "status": "in_progress",
                "attempt": 1,
                "acceptanceChecks": [],
                "latestEvidence": [{
                    "id": "evidence-invalid",
                    "evidenceType": "test_result",
                    "validityStatus": "invalid",
                    "summary": "failed",
                    "createdAt": "1"
                }],
                "blockers": [],
                "assignedRunId": "run-new",
                "childRunId": null
            }]),
            json!([]),
        );
        let mut proposal_value = continuation_payload("durable_v2")["proposal"].clone();
        proposal_value["runId"] = json!("run-new");
        proposal_value["activeTaskIds"] = json!(["task-1"]);
        proposal_value["evidenceIds"] = json!(["evidence-invalid"]);
        let proposal: RuntimeContinuationProposal = serde_json::from_value(proposal_value).unwrap();
        assert_eq!(
            validate_semantics(&proposal, &ledger, false).as_deref(),
            Some("runtime.continuation.evidence_unknown")
        );

        let approval_ledger = ledger_with(
            json!([]),
            json!([{
                "kind": "approval",
                "referenceId": "approval-old",
                "taskId": null,
                "runId": "run-old",
                "toolCallId": "tool-old",
                "summary": "old approval"
            }]),
        );
        let mut wait = continuation_payload("durable_v2")["proposal"].clone();
        wait["runId"] = json!("run-new");
        wait["decision"] = json!("wait_approval");
        wait["reasonCode"] = json!("approval_pending");
        wait["retryClass"] = json!("host_decides");
        wait["blockedDependencyRefs"] = json!(["approval-old"]);
        let wait: RuntimeContinuationProposal = serde_json::from_value(wait).unwrap();
        assert_eq!(
            validate_semantics(&wait, &approval_ledger, false).as_deref(),
            Some("runtime.continuation.approval_not_pending")
        );

        let current_approval_ledger = ledger_with(
            json!([]),
            json!([{
                "kind": "approval",
                "referenceId": "approval-current",
                "taskId": null,
                "runId": "run-new",
                "toolCallId": "tool-current",
                "summary": "current approval"
            }]),
        );
        let mut empty_ref = continuation_payload("durable_v2")["proposal"].clone();
        empty_ref["runId"] = json!("run-new");
        empty_ref["decision"] = json!("wait_approval");
        empty_ref["reasonCode"] = json!("approval_pending");
        empty_ref["retryClass"] = json!("host_decides");
        let empty_ref: RuntimeContinuationProposal = serde_json::from_value(empty_ref).unwrap();
        assert_eq!(
            validate_semantics(&empty_ref, &current_approval_ledger, false).as_deref(),
            Some("runtime.continuation.approval_reference_required")
        );
    }

    #[test]
    fn durable_continuation_cannot_treat_skipped_task_as_verified_completion() {
        let mut ledger = ledger_with(json!([]), json!([]));
        ledger.goal = Some(TaskLedgerGoalProjection {
            id: "goal-1".to_owned(),
            title: "durable".to_owned(),
            status: GoalStatus::Completed,
            completion_policy: "strict_v2".to_owned(),
        });
        ledger.completed_summary.push(TaskLedgerCompletedSummary {
            task_id: "task-skipped".to_owned(),
            outcome: TaskLedgerCompletedOutcome::Skipped,
            evidence_ids: Vec::new(),
            accepted_at: None,
        });
        let mut proposal_value = continuation_payload("durable_v2")["proposal"].clone();
        proposal_value["runId"] = json!("run-new");
        proposal_value["decision"] = json!("complete");
        proposal_value["reasonCode"] = json!("acceptance_passed");
        proposal_value["nextAction"] = json!("");
        proposal_value["retryClass"] = json!("none");
        let proposal: RuntimeContinuationProposal = serde_json::from_value(proposal_value).unwrap();
        assert_eq!(
            validate_semantics(&proposal, &ledger, false).as_deref(),
            Some("runtime.continuation.skipped_task_not_verified")
        );
        assert_eq!(validate_semantics(&proposal, &ledger, true), None);
    }
}

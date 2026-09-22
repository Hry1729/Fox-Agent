//! Frozen execution credentials and persistent single-claim execution attempts
//! (frozen contract v1.6, B1a).
//!
//! Two durable facts, both keyed by the data-root-unique `(run_id,
//! dispatch_id)` pair:
//!
//! 1. `kernel_execution_credentials` — the authoritative snapshot issued once
//!    per dispatch inside the final admission transaction. Re-issuing the same
//!    dispatch returns the stored snapshot verbatim; a different payload for
//!    the same dispatch is refused, so a dispatch can never be re-signed.
//!    Rows that predate this table simply do not exist: an old dispatch
//!    without a credential cannot start, and nothing backfills authorization.
//! 2. `kernel_execution_attempts` — the persistent single claim. Credential
//!    verification happens *before* the claim; only the claim winner may start
//!    the first execution attempt. A real-time refusal is persisted as a
//!    terminal refusal fact (no job row is fabricated to carry it), repeat
//!    requests read the existing fact, and a claimed-but-unknown outcome is
//!    never replayed.

use super::{now_ms, Database};
use fox_engine_protocol::{
    canonical_digest, encode_dispatch_id, ActionClass, AttemptOutcome, AttemptState,
    BackendRequirement, ControlPlaneState, ExecutionCredential, ExecutionKind, ExecutionReceipt,
    ExecutionStage, HostObservation, RealtimeRequirement, SideEffectState, TriState,
};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

/// One durable execution-attempt row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAttemptRow {
    pub run_id: String,
    pub dispatch_id: String,
    pub conversation_id: String,
    pub params_hash: String,
    pub state: AttemptState,
    /// NULL = start fact unknown; Some(false/true) once known (never regressed).
    pub start_confirmed: Option<bool>,
    pub terminal_state: Option<String>,
    pub error_code: Option<String>,
    pub claimed_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Operation class bound at claim time (a file is not a process).
    pub action_class: Option<ActionClass>,
    /// Live session policy version bound at issue time (None = fact missing).
    pub policy_version: Option<u64>,
    /// Budget ceiling bound at issue time (executor may only tighten).
    pub budget_ceiling_ms: Option<i64>,
    /// Parent revocation generation bound at issue time (None = no parent).
    pub parent_generation: Option<u64>,
}

fn action_class_str(value: ActionClass) -> &'static str {
    match value {
        ActionClass::Read => "read",
        ActionClass::Write => "write",
        ActionClass::Execute => "execute",
        ActionClass::Destructive => "destructive",
        ActionClass::SensitiveEgress => "sensitive_egress",
        ActionClass::Manage => "manage",
    }
}

fn attempt_state_str(value: &AttemptState) -> &'static str {
    match value {
        AttemptState::Claimed => "claimed",
        AttemptState::Completed => "completed",
        AttemptState::Refused { .. } => "refused",
        AttemptState::Launched => "launched",
        AttemptState::Unknown => "unknown",
    }
}

fn attempt_state_from_str(
    value: &str,
    refusal_code: Option<String>,
) -> Result<AttemptState, String> {
    match value {
        "claimed" => Ok(AttemptState::Claimed),
        "completed" => Ok(AttemptState::Completed),
        "refused" => Ok(AttemptState::Refused {
            code: refusal_code.unwrap_or_else(|| "policy_denied".to_owned()),
        }),
        "launched" => Ok(AttemptState::Launched),
        "unknown" => Ok(AttemptState::Unknown),
        other => Err(format!("unsupported stored attempt state: {other}")),
    }
}

fn action_class_from_str_opt(value: &str) -> Option<ActionClass> {
    match value {
        "read" => Some(ActionClass::Read),
        "write" => Some(ActionClass::Write),
        "execute" => Some(ActionClass::Execute),
        "destructive" => Some(ActionClass::Destructive),
        "sensitive_egress" => Some(ActionClass::SensitiveEgress),
        "manage" => Some(ActionClass::Manage),
        _ => None,
    }
}

/// Maps an admitted operation to the canonical Host target identity whose
/// observation may serve as the file baseline.
///
/// Ownership split (B1a-R2): the resolver is implemented by the Host file layer
/// (A's `tool_host` canonicalization) and passed IN by the caller. This module
/// never parses model paths itself and never derives a target from the model's
/// `expectedVersion`/`readVersion`. Returning `None` means "no authoritative
/// target identity could be derived", which keeps the baseline `None` and the
/// write-class action refused.
pub trait FileTargetResolver {
    /// The canonical target identity for one admitted operation, or `None`.
    fn resolve(&self, class: ActionClass, canonical_input_json: &str) -> Option<String>;
}

/// A resolver that never produces a target identity (the current Host state:
/// A's canonical read path is not wired yet).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFileTargetResolver;

impl FileTargetResolver for NoFileTargetResolver {
    fn resolve(&self, _class: ActionClass, _canonical_input_json: &str) -> Option<String> {
        None
    }
}

/// Build the authoritative credential for one dispatch from durable facts and
/// persist it inside the caller's admission transaction. The model never
/// contributes a field: identity comes from the run's durable rows,
/// classification from the canonical tool contract, and the backend requirement
/// stays unverified until a backend has actually been probed.
///
/// Returns `None` when the run resolves to no durable Host conversation: such a
/// dispatch carries no credential and can never be claimed (fail-closed).
pub fn issue_dispatch_credential_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    tool_call_id: &str,
    tool: &str,
    canonical_input_json: &str,
    now: i64,
) -> Result<Option<ExecutionCredential>, String> {
    let binding_json: Option<String> = transaction
        .query_row(
            "SELECT binding_json FROM run_control_bindings WHERE run_id=?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(json) = binding_json {
        let binding: fox_engine_protocol::RunControlBinding =
            serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if binding.run_id != run_id {
            return Err("credential_mismatch: frozen binding run differs".into());
        }
        if let Some(root) = binding.permission.project_root.as_deref() {
            let resolver = crate::runtime_host::managed_files::HostFileTargetResolver {
                project_root: std::path::Path::new(root),
            };
            return issue_dispatch_credential_with_resolver_in_tx(
                transaction,
                run_id,
                tool_call_id,
                tool,
                canonical_input_json,
                now,
                &resolver,
            );
        }
    }
    issue_dispatch_credential_with_resolver_in_tx(
        transaction,
        run_id,
        tool_call_id,
        tool,
        canonical_input_json,
        now,
        &NoFileTargetResolver,
    )
}

/// Same as [issue_dispatch_credential_in_tx], with the caller''s authoritative
/// target resolver. The resolver only supplies the target identity; the
/// observation itself must already exist in kernel_host_observations.
pub fn issue_dispatch_credential_with_resolver_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    tool_call_id: &str,
    tool: &str,
    canonical_input_json: &str,
    now: i64,
    file_target_resolver: &dyn FileTargetResolver,
) -> Result<Option<ExecutionCredential>, String> {
    // binding first, the legacy run row second. A dispatch whose run resolves
    // to neither carries NO credential and can therefore never be claimed
    // (fail-closed); nothing is invented here.
    let conversation_id: Option<String> = transaction
        .query_row(
            "SELECT COALESCE(b.conversation_id, l.conversation_id)
               FROM kernel_runs r
               LEFT JOIN run_control_bindings b ON b.run_id = r.run_id
               LEFT JOIN runs l ON l.id = r.run_id
              WHERE r.run_id=?1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    let Some(conversation_id) = conversation_id else {
        return Ok(None);
    };
    let (resolved_profile, policy_snapshot_id): (String, String) = transaction
        .query_row(
            "SELECT execution_profile_id, permission_snapshot_id
               FROM kernel_runs WHERE run_id=?1",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    // Budget ceiling: bound from the run's own durable binding when present.
    // The executor may only tighten it later, never widen it.
    let budget_ceiling_ms: i64 = transaction
        .query_row(
            "SELECT binding_json FROM run_control_bindings WHERE run_id=?1",
            params![run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .and_then(|json| serde_json::from_str::<fox_engine_protocol::RunControlBinding>(&json).ok())
        .map(|binding| binding.budgets.tool_execution_ms)
        .unwrap_or(0);
    let policy_version: Option<u64> = transaction
        .query_row(
            "SELECT version FROM kernel_execution_policies WHERE conversation_id=?1",
            [&conversation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let parent_generation: Option<u64> = transaction.query_row(
        "SELECT COALESCE(g.generation,0) FROM runs child JOIN runs parent ON parent.id=child.parent_run_id LEFT JOIN kernel_parent_generations g ON g.run_id=parent.id WHERE child.id=?1", [run_id], |r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let has_parent = parent_generation.is_some();
    // The file baseline is an authoritative Host observation, never a
    // model-declared `expectedVersion`. The observation is resolved through the
    // Host's target-mapping resolver (see `FileTargetResolver`): the resolver
    // supplies the canonical target identity, and the observation must already
    // exist in `kernel_host_observations`. Nothing is invented here, so a
    // write-class action without an observation stays `None` and is refused at
    // claim time.
    let action_class = operation_class(tool, canonical_input_json);
    let file_baseline: Option<HostObservation> = match file_target_resolver
        .resolve(action_class, canonical_input_json)
    {
        Some(target_identity) => host_observation_for_in_tx(transaction, run_id, &target_identity)?,
        None => None,
    };
    let credential = ExecutionCredential::new(
        encode_dispatch_id(run_id, tool_call_id)?,
        run_id.to_owned(),
        conversation_id,
        launch_params_hash(tool, canonical_input_json),
        action_class,
        file_baseline,
        resolved_profile,
        policy_snapshot_id,
        policy_version,
        budget_ceiling_ms,
        parent_generation,
        unverified_backend_requirement(),
    )?;
    if has_parent {
        let mut credential = credential;
        if !credential
            .realtime_requirements
            .contains(&RealtimeRequirement::ParentRevocation)
        {
            credential
                .realtime_requirements
                .push(RealtimeRequirement::ParentRevocation);
        }
        credential.credential_digest = credential.recompute_digest();
        issue_execution_credential_in_tx(transaction, &credential, now)?;
        return Ok(Some(credential));
    }
    issue_execution_credential_in_tx(transaction, &credential, now)?;
    Ok(Some(credential))
}

/// Persist the authoritative credential snapshot. Idempotent for a byte-identical
/// re-issue; refuses a re-sign of the same dispatch with different fields.
pub fn issue_execution_credential_in_tx(
    transaction: &Transaction<'_>,
    credential: &ExecutionCredential,
    now: i64,
) -> Result<(), String> {
    let json = serde_json::to_string(credential)
        .map_err(|error| format!("serialize execution credential: {error}"))?;
    let existing: Option<(String, String)> = transaction
        .query_row(
            "SELECT credential_json, credential_digest FROM kernel_execution_credentials
              WHERE run_id=?1 AND dispatch_id=?2",
            params![credential.run_id, credential.dispatch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((stored_json, stored_digest)) = existing {
        if stored_json != json || stored_digest != credential.credential_digest {
            return Err(format!(
                "credential re-sign conflict for {}/{}",
                credential.run_id, credential.dispatch_id
            ));
        }
        return Ok(());
    }
    let file_baseline_json = credential
        .file_baseline
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("serialize host observation: {error}"))?;
    transaction
        .execute(
            "INSERT INTO kernel_execution_credentials
                (run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
                 resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
                 credential_digest, credential_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                credential.run_id,
                credential.dispatch_id,
                credential.conversation_id,
                credential.intent_digest,
                action_class_str(credential.action_class),
                file_baseline_json,
                credential.resolved_profile,
                credential.policy_snapshot_id,
                credential.backend_requirement.required,
                credential.backend_requirement.evidence_digest,
                credential.credential_digest,
                json,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn read_credential_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    dispatch_id: &str,
) -> Result<Option<ExecutionCredential>, String> {
    let row: Option<String> = transaction
        .query_row(
            "SELECT credential_json FROM kernel_execution_credentials
              WHERE run_id=?1 AND dispatch_id=?2",
            params![run_id, dispatch_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(json) = row else { return Ok(None) };
    let credential: ExecutionCredential = serde_json::from_str(&json)
        .map_err(|error| format!("corrupt execution credential: {error}"))?;
    if credential.credential_digest != credential.recompute_digest() {
        return Err("persisted execution credential digest mismatch".into());
    }
    Ok(Some(credential))
}

fn read_attempt_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    dispatch_id: &str,
) -> Result<Option<ExecutionAttemptRow>, String> {
    transaction
        .query_row(
            "SELECT conversation_id, params_hash, state, refusal_code, start_confirmed,
                    terminal_state, error_code, claimed_by, created_at, updated_at,
                    action_class, policy_version, budget_ceiling_ms, parent_generation
               FROM kernel_execution_attempts
              WHERE run_id=?1 AND dispatch_id=?2",
            params![run_id, dispatch_id],
            |row| {
                let state: String = row.get(2)?;
                let refusal: Option<String> = row.get(3)?;
                let start: Option<i64> = row.get(4)?;
                let action_class: Option<String> = row.get(10)?;
                Ok(ExecutionAttemptRow {
                    run_id: run_id.to_owned(),
                    dispatch_id: dispatch_id.to_owned(),
                    conversation_id: row.get(0)?,
                    params_hash: row.get(1)?,
                    state: attempt_state_from_str(&state, refusal)
                        .map_err(|error| rusqlite::Error::InvalidParameterName(error))?,
                    start_confirmed: start.map(|value| value == 1),
                    terminal_state: row.get(5)?,
                    error_code: row.get(6)?,
                    claimed_by: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                    action_class: action_class.as_deref().and_then(action_class_from_str_opt),
                    policy_version: row.get(11)?,
                    budget_ceiling_ms: row.get(12)?,
                    parent_generation: row.get(13)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

impl Database {
    /// Issue the fixed file credential for the production Legacy Host path.
    ///
    /// Every input comes from durable Host facts in this transaction: the
    /// already-running legacy `tool_calls` row, its frozen run-control binding,
    /// the live policy version, a consumed approval when policy/hooks require
    /// one, and the Host observation for the canonical target. The credential
    /// row itself is the durable consumed-dispatch fact; its primary key makes
    /// a second issue byte-idempotent and refuses a re-sign.
    pub fn issue_legacy_file_credential(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        expected_policy_version: u64,
    ) -> Result<ExecutionCredential, String> {
        if run_id.trim().is_empty() || runtime_tool_call_id.trim().is_empty() {
            return Err("legacy credential identity must be non-empty".into());
        }
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let credential = issue_legacy_file_credential_in_tx(
                &transaction,
                run_id,
                runtime_tool_call_id,
                expected_policy_version,
                now_ms(),
            )
            .map_err(rusqlite::Error::InvalidParameterName)?;
            transaction.commit()?;
            Ok(credential)
        })
    }

    /// Issue (or idempotently re-read) the authoritative credential.
    pub fn issue_execution_credential(
        &self,
        credential: &ExecutionCredential,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            issue_execution_credential_in_tx(&transaction, credential, now_ms())
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
            transaction.commit()
        })
    }

    /// Read-only authoritative snapshot. This is the only source A/C verify
    /// against; there is no caller-constructible shortcut.
    pub fn read_execution_credential(
        &self,
        run_id: &str,
        dispatch_id: &str,
    ) -> Result<Option<ExecutionCredential>, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            read_credential_in_tx(&transaction, run_id, dispatch_id)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))
        })
    }

    /// Field-level verification against the stored snapshot. A tampered fixed
    /// field with a recomputed self-consistent digest is still refused.
    pub fn verify_execution_credential(
        &self,
        run_id: &str,
        presented: &ExecutionCredential,
    ) -> Result<(), String> {
        let stored = self
            .read_execution_credential(run_id, &presented.dispatch_id)?
            .ok_or_else(|| {
                "credential_missing: dispatch has no authoritative credential".to_owned()
            })?;
        presented.verify_against(&stored)
    }

    /// Re-read durable policy/cancellation/parent facts immediately before the
    /// caller's side effect. Resource/target checks remain the Host's duty.
    pub fn revalidate_execution_credential(
        &self,
        credential: &ExecutionCredential,
    ) -> Result<(), String> {
        self.with_connection(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let stored = read_credential_in_tx(&tx, &credential.run_id, &credential.dispatch_id)
                .map_err(rusqlite::Error::InvalidParameterName)?
                .ok_or_else(|| {
                    rusqlite::Error::InvalidParameterName("credential_missing".into())
                })?;
            credential
                .verify_against(&stored)
                .map_err(rusqlite::Error::InvalidParameterName)?;
            if let Some(code) = unsatisfied_requirement(&tx, &credential.run_id, credential)? {
                return Err(rusqlite::Error::InvalidParameterName(code.into()));
            }
            tx.commit()
        })
    }

    /// Persistent single claim. Order inside one immediate transaction:
    /// credential verification (before the claim) → caller scope check →
    /// claim CAS. Returns the existing fact for repeat deliveries.
    pub fn claim_execution_attempt(
        &self,
        run_id: &str,
        conversation_id: &str,
        presented: &ExecutionCredential,
        params_hash: &str,
        owner: &str,
    ) -> Result<AttemptOutcome, String> {
        if owner.trim().is_empty() || params_hash.trim().is_empty() {
            return Err("claim owner and params hash must be non-empty".into());
        }
        if params_hash != presented.intent_digest {
            return Err("credential_mismatch: request content differs from admitted intent".into());
        }
        if presented.run_id != run_id || presented.conversation_id != conversation_id {
            return Err(
                "credential_mismatch: presented identity differs from the caller scope".into(),
            );
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            // 1) Credential verification BEFORE the claim (read-only).
            let stored = read_credential_in_tx(&transaction, run_id, &presented.dispatch_id)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?
                .ok_or_else(|| {
                    rusqlite::Error::InvalidParameterName(
                        "credential_missing: dispatch has no authoritative credential".into(),
                    )
                })?;
            presented
                .verify_against(&stored)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
            // 2) Caller scope: the run's own durable conversation must match.
            // Authoritative runs carry a frozen binding; anything else falls
            // back to the legacy run row.
            let run_conversation: Option<String> = transaction
                .query_row(
                    "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?;
            let run_conversation = match run_conversation {
                Some(value) => value,
                None => transaction
                    .query_row(
                        "SELECT conversation_id FROM runs WHERE id=?1",
                        params![run_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| {
                        rusqlite::Error::InvalidParameterName(format!("unknown run {run_id}"))
                    })?,
            };
            if run_conversation != conversation_id {
                return Err(rusqlite::Error::InvalidParameterName(
                    "credential_mismatch: caller scope is not this run's conversation".into(),
                ));
            }
            // 3) Claim CAS: exactly one winner across connections.
            let now = now_ms();
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO kernel_execution_attempts
                    (run_id, dispatch_id, conversation_id, params_hash, state, refusal_code,
                     start_confirmed, terminal_state, error_code, claimed_by, created_at, updated_at,
                     action_class, policy_version, budget_ceiling_ms, parent_generation)
                 VALUES (?1, ?2, ?3, ?4, 'claimed', NULL, NULL, NULL, NULL, ?5, ?6, ?6, ?7, ?8, ?9, ?10)",
                params![
                    run_id,
                    presented.dispatch_id,
                    conversation_id,
                    params_hash,
                    owner,
                    now,
                    presented.action_class.as_str(),
                    presented.policy_version,
                    presented.budget_ceiling_ms,
                    presented.parent_revocation_generation,
                ],
            )?;
            if inserted == 1 {
                // 4) Real-time requirement evaluation happens in the SAME
                //    transaction as the claim: an unsatisfied requirement is a
                //    persistent terminal refusal of this single attempt.
                if let Some(code) = unsatisfied_requirement(&transaction, run_id, presented)? {
                    transaction.execute(
                        "UPDATE kernel_execution_attempts
                            SET state='refused', refusal_code=?3, updated_at=?4
                          WHERE run_id=?1 AND dispatch_id=?2 AND state='claimed'",
                        params![run_id, presented.dispatch_id, code, now],
                    )?;
                    transaction.commit()?;
                    return Ok(AttemptOutcome::AlreadyRefused {
                        code: code.to_string(),
                    });
                }
                transaction.commit()?;
                return Ok(AttemptOutcome::Claimed);
            }
            let row = read_attempt_in_tx(&transaction, run_id, &presented.dispatch_id)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?
                .ok_or_else(|| rusqlite::Error::InvalidParameterName(
                    "attempt row vanished after claim conflict".into(),
                ))?;
            if row.params_hash != params_hash {
                return Err(rusqlite::Error::InvalidParameterName(
                    "job_idempotency_conflict: same dispatch, different request content".into(),
                ));
            }
            transaction.commit()?;
            Ok(match row.state {
                AttemptState::Refused { code } => AttemptOutcome::AlreadyRefused { code },
                AttemptState::Launched => AttemptOutcome::AlreadyLaunched,
                AttemptState::Completed => AttemptOutcome::AlreadyCompleted,
                AttemptState::Unknown => AttemptOutcome::Unknown,
                AttemptState::Claimed => AttemptOutcome::Unknown,
            })
        })
    }

    /// Persist the terminal refusal of a first attempt (no job row is created).
    pub fn record_attempt_refusal(
        &self,
        run_id: &str,
        dispatch_id: &str,
        code: &str,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE kernel_execution_attempts
                    SET state='refused', refusal_code=?3, updated_at=?4
                  WHERE run_id=?1 AND dispatch_id=?2 AND state='claimed'",
                params![run_id, dispatch_id, code, now_ms()],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "attempt refusal lost its claimed CAS".into(),
                ));
            }
            transaction.commit()
        })
    }

    /// Record that the first execution attempt actually started. This is
    /// evidence-driven: the caller must hold real execution evidence (a spawned
    /// process identity, a recorded applying journal, ...). A claim alone is
    /// NOT evidence and must never reach this method.
    pub fn record_attempt_started(&self, run_id: &str, dispatch_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE kernel_execution_attempts
                    SET state='launched', start_confirmed=1, updated_at=?3
                  WHERE run_id=?1 AND dispatch_id=?2 AND state='claimed'",
                params![run_id, dispatch_id, now_ms()],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "attempt start lost its claimed CAS".into(),
                ));
            }
            transaction.commit()
        })
    }

    /// Record that the call finished WITHOUT starting an external operation.
    /// This is a terminal SUCCESS state (a read/query that completed): it is
    /// never a refusal and never carries an invented error code. Only a trusted
    /// control fact may reach this method.
    pub fn record_attempt_completed(
        &self,
        run_id: &str,
        dispatch_id: &str,
        external_started: bool,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE kernel_execution_attempts
                    SET state='completed', start_confirmed=CASE WHEN start_confirmed=1 THEN 1 ELSE ?3 END, terminal_state='completed',
                        updated_at=?4
                  WHERE run_id=?1 AND dispatch_id=?2 AND state IN ('claimed','launched')",
                params![
                    run_id,
                    dispatch_id,
                    if external_started { 1 } else { 0 },
                    now_ms()
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "attempt completion lost its claimed CAS".into(),
                ));
            }
            transaction.commit()
        })
    }

    /// Record a failed call whose side-effect outcome is unknown. The real
    /// error code is preserved and the start fact is never invented; an
    /// already-confirmed start stays confirmed.
    pub fn record_attempt_failure(
        &self,
        run_id: &str,
        dispatch_id: &str,
        code: &str,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE kernel_execution_attempts
                    SET state='unknown', error_code=?3, terminal_state='failed', updated_at=?4
                  WHERE run_id=?1 AND dispatch_id=?2 AND state IN ('claimed','launched')",
                params![run_id, dispatch_id, code, now_ms()],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "attempt failure lost its claimed CAS".into(),
                ));
            }
            transaction.commit()
        })
    }

    /// Mark a claimed/launched attempt unknown (crash/recovery boundary). The
    /// start fact is NEVER erased: an already confirmed start stays confirmed,
    /// only the outcome becomes unknown. The attempt is never replayed.
    pub fn mark_attempt_unknown(&self, run_id: &str, dispatch_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE kernel_execution_attempts
                    SET state='unknown', updated_at=?3
                  WHERE run_id=?1 AND dispatch_id=?2 AND state IN ('claimed','launched')",
                params![run_id, dispatch_id, now_ms()],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "attempt is not in an interruptible state".into(),
                ));
            }
            transaction.commit()
        })
    }

    /// Read one attempt row (read-only).
    pub fn read_execution_attempt(
        &self,
        run_id: &str,
        dispatch_id: &str,
    ) -> Result<Option<ExecutionAttemptRow>, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            read_attempt_in_tx(&transaction, run_id, dispatch_id)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))
        })
    }

    /// The durable (tool, canonical input) identity of one tool call. The
    /// execution boundary presents exactly these strings back, so a re-serialized
    /// payload can never fake a different intent digest.
    pub fn tool_call_identity(
        &self,
        run_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT tool, canonical_input_json FROM kernel_tool_calls
                      WHERE run_id=?1 AND tool_call_id=?2",
                    params![run_id, tool_call_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
    }

    /// Record an authoritative Host observation (durable read evidence).
    pub fn record_host_observation(
        &self,
        run_id: &str,
        conversation_id: &str,
        observation: &HostObservation,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            record_host_observation_in_tx(
                &transaction,
                run_id,
                conversation_id,
                observation,
                now_ms(),
            )
            .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
            transaction.commit()
        })
    }

    /// Read the authoritative observation for one target identity.
    pub fn host_observation_for(
        &self,
        run_id: &str,
        target_identity: &str,
    ) -> Result<Option<HostObservation>, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            host_observation_for_in_tx(&transaction, run_id, target_identity)
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))
        })
    }

    /// Build the stage receipt from durable facts only. Repeat requests and
    /// cross-run reads are bounded by `(run_id, dispatch_id)`; pass the
    /// caller's conversation to enforce scope.
    pub fn execution_receipt_for_conversation(
        &self,
        run_id: &str,
        dispatch_id: &str,
        conversation_id: &str,
    ) -> Result<Option<ExecutionReceipt>, String> {
        let receipt = self.execution_receipt(run_id, dispatch_id)?;
        let Some(receipt) = receipt else {
            return Ok(None);
        };
        let row = self.read_execution_attempt(run_id, dispatch_id)?;
        if let Some(row) = row {
            if row.conversation_id != conversation_id {
                return Err("credential_mismatch: receipt belongs to another conversation".into());
            }
        }
        Ok(Some(receipt))
    }

    /// Build the stage receipt from the durable attempt row. `None` when the
    /// dispatch has no attempt fact yet (nothing to report). The execution
    /// family comes from the bound operation class: a file write is never
    /// projected as a process, and a claim is never projected as a job.
    pub fn execution_receipt(
        &self,
        run_id: &str,
        dispatch_id: &str,
    ) -> Result<Option<ExecutionReceipt>, String> {
        let Some(row) = self.read_execution_attempt(run_id, dispatch_id)? else {
            return Ok(None);
        };
        let kind = match row.action_class {
            Some(ActionClass::Write) | Some(ActionClass::Destructive) => ExecutionKind::File,
            _ => ExecutionKind::Process,
        };
        let job_exists: bool = self.with_connection(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM kernel_jobs WHERE run_id=?1 AND idempotency_key=?2)",
                params![run_id, fox_engine_protocol::job_row_key(dispatch_id)],
                |r| r.get(0),
            )
        })?;
        let receipt = match row.state {
            AttemptState::Refused { code } => {
                // The family's own "admitted but not applied" stage: a file
                // action is never projected with a process stage.
                let stage = match kind {
                    ExecutionKind::File => ExecutionStage::FilePrepared,
                    ExecutionKind::Process if job_exists => ExecutionStage::JobCreated,
                    ExecutionKind::Process => ExecutionStage::Admitted,
                };
                ExecutionReceipt::new(
                    dispatch_id.to_owned(),
                    kind,
                    stage,
                    TriState::False,
                    if job_exists {
                        ControlPlaneState::NotApplied
                    } else {
                        ControlPlaneState::Prepared
                    },
                    if job_exists {
                        SideEffectState::NotApplied
                    } else {
                        SideEffectState::None
                    },
                    Some(code),
                    None,
                )
            }
            AttemptState::Claimed => {
                let stage = match kind {
                    ExecutionKind::File => ExecutionStage::FilePrepared,
                    ExecutionKind::Process => ExecutionStage::Admitted,
                };
                ExecutionReceipt::new(
                    dispatch_id.to_owned(),
                    kind,
                    // A claim is an admission/claim fact, NOT a job creation.
                    stage,
                    TriState::Unknown,
                    ControlPlaneState::Applying, // the claim record itself is durable
                    SideEffectState::Unknown,
                    None,
                    None,
                )
            }
            AttemptState::Completed => {
                let started = row.start_confirmed == Some(true);
                let stage = match (kind, started) {
                    (ExecutionKind::File, true) => ExecutionStage::FileCommitted,
                    (ExecutionKind::Process, true) => ExecutionStage::LaunchConfirmed,
                    (ExecutionKind::File, false) => ExecutionStage::FilePrepared,
                    (ExecutionKind::Process, false) => ExecutionStage::Admitted,
                };
                ExecutionReceipt::new(
                    dispatch_id.to_owned(),
                    kind,
                    stage,
                    if started {
                        TriState::True
                    } else {
                        TriState::False
                    },
                    ControlPlaneState::Committed,
                    if started {
                        SideEffectState::Committed
                    } else {
                        SideEffectState::None
                    },
                    None,
                    None,
                )
            }
            AttemptState::Launched => {
                let stage = match kind {
                    ExecutionKind::File => ExecutionStage::FileApplying,
                    ExecutionKind::Process => ExecutionStage::LaunchConfirmed,
                };
                ExecutionReceipt::new(
                    dispatch_id.to_owned(),
                    kind,
                    stage,
                    TriState::True,
                    ControlPlaneState::Applying,
                    SideEffectState::Applying,
                    None,
                    None,
                )
            }
            AttemptState::Unknown => {
                // A previously confirmed start keeps its `True`; only the
                // outcome becomes unknown.
                let started = match row.start_confirmed {
                    Some(true) => TriState::True,
                    _ => TriState::Unknown,
                };
                ExecutionReceipt::new(
                    dispatch_id.to_owned(),
                    kind,
                    match kind {
                        ExecutionKind::File => ExecutionStage::FileIndeterminate,
                        ExecutionKind::Process => ExecutionStage::Interrupted,
                    },
                    started,
                    ControlPlaneState::Uncertain,
                    SideEffectState::Uncertain,
                    Some("kernel.uncertain_execution".to_owned()),
                    if kind == ExecutionKind::Process {
                        Some("job.interrupted_unknown".to_owned())
                    } else {
                        None
                    },
                )
            }
        }
        .map_err(|error| error.to_string())?;
        Ok(Some(receipt))
    }
}

fn issue_legacy_file_credential_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    runtime_tool_call_id: &str,
    expected_policy_version: u64,
    now: i64,
) -> Result<ExecutionCredential, String> {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        bool,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    )> = transaction
        .query_row(
            "SELECT t.conversation_id,t.tool_name,t.input_json,t.status,
                    t.execution_location,t.requires_approval,r.status,b.binding_json,
                    r.conversation_id,b.conversation_id,b.authority,b.engine_id,b.binding_hash
               FROM tool_calls t
               JOIN runs r ON r.id=t.run_id
               JOIN run_control_bindings b ON b.run_id=t.run_id
              WHERE t.run_id=?1 AND t.runtime_tool_call_id=?2",
            params![run_id, runtime_tool_call_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((
        conversation_id,
        tool,
        canonical_input_json,
        tool_state,
        execution_location,
        requires_approval,
        run_state,
        binding_json,
        run_conversation_id,
        binding_conversation_id,
        binding_authority,
        binding_engine_id,
        binding_hash,
    )) = row
    else {
        return Err("legacy credential source tool call or frozen binding is missing".into());
    };
    if run_state != "running" || tool_state != "running" || execution_location != "host" {
        return Err("legacy credential requires a running Host tool call on a running Run".into());
    }
    if !matches!(tool.as_str(), "write_file" | "edit_file") {
        return Err("legacy credential only admits write_file or edit_file".into());
    }
    let binding: fox_engine_protocol::RunControlBinding =
        serde_json::from_str(&binding_json).map_err(|error| error.to_string())?;
    let computed_binding_hash = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(binding_json.as_bytes()))
    );
    if computed_binding_hash != binding_hash {
        return Err("persisted control binding hash mismatch".into());
    }
    binding.validate()?;
    if binding.authority != fox_engine_protocol::ExecutionAuthority::Legacy
        || binding_authority != "legacy"
        || binding.run_id != run_id
        || binding.conversation_id != conversation_id
        || binding.conversation_id != run_conversation_id
        || binding.conversation_id != binding_conversation_id
        || binding.engine_id != binding_engine_id
    {
        return Err("legacy credential source does not match its frozen binding".into());
    }
    if Database::run_control_permission_hash(&binding.permission)? != binding.permission_snapshot_id
    {
        return Err("frozen permission hash does not match its content".into());
    }
    let (current_policy_version, policy_mode): (u64, String) = transaction
        .query_row(
            "SELECT version,mode FROM kernel_execution_policies WHERE conversation_id=?1",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    if current_policy_version != expected_policy_version {
        return Err("policy_version_conflict".into());
    }
    if policy_mode == "read_only" {
        return Err("policy_denied".into());
    }
    if policy_mode == "ask" || requires_approval {
        let approved_and_claimed: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                      FROM approvals a
                      JOIN tool_calls approved_tool ON approved_tool.id=a.tool_call_id
                     WHERE approved_tool.run_id=?1
                       AND approved_tool.runtime_tool_call_id=?2
                       AND a.status='approved'
                       AND a.category='tool_execution'
                       AND a.claimed_at IS NOT NULL
                       AND a.claimed_by_run_id=?1
                       AND json_extract(a.decision_json,'$.approved')=1
                       AND json_extract(a.decision_json,'$.scope') IN ('once','conversation')
                 )",
                params![run_id, runtime_tool_call_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !approved_and_claimed {
            return Err("approval_required".into());
        }
    }
    let action_class = operation_class(&tool, &canonical_input_json);
    if action_class != ActionClass::Write {
        return Err("legacy file tool is not classified as a write".into());
    }
    let project_root = binding
        .permission
        .project_root
        .as_deref()
        .ok_or_else(|| "legacy file credential has no frozen project root".to_owned())?;
    let resolver = crate::runtime_host::managed_files::HostFileTargetResolver {
        project_root: std::path::Path::new(project_root),
    };
    let target_identity = resolver
        .resolve(action_class, &canonical_input_json)
        .ok_or_else(|| "legacy file credential target is invalid".to_owned())?;
    let file_baseline = host_observation_for_in_tx(transaction, run_id, &target_identity)?
        .ok_or_else(|| "observation_incomplete".to_owned())?;
    if file_baseline.observed_by_tool_call_id.trim().is_empty() {
        return Err("observation_incomplete".into());
    }
    let parent_generation: Option<u64> = transaction
        .query_row(
            "SELECT COALESCE(g.generation,0)
               FROM runs child
               JOIN runs parent ON parent.id=child.parent_run_id
               LEFT JOIN kernel_parent_generations g ON g.run_id=parent.id
              WHERE child.id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let credential = ExecutionCredential::new(
        encode_dispatch_id(run_id, runtime_tool_call_id)?,
        run_id.to_owned(),
        conversation_id,
        launch_params_hash(&tool, &canonical_input_json),
        action_class,
        Some(file_baseline),
        binding.execution_profile_id,
        binding.permission_snapshot_id,
        Some(current_policy_version),
        binding.budgets.tool_execution_ms,
        parent_generation,
        unverified_backend_requirement(),
    )?;
    issue_execution_credential_in_tx(transaction, &credential, now)?;
    read_credential_in_tx(transaction, run_id, &credential.dispatch_id)?
        .ok_or_else(|| "legacy credential was not persisted".to_owned())
}

/// Evaluate the credential's real-time requirements against the facts that
/// exist today. Returns the refusal code of the FIRST unsatisfied requirement.
/// A missing source is unsatisfied (fail-closed), never silently satisfied.
fn unsatisfied_requirement(
    transaction: &Transaction<'_>,
    run_id: &str,
    credential: &ExecutionCredential,
) -> Result<Option<&'static str>, rusqlite::Error> {
    for requirement in &credential.realtime_requirements {
        match requirement {
            RealtimeRequirement::Cancellation => {
                let cancelled: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status='cancelled')",
                        params![run_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(false);
                if cancelled {
                    return Ok(Some("cancelled"));
                }
            }
            RealtimeRequirement::ResourceGrant => {
                // The scope grant is verified by the Host policy layer before the
                // claim; the credential binds the scope identity and the Host
                // gateway re-checks it at proposal/admission time. Nothing is
                // synthesised here.
            }
            RealtimeRequirement::PolicyVersion => {
                let current: Option<u64> = transaction
                    .query_row(
                        "SELECT version FROM kernel_execution_policies WHERE conversation_id=?1",
                        [&credential.conversation_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if credential.policy_version.is_none() || current.is_none() {
                    return Ok(Some("policy_version_unavailable"));
                }
                if current != credential.policy_version {
                    return Ok(Some("policy_version_conflict"));
                }
                let mode: String = transaction.query_row(
                    "SELECT mode FROM kernel_execution_policies WHERE conversation_id=?1",
                    [&credential.conversation_id],
                    |r| r.get(0),
                )?;
                if mode == "read_only"
                    && !matches!(
                        credential.action_class,
                        ActionClass::Read | ActionClass::Manage
                    )
                {
                    return Ok(Some("policy_denied"));
                }
            }
            RealtimeRequirement::BackendEvidence => {
                if credential.backend_requirement.evidence_digest.is_none() {
                    return Ok(Some("sandbox_unavailable"));
                }
            }
            RealtimeRequirement::HostObservation => {
                let Some(baseline) = credential.file_baseline.as_ref() else {
                    return Ok(Some("observation_incomplete"));
                };
                let observed =
                    host_observation_for_in_tx(transaction, run_id, &baseline.target_identity)
                        .map_err(rusqlite::Error::InvalidParameterName)?;
                if observed.as_ref() != Some(baseline) {
                    return Ok(Some("observation_conflict"));
                }
            }
            RealtimeRequirement::ParentRevocation => {
                let current: Option<(u64,String)> = transaction.query_row(
                    "SELECT COALESCE(g.generation,0),parent.status FROM runs child JOIN runs parent ON parent.id=child.parent_run_id LEFT JOIN kernel_parent_generations g ON g.run_id=parent.id WHERE child.id=?1",[run_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                match current {
                    Some((generation, status))
                        if Some(generation) == credential.parent_revocation_generation
                            && status != "cancelled" => {}
                    Some(_) => return Ok(Some("parent_revoked")),
                    None => {
                        let has_parent:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND parent_run_id IS NOT NULL)",[run_id],|r|r.get(0))?;
                        if has_parent || credential.parent_revocation_generation.is_some() {
                            return Ok(Some("parent_revocation_unavailable"));
                        }
                    }
                }
            }
        }
    }
    if !matches!(
        credential.action_class,
        ActionClass::Read | ActionClass::Manage
    ) {
        let json: Option<String> = transaction
            .query_row(
                "SELECT binding_json FROM run_control_bindings WHERE run_id=?1",
                [run_id],
                |r| r.get(0),
            )
            .optional()?;
        let live_budget = json
            .and_then(|value| {
                serde_json::from_str::<fox_engine_protocol::RunControlBinding>(&value).ok()
            })
            .map(|binding| binding.budgets.tool_execution_ms);
        if credential.budget_ceiling_ms <= 0 || live_budget != Some(credential.budget_ceiling_ms) {
            return Ok(Some("budget_binding_unavailable"));
        }
    }
    Ok(None)
}

/// Content digest of one logical launch request (tool + canonical input).
pub fn launch_params_hash(tool: &str, canonical_input_json: &str) -> String {
    canonical_digest(&[("tool", tool), ("input", canonical_input_json)])
}

/// Backend requirement placeholder used until a backend is actually verified.
/// Evidence is never synthesized.
pub fn unverified_backend_requirement() -> BackendRequirement {
    BackendRequirement {
        required: "verified-backend".to_owned(),
        evidence_digest: None,
    }
}

/// The single classification rule shared by issuance and verification.
/// Unknown tools fail closed as the execution class. A `run_command` whose
/// action is a safe management operation (status/output/cancel) is `Manage`:
/// it never starts an external process and therefore does not require a
/// verified backend; its run/job scope checks still apply.
pub fn operation_class(tool: &str, canonical_input_json: &str) -> ActionClass {
    if tool == "run_command" {
        let action = serde_json::from_str::<serde_json::Value>(canonical_input_json)
            .ok()
            .and_then(|input| {
                input
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "sync".to_owned());
        if matches!(action.as_str(), "status" | "output" | "cancel") {
            return ActionClass::Manage;
        }
    }
    action_class_for_tool(tool)
}

/// The single classification rule from the canonical tool contract.
pub fn action_class_for_tool(tool: &str) -> ActionClass {
    let category = fox_engine_protocol::canonical_runtime_tool_contract(tool)
        .map(|(category, _, _)| category)
        .unwrap_or("process");
    ActionClass::from_tool_category(category)
}

/// Record an authoritative Host observation of a file target. Only a durable
/// Host read record may call this; a model-declared `expectedVersion` never
/// becomes an observation.
pub fn record_host_observation_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    conversation_id: &str,
    observation: &HostObservation,
    now: i64,
) -> Result<(), String> {
    let owned: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND conversation_id=?2)",
            params![run_id, conversation_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !owned
        || observation.target_identity.is_empty()
        || observation.version.is_empty()
        || observation.observed_by_tool_call_id.is_empty()
    {
        return Err("invalid Host observation scope or identity".into());
    }
    transaction
        .execute(
            "INSERT INTO kernel_host_observations
                (run_id, conversation_id, target_identity, version, observed_by_tool_call_id, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(run_id, target_identity) DO UPDATE SET
                version=excluded.version,
                observed_by_tool_call_id=excluded.observed_by_tool_call_id,
                observed_at=excluded.observed_at",
            params![
                run_id,
                conversation_id,
                observation.target_identity,
                observation.version,
                observation.observed_by_tool_call_id,
                now,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Read the authoritative observation for one target identity. The caller
/// (A1) supplies the canonical target identity computed by the Host file
/// layer; this module never canonicalizes paths itself.
pub fn host_observation_for_in_tx(
    transaction: &Transaction<'_>,
    run_id: &str,
    target_identity: &str,
) -> Result<Option<HostObservation>, String> {
    transaction
        .query_row(
            "SELECT target_identity, version, observed_by_tool_call_id
               FROM kernel_host_observations
              WHERE run_id=?1 AND target_identity=?2",
            params![run_id, target_identity],
            |row| {
                Ok(HostObservation {
                    target_identity: row.get(0)?,
                    version: row.get(1)?,
                    observed_by_tool_call_id: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_engine_protocol::{encode_dispatch_id, job_row_key};

    fn fixture() -> (Database, std::path::PathBuf, String, String) {
        let path =
            std::env::temp_dir().join(format!("fox-exec-admission-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conversation = db
            .create_conversation("fox-general", Some("exec admission test"), None, None)
            .unwrap();
        let run = db
            .create_run(&conversation.id, "exec admission run", None)
            .unwrap();
        (db, path, conversation.id, run.run.id)
    }

    fn legacy_file_fixture(
        mode: &str,
        tool_call_id: &str,
        requires_approval: bool,
    ) -> (Database, String, String, u64) {
        let (db, _, conversation, run) = fixture();
        let root = std::env::temp_dir().join(format!("fox-legacy-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root_text = root.to_string_lossy().into_owned();
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE conversations SET project_root=?2 WHERE id=?1",
                params![conversation, root_text],
            )?;
            connection.execute(
                "UPDATE runs SET status='running',started_at=?2 WHERE id=?1",
                params![run, now_ms()],
            )?;
            Ok(())
        })
        .unwrap();
        db.update_conversation_permission_mode(&conversation, mode)
            .unwrap();
        db.freeze_legacy_run_control(&run, "legacy-file-test")
            .unwrap();
        let input = serde_json::json!({"path":"document.txt","content":"new bytes"});
        let record = db
            .create_host_tool_call(
                &run,
                tool_call_id,
                "write_file",
                &input,
                if requires_approval {
                    "pending"
                } else {
                    "running"
                },
                requires_approval,
            )
            .unwrap();
        if requires_approval {
            let approval = db
                .create_approval(
                    &record.id,
                    "write document.txt",
                    &serde_json::json!({"availableDecisions":["allow_once"]}),
                )
                .unwrap();
            db.resolve_approval(&approval.id, crate::database::ApprovalDecision::AllowOnce)
                .unwrap()
                .expect("approval must resolve");
            assert!(db.claim_approved_tool_call(&run, tool_call_id).unwrap());
        }
        let input_json = serde_json::to_string(&input).unwrap();
        let resolver = crate::runtime_host::managed_files::HostFileTargetResolver {
            project_root: root.as_path(),
        };
        let target_identity = resolver
            .resolve(ActionClass::Write, &input_json)
            .expect("Host must resolve the frozen target");
        db.record_host_observation(
            &run,
            &conversation,
            &HostObservation {
                target_identity,
                version: "sha256:before".into(),
                observed_by_tool_call_id: "read-before-write".into(),
            },
        )
        .unwrap();
        let policy_version = db.execution_policy(&conversation).unwrap().version;
        (db, conversation, run, policy_version)
    }

    #[test]
    fn legacy_file_credential_is_issued_from_running_host_facts_once() {
        let (db, conversation, run, policy_version) =
            legacy_file_fixture("allow", "legacy-write", false);
        let issued = db
            .issue_legacy_file_credential(&run, "legacy-write", policy_version)
            .unwrap();
        assert_eq!(issued.run_id, run);
        assert_eq!(issued.conversation_id, conversation);
        assert_eq!(issued.action_class, ActionClass::Write);
        assert_eq!(issued.policy_version, Some(policy_version));
        assert!(issued.file_baseline.is_some());
        assert_eq!(
            db.issue_legacy_file_credential(&run, "legacy-write", policy_version)
                .unwrap(),
            issued,
            "a repeated delivery must return the exact stored credential"
        );
    }

    #[test]
    fn legacy_file_credential_requires_a_consumed_approval_in_ask_mode() {
        let (db, _, run, policy_version) =
            legacy_file_fixture("ask", "legacy-approved-write", true);
        assert!(db
            .issue_legacy_file_credential(&run, "legacy-approved-write", policy_version)
            .is_ok());

        let (unapproved, _, unapproved_run, unapproved_version) =
            legacy_file_fixture("ask", "legacy-unapproved-write", false);
        assert!(unapproved
            .issue_legacy_file_credential(
                &unapproved_run,
                "legacy-unapproved-write",
                unapproved_version,
            )
            .unwrap_err()
            .contains("approval_required"));
    }

    #[test]
    fn legacy_file_credential_refuses_read_only_and_stale_policy_versions() {
        let (db, _, run, version) =
            legacy_file_fixture("read_only", "legacy-read-only-write", false);
        assert!(db
            .issue_legacy_file_credential(&run, "legacy-read-only-write", version)
            .unwrap_err()
            .contains("policy_denied"));

        let (db, _, run, version) = legacy_file_fixture("allow", "legacy-stale-write", false);
        assert!(db
            .issue_legacy_file_credential(&run, "legacy-stale-write", version + 1)
            .unwrap_err()
            .contains("policy_version_conflict"));
    }

    fn credential_for(
        run_id: &str,
        conversation_id: &str,
        call: &str,
        action_class: ActionClass,
        file_baseline: Option<HostObservation>,
        policy_version: Option<u64>,
    ) -> ExecutionCredential {
        ExecutionCredential::new(
            encode_dispatch_id(run_id, call).unwrap(),
            run_id.to_owned(),
            conversation_id.to_owned(),
            if action_class == ActionClass::Write {
                launch_params_hash("write_file", "{\"path\":\"a.txt\"}")
            } else {
                launch_params_hash("read", "{}")
            },
            action_class,
            file_baseline,
            "legacy".to_owned(),
            "sha256:policy".to_owned(),
            policy_version,
            60_000,
            None,
            unverified_backend_requirement(),
        )
        .unwrap()
    }

    /// A read-class credential: the only class whose real-time requirements
    /// are satisfiable today (no backend, no observation, no policy version
    /// fact exists yet).
    fn read_credential(run_id: &str, conversation_id: &str, call: &str) -> ExecutionCredential {
        credential_for(run_id, conversation_id, call, ActionClass::Read, None, None)
    }

    /// The ORIGINAL B1a v74 credential table (pre-Manage CHECK), recreated
    /// exactly as MIGRATION_74 defined it, plus a recorded schema_migrations
    /// row at version 74 so the upgrade path is the real one.
    const V74_CREDENTIALS_TABLE: &str = "CREATE TABLE IF NOT EXISTS kernel_execution_credentials (
        run_id TEXT NOT NULL,
        dispatch_id TEXT NOT NULL,
        conversation_id TEXT NOT NULL,
        intent_digest TEXT NOT NULL,
        action_class TEXT NOT NULL CHECK(action_class IN ('read','write','execute','destructive','sensitive_egress')),
        file_baseline TEXT,
        resolved_profile TEXT NOT NULL,
        policy_snapshot_id TEXT NOT NULL,
        backend_required TEXT NOT NULL,
        backend_evidence_digest TEXT,
        credential_digest TEXT NOT NULL,
        credential_json TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY(run_id, dispatch_id)
    )";

    #[test]
    fn issuance_consumes_a_recorded_host_observation_for_the_resolved_target() {
        // R2 item 3: a NEW dispatch binds its file baseline from an authoritative
        // Host observation for the target the Host resolver derived. Writing the
        // observation table alone is not enough; issuance must read it.
        let (db, path, conversation, run) = fixture();
        let mut budgets = fox_engine_protocol::TimeBudgets::default();
        budgets.tool_execution_ms = 30_000;
        db.freeze_kernel_run_control(&run, "durable", budgets)
            .unwrap();
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_runs(run_id, engine_id, kernel_mode, capability_manifest_version,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash, frozen_config_json,
                     state, last_event_seq, created_at, updated_at, terminal_at)
                 VALUES (?1,'kernel','authoritative',1,'perm','durable','h','{}','running',0,1,1,NULL)",
                rusqlite::params![run],
            )?;
            Ok(())
        })
        .unwrap();
        // The Host file layer records a real read observation for the target.
        db.record_host_observation(
            &run,
            &conversation,
            &HostObservation {
                target_identity: "notes/a.txt".into(),
                version: "v7".into(),
                observed_by_tool_call_id: "read-9".into(),
            },
        )
        .unwrap();

        // A resolver standing in for A's canonical target mapping.
        struct NotesResolver;
        impl FileTargetResolver for NotesResolver {
            fn resolve(&self, class: ActionClass, canonical_input_json: &str) -> Option<String> {
                if class != ActionClass::Write {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(canonical_input_json)
                    .ok()
                    .and_then(|input| {
                        input
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(|path| format!("notes/{path}"))
                    })
            }
        }

        let issued = db
            .with_connection(|c| {
                let transaction =
                    c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let issued = issue_dispatch_credential_with_resolver_in_tx(
                    &transaction,
                    &run,
                    "call-1",
                    "write_file",
                    "{\"path\":\"a.txt\",\"expectedVersion\":\"v1-model-claim\"}",
                    1,
                    &NotesResolver,
                )
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
                transaction.commit()?;
                Ok(issued)
            })
            .unwrap()
            .expect("a write with a resolvable target is issued");
        // The baseline is the HOST OBSERVATION, never the model's declared version.
        assert_eq!(
            issued.file_baseline,
            Some(HostObservation {
                target_identity: "notes/a.txt".into(),
                version: "v7".into(),
                observed_by_tool_call_id: "read-9".into(),
            })
        );
        assert_ne!(
            issued
                .file_baseline
                .as_ref()
                .map(|observation| observation.version.clone()),
            Some("v1-model-claim".to_owned()),
            "the model's declared version must never become the baseline"
        );

        // A write whose target has no recorded observation keeps `None` and is
        // therefore refused at claim time (fail-closed).
        let unobserved = db
            .with_connection(|c| {
                let transaction =
                    c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let issued = issue_dispatch_credential_with_resolver_in_tx(
                    &transaction,
                    &run,
                    "call-2",
                    "write_file",
                    "{\"path\":\"b.txt\"}",
                    1,
                    &NotesResolver,
                )
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
                transaction.commit()?;
                Ok(issued)
            })
            .unwrap()
            .expect("the dispatch is issued");
        assert_eq!(unobserved.file_baseline, None);
        let outcome = db
            .claim_execution_attempt(
                &run,
                &conversation,
                &unobserved,
                &launch_params_hash("write_file", "{\"path\":\"b.txt\"}"),
                "owner-a",
            )
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "observation_incomplete".to_owned()
            }
        );

        // Observation does not bypass a live policy change after issuance.
        assert!(issued.policy_version.is_some());
        let policy = db.execution_policy(&conversation).unwrap();
        db.change_execution_policy(
            &conversation,
            "change-after-issue",
            policy.version,
            if policy.mode == "allow" {
                "ask"
            } else {
                "allow"
            },
        )
        .unwrap();
        let outcome = db
            .claim_execution_attempt(
                &run,
                &conversation,
                &issued,
                &launch_params_hash(
                    "write_file",
                    "{\"path\":\"a.txt\",\"expectedVersion\":\"v1-model-claim\"}",
                ),
                "owner-a",
            )
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "policy_version_conflict".to_owned()
            }
        );

        // Re-issuing the same dispatch with a DIFFERENT observation is refused:
        // the frozen snapshot is never re-signed.
        let mut tampered = issued.clone();
        tampered.file_baseline = Some(HostObservation {
            target_identity: "notes/a.txt".into(),
            version: "v8".into(),
            observed_by_tool_call_id: "read-10".into(),
        });
        tampered.credential_digest = tampered.recompute_digest();
        assert!(db.issue_execution_credential(&tampered).is_err());
        drop(db);
        let reopened = Database::open(path).unwrap();
        let stored = reopened
            .read_execution_credential(&run, &issued.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored, issued,
            "the issued snapshot is durable and unchanged"
        );
    }

    #[test]
    fn an_upgraded_v74_database_accepts_a_manage_credential_and_keeps_old_rows() {
        // R1-02: a database created by the ORIGINAL v74 schema must be able to
        // issue a Manage credential after the additive migration, while its
        // historical rows stay byte-identical and gain no authorization.
        let path =
            std::env::temp_dir().join(format!("fox-v74-upgrade-{}.db", uuid::Uuid::new_v4()));
        let (conversation_id, run_id) = {
            let db = Database::open(path.clone()).unwrap();
            let conversation = db
                .create_conversation("fox-general", Some("v74 upgrade"), None, None)
                .unwrap();
            let run = db.create_run(&conversation.id, "v74 run", None).unwrap();
            (conversation.id, run.run.id)
        };
        // Downgrade the credentials table to the exact v74 definition and mark
        // version 74 as applied (dropping 75) — the real pre-R2 database state.
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "DROP TABLE kernel_execution_credentials;
                     DELETE FROM schema_migrations WHERE version = 75;",
                )
                .unwrap();
            connection.execute_batch(V74_CREDENTIALS_TABLE).unwrap();
            // A historical (pre-R2) credential row, exactly as v74 stored it.
            let historical = read_credential(&run_id, &conversation_id, "legacy-call");
            connection
                .execute(
                    "INSERT INTO kernel_execution_credentials
                        (run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
                         resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
                         credential_digest, credential_json, created_at)
                     VALUES (?1,?2,?3,?4,'read',NULL,'legacy','sha256:policy','verified-backend',NULL,?5,?6,1)",
                    rusqlite::params![
                        run_id,
                        historical.dispatch_id,
                        conversation_id,
                        historical.intent_digest,
                        historical.credential_digest,
                        serde_json::to_string(&historical).unwrap(),
                    ],
                )
                .unwrap();
        }
        // Sanity: the OLD constraint really rejects Manage before the upgrade.
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let rejected = connection.execute(
                "INSERT INTO kernel_execution_credentials
                    (run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
                     resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
                     credential_digest, credential_json, created_at)
                 VALUES ('r','tool-dispatch:1:r:c','c','d','manage',NULL,'p','s','verified-backend',NULL,'g','{}',1)",
                [],
            );
            assert!(
                rejected.is_err(),
                "the original v74 CHECK must reject Manage"
            );
        }

        // Open through the real migration path (74 → 75 rebuild).
        let db = Database::open(path.clone()).unwrap();
        let recorded: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(
            recorded,
            crate::database::DATABASE_SCHEMA_VERSION,
            "the database must finish the complete current migration chain"
        );
        let migration_75_rows: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version=75",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(
            migration_75_rows, 1,
            "the additive migration must be recorded once"
        );
        // The historical row survived the rebuild unchanged.
        let stored = db
            .read_execution_credential(
                &run_id,
                &encode_dispatch_id(&run_id, "legacy-call").unwrap(),
            )
            .unwrap()
            .expect("the historical credential must survive the upgrade");
        assert_eq!(stored.action_class, ActionClass::Read);
        assert_eq!(
            stored.credential_digest,
            read_credential(&run_id, &conversation_id, "legacy-call").credential_digest
        );
        // A Manage credential can now be issued on the upgraded database.
        let manage = credential_for(
            &run_id,
            &conversation_id,
            "manage-call",
            ActionClass::Manage,
            None,
            None,
        );
        db.issue_execution_credential(&manage).unwrap();
        let round_trip = db
            .read_execution_credential(&run_id, &manage.dispatch_id)
            .unwrap()
            .expect("the Manage credential must be durable");
        assert_eq!(round_trip, manage);
        // Reopen: idempotent, and the constraint still holds.
        drop(db);
        let db = Database::open(path.clone()).unwrap();
        assert!(db
            .read_execution_credential(&run_id, &manage.dispatch_id)
            .unwrap()
            .is_some());
        // No authorization was backfilled: a dispatch with no credential is
        // still unclaimable.
        assert!(db
            .read_execution_credential(
                &run_id,
                &encode_dispatch_id(&run_id, "never-issued").unwrap()
            )
            .unwrap()
            .is_none());
        // The old class set is still enforced.
        let bogus = ExecutionCredential::new(
            encode_dispatch_id(&run_id, "bogus").unwrap(),
            run_id.clone(),
            conversation_id.clone(),
            "sha256:intent".into(),
            ActionClass::Read,
            None,
            "legacy".into(),
            "sha256:policy".into(),
            None,
            60_000,
            None,
            unverified_backend_requirement(),
        )
        .unwrap();
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_execution_credentials
                    (run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
                     resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
                     credential_digest, credential_json, created_at)
                 VALUES (?1,?2,?3,?4,'not-a-class',NULL,'p','s','verified-backend',NULL,'g','{}',1)",
                rusqlite::params![bogus.run_id, bogus.dispatch_id],
            )
        })
        .unwrap_err();
    }

    #[test]
    fn migration_74_upgrade_is_repeatable_and_adds_no_authorization() {
        let (db, path, conversation, run) = fixture();
        let before: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_execution_credentials",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(before, 0);
        // Reopen: migration 75 is idempotent and the tables survive.
        drop(db);
        let db = Database::open(path.clone()).unwrap();
        let credential = read_credential(&run, &conversation, "call-1");
        db.issue_execution_credential(&credential).unwrap();
        drop(db);
        let db = Database::open(path).unwrap();
        // A dispatch without a credential cannot start; nothing was backfilled.
        assert!(db
            .read_execution_credential(&run, "tool-dispatch:9:run-model:absent")
            .unwrap()
            .is_none());
        assert!(db
            .claim_execution_attempt(
                &run,
                &conversation,
                &read_credential(&run, &conversation, "absent"),
                &launch_params_hash("read", "{}"),
                "owner-a",
            )
            .is_err());
        assert_eq!(
            db.read_execution_credential(&run, &credential.dispatch_id)
                .unwrap()
                .unwrap(),
            credential
        );
    }

    #[test]
    fn credential_is_issued_once_and_never_re_signed() {
        let (db, _path, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "call-1");
        db.issue_execution_credential(&credential).unwrap();
        // Byte-identical re-issue is a no-op.
        db.issue_execution_credential(&credential).unwrap();
        // Same dispatch, different fixed field, recomputed self-consistent digest.
        let mut hostile = credential.clone();
        hostile.resolved_profile = "durable_v2".to_owned();
        hostile.credential_digest = hostile.recompute_digest();
        assert!(db.issue_execution_credential(&hostile).is_err());
        assert!(db.verify_execution_credential(&run, &hostile).is_err());
        db.verify_execution_credential(&run, &credential).unwrap();
    }

    #[test]
    fn independent_connections_claim_exactly_once() {
        let (db_a, path, conversation, run) = fixture();
        let mut credential = read_credential(&run, &conversation, "race-1");
        credential.intent_digest = launch_params_hash("read", "{\"path\":\"a.txt\"}");
        credential.credential_digest = credential.recompute_digest();
        db_a.issue_execution_credential(&credential).unwrap();
        // A second, independent connection to the same database file.
        let db_b = Database::open(path).unwrap();
        let params = launch_params_hash("read", "{\"path\":\"a.txt\"}");
        let first = db_a
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
            .unwrap();
        let second = db_b
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-b")
            .unwrap();
        assert_eq!(first, AttemptOutcome::Claimed);
        assert_ne!(first, second);
        // Repeat delivery on the winner's connection is read-only too.
        let third = db_a
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
            .unwrap();
        assert_ne!(third, AttemptOutcome::Claimed);
    }

    #[test]
    fn same_key_different_content_conflicts() {
        let (db, _path, conversation, run) = fixture();
        let mut credential = read_credential(&run, &conversation, "content-1");
        credential.intent_digest = launch_params_hash("read", "{\"path\":\"a.txt\"}");
        credential.credential_digest = credential.recompute_digest();
        db.issue_execution_credential(&credential).unwrap();
        let params_a = launch_params_hash("read", "{\"path\":\"a.txt\"}");
        let params_b = launch_params_hash("read", "{\"path\":\"b.txt\"}");
        assert_eq!(
            db.claim_execution_attempt(&run, &conversation, &credential, &params_a, "owner-a")
                .unwrap(),
            AttemptOutcome::Claimed
        );
        assert!(
            db_b_claim_same_dispatch(&db, &run, &conversation, &credential, &params_b).is_err()
        );
    }

    fn db_b_claim_same_dispatch(
        db: &Database,
        run_id: &str,
        conversation_id: &str,
        credential: &ExecutionCredential,
        params_hash: &str,
    ) -> Result<AttemptOutcome, String> {
        db.claim_execution_attempt(run_id, conversation_id, credential, params_hash, "owner-b")
    }

    #[test]
    fn cross_run_same_tool_call_id_does_not_collide() {
        let (db, _path, conversation_a, run_a) = fixture();
        let conversation_b = db
            .create_conversation("fox-general", Some("second conversation"), None, None)
            .unwrap();
        let run_b = db
            .create_run(&conversation_b.id, "second run", None)
            .unwrap();
        let cred_a = read_credential(&run_a, &conversation_a, "shared-call");
        let cred_b = read_credential(&run_b.run.id, &conversation_b.id, "shared-call");
        assert_ne!(cred_a.dispatch_id, cred_b.dispatch_id);
        assert_ne!(
            job_row_key(&cred_a.dispatch_id),
            job_row_key(&cred_b.dispatch_id)
        );
        db.issue_execution_credential(&cred_a).unwrap();
        db.issue_execution_credential(&cred_b).unwrap();
        let params = launch_params_hash("read", "{}");
        assert_eq!(
            db.claim_execution_attempt(&run_a, &conversation_a, &cred_a, &params, "owner-a")
                .unwrap(),
            AttemptOutcome::Claimed
        );
        assert_eq!(
            db.claim_execution_attempt(
                &run_b.run.id,
                &conversation_b.id,
                &cred_b,
                &params,
                "owner-b"
            )
            .unwrap(),
            AttemptOutcome::Claimed
        );
    }

    #[test]
    fn refusal_is_persistent_and_survives_reopen_without_a_job_row() {
        let (db, path, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "refuse-1");
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("read", "{}");
        assert_eq!(
            db.claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
                .unwrap(),
            AttemptOutcome::Claimed
        );
        db.record_attempt_refusal(&run, &credential.dispatch_id, "sandbox_unavailable")
            .unwrap();
        drop(db);
        // Reopen: the refusal is still readable and the dispatch cannot restart.
        let db = Database::open(path).unwrap();
        let outcome = db
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-b")
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "sandbox_unavailable".to_owned()
            }
        );
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.stage, ExecutionStage::Admitted);
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(receipt.reason_code.as_deref(), Some("sandbox_unavailable"));
        assert_eq!(receipt.control_plane, ControlPlaneState::Prepared);
        // No job row was fabricated to carry the refusal.
        assert!(
            db.with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1",
                    params![run],
                    |r| r.get::<_, i64>(0),
                )
            })
            .unwrap()
                == 0
        );
    }

    #[test]
    fn interrupted_claim_stays_unknown_and_never_replays() {
        let (db, path, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "crash-1");
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("read", "{}");
        assert_eq!(
            db.claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
                .unwrap(),
            AttemptOutcome::Claimed
        );
        drop(db);
        // Simulated interruption: reopen and mark the claimed attempt unknown.
        let db = Database::open(path).unwrap();
        db.mark_attempt_unknown(&run, &credential.dispatch_id)
            .unwrap();
        let outcome = db
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-b")
            .unwrap();
        assert_eq!(outcome, AttemptOutcome::Unknown);
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.stage, ExecutionStage::Interrupted);
        assert_eq!(receipt.execution_started, TriState::Unknown);
        assert_eq!(
            receipt.code_alias.as_deref(),
            Some("job.interrupted_unknown")
        );
        assert!(!receipt.allow_replay);
    }

    #[test]
    fn confirmed_start_is_never_regressed() {
        let (db, _path, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "start-1");
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("read", "{}");
        db.claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
            .unwrap();
        db.record_attempt_started(&run, &credential.dispatch_id)
            .unwrap();
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.execution_started, TriState::True);
        // A later failure path cannot flip the confirmed start back to false.
        assert!(receipt
            .clone()
            .with_execution_started(TriState::False)
            .is_err());
        // Recovery keeps the CONFIRMED START: only the outcome becomes unknown.
        db.mark_attempt_unknown(&run, &credential.dispatch_id)
            .unwrap();
        let row = db
            .read_execution_attempt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.start_confirmed, Some(true));
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.execution_started, TriState::True);
        assert_eq!(receipt.external_effect, SideEffectState::Uncertain);
        assert_eq!(
            receipt.code_alias.as_deref(),
            Some("job.interrupted_unknown")
        );
        assert!(!receipt.allow_replay);
    }

    #[test]
    fn claim_is_not_a_start_confirmation() {
        let (db, _path, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "claim-only");
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("read", "{}");
        assert_eq!(
            db.claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
                .unwrap(),
            AttemptOutcome::Claimed
        );
        let row = db
            .read_execution_attempt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        // A claim alone is NOT evidence that the target operation started.
        assert_eq!(row.start_confirmed, None);
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.stage, ExecutionStage::Admitted);
        assert_ne!(receipt.stage, ExecutionStage::JobCreated);
        assert_eq!(receipt.execution_started, TriState::Unknown);
        assert_eq!(receipt.control_plane, ControlPlaneState::Applying);
        // A start record requires the claimed state and real evidence.
        db.record_attempt_started(&run, &credential.dispatch_id)
            .unwrap();
        assert!(db
            .record_attempt_started(&run, &credential.dispatch_id)
            .is_err());
    }

    #[test]
    fn write_without_host_observation_is_refused() {
        let (db, _path, conversation, run) = fixture();
        // A write whose baseline is only a model-declared version (no Host
        // observation) must be refused at claim time; the refusal is durable.
        let credential = credential_for(
            &run,
            &conversation,
            "write-1",
            ActionClass::Write,
            None,
            None,
        );
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("write_file", "{\"path\":\"a.txt\"}");
        let outcome = db
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "observation_incomplete".to_owned()
            }
        );
        // The attempt is finished: a later claim reads the SAME refusal and
        // never starts the write (the first refusal is terminal).
        let outcome = db
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-b")
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "observation_incomplete".to_owned()
            }
        );
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.kind, ExecutionKind::File);
        // A refused file action has not reached any file stage yet.
        assert_eq!(receipt.stage, ExecutionStage::FilePrepared);
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(
            receipt.reason_code.as_deref(),
            Some("observation_incomplete")
        );
        // No job row was fabricated to carry the refusal.
        assert!(
            db.with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1",
                    params![run],
                    |r| r.get::<_, i64>(0),
                )
            })
            .unwrap()
                == 0
        );
        // Even with an observation the missing live policy version still refuses.
        let credential = credential_for(
            &run,
            &conversation,
            "write-2",
            ActionClass::Write,
            Some(HostObservation {
                target_identity: "a.txt".into(),
                version: "v1".into(),
                observed_by_tool_call_id: "read-1".into(),
            }),
            None,
        );
        db.record_host_observation(
            &run,
            &conversation,
            credential.file_baseline.as_ref().unwrap(),
        )
        .unwrap();
        db.issue_execution_credential(&credential).unwrap();
        let outcome = db
            .claim_execution_attempt(&run, &conversation, &credential, &params, "owner-a")
            .unwrap();
        assert_eq!(
            outcome,
            AttemptOutcome::AlreadyRefused {
                code: "policy_version_unavailable".to_owned()
            }
        );
    }

    #[test]
    fn dispatch_and_credential_roll_back_together_in_the_real_transaction() {
        // The dispatch fact and its credential must share one fate inside the
        // REAL admission transaction. Here the credential write is made to fail
        // by a durable trigger (the same way any mid-transaction failure would),
        // and the assertion is that the dispatch row does NOT survive.
        let (db, path, conversation, run) = fixture();
        // A frozen binding so the run resolves to a durable conversation.
        let mut budgets = fox_engine_protocol::TimeBudgets::default();
        budgets.tool_execution_ms = 30_000;
        db.freeze_kernel_run_control(&run, "durable", budgets)
            .unwrap();
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_runs(run_id, engine_id, kernel_mode, capability_manifest_version,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash, frozen_config_json,
                     state, last_event_seq, created_at, updated_at, terminal_at)
                 VALUES (?1,'kernel','authoritative',1,'perm','durable','h','{}','running',0,1,1,NULL)",
                rusqlite::params![run],
            )?;
            Ok(())
        })
        .unwrap();
        // Fail any credential insert for THIS dispatch (its real encoded id).
        let expected_dispatch_id = encode_dispatch_id(&run, "call-1").unwrap();
        db.execute_raw_sql(&format!(
            "CREATE TRIGGER reject_execution_credential
             BEFORE INSERT ON kernel_execution_credentials
             WHEN NEW.dispatch_id = '{expected_dispatch_id}'
             BEGIN SELECT RAISE(ABORT, 'injected credential failure'); END"
        ))
        .unwrap();
        let error = db
            .with_connection(|c| {
                let transaction = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                // The dispatch fact would be written first, exactly like the
                // real admission transaction does.
                transaction.execute(
                    "INSERT INTO kernel_effect_outbox
                        (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                         payload_json, status, attempts, lease_owner, leased_at, completed_at,
                         last_error, created_at, updated_at)
                     VALUES (?1,'dispatch:call-1','dispatch_tool','tool-dispatch:call-1','call-1','b1','{}','pending',0,NULL,NULL,NULL,NULL,1,1)",
                    rusqlite::params![run],
                )?;
                let issued = super::issue_dispatch_credential_in_tx(
                    &transaction,
                    &run,
                    "call-1",
                    "read",
                    "{\"path\":\"a.txt\"}",
                    1,
                );
                let issued_error = issued.err();
                // Roll the whole admission transaction back on the credential
                // failure, exactly like the production caller does.
                if issued_error.is_some() {
                    return Ok(Err(issued_error));
                }
                transaction.commit()?;
                Ok(Ok(()))
            })
            .unwrap();
        assert!(
            error.is_err(),
            "the credential failure must reach the caller"
        );
        // NEITHER the dispatch row NOR the credential row survived.
        let outbox: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='dispatch:call-1'",
                    rusqlite::params![run],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(
            outbox, 0,
            "the dispatch fact must not survive a failed credential"
        );
        let credentials: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_execution_credentials WHERE run_id=?1",
                    rusqlite::params![run],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(credentials, 0);
        // With the trigger removed the same transaction commits both.
        db.execute_raw_sql("DROP TRIGGER reject_execution_credential")
            .unwrap();
        let outcome = db
            .with_connection(|c| {
                let transaction =
                    c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                transaction.execute(
                    "INSERT INTO kernel_effect_outbox
                        (run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
                         payload_json, status, attempts, lease_owner, leased_at, completed_at,
                         last_error, created_at, updated_at)
                     VALUES (?1,'dispatch:call-1','dispatch_tool','tool-dispatch:call-1','call-1','b1','{}','pending',0,NULL,NULL,NULL,NULL,1,1)",
                    rusqlite::params![run],
                )?;
                super::issue_dispatch_credential_in_tx(
                    &transaction,
                    &run,
                    "call-1",
                    "read",
                    "{\"path\":\"a.txt\"}",
                    1,
                )
                .map_err(|error| rusqlite::Error::InvalidParameterName(error))?;
                transaction.commit()?;
                Ok::<(), rusqlite::Error>(())
            })
            .expect("the un-failed transaction commits both facts");
        let outbox: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='dispatch:call-1'",
                    rusqlite::params![run],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(outbox, 1, "both facts land together when nothing fails");
        drop(db);
        let reopened = Database::open(path).unwrap();
        let stored = reopened
            .read_execution_credential(&run, &encode_dispatch_id(&run, "call-1").unwrap())
            .unwrap();
        assert!(stored.is_some(), "the credential is durable");
    }

    #[test]
    fn host_observations_are_authoritative_and_model_claims_are_not() {
        let (db, _path, conversation, run) = fixture();
        // Nothing is recorded by issuance.
        assert!(db.host_observation_for(&run, "a.txt").unwrap().is_none());
        let observation = HostObservation {
            target_identity: "a.txt".into(),
            version: "v7".into(),
            observed_by_tool_call_id: "read-9".into(),
        };
        db.record_host_observation(&run, &conversation, &observation)
            .unwrap();
        let stored = db.host_observation_for(&run, "a.txt").unwrap().unwrap();
        assert_eq!(stored, observation);
        // A different target is a different observation.
        assert!(db.host_observation_for(&run, "b.txt").unwrap().is_none());
    }

    #[test]
    fn manage_actions_do_not_require_a_backend() {
        // Only an external launch requires the verified backend; the safe
        // management surface stays available with its scope checks.
        assert_eq!(
            operation_class("run_command", "{\"command\":\"echo hi\"}"),
            ActionClass::Execute
        );
        assert_eq!(
            operation_class(
                "run_command",
                "{\"action\":\"start\",\"command\":\"echo hi\"}"
            ),
            ActionClass::Execute
        );
        assert_eq!(
            operation_class(
                "run_command",
                "{\"action\":\"sync\",\"command\":\"echo hi\"}"
            ),
            ActionClass::Execute
        );
        for action in ["status", "output", "cancel"] {
            assert_eq!(
                operation_class(
                    "run_command",
                    &format!("{{\"action\":\"{action}\",\"jobId\":\"j\"}}")
                ),
                ActionClass::Manage,
                "{action} must stay a safe management operation"
            );
        }
        assert!(!ActionClass::Manage.requires_verified_backend());
        assert!(!ActionClass::Manage.requires_host_observation());
        assert!(ActionClass::Execute.requires_verified_backend());
    }

    #[test]
    fn cross_conversation_reads_and_claims_are_refused() {
        let (db, _path, conversation_a, run_a) = fixture();
        let conversation_b = db
            .create_conversation("fox-general", Some("other conversation"), None, None)
            .unwrap();
        let credential = read_credential(&run_a, &conversation_a, "scope-1");
        db.issue_execution_credential(&credential).unwrap();
        let params = launch_params_hash("read", "{}");
        // Claiming under another conversation's scope is refused.
        assert!(db
            .claim_execution_attempt(&run_a, &conversation_b.id, &credential, &params, "owner-b")
            .is_err());
        db.claim_execution_attempt(&run_a, &conversation_a, &credential, &params, "owner-a")
            .unwrap();
        // Scope-checked reads refuse the foreign conversation.
        assert!(db
            .execution_receipt_for_conversation(&run_a, &credential.dispatch_id, &conversation_b.id)
            .is_err());
        assert!(db
            .execution_receipt_for_conversation(&run_a, &credential.dispatch_id, &conversation_a)
            .unwrap()
            .is_some());
    }
    #[test]
    fn live_policy_change_refuses_previously_ready_credential_and_never_replays() {
        let (db, _, conversation, run) = fixture();
        let mut credential = read_credential(&run, &conversation, "live-policy");
        credential.policy_version = Some(db.execution_policy(&conversation).unwrap().version);
        credential
            .realtime_requirements
            .push(RealtimeRequirement::PolicyVersion);
        credential.credential_digest = credential.recompute_digest();
        db.issue_execution_credential(&credential).unwrap();
        db.update_conversation_permission_mode(&conversation, "allow")
            .unwrap();
        let result = db
            .claim_execution_attempt(
                &run,
                &conversation,
                &credential,
                &credential.intent_digest,
                "owner",
            )
            .unwrap();
        assert_eq!(
            result,
            AttemptOutcome::AlreadyRefused {
                code: "policy_version_conflict".into()
            }
        );
        db.update_conversation_permission_mode(&conversation, "ask")
            .unwrap();
        assert_eq!(
            db.claim_execution_attempt(
                &run,
                &conversation,
                &credential,
                &credential.intent_digest,
                "owner"
            )
            .unwrap(),
            result
        );
        assert!(db.revalidate_execution_credential(&credential).is_err());
    }
    #[test]
    fn parent_revocation_is_rechecked_after_claim_and_confirmed_start_is_preserved() {
        let (db, _, conversation, run) = fixture();
        let parent = uuid::Uuid::new_v4().to_string();
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO runs(id,conversation_id,status,model,created_at)
                 VALUES(?1,?2,'running','test-parent',1)",
                params![parent, conversation],
            )?;
            c.execute(
                "UPDATE runs SET parent_run_id=?2 WHERE id=?1",
                params![run, parent],
            )?;
            Ok(())
        })
        .unwrap();
        let mut credential = read_credential(&run, &conversation, "child");
        credential.parent_revocation_generation = Some(0);
        credential
            .realtime_requirements
            .push(RealtimeRequirement::ParentRevocation);
        credential.credential_digest = credential.recompute_digest();
        db.issue_execution_credential(&credential).unwrap();
        assert_eq!(
            db.claim_execution_attempt(
                &run,
                &conversation,
                &credential,
                &credential.intent_digest,
                "owner"
            )
            .unwrap(),
            AttemptOutcome::Claimed
        );
        db.revoke_execution_parent(&parent).unwrap();
        assert!(db
            .revalidate_execution_credential(&credential)
            .unwrap_err()
            .contains("parent_revoked"));
        db.record_attempt_started(&run, &credential.dispatch_id)
            .unwrap();
        db.record_attempt_completed(&run, &credential.dispatch_id, false)
            .unwrap();
        assert_eq!(
            db.execution_receipt(&run, &credential.dispatch_id)
                .unwrap()
                .unwrap()
                .execution_started,
            TriState::True
        );
    }
    #[test]
    fn refusal_after_real_job_creation_preserves_job_created_stage() {
        let (db, _, conversation, run) = fixture();
        let credential = read_credential(&run, &conversation, "refused-after-row");
        db.issue_execution_credential(&credential).unwrap();
        db.claim_execution_attempt(
            &run,
            &conversation,
            &credential,
            &credential.intent_digest,
            "owner",
        )
        .unwrap();
        db.kernel_job_start(&super::super::kernel_jobs::JobStartRequest {
            run_id: run.clone(),
            kind: "command".into(),
            idempotency_key: fox_engine_protocol::job_row_key(&credential.dispatch_id),
            params: serde_json::json!({}),
            deadline_ms: None,
            progress_total: None,
        })
        .unwrap();
        db.record_attempt_refusal(&run, &credential.dispatch_id, "sandbox_unavailable")
            .unwrap();
        let receipt = db
            .execution_receipt(&run, &credential.dispatch_id)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.stage, ExecutionStage::JobCreated);
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(receipt.control_plane, ControlPlaneState::NotApplied);
        assert_eq!(receipt.external_effect, SideEffectState::NotApplied);
    }
}

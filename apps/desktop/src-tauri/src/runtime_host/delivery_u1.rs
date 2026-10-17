//! Package 2-D U1: the rule that decides whether a user's acceptance of an
//! **unverified** delivery still stands, the entry that records one, and the
//! admission rule that decides whether such acceptances may end the Run.
//!
//! # The four questions this module keeps apart
//!
//! CHARTER §53 requires that "execution finished", "verification result", "user
//! accepted" and "may the Run continue/terminate" never collapse into one
//! boolean. This module owns only the third and fourth, and only as they relate
//! to the second:
//!
//! * the **verification result** stays where it was produced — the six-class
//!   decision and its durable envelope. Nothing here writes a class, a fact or a
//!   `finding_json` byte;
//! * the **acceptance** is a separate durable fact with its own table;
//! * the **Run's lifecycle** is decided by the shared route mapping; an
//!   acceptance only ever *permits* `Completed`, it never forces it.
//!
//! # Why validity is recomputed on every read
//!
//! An acceptance is a statement about a specific artifact: these bytes, this
//! managed version, this frozen contract, under this project authorization. All
//! of those can change afterwards. The projection row records what was accepted;
//! this module re-derives, from current durable facts, whether it still holds.
//!
//! The candidate check is the important one. Comparing the acceptance's stored
//! hash against the row's recorded `checkedHash` would compare two **database**
//! columns, both of which survive a file being rewritten underneath them — so
//! the acceptance would keep claiming a file the artifact view already reports
//! as `stale`. The check therefore re-reads the candidate through the same
//! authorized, bounded reader the verification itself uses, and hashes the bytes
//! it actually got.
//!
//! # The version identity (package 2-D repair, D-4)
//!
//! `managed_file_versions.storage_path` is written by the real write gate as an
//! **absolute** path. Both the binding read and the freshness re-check therefore
//! resolve the candidate through the same authorized root and compare the same
//! project-relative identity, and the version lookup itself goes through the
//! absolute path the ledger actually stores. Resolving only one side, or
//! resolving both sides to relative paths that each miss, is exactly the defect
//! this package repairs: it made the version criterion dead code on one path
//! shape and permanently ineffective on the other.

use super::MAX_VERIFY_BYTES;
use super::p2b_consumer::{self, StoredVerdict};
use super::p2b_decision::{DecisionFact, DeliveryClass, FactClass, FactOrigin};
use crate::database::{AcceptanceBinding, AcceptanceRow, AcceptanceWrite, Database};
use crate::runtime_host::attachment_compute::{
    authorized_delivery_root_typed, read_authorized_delivery_bytes_typed,
};
use crate::runtime_host::delivery_checks_v2 as leaf;

// The Kernel controller vocabulary this module commits through. `crate::kernel`
// is the desktop crate's re-export of `fox_agent_kernel`, which owns the Run
// state machine; nothing here redefines a state or an outcome.
use crate::kernel::{DeliveryAcceptanceFact, RunController, RunOutcome, RunState};

/// What a user asked the Host to do about one item's unverified delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAcceptanceDecision {
    /// The user takes responsibility for content that is not verified.
    Accept,
    /// The user states the delivery is not acceptable. A conclusion about the
    /// **execution**, not a verification failure.
    Reject,
    /// The user withdraws an acceptance they previously made.
    Revoke,
}

impl DeliveryAcceptanceDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryAcceptanceDecision::Accept => "accepted",
            DeliveryAcceptanceDecision::Reject => "rejected",
            DeliveryAcceptanceDecision::Revoke => "revoked",
        }
    }
}

/// Parse the wire decision through the typed vocabulary.
///
/// A string that is not one of the three is `None`, never a default: an
/// unrecognised decision must not silently become "accept".
pub fn parse_acceptance_decision(value: &str) -> Option<DeliveryAcceptanceDecision> {
    match value.trim() {
        "accept" => Some(DeliveryAcceptanceDecision::Accept),
        "reject" => Some(DeliveryAcceptanceDecision::Reject),
        "revoke" => Some(DeliveryAcceptanceDecision::Revoke),
        _ => None,
    }
}

/// Why an acceptance is not currently effective.
///
/// Every variant is a distinct answer, and none of them is "the content
/// failed". `is_definite` separates a definite invalidation from one where the
/// current state simply could not be established: an unreadable candidate must
/// never be reported as drift, because that would invent a comparison, and it
/// must never be used to complete a Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceIneffective {
    /// No acceptance row exists, or it was revoked / rejected.
    Absent,
    /// The row is no longer `pending`: a real decision already terminated it.
    RowNotPending { status: String },
    /// The row's durable record is not a decodable v2 envelope naming this row.
    NotAVerifiableRecord,
    /// The row's class is not `Unconfirmed`.
    ClassNotUnconfirmed { class: String },
    /// The row carries a security veto, or a Refused / Invalid fact.
    BlockedByFact { detail: String },
    /// The candidate bytes no longer hash to what was accepted.
    CandidateDrift { expected: String, actual: String },
    /// The candidate could not be re-read under the current authorization.
    CandidateUnreadable { detail: String },
    /// The managed-file version is no longer the accepted one.
    VersionDrift,
    /// The frozen contract no longer hashes to what was accepted.
    ContractDrift { expected: String, actual: String },
    /// The project authorization no longer resolves to the frozen root.
    AuthorizationWithdrawn,
}

impl AcceptanceIneffective {
    pub fn as_str(&self) -> &'static str {
        match self {
            AcceptanceIneffective::Absent => "absent",
            AcceptanceIneffective::RowNotPending { .. } => "row_not_pending",
            AcceptanceIneffective::NotAVerifiableRecord => "not_a_verifiable_record",
            AcceptanceIneffective::ClassNotUnconfirmed { .. } => "class_not_unconfirmed",
            AcceptanceIneffective::BlockedByFact { .. } => "blocked_by_fact",
            AcceptanceIneffective::CandidateDrift { .. } => "candidate_drift",
            AcceptanceIneffective::CandidateUnreadable { .. } => "candidate_unreadable",
            AcceptanceIneffective::VersionDrift => "version_drift",
            AcceptanceIneffective::ContractDrift { .. } => "contract_drift",
            AcceptanceIneffective::AuthorizationWithdrawn => "authorization_withdrawn",
        }
    }

    /// A bounded human-facing line. Never a verdict about the content.
    pub fn detail(&self) -> String {
        match self {
            AcceptanceIneffective::Absent => "该交付项没有当前有效的接受记录".to_owned(),
            AcceptanceIneffective::RowNotPending { status } => format!(
                "该交付行已由一次真实判决终结（状态 {status}），接受不能事后生效"
            ),
            AcceptanceIneffective::NotAVerifiableRecord => {
                "该行的耐久记录不是可核验的 v2 同一行信封，不能作为接受对象".to_owned()
            }
            AcceptanceIneffective::ClassNotUnconfirmed { class } => {
                format!("该行当前类别为 {class}，只有未核验项可被接受")
            }
            AcceptanceIneffective::BlockedByFact { detail } => {
                format!("该行存在不可由用户假设的风险：{detail}")
            }
            AcceptanceIneffective::CandidateDrift { expected, actual } => format!(
                "候选文件已偏离接受时核验的字节（期望 {expected}，当前 {actual}），接受失效"
            ),
            AcceptanceIneffective::CandidateUnreadable { detail } => format!(
                "候选文件当前无法在授权范围内读取（{detail}），接受有效性未确认"
            ),
            AcceptanceIneffective::VersionDrift => {
                "候选的受管版本已不是接受时登记的那一版，接受失效".to_owned()
            }
            AcceptanceIneffective::ContractDrift { expected, actual } => format!(
                "该行的冻结合同已改变（接受时 {expected}，当前 {actual}），接受失效"
            ),
            AcceptanceIneffective::AuthorizationWithdrawn => {
                "项目授权已不在接受时的范围内，接受失效".to_owned()
            }
        }
    }

    /// Whether this is a definite invalidation, as opposed to one where the
    /// current state could not be established.
    pub fn is_definite(&self) -> bool {
        !matches!(
            self,
            AcceptanceIneffective::Absent
                | AcceptanceIneffective::CandidateUnreadable { .. }
                | AcceptanceIneffective::AuthorizationWithdrawn
        )
    }
}

impl std::fmt::Display for AcceptanceIneffective {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail())
    }
}

/// The wire shape of a blocked reason (package 2-D repair, D-3).
///
/// A hand-written DTO rather than `#[serde]` on [`AcceptanceIneffective`]: the
/// enum is an external-tagged Rust enum, so serializing it directly yields
/// `"Absent"` for the unit variant and `{"row_not_pending":{"status":…}}` for the
/// rest. The frontend reads `kind` + `detail`, so a directly-serialized enum made
/// every specific reason unreachable and left only the generic fallback — a
/// contract mismatch `tsc` cannot see, because the TS type was self-consistent.
///
/// This struct is the single place the wire shape is decided. It is produced from
/// [`AcceptanceIneffective::as_str`] and [`AcceptanceIneffective::detail`], so the
/// classification itself is unchanged and there is still exactly one source of
/// the per-variant reason text. `kind` is a closed vocabulary; an unknown value
/// is a frontend concern and falls back to `detail`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceBlocked {
    /// One of the closed tokens [`AcceptanceIneffective::as_str`] returns.
    pub kind: String,
    /// A bounded, human-facing line. Never a verdict about the content.
    pub detail: String,
}

impl AcceptanceIneffective {
    /// The wire shape of this reason.
    pub fn blocked(&self) -> AcceptanceBlocked {
        AcceptanceBlocked {
            kind: self.as_str().to_owned(),
            detail: self.detail(),
        }
    }
}

/// An acceptance that currently stands.
///
/// `rename_all = "camelCase"` on purpose: this crosses the IPC boundary into the
/// frontend, which reads `acceptedBy` / `candidatePath` / `specHash`. Without it
/// the fields arrive snake_cased and the card renders "已接受（undefined）".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveAcceptance {
    pub run_id: String,
    pub item_key: String,
    /// Local trusted-session identifier. **Not** a user identity: this product
    /// has no user-identity mechanism, and the command layer refuses to take an
    /// actor from the request body.
    pub accepted_by: String,
    pub accepted_at: i64,
    pub candidate_path: String,
    pub candidate_sha256: String,
    pub spec_hash: String,
}

/// Why a resolve request wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceResolveError {
    /// A precondition failed. Nothing was written.
    Refused(AcceptanceIneffective),
    /// The verdict was valid input but the write failed.
    Storage(String),
    /// The Run already reached a terminal state, so a **new** accept or reject is
    /// no longer a legal operation for it.
    ///
    /// Deliberately not a `Refused(AcceptanceIneffective)`: the row itself may be
    /// perfectly acceptable, and folding "the Run is over" into the row-level
    /// reasons would both invent a new `blocked.kind` and misdescribe the cause.
    /// Revoke stays legal — see `resolve_delivery_acceptance`'s Revoke arm.
    RunTerminal { state: String },
}
impl std::fmt::Display for AcceptanceResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptanceResolveError::Refused(reason) => {
                formatter.write_str(&format!("接受被拒绝：{}", reason.detail()))
            }
            AcceptanceResolveError::Storage(detail) => formatter.write_str(detail),
            AcceptanceResolveError::RunTerminal { state } => write!(
                formatter,
                "任务已结束（{state}），不能再接受或拒绝；只能撤销此前的接受"
            ),
        }
    }
}

/// What the item-detail surface needs to render one row's acceptance state.
///
/// Deliberately three separate answers, because they are three separate
/// questions: what the **verification** concluded, what the **user** has decided
/// so far, and whether accepting is even **possible** right now. Collapsing them
/// into one status string is what would let "the user accepted" be read as
/// "the content passed".
///
/// `rename_all = "camelCase"` on purpose: this crosses the IPC boundary into the
/// frontend, which reads `runTerminal` / `verificationClass` / `itemKey`. Without
/// it those arrive snake_cased and read as `undefined` — which, for `runTerminal`,
/// silently re-opens the accept/reject buttons on a Run that has already ended.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceState {
    pub run_id: String,
    pub item_key: String,
    /// The row's durable class token, read from its own verdict envelope.
    pub verification_class: String,
    /// Whether the row may be accepted at all right now. `None` when it may.
    ///
    /// The explicit DTO (D-3), not the enum: the frontend reads `kind` and
    /// `detail`, and a directly-serialized external-tagged enum reaches neither.
    pub blocked: Option<AcceptanceBlocked>,
    /// An acceptance that currently stands.
    pub accepted: Option<EffectiveAcceptance>,
    /// Whether the user has explicitly rejected this item.
    pub rejected: bool,
    /// Whether the Run has reached a terminal state.
    ///
    /// The frontend needs this to decide which buttons exist at all: a parked Run
    /// may be accepted or rejected, and a Run that already ended may only have its
    /// earlier acceptance revoked. Without it, "the panel only renders while
    /// parked" was the only thing keeping a finished Run's rows from offering new
    /// accept/reject buttons — which is exactly the R-1 defect, because it also
    /// hid the revoke entry the user still needs.
    ///
    /// The backend enforces the same rule; this field only lets the UI reflect it.
    pub run_terminal: bool,
}

/// Read one item's acceptance state for display.
///
/// This is the second of the two callers allowed to run the full freshness rule
/// (the terminal admission is the first). It is **not** called by the checklist
/// projection, which stays a single SQL statement.
pub fn delivery_acceptance_state(
    database: &Database,
    run_id: &str,
    item_key: &str,
) -> Result<AcceptanceState, String> {
    let Some(row) = database
        .read_delivery_acceptance_row(run_id, item_key)
        .map_err(|error| error)?
    else {
        return Ok(AcceptanceState {
            run_id: run_id.to_owned(),
            item_key: item_key.to_owned(),
            verification_class: "unknown".to_owned(),
            blocked: Some(AcceptanceIneffective::Absent.blocked()),
            accepted: None,
            rejected: false,
            run_terminal: run_is_terminal(database, run_id),
        });
    };
    let absolute = absolute_candidate_path(database, run_id, &row.candidate_path()).ok();
    let binding = database
        .delivery_acceptance_binding(run_id, item_key, &row, absolute.as_deref())
        .map_err(|error| error)?;
    let finding: serde_json::Value = row
        .finding_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(serde_json::Value::Null);
    let verification_class = match p2b_consumer::decode_verdict(&finding) {
        StoredVerdict::V2(stored) => stored.class.as_str().to_owned(),
        StoredVerdict::Legacy => "legacy".to_owned(),
        StoredVerdict::Corrupt { .. } => "corrupt".to_owned(),
    };
    let accepted = match effective_delivery_acceptance(database, run_id, item_key) {
        Ok(accepted) => accepted,
        Err(_) => None,
    };
    // The blocked reason is only meaningful when nothing is currently accepted:
    // once an acceptance stands, the row is accepted and the reason it *would*
    // have been blocked is history. `None` when the row is acceptable and nobody
    // has accepted yet — that is "waiting for the user", not "blocked".
    let blocked = if accepted.is_some() {
        None
    } else {
        check_acceptance_preconditions(database, &row, &binding)
            .err()
            .map(|reason| reason.blocked())
    };
    Ok(AcceptanceState {
        run_id: run_id.to_owned(),
        item_key: item_key.to_owned(),
        verification_class,
        blocked,
        accepted,
        rejected: row.rejected_at.is_some(),
        run_terminal: run_is_terminal(database, run_id),
    })
}

/// Whether the Run has reached a terminal state.
///
/// Read once per state query, and by the accept/reject guard. A Run that is
/// merely *not parked* — `cancelling`, for instance — is **not** terminal, and
/// that distinction is load-bearing: a Run in flight can still legally take a
/// new acceptance, and one that has ended cannot.
pub(crate) fn run_is_terminal(database: &Database, run_id: &str) -> bool {
    database
        .kernel_rehydrate(run_id)
        .ok()
        .flatten()
        .is_some_and(|data| data.state.is_terminal())
}

// ---------------------------------------------------------------------------
// The terminal admission (package 2-D repair, D-1)
// ---------------------------------------------------------------------------

/// Whether the Run's deliveries may now end it, and if not, why.
///
/// The admission is about the **whole Run**, not one item: a single remaining
/// unaccepted unverified item keeps the Run parked, and a single blocked sibling
/// keeps it parked regardless of how many items were accepted. That is the
/// product combination the user chose (§89): "all items that need accepting are
/// valid, and there is no repair/failure/security sibling".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceAdmission {
    /// True only when every required item is satisfied and nothing blocks.
    pub admitted: bool,
    /// A bounded human-facing line explaining a refusal.
    pub reason: String,
    /// Items that are neither verified nor effectively accepted.
    pub outstanding: Vec<String>,
    /// Items that block admission outright.
    pub blocking: Vec<String>,
    /// The acceptances that would be recorded if the Run ends now.
    pub acceptances: Vec<EffectiveAcceptance>,
}

/// Why the admission refused, as a stable token for a caller that must report it
/// without matching on a message string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The Run is not parked on a delivery review.
    NotParked,
    /// Some required item has no effective acceptance and no verified pass.
    OutstandingItems,
    /// A sibling item carries a refusal, an invalid spec or a definite failure.
    BlockedSibling,
    /// A sibling still has a live repair round.
    RepairableSibling,
    /// The current authorization could not be established.
    AuthorizationUnavailable,
}

impl AdmissionRefusal {
    /// The stable token for this refusal, for logs and for the command's audit
    /// trail. Not the same as `detail()`, which is the human-readable line.
    pub fn as_str(self) -> &'static str {
        match self {
            AdmissionRefusal::NotParked => "not_parked",
            AdmissionRefusal::OutstandingItems => "outstanding_items",
            AdmissionRefusal::BlockedSibling => "blocked_sibling",
            AdmissionRefusal::RepairableSibling => "repairable_sibling",
            AdmissionRefusal::AuthorizationUnavailable => "authorization_unavailable",
        }
    }

    pub fn detail(self) -> String {
        match self {
            AdmissionRefusal::NotParked => "该任务未停在交付待接受状态".to_owned(),
            AdmissionRefusal::OutstandingItems => {
                "仍有未核验项没有被有效接受，任务不能结束".to_owned()
            }
            AdmissionRefusal::BlockedSibling => {
                "同任务内存在安全拒绝、合同不可用或确定失败的交付项，不能以接受方式结束".to_owned()
            }
            AdmissionRefusal::RepairableSibling => {
                "同任务内仍有可修复的交付项，任务不能以接受方式结束".to_owned()
            }
            AdmissionRefusal::AuthorizationUnavailable => {
                "当前项目授权无法确认，不能据此结束任务".to_owned()
            }
        }
    }
}

/// Whether the Run's deliveries may now end it.
///
/// Reads current durable facts only; it writes nothing. The caller is expected to
/// re-run this inside the same transaction that writes the terminal, so a row
/// that changed between the two cannot be admitted on the strength of the first
/// read.
///
/// A row counts as satisfied when either
/// * it is genuinely passed (its own status says `passed` and the shared rule does
///   not withdraw it), or
/// * it has an acceptance that currently stands.
///
/// Anything else is outstanding. A row whose class is `Refused`, `Invalid` or
/// `DefiniteFailure`, or that carries a security veto, blocks the whole Run: a
/// user accepting one item must not be able to end a Run whose sibling was
/// refused by a checker.
pub fn evaluate_delivery_acceptance_admission(
    database: &Database,
    run_id: &str,
) -> Result<AcceptanceAdmission, AdmissionRefusal> {
    // The Run must actually be parked. A Run that is `running`, terminal, or
    // parked for some other reason has no acceptance to aggregate.
    let parked = database
        .kernel_rehydrate(run_id)
        .map_err(|_| AdmissionRefusal::AuthorizationUnavailable)?
        .is_some_and(|data| data.state == RunState::AwaitingDeliveryReview);
    if !parked {
        return Err(AdmissionRefusal::NotParked);
    }
    // The project authorization must still resolve: an acceptance made under a
    // withdrawn authorization is not a basis for ending anything.
    if authorized_delivery_root_typed(database, run_id).is_err() {
        return Err(AdmissionRefusal::AuthorizationUnavailable);
    }

    let items = database
        .delivery_checklist(run_id)
        .map_err(|_| AdmissionRefusal::AuthorizationUnavailable)?;
    let mut outstanding = Vec::new();
    let mut blocking = Vec::new();
    let mut acceptances = Vec::new();

    for item in &items {
        let Some(row) = database
            .read_delivery_acceptance_row(run_id, &item.item_key)
            .map_err(|_| AdmissionRefusal::AuthorizationUnavailable)?
        else {
            // A checklist row with no durable finding at all is not something an
            // acceptance can cover.
            outstanding.push(item.item_key.clone());
            continue;
        };
        let absolute = absolute_candidate_path(database, run_id, &row.candidate_path()).ok();
        let binding = database
            .delivery_acceptance_binding(run_id, &item.item_key, &row, absolute.as_deref())
            .map_err(|_| AdmissionRefusal::AuthorizationUnavailable)?;

        // A genuinely passed row needs no acceptance and blocks nothing.
        if item.status == "passed"
            && !crate::runtime_host::delivery::passed_qualification_of_row(
                &item.status,
                row.finding_json.as_deref(),
                Some(&row.requirements_json),
                &item.item_key,
            )
            .withdraws_pass()
        {
            continue;
        }

        match check_acceptance_preconditions(database, &row, &binding) {
            Ok(()) => {
                if live_acceptance(&row) {
                    acceptances.push(EffectiveAcceptance {
                        run_id: run_id.to_owned(),
                        item_key: item.item_key.clone(),
                        accepted_by: row.accepted_by.clone(),
                        accepted_at: row.accepted_at,
                        candidate_path: binding.candidate_path.clone(),
                        candidate_sha256: binding.candidate_sha256.clone(),
                        spec_hash: binding.spec_hash.clone(),
                    });
                } else {
                    outstanding.push(item.item_key.clone());
                }
            }
            // A row that is simply not acceptable is outstanding unless it is
            // blocked for a reason that also blocks the whole Run.
            Err(AcceptanceIneffective::BlockedByFact { .. })
            | Err(AcceptanceIneffective::ClassNotUnconfirmed { .. }) => {
                blocking.push(item.item_key.clone());
            }
            Err(_) => outstanding.push(item.item_key.clone()),
        }

        // A sibling with a live repair round keeps the Run repairable, so it can
        // never be ended by acceptance.
        if row_reason_is_repairable(&row) {
            blocking.push(item.item_key.clone());
        }
    }

    if !blocking.is_empty() {
        let repairable = blocking
            .iter()
            .any(|key| row_reason_is_repairable_key(database, run_id, key));
        return Err(if repairable {
            AdmissionRefusal::RepairableSibling
        } else {
            AdmissionRefusal::BlockedSibling
        });
    }
    if !outstanding.is_empty() {
        return Err(AdmissionRefusal::OutstandingItems);
    }
    if acceptances.is_empty() {
        // Nothing needed accepting and nothing was accepted: this Run has no
        // unverified item to end on, so it is not an acceptance admission at all.
        return Err(AdmissionRefusal::OutstandingItems);
    }

    Ok(AcceptanceAdmission {
        admitted: true,
        reason: "所有需接受的未核验项均已有效接受，且无阻断兄弟项".to_owned(),
        outstanding,
        blocking,
        acceptances,
    })
}

/// Whether the row's own durable verdict carries a `RepairNeeded` class.
fn row_reason_is_repairable(row: &AcceptanceRow) -> bool {
    row.finding_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|finding| {
            finding
                .get("deliveryVerdict")
                .and_then(|verdict| verdict.get("class"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|class| class == "repairNeeded")
}

fn row_reason_is_repairable_key(database: &Database, run_id: &str, item_key: &str) -> bool {
    database
        .read_delivery_acceptance_row(run_id, item_key)
        .ok()
        .flatten()
        .is_some_and(|row| row_reason_is_repairable(&row))
}

// ---------------------------------------------------------------------------
// The terminal commit (package 2-D repair, D-1)
// ---------------------------------------------------------------------------

/// What ending the Run on an acceptance produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceTerminal {
    /// Every required item was satisfied and the Run completed.
    Completed {
        acceptances: Vec<EffectiveAcceptance>,
    },
    /// The user rejected an item: an execution-layer conclusion, not a
    /// verification failure.
    Rejected {
        item_key: String,
        code: &'static str,
        message: String,
    },
    /// The admission did not hold; nothing was written.
    Refused {
        refusal: AdmissionRefusal,
        detail: String,
    },
}

impl AcceptanceTerminal {
    /// The stable token for a refused terminal, for logs and for the audit trail.
    /// `detail` is the human-readable line; this is what a caller matches on.
    pub fn refusal_token(&self) -> Option<&'static str> {
        match self {
            AcceptanceTerminal::Refused { refusal, .. } => Some(refusal.as_str()),
            _ => None,
        }
    }

    /// The human-readable line for a refused terminal. Empty for the two
    /// outcomes that actually ended the Run, which have nothing to explain.
    pub fn detail(&self) -> String {
        match self {
            AcceptanceTerminal::Refused { detail, .. } => detail.clone(),
            _ => String::new(),
        }
    }
}

/// The canonical code and message for a user rejection.
///
/// A conclusion about the **execution**, never about the content: no
/// `DecisionFact` is written, no class is invented, and the six-class policy is
/// untouched. The vocabulary is frozen here so a caller cannot choose its own
/// reason for a failed Run.
pub const REJECTED_BY_USER_CODE: &str = "delivery.rejected_by_user";

pub fn rejected_by_user_message(item_key: &str) -> String {
    format!("用户判定交付项 {item_key} 不可接受，任务未通过")
}

/// End (or refuse to end) a parked Run on the strength of user acceptances.
///
/// # The commit path
///
/// This is **not** a SQL update of `kernel_runs.state`. It rehydrates a
/// `RunController` from the durable facts, asks it for the terminal effects, and
/// commits them through `Database::kernel_commit_decision` — the same durable
/// decision commit every other Host transition uses, with its
/// `run_transition_allowed` check, its `WaitingApproval ⟺ deadline` invariant and
/// its event/replay exactness. A Run whose transition is not allowed therefore
/// cannot be terminated by this function, and a replayed call writes nothing.
///
/// # The admission window
///
/// The admission is evaluated **before** the controller is rehydrated, and the
/// terminal is committed in one transaction. The gap between them is the
/// concurrency window this package claims: it is bounded to a single writer on
/// one database, and `kernel_commit_decision` refuses a transition that is no
/// longer legal. It is **not** a claim about multiple processes or about the file
/// system being atomic with SQLite.
pub fn commit_delivery_acceptance_terminal(
    database: &Database,
    run_id: &str,
    now_wall_ms: i64,
) -> Result<AcceptanceTerminal, String> {
    let admission = match evaluate_delivery_acceptance_admission(database, run_id) {
        Ok(admission) => admission,
        Err(refusal) => {
            return Ok(AcceptanceTerminal::Refused {
                refusal,
                detail: refusal.detail(),
            })
        }
    };

    let Some(rehydrated) = database
        .kernel_rehydrate(run_id)
        .map_err(|error| error)?
    else {
        return Ok(AcceptanceTerminal::Refused {
            refusal: AdmissionRefusal::NotParked,
            detail: AdmissionRefusal::NotParked.detail(),
        });
    };
    let mut controller = RunController::rehydrate(rehydrated)
        .map_err(|error| error.to_string())?;
    // A second, in-process guard: the controller must still be parked. The
    // durable commit re-checks the transition, so this is a cheap early refusal
    // rather than the authority.
    if controller.state() != RunState::AwaitingDeliveryReview {
        return Ok(AcceptanceTerminal::Refused {
            refusal: AdmissionRefusal::NotParked,
            detail: AdmissionRefusal::NotParked.detail(),
        });
    }

    let facts: Vec<DeliveryAcceptanceFact> = admission
        .acceptances
        .iter()
        .map(|acceptance| DeliveryAcceptanceFact {
            item_key: acceptance.item_key.clone(),
            actor: acceptance.accepted_by.clone(),
            accepted_at: acceptance.accepted_at,
            candidate_sha256: acceptance.candidate_sha256.clone(),
            spec_hash: acceptance.spec_hash.clone(),
            acceptance_class: "unconfirmed",
        })
        .collect();

    let mut effects = controller
        .terminate(RunOutcome::Completed)
        .into_iter()
        .collect::<Vec<_>>();
    // The acceptance marker is a separate event, committed in the same
    // write-set, so the two can never disagree about why the Run ended.
    effects.extend(
        controller
            .note_delivery_acceptance(&facts)
            .map_err(|error| error.to_string())?,
    );
    let command = controller.persist_command(&effects);
    database.kernel_commit_decision(run_id, now_wall_ms, &command)?;

    Ok(AcceptanceTerminal::Completed {
        acceptances: admission.acceptances,
    })
}

/// Whether a refused terminal is the benign race or a real inconsistency.
///
/// A Run that is no longer parked when the terminal is committed was ended by
/// something else first — a cancellation, a competing completion — and the
/// acceptance still stands. Every other refusal means the acceptance stands but
/// the Run did not end, which the caller must be told about rather than being
/// shown a silent success.
pub fn terminal_refusal_is_benign(terminal: &AcceptanceTerminal) -> bool {
    matches!(
        terminal,
        AcceptanceTerminal::Refused {
            refusal: AdmissionRefusal::NotParked,
            ..
        }
    )
}

/// End a parked Run because the user rejected one of its items.
///
/// The same real decision-commit path as
/// [`commit_delivery_acceptance_terminal`], with the class's own canonical
/// failure reason. No `DecisionFact` is written and no verification class is
/// invented: this is a conclusion about the execution.
pub fn commit_delivery_rejection_terminal(
    database: &Database,
    run_id: &str,
    item_key: &str,
    now_wall_ms: i64,
) -> Result<AcceptanceTerminal, String> {
    let Some(rehydrated) = database
        .kernel_rehydrate(run_id)
        .map_err(|error| error)?
    else {
        return Ok(AcceptanceTerminal::Refused {
            refusal: AdmissionRefusal::NotParked,
            detail: AdmissionRefusal::NotParked.detail(),
        });
    };
    let mut controller = RunController::rehydrate(rehydrated)
        .map_err(|error| error.to_string())?;
    if controller.state() != RunState::AwaitingDeliveryReview {
        return Ok(AcceptanceTerminal::Refused {
            refusal: AdmissionRefusal::NotParked,
            detail: AdmissionRefusal::NotParked.detail(),
        });
    }
    let effects = controller.terminate(RunOutcome::Failed {
        code: REJECTED_BY_USER_CODE.into(),
        message: rejected_by_user_message(item_key),
    });
    let command = controller.persist_command(&effects);
    database.kernel_commit_decision(run_id, now_wall_ms, &command)?;
    Ok(AcceptanceTerminal::Rejected {
        item_key: item_key.to_owned(),
        code: REJECTED_BY_USER_CODE,
        message: rejected_by_user_message(item_key),
    })
}

/// Record a user's decision about one item's unverified delivery.
///
/// The acceptance preconditions are evaluated **before** any write, and the
/// write itself is a single `BEGIN IMMEDIATE` transaction. That gives atomicity
/// against a competing writer on the same database — which is the whole of the
/// concurrency claim. It is not a claim about multiple processes.
///
/// Idempotence:
///
/// * an identical accept that already stands writes nothing (`Unchanged`);
// * a re-accept after a revocation is a **new** decision and does write, so the
// * first acceptance is never lost (the projection row is replaced, the audit
// * row is not);
/// * a repeated rejection and a repeated revocation are no-ops.
///
/// # Terminal admission (package 2-D repair, D-1)
///
/// Recording an acceptance is only half of the user-visible loop: once every item
/// that needed accepting has one, the Run has to actually end. This function
/// therefore evaluates the whole-Run admission after a successful **accept** and,
/// when it holds, commits the terminal through the real Controller decision path
/// (see [`commit_delivery_acceptance_terminal`]). A **reject** commits the
/// execution-layer failure terminal instead.
///
/// The two halves are deliberately separate calls rather than one transaction:
/// the acceptance write and the terminal write have different failure modes, and
/// folding them would either make a failed terminal lose the acceptance or make a
/// lost acceptance look like a completed Run. The terminal commit re-checks the
/// transition durably, so an acceptance written by a racing request cannot by
/// itself end the Run.
pub fn resolve_delivery_acceptance(
    database: &Database,
    run_id: &str,
    item_key: &str,
    decision: DeliveryAcceptanceDecision,
    actor: &str,
    now: i64,
) -> Result<AcceptanceWrite, AcceptanceResolveError> {
    let actor = actor.trim();
    if actor.is_empty() || actor.len() > 256 {
        return Err(AcceptanceResolveError::Storage(
            "接受主体不是受信本地会话标识".into(),
        ));
    }
    // R-1: a Run that has already ended cannot take a **new** accept or reject.
    //
    // Without this guard the acceptance would be recorded and the terminal commit
    // would refuse with `NotParked`, which the benign-race rule then drops — so a
    // finished Run would silently accumulate new acceptances nobody asked for.
    // Revoke is deliberately exempt: withdrawing an acceptance made while the Run
    // was still parked is exactly what the user needs after it completes.
    if decision != DeliveryAcceptanceDecision::Revoke && run_is_terminal(database, run_id) {
        let state = database
            .kernel_rehydrate(run_id)
            .ok()
            .flatten()
            .map(|data| data.state.as_str().to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        return Err(AcceptanceResolveError::RunTerminal { state });
    }
    match decision {
        DeliveryAcceptanceDecision::Accept => {
            let Some(row) = database
                .read_delivery_acceptance_row(run_id, item_key)
                .map_err(AcceptanceResolveError::Storage)?
            else {
                return Err(AcceptanceResolveError::Refused(
                    AcceptanceIneffective::Absent,
                ));
            };
            // An acceptance that already stands is not rewritten: re-clicking must
            // not move `accepted_at` and make the record look freshly made. This
            // is a question about the projection row, not about the row's current
            // acceptability — a row that has since become unacceptable keeps its
            // acceptance until something withdraws it, and the freshness rule is
            // what reports that.
            if live_acceptance(&row) {
                return Ok(AcceptanceWrite::Unchanged);
            }
            // The absolute candidate path, resolved through the current
            // authorized root. This is the one form the version ledger stores,
            // so both the binding and the freshness re-check must use it.
            let absolute = absolute_candidate_path(database, run_id, &row.candidate_path()).ok();
            let binding = database
                .delivery_acceptance_binding(run_id, item_key, &row, absolute.as_deref())
                .map_err(AcceptanceResolveError::Storage)?;
            // Re-run the full rule against the row we are about to record, so a
            // row that stopped being acceptable cannot be accepted on the
            // strength of an earlier read.
            check_acceptance_preconditions(database, &row, &binding)
                .map_err(AcceptanceResolveError::Refused)?;
            let written = database
                .write_delivery_acceptance(run_id, item_key, &binding, actor, now)
                .map_err(AcceptanceResolveError::Storage)?;
            if written == AcceptanceWrite::Written {
                let snapshot = acceptance_snapshot(
                    run_id,
                    item_key,
                    &binding,
                    binding.managed_version_id.as_deref(),
                );
                database
                    .append_delivery_acceptance_audit(
                        run_id,
                        item_key,
                        decision.as_str(),
                        actor,
                        now,
                        Some("用户接受未核验交付，不代表核验通过"),
                        Some(&snapshot),
                    )
                    .map_err(AcceptanceResolveError::Storage)?;
            }
            // D-1: an acceptance is only half the loop. Once every item that
            // needed accepting has one, the Run has to end. The terminal is a
            // separate durable decision (see the function's doc), so a failure
            // here does not lose the acceptance that was just recorded.
            //
            // A `NotParked` refusal is the benign race — something else already
            // ended this Run — and is dropped. Every other refusal means the
            // acceptance stands but the Run did not end, which is exactly the
            // state the caller has to know about rather than a silent success.
            let terminal = commit_delivery_acceptance_terminal(database, run_id, now)
                .map_err(AcceptanceResolveError::Storage)?;
            if !terminal_refusal_is_benign(&terminal) {
                if let Some(token) = terminal.refusal_token() {
                    return Err(AcceptanceResolveError::Storage(format!(
                        "接受已记录，但任务未能结束（{token}）：{}",
                        terminal.detail()
                    )));
                }
            }
            Ok(written)
        }
        DeliveryAcceptanceDecision::Reject => {
            let Some(row) = database
                .read_delivery_acceptance_row(run_id, item_key)
                .map_err(AcceptanceResolveError::Storage)?
            else {
                return Err(AcceptanceResolveError::Refused(
                    AcceptanceIneffective::Absent,
                ));
            };
            // A rejection is allowed for a row that is not acceptable: "the user
            // looked and said no" is exactly the case where there is nothing to
            // accept. Only the identity preconditions apply.
            let absolute = absolute_candidate_path(database, run_id, &row.candidate_path()).ok();
            let binding = database
                .delivery_acceptance_binding(run_id, item_key, &row, absolute.as_deref())
                .map_err(AcceptanceResolveError::Storage)?;
            if binding.candidate_sha256.is_empty() {
                return Err(AcceptanceResolveError::Refused(
                    AcceptanceIneffective::NotAVerifiableRecord,
                ));
            }
            if row.rejected_at.is_some() {
                return Ok(AcceptanceWrite::Unchanged);
            }
            let written = database
                .write_delivery_rejection(run_id, item_key, &binding, actor, now)
                .map_err(AcceptanceResolveError::Storage)?;
            if written == AcceptanceWrite::Written {
                let snapshot = acceptance_snapshot(
                    run_id,
                    item_key,
                    &binding,
                    binding.managed_version_id.as_deref(),
                );
                database
                    .append_delivery_acceptance_audit(
                        run_id,
                        item_key,
                        decision.as_str(),
                        actor,
                        now,
                        Some("用户判定该交付不可接受"),
                        Some(&snapshot),
                    )
                    .map_err(AcceptanceResolveError::Storage)?;
            }
            // D-1: a rejection is an execution-layer conclusion, so it ends the
            // Run with the canonical failure reason. Separate durable decision,
            // same real Controller path.
            let _ = commit_delivery_rejection_terminal(database, run_id, item_key, now)
                .map_err(AcceptanceResolveError::Storage)?;
            Ok(written)
        }
        DeliveryAcceptanceDecision::Revoke => {
            let written = database
                .write_delivery_revocation(run_id, item_key, now)
                .map_err(AcceptanceResolveError::Storage)?;
            if written == AcceptanceWrite::Written {
                database
                    .append_delivery_acceptance_audit(
                        run_id,
                        item_key,
                        decision.as_str(),
                        actor,
                        now,
                        Some("用户撤销此前对未核验交付的接受"),
                        None,
                    )
                    .map_err(AcceptanceResolveError::Storage)?;
            }
            Ok(written)
        }
    }
}

/// Whether an acceptance for this item currently stands, and why not when it
/// does not.
///
/// The **only** production reader of acceptance validity. The checklist
/// projection deliberately does not call it: that projection answers "may this
/// row be shown as a pass" with one SQL statement, and adding an authorized file
/// read there would change its IO profile on every conversation load.
/// Whether the projection row records an acceptance that has not been withdrawn
/// or rejected.
///
/// A separate question from [`check_acceptance_preconditions`]: this one is about
/// the **existence** of a current acceptance, that one about whether the row may
/// be accepted at all. A row with no acceptance yet answers "no" here and is
/// still perfectly acceptable.
fn live_acceptance(row: &AcceptanceRow) -> bool {
    row.accepted_at > 0 && row.revoked_at.is_none() && row.rejected_at.is_none()
}

pub fn effective_delivery_acceptance(
    database: &Database,
    run_id: &str,
    item_key: &str,
) -> Result<Option<EffectiveAcceptance>, AcceptanceIneffective> {
    let Some(row) = database
        .read_delivery_acceptance_row(run_id, item_key)
        .map_err(|_| AcceptanceIneffective::CandidateUnreadable {
            detail: "接受行读取失败".to_owned(),
        })?
    else {
        return Ok(None);
    };
    // Nothing was ever accepted, or it was withdrawn / rejected. That is a
    // legitimate "no acceptance stands", not a failure.
    if !live_acceptance(&row) {
        return Ok(None);
    }
    let absolute = absolute_candidate_path(database, run_id, &row.candidate_path()).ok();
    let binding = database
        .delivery_acceptance_binding(run_id, item_key, &row, absolute.as_deref())
        .map_err(|_| AcceptanceIneffective::CandidateUnreadable {
            detail: "接受身份读取失败".to_owned(),
        })?;
    match check_acceptance_preconditions(database, &row, &binding) {
        Ok(()) => Ok(Some(EffectiveAcceptance {
            run_id: run_id.to_owned(),
            item_key: item_key.to_owned(),
            accepted_by: row.accepted_by.clone(),
            accepted_at: row.accepted_at,
            candidate_path: binding.candidate_path,
            candidate_sha256: binding.candidate_sha256,
            spec_hash: binding.spec_hash,
        })),
        Err(AcceptanceIneffective::Absent) => Ok(None),
        Err(reason) => Err(reason),
    }
}

/// The acceptance rule, in its fixed order. Every check reads current durable
/// facts; none of them writes.
///
/// This answers "**may** this row be accepted right now?" — a question about the
/// row, not about an existing acceptance. Whether an acceptance exists and still
/// stands is [`Self::live_acceptance`] plus the same rule; keeping them apart is
/// what stops a caller from reading "nothing was accepted yet" as "this row
/// cannot be accepted".
fn check_acceptance_preconditions(
    database: &Database,
    row: &AcceptanceRow,
    binding: &AcceptanceBinding,
) -> Result<(), AcceptanceIneffective> {
    // 1. The row must still be pending. A row a real decision already terminated
    //    is not something a later acceptance can reopen or second-guess.
    if row.status != "pending" {
        return Err(AcceptanceIneffective::RowNotPending {
            status: row.status.clone(),
        });
    }
    // 2. The row's own durable record must be a decodable v2 envelope naming
    //    THIS row. A legacy v1 record, a corrupt one, and an envelope naming a
    //    different item are all "not an acceptable Unconfirmed" — the first
    //    because it was never verified by the v2 checker at all, the others
    //    because they are unusable records rather than risks to assume.
    let finding: serde_json::Value = row
        .finding_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(serde_json::Value::Null);
    let stored = match p2b_consumer::decode_verdict(&finding) {
        StoredVerdict::V2(stored) => stored,
        StoredVerdict::Legacy | StoredVerdict::Corrupt { .. } => {
            return Err(AcceptanceIneffective::NotAVerifiableRecord)
        }
    };
    if stored.item_key != row.item_key {
        return Err(AcceptanceIneffective::NotAVerifiableRecord);
    }
    // 3. The class must be exactly `Unconfirmed`. A pass needs no acceptance; a
    //    failure or a refusal must not be accepted away.
    if stored.class != DeliveryClass::Unconfirmed {
        return Err(AcceptanceIneffective::ClassNotUnconfirmed {
            class: stored.class.as_str().to_owned(),
        });
    }
    // 4. No security veto and no Refused / Invalid fact. These are not risks a
    //    user may assume on the artifact's behalf.
    if let Some(detail) = blocking_fact_detail(&stored.facts) {
        return Err(AcceptanceIneffective::BlockedByFact { detail });
    }
    // 5. The frozen contract must still hash to what was accepted. Only
    //    meaningful once an acceptance exists: a first acceptance has nothing to
    //    compare against, and comparing against an empty column would refuse
    //    every first accept.
    let spec_hash = spec_digest(&row.requirements_json);
    if live_acceptance(&row) && spec_hash != row.accepted_spec_hash {
        return Err(AcceptanceIneffective::ContractDrift {
            expected: row.accepted_spec_hash.clone(),
            actual: spec_hash,
        });
    }
    // 6. The project authorization must still resolve, and the candidate must
    //    still be inside it.
    let root = match authorized_delivery_root_typed(database, &row.run_id) {
        Ok(root) => root,
        Err(_) => return Err(AcceptanceIneffective::AuthorizationWithdrawn),
    };
    let path = row.candidate_path();
    let relative = super::project_relative_path_for_u1(&path, root.to_str());
    let Some(relative) = relative else {
        return Err(AcceptanceIneffective::CandidateUnreadable {
            detail: "候选路径不在当前授权项目内".to_owned(),
        });
    };
    // The absolute form the version ledger actually stores. Resolved once here so
    // the byte re-read below and the version identity in step 8 use the same
    // spelling — a candidate that cannot be resolved is an error, not a silent
    // skip of the version criterion.
    absolute_candidate_path(database, &row.run_id, &path).map_err(|reason| reason)?;
    // 7. The candidate must still be the bytes that were accepted. A controlled
    //    re-read, never a comparison of two stored columns.
    match read_authorized_delivery_bytes_typed(&root, &relative, MAX_VERIFY_BYTES) {
        Ok(bytes) => {
            let actual = digest_hex(&bytes);
            if actual != binding.candidate_sha256 {
                return Err(AcceptanceIneffective::CandidateDrift {
                    expected: binding.candidate_sha256.clone(),
                    actual,
                });
            }
        }
        Err(failure) => {
            if matches!(
                failure,
                crate::runtime_host::attachment_compute::DeliveryReadFailure::AuthorizationRefused { .. }
            ) {
                return Err(AcceptanceIneffective::AuthorizationWithdrawn);
            }
            return Err(AcceptanceIneffective::CandidateUnreadable {
                detail: format!("{failure:?}"),
            });
        }
    }
    // 8. The managed-file version must still be the one that was accepted.
    //
    //    The identity is the **absolute** `storage_path` the real write gate
    //    records, resolved through the authorized root — the same form on both
    // 8. The managed-file version must still be the one that was accepted.
    //
    //    The identity is the **absolute** `storage_path` the real write gate
    //    records, resolved through the authorized root — the same form on both
    //    sides. Looking the accepted version up from the raw `target_path` while
    //    checking the current one from a project-relative path is the D-4 defect:
    //    on a relative `target_path` the binding missed and the whole criterion
    //    was skipped; on an absolute one the binding hit but the re-check missed,
    //    so an acceptance could never stand. There is a second layer of the same
    //    defect — the authorized root resolves through the Windows extended-length
    //    namespace (`\\?\E:\…`) while the ledger stores plain paths (`E:\…`) — and
    //    `absolute_candidate_path` strips the prefix so the two spellings meet.
    //
    //    The binding re-resolves the version on every read, so comparing it with
    //    what the acceptance recorded **is** the criterion. Both sides may be
    //    `None` (an artifact never written through the managed-file gate): that is
    //    a legitimate "no version", not a drift, and it must stay distinguishable
    //    from a lookup that failed.
    if live_acceptance(&row) && row.accepted_version_id != binding.managed_version_id {
        return Err(AcceptanceIneffective::VersionDrift);
    }
    Ok(())
}

/// The candidate's absolute path under the current authorized root, in the
/// spelling the write gate records.
///
/// `managed_file_versions.storage_path` is written by the real write gate as an
/// absolute **plain** path (`E:\…`), while `authorized_delivery_root_typed`
/// resolves the root through the Windows extended-length namespace
/// (`\\?\E:\…`). Comparing the two spellings directly would miss every version
/// row, which is why the prefix is stripped here — a second, independent layer of
/// the D-4 defect on top of the relative/absolute one.
///
/// A candidate that cannot be resolved is an error, not a silent `None`: a lookup
/// failure must never be mistaken for "this artifact has no managed version",
/// because that would let a broken path slip through the version criterion.
fn absolute_candidate_path(
    database: &Database,
    run_id: &str,
    candidate_path: &str,
) -> Result<std::path::PathBuf, AcceptanceIneffective> {
    let root = match authorized_delivery_root_typed(database, run_id) {
        Ok(root) => root,
        Err(_) => return Err(AcceptanceIneffective::AuthorizationWithdrawn),
    };
    let Some(relative) = super::project_relative_path_for_u1(candidate_path, root.to_str()) else {
        return Err(AcceptanceIneffective::CandidateUnreadable {
            detail: "候选路径不在当前授权项目内".to_owned(),
        });
    };
    Ok(plain_storage_path(&root.join(relative)))
}

/// Strip the Windows extended-length prefix from a resolved path.
///
/// `\\?\E:\dir\file` → `E:\dir\file`. A path that does not carry the prefix is
/// returned unchanged, so this is a no-op on every other platform.
fn plain_storage_path(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => std::path::PathBuf::from(stripped),
        None => path.to_path_buf(),
    }
}


fn spec_digest(requirements_json: &str) -> String {
    let value: serde_json::Value =
        serde_json::from_str(requirements_json).unwrap_or(serde_json::json!({}));
    digest_hex(serde_json::to_vec(&value).unwrap_or_default().as_slice())
}

fn acceptance_snapshot(
    run_id: &str,
    item_key: &str,
    binding: &AcceptanceBinding,
    accepted_version_id: Option<&str>,
) -> String {
    serde_json::to_string(&serde_json::json!({
        "runId": run_id,
        "itemKey": item_key,
        "candidatePath": binding.candidate_path,
        "candidateSha256": binding.candidate_sha256,
        // The version the acceptance binds to, read from the row's own projection
        // (what was recorded), not re-derived: the audit records the decision as
        // it was made.
        "managedVersionId": accepted_version_id,
        "specHash": binding.spec_hash,
        "ruleId": binding.rule_id,
        "checkerVersion": binding.checker_version,
        "acceptanceClass": "unconfirmed",
    }))
    .unwrap_or_else(|_| "{}".to_owned())
}

/// The first fact that makes this row unacceptable to assume a risk for.
///
/// A security veto outranks everything: it is a trust refusal, not a content
/// risk, and no user decision may stand in for it. A source-identity drift is
/// the same kind of fact wearing a different code, so it is named explicitly
/// rather than being caught only by its `Refused` class.
fn blocking_fact_detail(facts: &[DecisionFact]) -> Option<String> {
    for fact in facts {
        if fact.class == FactClass::Refused {
            return Some(format!(
                "安全/信任拒绝 {}",
                fact.reason_code.as_deref().unwrap_or("")
            ));
        }
    }
    for fact in facts {
        if fact.reason_code.as_deref()
            == Some(leaf::reason_code::security::SOURCE_IDENTITY_MISMATCH)
        {
            return Some("来源身份偏离".to_owned());
        }
    }
    if facts.iter().any(|fact| {
        fact.origin == FactOrigin::Spec && fact.class == FactClass::Invalid
    }) {
        return Some("合同规格不可用".to_owned());
    }
    None
}

fn digest_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
#[path = "delivery_u1_tests.rs"]
mod tests;












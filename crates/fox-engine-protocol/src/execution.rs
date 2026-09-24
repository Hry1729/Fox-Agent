//! Execution admission: normative dispatch identity, fixed credentials,
//! single-claim results and stage receipts (frozen contract v1.6).
//!
//! Everything here is pure and dependency-free so the Rust Host, the database
//! layer and the A/C adapters share one definition. The normative dispatch
//! identity uses a UTF-8 byte-length prefix so colons inside either id can
//! never forge a boundary; decoding rejects non-canonical input (leading
//! zeros, out-of-range lengths, illegal splits) and never panics.

use serde::{Deserialize, Serialize};

/// Self-contained SHA-256 so persisted digests stay stable without adding a
/// dependency to this crate (Cargo manifests are not writable in this batch).
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for value in h {
        out.push_str(&format!("{value:08x}"));
    }
    out
}

/// Maximum UTF-8 byte length of either dispatch id component (first-version
/// constraint chosen by the frozen contract; not claimed to come from the
/// existing product).
pub const MAX_ID_BYTES: usize = 1024;

/// Normative encoding: `tool-dispatch:<n>:<run_id>:<tool_call_id>` where `n`
/// is the UTF-8 byte length of `run_id` in decimal without leading zeros.
/// Both components must be valid UTF-8 of 1..=1024 bytes; colons are legal.
pub fn encode_dispatch_id(run_id: &str, tool_call_id: &str) -> Result<String, String> {
    let run_bytes = run_id.as_bytes().len();
    let call_bytes = tool_call_id.as_bytes().len();
    if run_bytes == 0 || call_bytes == 0 {
        return Err("invalid_arguments: dispatch id components must be non-empty".into());
    }
    if run_bytes > MAX_ID_BYTES || call_bytes > MAX_ID_BYTES {
        return Err(format!(
            "invalid_arguments: dispatch id component exceeds {MAX_ID_BYTES} bytes"
        ));
    }
    Ok(format!("tool-dispatch:{run_bytes}:{run_id}:{tool_call_id}"))
}

/// Normative decoding. The length field is validated first (decimal, no
/// leading zero, 1..=1024); slicing then uses only checked accessors so no
/// arithmetic can overflow; both decoded components are re-checked for the
/// byte bounds. Any malformed input is an `Err`, never a panic.
pub fn decode_dispatch_id(encoded: &str) -> Result<(String, String), String> {
    let rest = encoded
        .strip_prefix("tool-dispatch:")
        .ok_or("invalid_arguments: missing dispatch prefix")?;
    let (length, body) = rest
        .split_once(':')
        .ok_or("invalid_arguments: missing length separator")?;
    if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid_arguments: length is not decimal".into());
    }
    if length.len() > 1 && length.starts_with('0') {
        return Err("invalid_arguments: length has a leading zero".into());
    }
    let n: usize = length
        .parse()
        .map_err(|_| "invalid_arguments: length is not a number".to_string())?;
    if n == 0 || n > MAX_ID_BYTES {
        return Err(format!(
            "invalid_arguments: length is outside 1..={MAX_ID_BYTES}"
        ));
    }
    let body_bytes = body.as_bytes();
    if body_bytes.get(n) != Some(&b':') {
        return Err("invalid_arguments: length does not match the body".into());
    }
    let run_bytes = body_bytes
        .get(..n)
        .ok_or("invalid_arguments: run id slice is out of range")?;
    let call_bytes = body_bytes
        .get(n + 1..)
        .ok_or("invalid_arguments: tool call id slice is out of range")?;
    let run_id = std::str::from_utf8(run_bytes)
        .map_err(|_| "invalid_arguments: run id is not UTF-8".to_string())?
        .to_owned();
    let tool_call_id = std::str::from_utf8(call_bytes)
        .map_err(|_| "invalid_arguments: tool call id is not UTF-8".to_string())?
        .to_owned();
    if run_id.as_bytes().is_empty() || tool_call_id.as_bytes().is_empty() {
        return Err("invalid_arguments: decoded id component is empty".into());
    }
    if run_id.as_bytes().len() > MAX_ID_BYTES || tool_call_id.as_bytes().len() > MAX_ID_BYTES {
        return Err(format!(
            "invalid_arguments: decoded id component exceeds {MAX_ID_BYTES} bytes"
        ));
    }
    Ok((run_id, tool_call_id))
}

/// The Host-derived job row key: strictly `job:` plus the canonical dispatch
/// string. Model-supplied idempotency keys never take part in uniqueness.
pub fn job_row_key(dispatch_id: &str) -> String {
    format!("job:{dispatch_id}")
}

/// Canonical digest helper: stable, length-prefixed, human-diffable.
pub fn canonical_digest(parts: &[(&str, &str)]) -> String {
    let mut canonical = String::new();
    for (key, value) in parts {
        canonical.push_str(&format!("{key}={}:{value}\n", value.len()));
    }
    format!("sha256:{}", sha256_hex(canonical.as_bytes()))
}

/// Action classification of one admitted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    Read,
    Write,
    Execute,
    Destructive,
    SensitiveEgress,
    /// Safe management surface (status/output/cancel of an already-admitted
    /// job). It never starts an external process, so it does not require a
    /// verified execution backend; scope checks still apply.
    Manage,
}

impl ActionClass {
    /// Conservative classification from the canonical tool contract category.
    /// `process` (opaque execution) requires a verified backend; writes and
    /// reads keep the existing controlled-tool behaviour.
    pub fn from_tool_category(category: &str) -> Self {
        match category {
            "process" => Self::Execute,
            "project-write" => Self::Write,
            _ => Self::Read,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Execute => "execute",
            Self::Destructive => "destructive",
            Self::SensitiveEgress => "sensitive_egress",
            Self::Manage => "manage",
        }
    }

    /// True when this class may only run behind a verified execution backend.
    pub fn requires_verified_backend(self) -> bool {
        matches!(
            self,
            Self::Execute | Self::Destructive | Self::SensitiveEgress
        )
    }

    /// True when this class needs an authoritative Host file observation as its
    /// baseline (a model-declared version is NOT an observation).
    pub fn requires_host_observation(self) -> bool {
        matches!(self, Self::Write | Self::Destructive)
    }
}

/// Real-time requirements the executor must re-verify before any side effect.
/// A requirement whose authoritative source does not exist yet is *unsatisfied*
/// (fail-closed); it is never silently treated as met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeRequirement {
    /// Live session policy version (source pending in a later batch).
    PolicyVersion,
    /// The parent's current revocation generation.
    ParentRevocation,
    /// The scope ResourceGrant is still valid.
    ResourceGrant,
    /// Run/tool cancellation state.
    Cancellation,
    /// A verified execution backend.
    BackendEvidence,
    /// An authoritative Host observation of the file target/version.
    HostObservation,
}

/// The requirements one class must satisfy at execution time.
pub fn requirements_for(class: ActionClass) -> Vec<RealtimeRequirement> {
    match class {
        ActionClass::Read | ActionClass::Manage => vec![RealtimeRequirement::Cancellation],
        ActionClass::Write => vec![
            RealtimeRequirement::Cancellation,
            RealtimeRequirement::ResourceGrant,
            // The file-specific blocker is reported before the session-level
            // one: it is the more actionable refusal.
            RealtimeRequirement::HostObservation,
            RealtimeRequirement::PolicyVersion,
            RealtimeRequirement::ParentRevocation,
        ],
        ActionClass::Execute | ActionClass::Destructive | ActionClass::SensitiveEgress => vec![
            RealtimeRequirement::Cancellation,
            RealtimeRequirement::ResourceGrant,
            RealtimeRequirement::PolicyVersion,
            RealtimeRequirement::BackendEvidence,
            RealtimeRequirement::ParentRevocation,
        ],
    }
}

/// An authoritative Host observation of a file target: which durable Host read
/// record saw which version of which target. A model-declared
/// `expectedVersion` is a *precondition claim*, never an observation.
///
/// REV-05: the record also carries **what was actually delivered**. Reading one
/// line establishes a line-range observation of that version, not a whole-file
/// one, and an extracted Office view is never the document's source text. The
/// fields default to the most conservative values so an older persisted
/// credential (which predates them) can never gain whole-file replacement
/// eligibility it did not have.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostObservation {
    pub target_identity: String,
    pub version: String,
    /// The durable Host tool-call record that produced this observation.
    pub observed_by_tool_call_id: String,
    /// Classification label only. It never expresses coverage strength; the
    /// concrete `range_start`/`range_end` and `covered_whole_file` do.
    #[serde(default)]
    pub view_kind: ObservationView,
    /// True only when the Host delivered the entire content of `version`.
    #[serde(default)]
    pub covered_whole_file: bool,
    /// Delivered range in UTF-16 code units, the single coordinate system the
    /// read tool's `offset`/`limit`/`nextOffset` already use. `None` for views
    /// with no text range (Office extract, missing file).
    #[serde(default)]
    pub range_start: Option<u64>,
    #[serde(default)]
    pub range_end: Option<u64>,
    /// Size of the whole text of `version` in UTF-16 code units.
    #[serde(default)]
    pub total_units: Option<u64>,
    /// The delivered page was itself cut short (output or read budget).
    #[serde(default)]
    pub truncated: bool,
}

/// What kind of view produced an observation. A label, not a coverage claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationView {
    /// The Host delivered the whole file content for this version.
    FullFile,
    /// A line range (`startLine`/`lineCount`).
    LineRange,
    /// A UTF-16 code-unit window (`offset`/`limit`).
    UnitWindow,
    /// Text extracted from a binary/Office container; not the source text.
    OfficeExtract,
    /// A row written before coverage tracking existed. Fail-closed.
    LegacyUnknown,
    /// A verified absent target.
    Missing,
}

impl Default for ObservationView {
    fn default() -> Self {
        Self::LegacyUnknown
    }
}

impl ObservationView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullFile => "full_file",
            Self::LineRange => "line_range",
            Self::UnitWindow => "unit_window",
            Self::OfficeExtract => "office_extract",
            Self::LegacyUnknown => "legacy_unknown",
            Self::Missing => "missing",
        }
    }

    /// Parse a stored label. Unknown labels are `LegacyUnknown` (fail-closed)
    /// rather than an error, so an upgraded database never gains eligibility.
    pub fn from_str(value: &str) -> Result<Self, String> {
        Ok(match value {
            "full_file" => Self::FullFile,
            "line_range" => Self::LineRange,
            "unit_window" => Self::UnitWindow,
            "office_extract" => Self::OfficeExtract,
            "legacy_unknown" => Self::LegacyUnknown,
            "missing" => Self::Missing,
            _ => Self::LegacyUnknown,
        })
    }
}

impl HostObservation {
    /// Whether the Host's SOURCE read delivered the whole content of `version`
    /// at the tool-result boundary.
    ///
    /// This is **not** a statement about what the model received. The model view
    /// is a later, narrower projection of the same durable result (a long body
    /// is kept as a head/tail view), so a `true` here can coexist with a model
    /// that never saw the middle of the file. Whole-file replacement admission
    /// must therefore combine this fact with the Host's own delivery check —
    /// `kernel_execution_admission::model_delivery_covered_whole_file_in_tx`,
    /// which replays the production projection on the durable row — and must
    /// treat an unknown delivery as *not* authorized.
    ///
    /// The fields this reads are the Host's own durable record; no model text
    /// and no tool-result self-description can influence it.
    pub fn authorizes_whole_file_replacement(&self) -> bool {
        self.covered_whole_file && self.view_kind == ObservationView::FullFile
    }

    /// Whether the delivered range covers `[start, end)` in UTF-16 code units.
    /// A missing or unknown range covers nothing.
    pub fn covers_units(&self, start: u64, end: u64) -> bool {
        match (self.range_start, self.range_end) {
            (Some(from), Some(to)) => from <= start && end <= to,
            _ => false,
        }
    }
}

/// What the executor reports about the target operation it just ran. This is
/// the Host-internal trusted evidence channel: it is produced by the code that
/// actually performed (or refused) the operation, NEVER inferred from the shape
/// of a tool result.
///
/// A Rust `Ok(..)` says nothing here: Fox tools return `Ok(json)` with
/// `isError=true` for business failures, and a successful read/query is not the
/// start of an external process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEvidence {
    /// The executor refused or completed the call WITHOUT starting the target
    /// operation (pre-execution refusal, validation failure, a read/query that
    /// finished). The attempt is terminal and no start fact exists.
    NotStarted,
    /// The executor has positive evidence that the target operation started
    /// (a recorded applying journal, a spawned process identity, ...). Only the
    /// executor can assert this.
    Started,
    /// The outcome of an already-started operation is unknown (crash,
    /// interruption, lost transport). Never replayed.
    Unknown,
}

/// The call-level outcome of an executor, INDEPENDENT of whether an external
/// operation was started. Only a trusted control branch (the Host's own
/// admission/scope/backend gates, or the executor's explicit refusal path) may
/// produce `Refused`; a result shape never can.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    /// The call ran and produced a successful result.
    Completed,
    /// A trusted control branch refused the call before any operation.
    Refused { code: String },
    /// The call failed; the real code is preserved and no side-effect fact is
    /// invented from the failure shape.
    Failed { code: String },
}

/// The settlement decision the Host derives from trusted evidence and the
/// call outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// Confirm the start fact (only from `Started`).
    ConfirmStart,
    /// The call finished without an external start (terminal success).
    Complete,
    /// Terminal refusal with no start fact.
    Refuse,
    /// Mark the outcome unknown, preserving any already-confirmed start.
    MarkUnknown,
}

/// Map trusted evidence + call outcome to the durable settlement.
///
/// `Started` is the ONLY way a start fact can be confirmed, and a confirmed
/// start is never turned off. `Unknown` evidence always settles unknown,
/// whatever the call outcome was: the side effect of a side-effecting executor
/// cannot be read off a result shape. Only a `NotStarted` fact that came from a
/// trusted control branch can settle a terminal success or refusal.
pub fn settlement_for(evidence: ExecutionEvidence, outcome: &CallOutcome) -> Settlement {
    match evidence {
        ExecutionEvidence::Started => Settlement::ConfirmStart,
        ExecutionEvidence::Unknown => Settlement::MarkUnknown,
        ExecutionEvidence::NotStarted => match outcome {
            CallOutcome::Completed => Settlement::Complete,
            CallOutcome::Refused { .. } | CallOutcome::Failed { .. } => Settlement::Refuse,
        },
    }
}

/// Backend requirement bound into a credential. `evidence_digest` is `None`
/// until a backend has actually been verified; it is never synthesized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendRequirement {
    pub required: String,
    pub evidence_digest: Option<String>,
}

/// The immutable execution credential issued once per dispatch inside the
/// final admission transaction. The model never constructs it: the Host
/// derives every field from durable facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionCredential {
    pub dispatch_id: String,
    pub run_id: String,
    pub conversation_id: String,
    /// Digest over the frozen tool + canonical input (what was admitted).
    pub intent_digest: String,
    pub action_class: ActionClass,
    /// Authoritative Host observation of the file target. `None` means no
    /// observation existed at issue time — a write-class action must then be
    /// refused, never executed against a model-declared version.
    pub file_baseline: Option<HostObservation>,
    pub resolved_profile: String,
    /// Durable policy identity bound at issue time (the frozen permission
    /// snapshot id).
    pub policy_snapshot_id: String,
    /// Live session policy version. `None` = no such durable fact exists yet;
    /// the live-version re-verification requirement stays listed and
    /// unsatisfied rather than being silently skipped.
    pub policy_version: Option<u64>,
    /// Budget bound at issue time (milliseconds). The executor may only
    /// tighten it, never widen it.
    pub budget_ceiling_ms: i64,
    /// The parent's revocation generation observed at issue time. `None` = no
    /// parent fact (root run); a child must re-read the parent's *current*
    /// generation at execution.
    pub parent_revocation_generation: Option<u64>,
    /// What the executor must re-verify in real time before any side effect.
    pub realtime_requirements: Vec<RealtimeRequirement>,
    pub backend_requirement: BackendRequirement,
    pub credential_digest: String,
    // --- Whole-file-replacement authorization (REV-05 / coordinator M3) ---
    //
    // A `write_file` that replaces the whole file is a stronger operation than a
    // precise edit. When the bound observation did not deliver the whole
    // content, this dispatch additionally requires the purpose-specific
    // replacement authorization, consumed atomically with the persistent claim.
    //
    // All three fields are `#[serde(default)]` and are deliberately EXCLUDED
    // from `recompute_digest`, so a credential row persisted before they
    // existed still deserializes and still passes its digest check.
    #[serde(default)]
    pub requires_replace_grant: bool,
    #[serde(default)]
    pub replace_candidate_digest: Option<String>,
    #[serde(default)]
    pub replace_request_digest: Option<String>,
}

impl ExecutionCredential {
    /// Build and sign a credential. The digest covers every fixed field, so a
    /// later field change is detectable even when the caller recomputes it.
    pub fn new(
        dispatch_id: String,
        run_id: String,
        conversation_id: String,
        intent_digest: String,
        action_class: ActionClass,
        file_baseline: Option<HostObservation>,
        resolved_profile: String,
        policy_snapshot_id: String,
        policy_version: Option<u64>,
        budget_ceiling_ms: i64,
        parent_revocation_generation: Option<u64>,
        backend_requirement: BackendRequirement,
    ) -> Result<Self, String> {
        if dispatch_id.trim().is_empty()
            || run_id.trim().is_empty()
            || conversation_id.trim().is_empty()
            || intent_digest.trim().is_empty()
            || resolved_profile.trim().is_empty()
            || policy_snapshot_id.trim().is_empty()
        {
            return Err("credential identity fields must be non-empty".into());
        }
        if budget_ceiling_ms < 0 {
            return Err("credential budget ceiling must not be negative".into());
        }
        // The dispatch identity must decode back to exactly this run/tool pair.
        let (decoded_run, decoded_call) = decode_dispatch_id(&dispatch_id)?;
        if decoded_run != run_id {
            return Err("credential dispatch id does not belong to this run".into());
        }
        if decoded_call.trim().is_empty() {
            return Err("credential dispatch id has an empty tool call".into());
        }
        let mut credential = Self {
            credential_digest: String::new(),
            dispatch_id,
            run_id,
            conversation_id,
            intent_digest,
            action_class,
            file_baseline,
            resolved_profile,
            policy_snapshot_id,
            policy_version,
            budget_ceiling_ms,
            parent_revocation_generation,
            realtime_requirements: requirements_for(action_class),
            backend_requirement,
            requires_replace_grant: false,
            replace_candidate_digest: None,
            replace_request_digest: None,
        };
        credential.credential_digest = credential.recompute_digest();
        Ok(credential)
    }

    /// Recompute the digest from the current fields.
    pub fn recompute_digest(&self) -> String {
        canonical_digest(&[
            ("dispatchId", &self.dispatch_id),
            ("runId", &self.run_id),
            ("conversationId", &self.conversation_id),
            ("intentDigest", &self.intent_digest),
            ("actionClass", self.action_class.as_str()),
            (
                "fileBaseline",
                &match &self.file_baseline {
                    Some(observation) => format!(
                        "{}@{}#{}",
                        observation.target_identity,
                        observation.version,
                        observation.observed_by_tool_call_id
                    ),
                    None => "-".to_owned(),
                },
            ),
            ("resolvedProfile", &self.resolved_profile),
            ("policySnapshotId", &self.policy_snapshot_id),
            (
                "policyVersion",
                &self
                    .policy_version
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
            ),
            ("budgetCeilingMs", &self.budget_ceiling_ms.to_string()),
            (
                "parentRevocationGeneration",
                &self
                    .parent_revocation_generation
                    .map(|generation| generation.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
            ),
            (
                "realtimeRequirements",
                &self
                    .realtime_requirements
                    .iter()
                    .map(|requirement| format!("{requirement:?}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ("backendRequired", &self.backend_requirement.required),
            (
                "backendEvidence",
                self.backend_requirement.evidence_digest.as_deref().unwrap_or("-"),
            ),
        ])
    }

    /// Bind the purpose-specific whole-file-replacement authorization to this
    /// dispatch. Called at issuance from durable Host facts only: the target,
    /// the observed version, the candidate content digest and the request
    /// digest. An ordinary one-shot approval or a reusable grant can never
    /// substitute for it.
    pub fn with_replace_grant(mut self, candidate_digest: String, request_digest: String) -> Self {
        self.requires_replace_grant = true;
        self.replace_candidate_digest = Some(candidate_digest);
        self.replace_request_digest = Some(request_digest);
        self
    }

    /// Field-level equality against the authoritative snapshot is normative;
    /// the digest is an optimization, never the only check.
    pub fn verify_against(&self, stored: &ExecutionCredential) -> Result<(), String> {
        if self.dispatch_id != stored.dispatch_id
            || self.run_id != stored.run_id
            || self.conversation_id != stored.conversation_id
            || self.intent_digest != stored.intent_digest
            || self.action_class != stored.action_class
            || self.file_baseline != stored.file_baseline
            || self.resolved_profile != stored.resolved_profile
            || self.policy_snapshot_id != stored.policy_snapshot_id
            || self.policy_version != stored.policy_version
            || self.budget_ceiling_ms != stored.budget_ceiling_ms
            || self.parent_revocation_generation != stored.parent_revocation_generation
            || self.realtime_requirements != stored.realtime_requirements
            || self.backend_requirement != stored.backend_requirement
            // The whole-file-replacement authorization is part of the
            // authoritative snapshot. Without this comparison a presented
            // credential could clear the flag (or swap either digest) and skip
            // the dedicated authorization entirely.
            || self.requires_replace_grant != stored.requires_replace_grant
            || self.replace_candidate_digest != stored.replace_candidate_digest
            || self.replace_request_digest != stored.replace_request_digest
        {
            return Err("credential_mismatch: differs from the authoritative issued snapshot".into());
        }
        if self.credential_digest != stored.credential_digest
            || self.credential_digest != self.recompute_digest()
        {
            return Err("credential_mismatch: digest does not match the snapshot".into());
        }
        Ok(())
    }
}

/// The A/C seam carrier. It is the same value the executor presents back to
/// the Host; the Host verifies it against the stored snapshot before any
/// claim. Kept as a distinct name for the adapter-facing documentation.
pub type ExecutionAdmission = ExecutionCredential;

/// Durable state of one execution attempt (one dispatch, at most one try).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptState {
    Claimed,
    /// The call finished without starting an external operation (a terminal
    /// success, e.g. a read/query). `start_confirmed` records the fact.
    Completed,
    Refused { code: String },
    Launched,
    Unknown,
}

/// Outcome of a single atomic claim across independent connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// This caller won the claim and may start the first execution attempt.
    Claimed,
    /// A previous attempt was refused before job creation; the refusal is final.
    AlreadyRefused { code: String },
    /// A previous attempt already launched; repeat requests are read-only.
    AlreadyLaunched,
    /// A previous call completed without starting an external operation; repeat
    /// requests are read-only and report the same terminal success.
    AlreadyCompleted,
    /// Claimed earlier, outcome unknown (crash/recovery); never replayed.
    Unknown,
}

/// Three-valued fact: unknown is never collapsed into true or false.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriState {
    True,
    False,
    Unknown,
}

/// Control-plane persistence state, enumerated per stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlPlaneState {
    None,
    Prepared,
    Applying,
    Committed,
    NotApplied,
    Uncertain,
    Unknown,
}

/// External (target) side-effect state, kept separate from the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectState {
    None,
    Prepared,
    Applying,
    Committed,
    NotApplied,
    Uncertain,
    Unknown,
}

/// Execution family: file and process stages are expressed separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    Process,
    File,
}

/// Stage enumeration. Process and file stages never mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStage {
    // process family
    Admitted,
    JobCreated,
    LaunchConfirmed,
    Interrupted,
    // file family
    FilePrepared,
    FileApplying,
    FileCommitted,
    FileNotApplied,
    FileRecoveryRequired,
    FileIndeterminate,
}

impl ExecutionStage {
    pub fn belongs_to(self, kind: ExecutionKind) -> bool {
        matches!(
            (kind, self),
            (
                ExecutionKind::Process,
                ExecutionStage::Admitted
                    | ExecutionStage::JobCreated
                    | ExecutionStage::LaunchConfirmed
                    | ExecutionStage::Interrupted
            ) | (
                ExecutionKind::File,
                ExecutionStage::FilePrepared
                    | ExecutionStage::FileApplying
                    | ExecutionStage::FileCommitted
                    | ExecutionStage::FileNotApplied
                    | ExecutionStage::FileRecoveryRequired
                    | ExecutionStage::FileIndeterminate
            )
        )
    }
}

/// One stage receipt. Illegal kind/stage or state combinations are rejected at
/// construction, so a receipt can never claim a fact its stage cannot hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionReceipt {
    pub dispatch_id: String,
    pub kind: ExecutionKind,
    pub stage: ExecutionStage,
    pub execution_started: TriState,
    pub control_plane: ControlPlaneState,
    pub external_effect: SideEffectState,
    pub reason_code: Option<String>,
    /// Same-fact alias of an uncertain outcome (`job.interrupted_unknown`).
    pub code_alias: Option<String>,
    pub resumable: bool,
    /// Always false in the first version; kept explicit so wiring cannot
    /// "helpfully" replay a side-effecting attempt.
    pub allow_replay: bool,
}

impl ExecutionReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dispatch_id: String,
        kind: ExecutionKind,
        stage: ExecutionStage,
        execution_started: TriState,
        control_plane: ControlPlaneState,
        external_effect: SideEffectState,
        reason_code: Option<String>,
        code_alias: Option<String>,
    ) -> Result<Self, String> {
        if dispatch_id.trim().is_empty() {
            return Err("receipt dispatch id must be non-empty".into());
        }
        if !stage.belongs_to(kind) {
            return Err(format!("stage {stage:?} does not belong to kind {kind:?}"));
        }
        Ok(Self {
            dispatch_id,
            kind,
            stage,
            execution_started,
            control_plane,
            external_effect,
            reason_code,
            code_alias,
            resumable: false,
            allow_replay: false,
        })
    }

    /// A confirmed start can never be regressed to "not started".
    pub fn with_execution_started(mut self, started: TriState) -> Result<Self, String> {
        if self.execution_started == TriState::True && started != TriState::True {
            return Err("a confirmed start cannot be regressed".into());
        }
        self.execution_started = started;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = "a".repeat(1000);
        assert_eq!(
            sha256_hex(long.as_bytes()),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn encoding_roundtrips_and_rejects_noncanonical_input() {
        // Colon-collision pairs stay distinct.
        let a = encode_dispatch_id("a:b", "c").unwrap();
        let b = encode_dispatch_id("a", "b:c").unwrap();
        assert_ne!(a, b);
        assert_eq!(a, "tool-dispatch:3:a:b:c");
        assert_eq!(b, "tool-dispatch:1:a:b:c");
        assert_eq!(decode_dispatch_id(&a).unwrap(), ("a:b".to_owned(), "c".to_owned()));
        assert_eq!(decode_dispatch_id(&b).unwrap(), ("a".to_owned(), "b:c".to_owned()));
        // Chinese UTF-8 counts bytes, not characters.
        let zh = encode_dispatch_id("运行:甲", "调用:乙").unwrap();
        assert_eq!(zh, "tool-dispatch:10:运行:甲:调用:乙");
        assert_eq!(decode_dispatch_id(&zh).unwrap(), ("运行:甲".to_owned(), "调用:乙".to_owned()));
        // Boundary lengths.
        let max = encode_dispatch_id(&"r".repeat(1024), &"c".repeat(1024)).unwrap();
        assert_eq!(
            decode_dispatch_id(&max).unwrap(),
            ("r".repeat(1024), "c".repeat(1024))
        );
        // Rejections (O review8 counter-examples included).
        assert!(encode_dispatch_id("", "c").is_err());
        assert!(encode_dispatch_id("r", "").is_err());
        assert!(encode_dispatch_id(&"r".repeat(1025), "c").is_err());
        assert!(encode_dispatch_id("r", &"c".repeat(1025)).is_err());
        assert!(decode_dispatch_id("tool-dispatch:01:r:c").is_err());
        assert!(decode_dispatch_id("tool-dispatch:00:r:c").is_err());
        assert!(decode_dispatch_id("tool-dispatch:0::c").is_err());
        assert!(decode_dispatch_id("tool-dispatch:x:a:b").is_err());
        assert!(decode_dispatch_id("tool-dispatch:9:a:b").is_err());
        assert!(decode_dispatch_id("other:1:a:b").is_err());
        assert!(decode_dispatch_id("tool-dispatch:1:").is_err());
        assert!(decode_dispatch_id("tool-dispatch:1:r:").is_err());
        assert!(decode_dispatch_id("tool-dispatch:1r:c").is_err());
        assert!(decode_dispatch_id("tool-dispatch:2:运行:c").is_err());
        assert!(decode_dispatch_id("tool-dispatch:3:运行:c").is_err());
        let huge = format!("tool-dispatch:{}:r:c", usize::MAX);
        assert!(decode_dispatch_id(&huge).is_err());
        let digits = format!("tool-dispatch:{}:r:c", "9".repeat(64));
        assert!(decode_dispatch_id(&digits).is_err());
        let over_call = format!("tool-dispatch:1:r:{}", "c".repeat(1025));
        assert!(decode_dispatch_id(&over_call).is_err());
        let over_run = format!("tool-dispatch:1025:{}:c", "r".repeat(1025));
        assert!(decode_dispatch_id(&over_run).is_err());
    }

    #[test]
    fn credential_digest_binds_every_fixed_field() {
        let observation = HostObservation {
            target_identity: "a.txt".into(),
            version: "v7".into(),
            observed_by_tool_call_id: "read-1".into(),
            ..Default::default()
        };
        let credential = ExecutionCredential::new(
            encode_dispatch_id("run-a", "call-1").unwrap(),
            "run-a".into(),
            "conversation-a".into(),
            "sha256:intent".into(),
            ActionClass::Write,
            Some(observation.clone()),
            "legacy".into(),
            "sha256:policy".into(),
            None,
            60_000,
            None,
            BackendRequirement {
                required: "verified-backend".into(),
                evidence_digest: None,
            },
        )
        .unwrap();
        assert_eq!(credential.credential_digest, credential.recompute_digest());
        // Same dispatch, tampered fixed field, recomputed self-consistent digest.
        let mut hostile = credential.clone();
        hostile.resolved_profile = "durable_v2".into();
        hostile.credential_digest = hostile.recompute_digest();
        assert!(hostile.verify_against(&credential).is_err());
        assert!(credential.verify_against(&credential).is_ok());
        // Tampering with a dynamic requirement field is also a mismatch.
        let mut hostile_requirement = credential.clone();
        hostile_requirement.realtime_requirements = vec![RealtimeRequirement::Cancellation];
        hostile_requirement.credential_digest = hostile_requirement.recompute_digest();
        assert!(hostile_requirement.verify_against(&credential).is_err());
        // A dispatch id from another run cannot be signed for this run.
        assert!(ExecutionCredential::new(
            encode_dispatch_id("run-b", "call-1").unwrap(),
            "run-a".into(),
            "conversation-a".into(),
            "sha256:intent".into(),
            ActionClass::Write,
            None,
            "legacy".into(),
            "sha256:policy".into(),
            None,
            60_000,
            None,
            BackendRequirement { required: "verified-backend".into(), evidence_digest: None },
        )
        .is_err());
        // A write without a Host observation keeps its requirement listed.
        assert!(credential
            .realtime_requirements
            .contains(&RealtimeRequirement::HostObservation));
        assert!(credential
            .realtime_requirements
            .contains(&RealtimeRequirement::PolicyVersion));
    }

    #[test]
    fn receipts_reject_illegal_stage_combinations_and_regression() {
        let receipt = ExecutionReceipt::new(
            "tool-dispatch:5:run-a:call-1".into(),
            ExecutionKind::Process,
            ExecutionStage::LaunchConfirmed,
            TriState::True,
            ControlPlaneState::Applying,
            SideEffectState::Applying,
            None,
            None,
        )
        .unwrap();
        assert!(receipt.clone().with_execution_started(TriState::False).is_err());
        assert_eq!(
            receipt.with_execution_started(TriState::True).unwrap().execution_started,
            TriState::True
        );
        // A file stage cannot be worn by a process receipt.
        assert!(ExecutionReceipt::new(
            "tool-dispatch:5:run-a:call-1".into(),
            ExecutionKind::Process,
            ExecutionStage::FilePrepared,
            TriState::False,
            ControlPlaneState::Prepared,
            SideEffectState::None,
            None,
            None,
        )
        .is_err());
        assert!(ExecutionReceipt::new(
            "tool-dispatch:5:run-a:call-1".into(),
            ExecutionKind::File,
            ExecutionStage::FilePrepared,
            TriState::False,
            ControlPlaneState::Prepared,
            SideEffectState::None,
            None,
            None,
        )
        .is_ok());
    }
}

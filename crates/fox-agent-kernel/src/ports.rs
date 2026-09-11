//! Fox Agent Kernel — hexagonal ports.
//!
//! The Kernel decision core is pure and synchronous: identical inputs produce
//! identical decisions and state transitions, so it runs in plain Rust tests.
//! The Kernel returns [`crate::controller::Effect`]s describing what the
//! host must do (append an event, dispatch a tool, start/cancel an engine turn,
//! rebuild a snapshot); adapters — Tauri commands, the SQLite repository, the
//! Node/JSONL sidecar — perform the actual IO. The Kernel core never depends on
//! `tauri::AppHandle`, rusqlite, Pi types or React/TypeScript types.

use crate::state::ApprovalDecision;
use serde::{Deserialize, Serialize};

/// Execution clock. Two distinct time domains, per the Kernel time model:
///
/// * **monotonic** — in-process elapsed execution time. It never moves
///   backwards but is NOT comparable across process restarts, so the run's
///   accumulated execution time is persisted and restored, not derived from a
///   fresh monotonic reading after restart.
/// * **wall** — real clock time, used for persistent human-approval and scheduled
///   retry deadlines so waits survive restart. A backwards wall jump is detected
///   against the persisted anchor and fails closed rather than waiting forever.
/// A paired reading of both time domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockReading {
    pub monotonic_ms: i64,
    pub wall_ms: i64,
}

pub trait Clock: Send + Sync {
    fn now_monotonic_ms(&self) -> i64;
    fn now_wall_ms(&self) -> i64;
    fn read(&self) -> ClockReading {
        ClockReading {
            monotonic_ms: self.now_monotonic_ms(),
            wall_ms: self.now_wall_ms(),
        }
    }
}

/// Per-run and per-tool cancellation. Tokens are scoped by run id / tool call id
/// so cancelling one run never tears down another run's in-flight work.
pub trait CancellationPort: Send + Sync {
    fn request_run_cancel(&self, run_id: &str);
    fn is_run_cancelled(&self, run_id: &str) -> bool;
    fn request_tool_cancel(&self, run_id: &str, tool_call_id: &str);
    fn is_tool_cancelled(&self, run_id: &str, tool_call_id: &str) -> bool;
}

/// Immutable facts frozen at run creation. A run never silently changes engine,
/// mode, policy or profile mid-flight. Serialized verbatim into the durable run
/// row so a rehydrated run freezes the exact same identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFrozenConfig {
    pub engine_id: String,
    pub kernel_mode: String,
    pub capability_manifest_version: u32,
    /// Immutable hash (or canonical JSON hash) of the full Capability Manifest
    /// snapshot, not just the version number. A run freezes the manifest it
    /// started with; a restart rehydrates this identity and never silently
    /// adopts a different manifest.
    pub capability_manifest_hash: String,
    pub permission_snapshot_id: String,
    pub execution_profile_id: String,
    pub prompt_config_hash: String,
    /// Distinct, separately-modelled time budgets (milliseconds). Human approval
    /// wait is NOT modelled with the ordinary host-request timeout. The current
    /// controller enforces run execution, tool-execution and approval-wait budgets;
    /// model-request timeout still requires production adapter wiring.
    pub model_request_timeout_ms: i64,
    pub tool_execution_timeout_ms: i64,
    pub run_execution_budget_ms: i64,
    /// Approval wall-clock. The run execution budget is suspended while waiting.
    pub approval_wait_timeout_ms: i64,
    /// Provider HTTP retry and whole-turn retry are independent policies.
    pub provider_max_retries: u32,
    pub turn_max_retries: u32,
}

/// A single tool call proposed by the engine within a turn/batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub tool_call_id: String,
    pub tool: String,
    /// Canonical input JSON after all non-safety extensions have run. The Kernel
    /// approves these final parameters; the Resource Gateway re-validates them.
    pub canonical_input_json: String,
    pub source_order: usize,
}

/// Kernel policy decision for a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Policy pre-approves the tool; no human prompt (auto-run within policy).
    Allow,
    /// A human must decide; the run enters `waiting_approval`.
    RequireApproval,
    /// Out of policy / high risk; hard deny.
    Deny { reason: String },
}

/// Policy authority. In authoritative mode this is the single decision source;
/// React, Node and Rust adapters must not keep conflicting approval logic.
pub trait PolicyDecisionPort: Send + Sync {
    fn decide(
        &self,
        run_id: &str,
        permission_snapshot_id: &str,
        tool: &str,
        canonical_input_json: &str,
    ) -> PolicyDecision;
}

/// Append-only event store. Events are the durable fact stream; the snapshot is a
/// projection of them. Every append is idempotent on `(run_id, seq)`.
pub trait EventStorePort: Send + Sync {
    fn append_event(
        &self,
        run_id: &str,
        seq: u64,
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), String>;
}

/// Read-model projection. The snapshot is rebuilt from durable facts; React reads
/// only the snapshot and never infers authoritative state from scattered events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSnapshot {
    pub run_id: String,
    pub state: String,
    pub engine_id: String,
    pub kernel_mode: String,
    pub waiting_approval_tool_call_ids: Vec<String>,
    pub pending_tool_call_ids: Vec<String>,
    pub completed_tool_call_ids: Vec<String>,
    pub last_event_seq: u64,
}

pub trait SnapshotProjectionPort: Send + Sync {
    fn rebuild(&self, run_id: &str) -> Result<RunSnapshot, String>;
    fn publish(&self, snapshot: &RunSnapshot) -> Result<(), String>;
}

/// Kind of durable external effect tracked by the outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxEffectKind {
    InitialModel,
    DispatchTool,
    RequestApproval,
    CancelEngineTurn,
    CancelToolCall,
    DeliverToolBatch,
    PublishSnapshot,
}

impl OutboxEffectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OutboxEffectKind::InitialModel => "initial_model",
            OutboxEffectKind::DispatchTool => "dispatch_tool",
            OutboxEffectKind::RequestApproval => "request_approval",
            OutboxEffectKind::CancelEngineTurn => "cancel_engine_turn",
            OutboxEffectKind::CancelToolCall => "cancel_tool_call",
            OutboxEffectKind::DeliverToolBatch => "deliver_tool_batch",
            OutboxEffectKind::PublishSnapshot => "publish_snapshot",
        }
    }

    pub fn parse(value: &str) -> Option<OutboxEffectKind> {
        match value {
            "initial_model" => Some(OutboxEffectKind::InitialModel),
            "dispatch_tool" => Some(OutboxEffectKind::DispatchTool),
            "request_approval" => Some(OutboxEffectKind::RequestApproval),
            "cancel_engine_turn" => Some(OutboxEffectKind::CancelEngineTurn),
            "cancel_tool_call" => Some(OutboxEffectKind::CancelToolCall),
            "deliver_tool_batch" => Some(OutboxEffectKind::DeliverToolBatch),
            "publish_snapshot" => Some(OutboxEffectKind::PublishSnapshot),
            _ => None,
        }
    }
}

/// Outbox lifecycle. External actions are committed as `pending` in the same
/// transaction as the decision, then `leased` by an executor and finally
/// `completed`/`failed`. A `leased` row left by a crashed process is recovered by
/// idempotency-key reconciliation, never blindly re-executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    Pending,
    Leased,
    Completed,
    Failed,
}

impl OutboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OutboxStatus::Pending => "pending",
            OutboxStatus::Leased => "leased",
            OutboxStatus::Completed => "completed",
            OutboxStatus::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<OutboxStatus> {
        match value {
            "pending" => Some(OutboxStatus::Pending),
            "leased" => Some(OutboxStatus::Leased),
            "completed" => Some(OutboxStatus::Completed),
            "failed" => Some(OutboxStatus::Failed),
            _ => None,
        }
    }
}

/// A durable external effect row. The `idempotency_key` is stable across retries
/// and restarts so the executor performs the logical side effect at most once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEffect {
    pub effect_key: String,
    pub kind: OutboxEffectKind,
    pub idempotency_key: String,
    pub tool_call_id: Option<String>,
    pub batch_id: Option<String>,
    pub payload_json: String,
    pub status: OutboxStatus,
    pub attempts: u32,
}

/// Provider-HTTP vs whole-turn retry accounting. The two are independent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RetryState {
    pub provider_attempts: u32,
    pub provider_max: u32,
    pub turn_attempts: u32,
    pub turn_max: u32,
    /// Incomplete model answers are retried per logical request, bounded by
    /// turn_max. A new request requires a new tool batch; run/tool budgets still
    /// bound the whole run. Kept separate from whole-turn and provider retries.
    #[serde(default)]
    pub completion_effect_key: Option<String>,
    #[serde(default)]
    pub completion_attempts: u32,
    /// Durable wall-clock anchor and due time for a scheduled whole-turn retry.
    /// Both are needed to survive restart and to fail closed on wall-clock rollback.
    #[serde(default)]
    pub scheduled_at_wall_ms: Option<i64>,
    #[serde(default)]
    pub due_wall_ms: Option<i64>,
    /// A settled model failure is waiting for a new owned dispatch. Unlike a
    /// legacy observed retry, resuming the delay must not arm the model clock.
    #[serde(default)]
    pub model_dispatch_pending: bool,
}

/// Context-compaction accounting, kept distinct from retry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CompactionState {
    pub compactions: u32,
    pub last_reason: Option<String>,
    #[serde(default)]
    pub pending: Option<PendingCompaction>,
}

/// Durable single-flight intent. Full input and output are append-only events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCompaction {
    pub id: String,
    pub owner: Option<String>,
}

/// One tool call as projected into the rich snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotToolCall {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    pub source_order: usize,
    pub state: String,
    pub result_json: Option<String>,
    pub approval_state: Option<String>,
    pub dispatch_idempotency_key: Option<String>,
}

/// One tool batch as projected into the rich snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotToolBatch {
    pub batch_id: String,
    pub ordered_tool_call_ids: Vec<String>,
    pub barrier_emitted: bool,
}

/// Rich read model built from a single consistent SQLite read transaction. It
/// never mixes two commit points: all rows are read under one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelSnapshot {
    pub run_id: String,
    pub turn_id: Option<String>,
    pub state: String,
    pub engine_id: String,
    pub kernel_mode: String,
    pub capability_manifest_hash: Option<String>,
    pub last_event_seq: u64,
    pub running_elapsed_ms: i64,
    pub approval_deadline_wall_ms: Option<i64>,
    pub terminal_written: bool,
    pub batches: Vec<SnapshotToolBatch>,
    pub tool_calls: Vec<SnapshotToolCall>,
    pub pending_effects: Vec<OutboxEffect>,
    pub retry: RetryState,
    pub compaction: CompactionState,
}

/// Resource Gateway / Tool Host boundary. The Kernel decides *whether* a tool
/// runs; the host executes it via this port and the gateway re-validates the
/// final parameters against the frozen permission snapshot before any side effect.
pub trait ToolDispatchPort: Send + Sync {
    fn dispatch(
        &self,
        run_id: &str,
        tool_call_id: &str,
        tool: &str,
        canonical_input_json: &str,
    ) -> Result<(), String>;
}

/// Engine adapter boundary. The Kernel does not implement Pi/DeepSeek/Codex
/// streaming protocols; it instructs the host to start or cancel a turn and the
/// adapter translates engine events back into Kernel inputs.
pub trait EnginePort: Send + Sync {
    fn start_turn(&self, run_id: &str, turn_id: &str, prompt: &str) -> Result<(), String>;
    fn cancel_turn(&self, run_id: &str, turn_id: &str) -> Result<(), String>;
}

/// Approval resolution carried out by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResolution {
    pub run_id: String,
    pub tool_call_id: String,
    pub decision: ApprovalDecision,
}

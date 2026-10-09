//! Durable coordinator for opt-in authoritative RuntimeHost execution.
//! Default Legacy mode and production rollout gates remain separate.
//! The executor receives a frozen binding and must revalidate resources itself.
use crate::{
    database::Database,
    kernel::{
        self, CancellationPort, CancellationRegistry, Clock, ClockReading, Effect, KernelError,
        PolicyDecisionPort, RunController, ToolCallRequest,
    },
};
use fox_engine_protocol::{ExecutionAuthority, RunControlBinding};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

mod compaction;
pub(super) mod live;
mod receipt;
pub(crate) use receipt::append_execution_receipt;
mod steering;

/// Returned by a model transport when a terminal decision lost a race with a
/// mid-run request that was accepted moments earlier.
///
/// It is a scheduling outcome, not an execution failure: the drive loop
/// re-plans the Run from durable facts, where the freshly accepted row is
/// visible and becomes an ordinary additional-input round. Reported as a plain
/// error it would turn a normal user action into a task failure.
pub(crate) const STEERING_REPLAN: &str = "kernel.steering_replan";

/// Translate a lost terminal race into the re-plan signal.
///
/// The decision write-set rolled back, so nothing is committed and no engine
/// work was consumed; re-entering the drive loop re-reads durable facts (where
/// the freshly accepted request is still `received`) and plans an additional
/// input round for it.
/// Normalize a decision error: a lost terminal race becomes the re-plan signal,
/// everything else is passed through unchanged.
pub(crate) fn replan_or_error(error: String) -> String {
    if error.starts_with(kernel::STEERING_COMPETITION) {
        STEERING_REPLAN.to_string()
    } else {
        error
    }
}

fn checkpoint_hash(value: &Value) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(value.to_string().as_bytes()))
    )
}

/// Rebuild the pre-response transcript from the very frame leased to Node.
/// The caller may not invent a middle segment or a historyStart for a park.
pub(crate) fn bound_model_delivery_history(
    run_id: &str,
    payload: &Value,
    live_history: Option<&[Value]>,
) -> Result<(Vec<Value>, usize), String> {
    if let Some(history) = live_history {
        let directive: fox_engine_protocol::KernelRoundDirective =
            serde_json::from_value(payload.clone()).map_err(|_| "invalid bound live directive")?;
        directive.validate()?;
        let mut delivered = history.to_vec();
        let start = delivered.len();
        delivered.extend(directive.host_job_notices.iter()
            .map(fox_engine_protocol::HostJobNotice::history_marker));
        return Ok((delivered,start));
    }
    if payload.get("input").is_some() {
        let frame: fox_engine_protocol::KernelInitialModelFrame =
            serde_json::from_value(payload.clone()).map_err(|_| "invalid bound initial frame")?;
        frame.validate()?;
        let mut delivered = frame.input.messages;
        let start = delivered.len();
        delivered.extend(frame.host_job_notices.iter()
            .map(fox_engine_protocol::HostJobNotice::history_marker));
        return Ok((delivered,start));
    }
    let frame: fox_engine_protocol::KernelBatchResumeFrame =
        serde_json::from_value(payload.clone()).map_err(|_| "invalid bound batch frame")?;
    frame.validate()?;
    let mut delivered=bound_batch_live_history(run_id,&frame.history,
        &frame.assistant_message,&frame.tools,&frame.steering);
    let start = delivered.len();
    delivered.extend(frame.host_job_notices.iter()
        .map(fox_engine_protocol::HostJobNotice::history_marker));
    Ok((delivered,start))
}

/// Exact model text for a previously accepted steering row. The lease-side
/// source replay and Host dispatch builder must use the same projection.
pub(crate) fn bound_steering_user_message(content:&str, received_at:i64) -> Value {
    serde_json::json!({"role":"user","content":[{"type":"text",
        "text":steering::steering_notice_text(content)}],"timestamp":received_at})
}

/// The same middle segment is sent by replacement batch frames and live Pi
/// directives, then replayed under the model lease inside SQLite.
pub(crate) fn bound_batch_live_history(
    run_id:&str,history:&[Value],assistant:&Value,
    tools:&[fox_engine_protocol::KernelSettledToolResult],
    steering:&[fox_engine_protocol::KernelSteeringNotice],
) -> Vec<Value> {
    let mut delivered=history.to_vec();
    delivered.push(assistant.clone());
    delivered.extend(steering::settled_tool_result_messages(run_id,tools,assistant));
    delivered.extend(steering.iter().map(|notice|bound_steering_user_message(
        &notice.content,notice.received_at.unwrap_or(0))));
    delivered
}

#[cfg(test)]
mod notice_lease_test_barrier {
    use std::collections::HashMap;
    use std::sync::{Mutex,OnceLock};
    type Hook=Box<dyn Fn() + Send + 'static>;
    fn hooks()->&'static Mutex<HashMap<String,Hook>> {
        static HOOKS:OnceLock<Mutex<HashMap<String,Hook>>>=OnceLock::new();
        HOOKS.get_or_init(||Mutex::new(HashMap::new()))
    }
    pub(super) fn install(run:&str,hook:Hook) {
        hooks().lock().expect("notice barrier lock").insert(run.to_owned(),hook);
    }
    pub(super) fn fire(run:&str) {
        let hook=hooks().lock().expect("notice barrier lock").remove(run);
        if let Some(hook)=hook { hook(); }
    }
}

enum DecisionLease<'a> {
    Approval(u64),
    WaitingWakePolicy(u64),
    WaitingAccount,
    ModelRetry(&'a str, &'a str),
    Initial(&'a str),
    Continuation(&'a str, &'a str),
    ContinuationDispatch(&'a str, &'a str, Option<&'a crate::database::ModelNoticeInput<'a>>),
    Tool(&'a str, &'a str),
    Batch(&'a str, &'a str),
    /// Claim a settled batch's pending delivery and arm its model request.
    BatchDispatch(&'a str, &'a str, Option<&'a crate::database::ModelNoticeInput<'a>>),
}

/// What a round that is about to finish the Run must do about the task's promised
/// deliverables.
///
/// Every entry point that can finish a Run runs the same two-phase delivery write
/// the live loop runs. Without it a Run that finishes through a per-round entry
/// point — the batch response path a provider retry falls back to, or the initial
/// request path — completes with its checklist rows still `pending` for ever, and
/// a failing deliverable never arms its repair round.
enum CompletionDelivery {
    /// No checklist, or every promised artifact passed: the Run may complete. The
    /// mark is stamped on the completing decision when verdicts were staged.
    Complete { mark: Option<String> },
    /// A promised artifact failed: this round arms the bounded repair round
    /// instead of finishing, exactly as the live loop does.
    Repair {
        mark: String,
        prompt: String,
        input: fox_engine_protocol::KernelInitialModelInput,
    },
}

impl CompletionDelivery {
    /// The mark this completion's decision must carry, when verdicts were staged.
    fn mark(&self) -> Option<&str> {
        match self {
            CompletionDelivery::Complete { mark } => mark.as_deref(),
            CompletionDelivery::Repair { mark, .. } => Some(mark.as_str()),
        }
    }
}

/// The flat wait a retry must never beat for the classes that are **not** a
/// provably transient connection/server loss, and the last rung of the ladder.
/// One definition, so the policy cannot drift between the scheduler and the
/// tests that guard the retry lane.
pub(crate) const MODEL_RETRY_INTERVAL_MS: i64 = 10_000;

/// ---------------------------------------------------------------------------
/// Classified model-retry policy
/// ---------------------------------------------------------------------------
///
/// A settled model failure is evidence, never a retry permission. The Host
/// classifies it and decides whether re-issuing the **identical** request can
/// plausibly help; the Kernel owns the durable accounting, the Run-scoped
/// budget and the terminal write (`RunController::schedule_model_retry_admitted`).
///
/// DELAY / COUNT TABLE (one unrecovered model request)
///
/// | settled evidence                                        | class                     | retried | wait                                    |
/// |---------------------------------------------------------|---------------------------|---------|-----------------------------------------|
/// | provider 500/502/503/504/529, no server wait             | transient_server_error    | yes     | ladder 1s,2s,4s,8s,10s ±10% jitter      |
/// | model_transport_failure with no diagnostic (the Host itself observed the loss: reaped worker / expired model window) | transient_connection_failure | yes | ladder 1s,2s,4s,8s,10s ±10% jitter |
/// | model_transport_failure `stream_interrupted`             | stream_interrupted        | yes     | ladder                                  |
/// | model_transport_failure `reason=unknown` (auth / permission / invalid-argument / thrown connection error — not distinguishable on today's wire) | unknown_evidence | yes | flat 10s, never dense |
/// | incomplete_response `stream_interrupted`                 | stream_interrupted        | yes     | ladder (first rung 1s)                  |
/// | incomplete_response `empty_final_answer`, stop != length  | empty_answer              | yes     | ladder (first rung 1s)                  |
/// | any retryable category with Retry-After / Retry-After-Ms | server_directed_wait      | yes     | max(Retry-After, ladder rung); jitter only upward |
/// | provider 429, no server wait (rate/quota)                | rate_limited              | yes     | flat 10s (never dense)                  |
/// | model_timeout (first-response / idle / whole round)      | model_timeout             | yes     | flat 10s (never a 1 s retry)            |
/// | incomplete_response `missing_final_marker`               | missing_final_marker      | yes     | flat 10s; the resend carries the Host continuation prompt |
/// | incomplete_response `incomplete_tool_proposal`           | incomplete_tool_proposal  | yes     | flat 10s                                |
/// | incomplete_response reason absent/unrecognised           | unknown_evidence          | yes     | flat 10s; recorded as `unknown`, never guessed |
/// | provider status outside the protocol's transient set      | provider_error_not_retryable | **no** | terminal now                           |
/// | unrecognised category                                    | unknown_category          | **no**  | terminal now                            |
/// | Run-scoped cumulative budget already spent               | retry_budget_spent        | **no**  | terminal now                            |
/// | next wait does not fit the whole-Run elapsed bound        | run_budget_exhausted      | **no**  | terminal now                            |
///
/// LENGTH TRUNCATION (`stopReason=length`, so no acceptable body and no
/// acceptable tool proposal exist) is its own family, decided before the ladder:
/// it is never re-sent on the network rungs. Instead the Run gets **one**
/// controlled recovery, and that recovery may only be dispatched when it really
/// changes the request — a request with no effective change is refused.
///
/// The lever is decided from the adapter's **own** request-boundary record (the
/// `config.sent` layer of this Run's durable `usage.request`), never from a local
/// level enumeration, never from a configuration difference and never from the
/// `resolved` layer:
///
/// | settled evidence                                        | class                        | next                                    |
/// |---------------------------------------------------------|------------------------------|-----------------------------------------|
/// | `length` + no body counters (`textChars=0`, no tool call) | `length_truncated_no_body`   | one controlled recovery (30s, not a ladder rung) |
/// | `length` + public text produced                          | `length_truncated_partial_body` | one controlled recovery              |
/// | `length` + a tool proposal                               | `length_truncated_tool_proposal` | one controlled recovery              |
/// | `length` with no body counters observed                   | `length_truncated_unknown_body` | one controlled recovery              |
/// | recovery already spent by this Run                        | `length_recovery_spent`      | terminal, Host explanation from durable facts |
/// | unified retry budget already spent                        | `length_recovery_unavailable_budget` | terminal, Host explanation        |
/// | no proven effective lever (parameter not serialized **and** instruction undeliverable) | `length_recovery_no_effective_lever` | terminal, Host explanation |
///
/// Three exits, all recorded with the evidence behind them:
///  1. the wire record shows the adapter serializes `reasoning_effort` and the
///     lowered level changes that value → the parameter alone is the lever;
///  2. the adapter is not shown to serialize it, but the instruction can be
///     delivered → **instruction-only** recovery: no local parameter is applied at
///     all, and the record says so (`effective:false`, `lever:"instruction_only"`,
///     `recoveryParameters:{}`);
///  3. neither → the recovery is **refused**: no request is dispatched and the Run
///     ends through the existing honest terminal.
///
/// COUNTING RULE: one unrecovered model request may spend at most
/// `min(5, max(provider_max, turn_max))` automatic retries **in total**, counted
/// across every category, retry class and effect key
/// (`RetryState::model_attempts`). A change of error category or of effect key
/// never resets it. Only a model response that was actually accepted resets it.
/// This is deliberately not "5 retries per round × 5 rounds".
///
/// TOTAL-DURATION RULE: the retry ladder is bounded by the Run-scoped budget
/// above and, independently, by the whole-Run elapsed budget: a retry whose wait
/// cannot fit inside the remaining Run budget is refused (and an effect-key
/// change cannot bypass that, because the bound is measured from the Run's own
/// accumulated running time, not from the effect key).
pub(crate) const MODEL_RETRY_LADDER_MS: [i64; 5] = [1_000, 2_000, 4_000, 8_000, 10_000];

/// ±10% jitter, expressed in per-mille units.
pub(crate) const MODEL_RETRY_JITTER_PERMILLE: i64 = 100;

/// Hard bound on automatic model retries for one unrecovered model request.
pub(crate) const MODEL_RETRY_MAX_ATTEMPTS: u32 = kernel::MODEL_RETRY_MAX_ATTEMPTS;

/// A length-truncated round gets exactly one controlled recovery per Run, and
/// that recovery spends the unified retry budget as well.
pub(crate) const MODEL_LENGTH_RECOVERY_MAX_ATTEMPTS: u32 = kernel::MODEL_LENGTH_RECOVERY_MAX_ATTEMPTS;

/// Wait before the one controlled length recovery.
///
/// Deliberately **not** a rung of the network ladder (1/2/4/8/10 s) and not the
/// flat completion interval: the request itself changes, so this is a one-off
/// scheduling pause for the provider to settle a fresh request shape — and it is
/// still measured against the whole-Run elapsed bound before it is admitted.
pub(crate) const MODEL_LENGTH_RECOVERY_INTERVAL_MS: i64 = 30_000;

/// Bounds on what the recovery may add to the model input and record durably.
pub(crate) const LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES: usize = 3_072;
pub(crate) const LENGTH_RECOVERY_MAX_RECORD_BYTES: usize = 4_096;
/// How many outstanding checklist items the instruction may name.
pub(crate) const LENGTH_RECOVERY_MAX_NAMED_ITEMS: usize = 5;
/// How long one named item may be after sanitization.
pub(crate) const LENGTH_RECOVERY_MAX_ITEM_CHARS: usize = 80;

/// The reasoning levels the worker's own profile resolution accepts, lowest
/// first. Lowering means moving *down* this list; nothing above the original is
/// ever requested.
pub(crate) const MODEL_THINKING_LEVELS: [&str; 6] =
    ["off", "low", "medium", "high", "xhigh", "max"];

/// The levels a recovery may actually request.
///
/// `off` is deliberately excluded: the worker's own request builder maps the
/// resolved level onto the provider's `reasoning_effort` for the generic
/// OpenAI-compatible transport, whose portable values start at `low`. Asking for
/// `off` there would send an effort value the provider need not accept, so the
/// recovery stops at `low` and reports the instruction-only fallback instead of
/// inventing a parameter.
pub(crate) const MODEL_PORTABLE_THINKING_LEVELS: [&str; 5] =
    ["low", "medium", "high", "xhigh", "max"];

/// The exact Host sentence for a length-truncated Run whose answer never
/// completed. Never claims completion, and never pretends to be a model reply.
pub(crate) const LENGTH_TRUNCATED_TERMINAL: &str =
    "模型响应达到长度上限。已保留现有文件，最终答复未完成。";

/// How a classified failure may be re-issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryWait {
    /// Classified ladder rung, scaled by ±10% jitter.
    Ladder,
    /// Never earlier than the server's own wait; jitter is applied upward only.
    ServerDirected,
    /// A resend is allowed, but only at the conservative flat interval.
    Flat,
    /// The one controlled length-truncation recovery: the request itself changes
    /// (lowered reasoning level and/or a recovery instruction derived from durable
    /// delivery facts), so it is not a network-backoff rung and happens once.
    ControlledRecovery,
    /// Refused: an identical resend cannot cure this cause.
    Refused,
}

/// One Host classification of a settled model failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelRetryClass {
    /// Bounded token recorded on the durable scheduling event and the retry state.
    pub(crate) token: &'static str,
    pub(crate) wait: RetryWait,
    /// User-facing explanation used when `wait == Refused`.
    pub(crate) refusal: &'static str,
}

const fn retry_class(token: &'static str, wait: RetryWait) -> ModelRetryClass {
    ModelRetryClass { token, wait, refusal: "" }
}

const fn refused_class(token: &'static str, refusal: &'static str) -> ModelRetryClass {
    ModelRetryClass { token, wait: RetryWait::Refused, refusal }
}

/// The one controlled length recovery, and the fact that its instruction is only
/// a **soft** constraint: it can ask the model to stop planning and answer now,
/// but nothing in the prompt can guarantee that a model with a large reasoning
/// budget stops thinking. The hard lever is a lowered reasoning level *that the
/// adapter is proven to serialize* (see [`plan_length_recovery_lever`]); when no
/// such parameter exists the instruction alone is the lever, and when that cannot
/// be delivered either the recovery is refused and the Run ends truthfully.
pub(crate) const RETRY_REFUSAL_PROVIDER_STATUS: &str =
    "模型服务以不可重试的方式拒绝了请求，自动重试已停止。已完成的工具结果已保留，请检查配置或凭据后继续。";
pub(crate) const RETRY_REFUSAL_UNKNOWN_CATEGORY: &str =
    "模型失败类别无法识别，为避免盲目重发，自动重试已停止。已完成的工具结果已保留，请稍后继续。";
pub(crate) const RETRY_REFUSAL_BUDGET_SPENT: &str =
    "自动重试次数已用完（同一次未恢复的模型请求最多 5 次，跨失败类别累计）。已完成的工具结果已保留；请发送“继续完成剩余工作”。";
pub(crate) const RETRY_REFUSAL_RUN_BUDGET: &str =
    "剩余运行时长不足以容纳下一次自动重试，自动重试已停止。已完成的工具结果已保留，请稍后继续。";

pub(crate) const RETRY_CLASS_TRANSIENT_SERVER: ModelRetryClass =
    retry_class("transient_server_error", RetryWait::Ladder);
pub(crate) const RETRY_CLASS_TRANSIENT_CONNECTION: ModelRetryClass =
    retry_class("transient_connection_failure", RetryWait::Ladder);
pub(crate) const RETRY_CLASS_STREAM_INTERRUPTED: ModelRetryClass =
    retry_class("stream_interrupted", RetryWait::Ladder);
pub(crate) const RETRY_CLASS_EMPTY_ANSWER: ModelRetryClass =
    retry_class("empty_answer", RetryWait::Ladder);
pub(crate) const RETRY_CLASS_SERVER_DIRECTED: ModelRetryClass =
    retry_class("server_directed_wait", RetryWait::ServerDirected);
pub(crate) const RETRY_CLASS_RATE_LIMITED: ModelRetryClass =
    retry_class("rate_limited", RetryWait::Flat);
pub(crate) const RETRY_CLASS_MODEL_TIMEOUT: ModelRetryClass =
    retry_class("model_timeout", RetryWait::Flat);
pub(crate) const RETRY_CLASS_MISSING_MARKER: ModelRetryClass =
    retry_class("missing_final_marker", RetryWait::Flat);
pub(crate) const RETRY_CLASS_INCOMPLETE_TOOL_PROPOSAL: ModelRetryClass =
    retry_class("incomplete_tool_proposal", RetryWait::Flat);
pub(crate) const RETRY_CLASS_UNKNOWN_EVIDENCE: ModelRetryClass =
    retry_class("unknown_evidence", RetryWait::Flat);
pub(crate) const RETRY_CLASS_PROVIDER_NOT_RETRYABLE: ModelRetryClass =
    refused_class("provider_error_not_retryable", RETRY_REFUSAL_PROVIDER_STATUS);
pub(crate) const RETRY_CLASS_UNKNOWN_CATEGORY: ModelRetryClass =
    refused_class("unknown_category", RETRY_REFUSAL_UNKNOWN_CATEGORY);
pub(crate) const RETRY_CLASS_BUDGET_SPENT: ModelRetryClass =
    refused_class("retry_budget_spent", RETRY_REFUSAL_BUDGET_SPENT);
pub(crate) const RETRY_CLASS_RUN_BUDGET: ModelRetryClass =
    refused_class("run_budget_exhausted", RETRY_REFUSAL_RUN_BUDGET);

/// Length truncation is its **own** class family: the provider's output limit was
/// reached, so `empty_final_answer` / `missing_final_marker` are only symptoms
/// and the class token must name the length cause. The three shapes the worker
/// can observe stay distinguishable, and an unobserved body shape is reported as
/// unknown rather than guessed.
pub(crate) const RETRY_CLASS_LENGTH_NO_BODY: ModelRetryClass =
    retry_class("length_truncated_no_body", RetryWait::ControlledRecovery);
pub(crate) const RETRY_CLASS_LENGTH_PARTIAL_BODY: ModelRetryClass =
    retry_class("length_truncated_partial_body", RetryWait::ControlledRecovery);
pub(crate) const RETRY_CLASS_LENGTH_TOOL_PROPOSAL: ModelRetryClass =
    retry_class("length_truncated_tool_proposal", RetryWait::ControlledRecovery);
pub(crate) const RETRY_CLASS_LENGTH_UNKNOWN_BODY: ModelRetryClass =
    retry_class("length_truncated_unknown_body", RetryWait::ControlledRecovery);
/// The one recovery was already spent by this Run (or its budget is gone).
pub(crate) const RETRY_CLASS_LENGTH_RECOVERY_SPENT: ModelRetryClass =
    refused_class("length_recovery_spent", LENGTH_TRUNCATED_TERMINAL);
pub(crate) const RETRY_CLASS_LENGTH_RECOVERY_BUDGET: ModelRetryClass =
    refused_class("length_recovery_unavailable_budget", LENGTH_TRUNCATED_TERMINAL);
/// No proven effective lever: the adapter is not shown to serialize the lowered
/// parameter and this Run cannot deliver the instruction either, so the one
/// recovery would be a request with no real change. It is refused instead.
pub(crate) const RETRY_CLASS_LENGTH_RECOVERY_NO_LEVER: ModelRetryClass =
    refused_class("length_recovery_no_effective_lever", LENGTH_TRUNCATED_TERMINAL);

/// Whether this classification is the length-truncation family.
pub(crate) fn is_length_truncation(class: ModelRetryClass) -> bool {
    class.wait == RetryWait::ControlledRecovery
}

/// The three body shapes a length-truncated round can show, plus the honest
/// "only the stop reason was observed" case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LengthTruncationShape {
    /// Thinking (or nothing) was produced, but no acceptable public body.
    NoBody,
    /// Public text was produced and cut off mid-answer.
    PartialBody,
    /// A tool proposal was cut off, so no acceptable proposal exists.
    ToolProposal,
    /// The round was rejected for length, but the worker observed no body counters.
    UnknownBody,
}

impl LengthTruncationShape {
    pub(crate) fn class(self) -> ModelRetryClass {
        match self {
            LengthTruncationShape::NoBody => RETRY_CLASS_LENGTH_NO_BODY,
            LengthTruncationShape::PartialBody => RETRY_CLASS_LENGTH_PARTIAL_BODY,
            LengthTruncationShape::ToolProposal => RETRY_CLASS_LENGTH_TOOL_PROPOSAL,
            LengthTruncationShape::UnknownBody => RETRY_CLASS_LENGTH_UNKNOWN_BODY,
        }
    }

    pub(crate) fn token(self) -> &'static str {
        self.class().token
    }
}

/// The consequence of one classification, before the Kernel's durable guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelRetryPlan {
    pub(crate) class: &'static str,
    /// How this wait was chosen; `ControlledRecovery` is the only one that
    /// changes the request instead of just re-sending it.
    pub(crate) wait: RetryWait,
    /// `None` refuses every automatic resend for this settled failure.
    pub(crate) delay_ms: Option<i64>,
    pub(crate) refusal: &'static str,
}

impl ModelRetryPlan {
    pub(crate) fn retry(class: ModelRetryClass, delay_ms: i64) -> Self {
        Self {
            class: class.token,
            wait: class.wait,
            delay_ms: Some(delay_ms.max(1)),
            refusal: "",
        }
    }

    pub(crate) fn refused(class: ModelRetryClass) -> Self {
        Self { class: class.token, wait: RetryWait::Refused, delay_ms: None, refusal: class.refusal }
    }

    /// Whether a plan needs a Host-owned length-recovery record attached.
    pub(crate) fn needs_recovery_record(&self) -> bool {
        self.wait == RetryWait::ControlledRecovery && self.delay_ms.is_some()
    }
}

/// Whether the settled failure is reliable evidence of a **length-truncated**
/// round: the provider hit its output limit, so no resend of the identical
/// request can produce more room for an answer.
///
/// Two observed forms are admitted, and the durable record says which one was
/// seen:
///  * the provider's own stop reason is `length`;
///  * the worker's sub-category is `output_length_limit`, which its own branch
///    only emits when the round's message carried stop reason `length`.
///
/// An absent observation is never guessed: a failure carrying neither form is not
/// a length truncation, whatever else it looks like.
pub(crate) fn length_truncation_evidence(
    failure: &fox_engine_protocol::KernelModelFailure,
) -> Option<&'static str> {
    if failure.category != "incomplete_response" {
        return None;
    }
    let diagnostic = failure.diagnostic.as_ref();
    let reason = diagnostic.and_then(|item| item.reason.as_deref());
    let stop_reason = diagnostic.and_then(|item| item.stop_reason.as_deref());
    match (stop_reason, reason) {
        (Some("length"), _) => Some("stop_reason_length"),
        (_, Some("output_length_limit")) => Some("worker_output_length_limit"),
        _ => None,
    }
}

/// Which shape a length-truncated round showed, from the counters the worker
/// actually observed. Absent counters produce `UnknownBody`, never a fabricated 0.
pub(crate) fn length_truncation_shape(
    failure: &fox_engine_protocol::KernelModelFailure,
) -> LengthTruncationShape {
    let diagnostic = failure.diagnostic.as_ref();
    let text = diagnostic.and_then(|item| item.text_chars);
    let tools = diagnostic.and_then(|item| item.tool_call_count);
    let reason = diagnostic.and_then(|item| item.reason.as_deref());
    if tools.is_some_and(|count| count > 0) || reason == Some("incomplete_tool_proposal") {
        return LengthTruncationShape::ToolProposal;
    }
    match text {
        Some(0) => LengthTruncationShape::NoBody,
        Some(_) => LengthTruncationShape::PartialBody,
        // The worker's own branch only names `output_length_limit` when the round
        // did produce public text, so that naming is itself evidence of a partial
        // body — the counter stays absent rather than being invented.
        None if reason == Some("output_length_limit") => LengthTruncationShape::PartialBody,
        None if reason == Some("empty_final_answer") => LengthTruncationShape::NoBody,
        None => LengthTruncationShape::UnknownBody,
    }
}

/// Classify one settled failure from the evidence it actually carries.
///
/// Only enumerated, Host-sanitized facts are read: the category, the admitted
/// HTTP status, a server-directed wait, and the diagnostic sub-category plus the
/// provider stop reason. An absent or unrecognised reason is classified as
/// `unknown_evidence`; it is never guessed into a dense ladder.
pub(crate) fn classify_model_failure(
    failure: &fox_engine_protocol::KernelModelFailure,
) -> ModelRetryClass {
    let reason = failure
        .diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.reason.as_deref())
        .unwrap_or("unknown");
    match failure.category.as_str() {
        "provider_unavailable" => {
            if failure.retry_after_ms.is_some_and(|delay| delay > 0) {
                RETRY_CLASS_SERVER_DIRECTED
            } else if failure.http_status == Some(429) {
                // A rate/quota limit with no server-directed wait. A dense ladder
                // would hammer a budget that is already spent.
                RETRY_CLASS_RATE_LIMITED
            } else if matches!(failure.http_status, Some(500 | 502 | 503 | 504 | 529)) {
                RETRY_CLASS_TRANSIENT_SERVER
            } else {
                // Not a status the frozen protocol admits as retryable: an auth,
                // permission or invalid-argument rejection is never dense-retried.
                RETRY_CLASS_PROVIDER_NOT_RETRYABLE
            }
        }
        "model_timeout" => RETRY_CLASS_MODEL_TIMEOUT,
        // The transport category covers two very different evidence states:
        //
        //  * the Host itself observed the loss (its own model window expired, or
        //    the worker/pipe died and was reaped) — the request never completed
        //    and no provider verdict ever arrived. That is a transient connection
        //    failure, and it is the only transport shape admitted to the ladder.
        //  * the worker reported a round that "ended without a successful stop"
        //    and named no cause (`reason=unknown`). That is the shape an auth,
        //    permission or invalid-argument rejection takes on today's wire, where
        //    the provider's non-retryable status is deliberately never carried.
        //    The Host cannot tell it from a thrown connection error, so it must
        //    not dense-retry it: it gets the conservative flat interval instead.
        "model_transport_failure" => {
            match failure.diagnostic.as_ref().and_then(|diagnostic| {
                diagnostic.reason.as_deref()
            }) {
                // A named transport branch is evidence of its own.
                Some("stream_interrupted") => RETRY_CLASS_STREAM_INTERRUPTED,
                None => RETRY_CLASS_TRANSIENT_CONNECTION,
                Some(_) => RETRY_CLASS_UNKNOWN_EVIDENCE,
            }
        }
        // Length truncation outranks the symptom: a round the provider cut off at
        // its output limit is `length_truncated_*`, never a bare
        // `empty_final_answer` or `missing_final_marker`. Everything else keeps
        // its own sub-category.
        "incomplete_response" => {
            if length_truncation_evidence(failure).is_some() {
                return length_truncation_shape(failure).class();
            }
            match reason {
                "empty_final_answer" => RETRY_CLASS_EMPTY_ANSWER,
                "stream_interrupted" => RETRY_CLASS_STREAM_INTERRUPTED,
                "incomplete_tool_proposal" => RETRY_CLASS_INCOMPLETE_TOOL_PROPOSAL,
                "missing_final_marker" => RETRY_CLASS_MISSING_MARKER,
                _ => RETRY_CLASS_UNKNOWN_EVIDENCE,
            }
        }
        _ => RETRY_CLASS_UNKNOWN_CATEGORY,
    }
}

/// Deterministic jitter in `[-100, +100]` per-mille.
///
/// Derived from durable identity instead of a random source, so a scheduled
/// retry's due time is reproducible and a restart never re-rolls it (the due
/// time is persisted anyway), and so a test can assert an exact value.
pub(crate) fn model_retry_jitter_permille(run_id: &str, effect_key: &str, attempt: u32) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(effect_key.as_bytes());
    hasher.update([0x1f]);
    hasher.update(attempt.to_le_bytes());
    let digest = hasher.finalize();
    let raw = u64::from_le_bytes(digest[..8].try_into().expect("sha256 prefix"));
    (raw % (2 * MODEL_RETRY_JITTER_PERMILLE as u64 + 1)) as i64 - MODEL_RETRY_JITTER_PERMILLE
}

/// Ladder rung for the `attempt`-th retry of this Run (0-based).
pub(crate) fn model_retry_ladder_ms(attempt: u32) -> i64 {
    MODEL_RETRY_LADDER_MS[(attempt as usize).min(MODEL_RETRY_LADDER_MS.len() - 1)]
}

/// Plan the next automatic retry for one settled failure.
///
/// `retry` is the durable Run-scoped state: `model_attempts` is the cumulative
/// count that no category change and no new effect key may reset, and
/// `model_attempts`/`provider_max`/`turn_max` together define the budget.
pub(crate) fn plan_model_retry(
    failure: &fox_engine_protocol::KernelModelFailure,
    retry: &kernel::RetryState,
    run_id: &str,
    effect_key: &str,
) -> ModelRetryPlan {
    let class = classify_model_failure(failure);
    if class.wait == RetryWait::Refused {
        return ModelRetryPlan::refused(class);
    }
    let budget = retry
        .provider_max
        .max(retry.turn_max)
        .min(MODEL_RETRY_MAX_ATTEMPTS);
    // A length truncation is decided before the ladder: the request itself has to
    // change, and the Run's own evidence rules govern it.
    if class.wait == RetryWait::ControlledRecovery {
        if retry.length_recovery_attempts >= MODEL_LENGTH_RECOVERY_MAX_ATTEMPTS {
            // One recovery per Run. A restart, a category change, a new effect key
            // and a later accepted response all keep this counter, so the Run ends
            // honestly instead of looping the same truncation.
            return ModelRetryPlan::refused(RETRY_CLASS_LENGTH_RECOVERY_SPENT);
        }
        if retry.model_attempts >= budget {
            // The recovery spends the unified budget; it may not bypass it.
            return ModelRetryPlan::refused(RETRY_CLASS_LENGTH_RECOVERY_BUDGET);
        }
        return ModelRetryPlan::retry(class, MODEL_LENGTH_RECOVERY_INTERVAL_MS);
    }
    if retry.model_attempts >= budget {
        return ModelRetryPlan::refused(RETRY_CLASS_BUDGET_SPENT);
    }
    let attempt = retry.model_attempts;
    let jitter = model_retry_jitter_permille(run_id, effect_key, attempt);
    let ladder = model_retry_ladder_ms(attempt);
    let delay = match class.wait {
        RetryWait::Ladder => {
            // ±10% around the rung; never negative and never zero-length.
            (ladder * (1_000 + jitter) / 1_000).max(1)
        }
        RetryWait::ServerDirected => {
            // The server's own wait always wins and is never shortened: the
            // jitter is added upward only.
            let server_wait = failure.retry_after_ms.unwrap_or(0) as i64;
            let base = server_wait.max(ladder);
            base + (base * jitter.max(0) / 1_000)
        }
        RetryWait::Flat => MODEL_RETRY_INTERVAL_MS,
        RetryWait::ControlledRecovery | RetryWait::Refused => {
            unreachable!("length recovery and refusals return above")
        }
    };
    ModelRetryPlan::retry(class, delay)
}

// ---------------------------------------------------------------------------
// The one controlled length-truncation recovery
// ---------------------------------------------------------------------------

/// What the durable delivery facts say about the Run's promised artifacts, from
/// which the recovery instruction is generated. Never a hard-coded task, file or
/// answer: only the Host's own checklist statuses and bounded display names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LengthRecoveryDeliveryKind {
    /// Every promised artifact passed deterministic verification.
    VerifiedDeliverables,
    /// Artifacts exist but their content is unverified / still pending checks.
    UnverifiedArtifacts,
    /// A promised artifact is missing or failed its checks.
    IncompleteDeliverables,
    /// No file checklist at all: the ledger cannot decide, so completion is
    /// unknown and must not be claimed.
    NoChecklist,
}

impl LengthRecoveryDeliveryKind {
    pub(crate) fn token(&self) -> &'static str {
        match self {
            LengthRecoveryDeliveryKind::VerifiedDeliverables => "verified_deliverables",
            LengthRecoveryDeliveryKind::UnverifiedArtifacts => "unverified_artifacts",
            LengthRecoveryDeliveryKind::IncompleteDeliverables => "incomplete_deliverables",
            LengthRecoveryDeliveryKind::NoChecklist => "no_checklist",
        }
    }
}

/// Bounded, state-derived facts for one recovery instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LengthRecoveryDelivery {
    pub(crate) kind: LengthRecoveryDeliveryKind,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) pending: usize,
    /// Bounded, sanitized display names of items that are not verified passed.
    pub(crate) outstanding: Vec<String>,
}

impl LengthRecoveryDelivery {
    /// The instruction the model actually receives. It is a **soft** constraint:
    /// a prompt can ask the model to stop planning, but it cannot guarantee that
    /// a model with a large reasoning budget stops thinking.
    pub(crate) fn instruction(&self) -> String {
        let items = if self.outstanding.is_empty() {
            String::new()
        } else {
            format!("未完成的核验项：{}。", self.outstanding.join("、"))
        };
        let body = match self.kind {
            LengthRecoveryDeliveryKind::VerifiedDeliverables => format!(
                "Fox 长度恢复提示：上一条回复被模型输出长度上限截断，没有得到最终答复。Host 已核验本次任务的交付物全部通过（{} 项）。请只给出一段简短的完成说明：不要重复读取、计算或写入任何文件，不要展开长篇分析或规划，直接遵守系统提示中的最终答复格式结束本轮。",
                self.passed
            ),
            LengthRecoveryDeliveryKind::UnverifiedArtifacts => format!(
                "Fox 长度恢复提示：上一条回复被模型输出长度上限截断。Host 已生成 {} 项交付物，但其中 {} 项内容尚未通过核验，因此**任务还未完成**。请立即用简短答复说明：已经确认的事实、仍然需要核验或补做的工作{items}。不要重复已经成功的写入，不要再展开长篇分析，直接遵守系统提示中的最终答复格式结束本轮。",
                self.passed + self.pending + self.failed,
                self.pending + self.failed,
                items = items
            ),
            LengthRecoveryDeliveryKind::IncompleteDeliverables => format!(
                "Fox 长度恢复提示：上一条回复被模型输出长度上限截断。Host 的核验结果显示交付物缺失或不合格（已通过 {} 项，未通过 {} 项），因此**任务尚未完成**。请立即用简短答复说明：已完成的部分、缺失或不合格的具体交付物、以及当前的具体阻塞{items}。不要只给出成功结论，不要重复已经成功的写入，也不要展开长篇分析，直接遵守系统提示中的最终答复格式结束本轮。",
                self.passed, self.failed,
                items = items
            ),
            LengthRecoveryDeliveryKind::NoChecklist => format!(
                "Fox 长度恢复提示：上一条回复被模型输出长度上限截断。Host 没有可用于内容核验的交付物清单，因此**无法确认任务是否完成**。请立即用简短答复说明：已经确认的事实、尚未核验的部分{items}。不要声称任务已完成，不要重复已经成功的写入，也不要展开长篇分析，直接遵守系统提示中的最终答复格式结束本轮。",
                items = items
            ),
        };
        bound_instruction(body)
    }
}

/// Keep the instruction inside the dispatch's reserved bytes, and never cut a
/// multi-byte character in half.
fn bound_instruction(text: String) -> String {
    if text.len() <= LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES {
        return text;
    }
    let mut end = LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Sanitize one checklist display name into a bounded, single-line fact.
fn bound_item_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut bounded: String = trimmed.chars().take(LENGTH_RECOVERY_MAX_ITEM_CHARS).collect();
    if trimmed.chars().count() > LENGTH_RECOVERY_MAX_ITEM_CHARS {
        bounded.push('…');
    }
    Some(bounded)
}

/// The Host-side original parameters of the request that was truncated, as the
/// frozen configuration actually requested them. Only fields the Host set are
/// present — an absent value stays absent, it is not defaulted here.
pub(crate) fn original_model_parameters(service: &Value) -> Value {
    let mut original = serde_json::Map::new();
    let profile = service.get("modelProfile");
    for key in ["reasoning", "thinkingLevel", "maxOutputTokens"] {
        let value = service
            .get(key)
            .or_else(|| profile.and_then(|profile| profile.get(key)));
        if let Some(value) = value {
            original.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(original)
}

/// The reasoning level the worker's resolution actually applied for this Run:
/// an explicit request when present, else the family default. `None` means the
/// Host has no evidence of a reasoning parameter at all.
fn requested_thinking_level(service: &Value) -> Option<&str> {
    let profile = service.get("modelProfile");
    profile
        .and_then(|profile| profile.get("thinkingLevel"))
        .or_else(|| service.get("thinkingLevel"))
        .and_then(Value::as_str)
}

/// Whether the frozen configuration explicitly disabled reasoning.
fn reasoning_disabled(service: &Value) -> bool {
    service
        .get("reasoning")
        .or_else(|| service.get("modelProfile").and_then(|profile| profile.get("reasoning")))
        == Some(&Value::Bool(false))
}

/// Lower one thinking level by moving **down** the portable level list. Returns
/// `None` when there is no lower level the recovery may safely request: the level
/// is already `low` (the floor the generic transport supports), or the frozen
/// configuration explicitly turns reasoning off.
pub(crate) fn lower_thinking_level(level: Option<&str>) -> Option<&'static str> {
    let Some(level) = level else {
        // No explicit level: the worker's own family heuristics default a
        // reasoning-capable model to "medium", so "low" is a strict lowering and
        // is ignored outright when the model turns out not to reason at all.
        return Some("low");
    };
    let index = MODEL_PORTABLE_THINKING_LEVELS
        .iter()
        .position(|item| *item == level)?;
    MODEL_PORTABLE_THINKING_LEVELS.get(index.checked_sub(1)?).copied()
}

/// The adapter's *observed* ability to serialize a reasoning parameter for this
/// Run's model and transport.
///
/// The signal is deliberately a **wire fact**, not a local intention: it is the
/// `config.sent` layer of this Run's own durable `usage.request` record — the
/// layer the runtime writes from the request body it actually serialized
/// (`services/agent-runtime/src/model-usage-runtime.mjs::safeSentConfig`). The
/// `resolved` layer is explicitly **not** accepted as evidence: it describes what
/// capability resolution produced, not what was sent.
///
/// Only the field whose code path has been read and verified is accepted:
/// `reasoning_effort`, which the OpenAI-completions adapter writes from the
/// resolved thinking level solely when that transport's compat declares
/// `supportsReasoningEffort`. Other shapes (`reasoning.effort`, `thinking.*`) are
/// not tied to the level by any path this recovery can verify, so they are
/// reported as not effective rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdapterReasoningEvidence {
    /// The whole observed `sent` layer of the previous model attempt, when the
    /// runtime recorded one. Absent means "no wire record", never "no field".
    pub(crate) observed_sent: Option<Value>,
    /// The reasoning-effort value the adapter really serialized, when it did.
    pub(crate) serialized_effort: Option<String>,
}

impl AdapterReasoningEvidence {
    /// How this evidence was obtained, recorded durably with the recovery.
    pub(crate) const SOURCE: &'static str =
        "durable usage.request config.sent layer of this Run's previous model attempt";

    pub(crate) fn from_sent(sent: Option<Value>) -> Self {
        let serialized_effort = sent
            .as_ref()
            .and_then(|sent| sent.get("reasoning_effort"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Self { observed_sent: sent, serialized_effort }
    }
}

/// The lever the one controlled recovery will actually pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LengthRecoveryLever {
    /// What the Host *intended* to change locally (may be empty).
    pub(crate) intended: Value,
    /// What is really applied to the derived request configuration. Empty when the
    /// parameter cannot be shown to change the request.
    pub(crate) applied: Value,
    /// Whether the lowered parameter is proven to enter the real request.
    pub(crate) effective: bool,
    /// The wire evidence behind `effective`.
    pub(crate) evidence: AdapterReasoningEvidence,
    /// What the adapter is expected to serialize after the change (from the same
    /// verified field), for the record — never presented as an observation.
    pub(crate) expected_sent: Value,
    /// Why the parameter is or is not effective.
    pub(crate) reason: &'static str,
}

impl LengthRecoveryLever {
    pub(crate) fn lever(&self) -> &'static str {
        if self.effective {
            "parameters"
        } else {
            "instruction_only"
        }
    }
}

/// Decide whether lowering the reasoning level can be **shown** to change the real
/// request, from the adapter's own serialization record.
///
/// This replaces the earlier (and wrong) rule that treated a legal, lowerable
/// local `thinkingLevel` as proof of effectiveness: a local configuration change
/// that the adapter never serializes would otherwise buy a request with no
/// effective change at all. The three honest outcomes are:
///  * the wire record shows the reasoning field and the lowered value differs →
///    the parameter really changes the request;
///  * the wire record shows no such field (or no wire record exists) → the
///    parameter cannot be shown to change anything, so it is **not** applied and
///    the recovery falls back to the instruction alone;
///  * neither a proven parameter nor a deliverable instruction → the caller
///    refuses the recovery instead of dispatching a request with no real change.
pub(crate) fn plan_length_recovery_lever(
    service: &Value,
    evidence: AdapterReasoningEvidence,
) -> LengthRecoveryLever {
    let intended = if reasoning_disabled(service) {
        json!({})
    } else {
        match lower_thinking_level(requested_thinking_level(service)) {
            Some(level) => json!({"thinkingLevel": level}),
            None => json!({}),
        }
    };
    let intended_level = intended.get("thinkingLevel").and_then(Value::as_str);
    let (effective, reason, expected_sent) = match (intended_level, evidence.serialized_effort.as_deref()) {
        (None, _) => (
            false,
            "no supported reasoning parameter to lower for this frozen configuration",
            json!({}),
        ),
        (Some(_), None) if evidence.observed_sent.is_none() => (
            false,
            "no durable wire record for this Run's previous attempt: effectiveness is unproven",
            json!({}),
        ),
        (Some(_), None) => (
            false,
            "the adapter's own sent layer carried no reasoning_effort: the level is not serialized by this transport",
            json!({}),
        ),
        (Some(level), Some(serialized)) if serialized == level => (
            false,
            "the lowered level equals the value already serialized: no observable change",
            json!({}),
        ),
        (Some(level), Some(_)) => (
            true,
            "the adapter serialized reasoning_effort before, and the lowered level changes that value",
            json!({"reasoning_effort": level}),
        ),
    };
    LengthRecoveryLever {
        applied: if effective { intended.clone() } else { json!({}) },
        intended,
        effective,
        evidence,
        expected_sent,
        reason,
    }
}

/// Apply the recovery parameters to a frozen model service. Returns the derived
/// service and the keys it actually changed.
pub(crate) fn apply_recovery_parameters(
    service: &Value,
    parameters: &Value,
) -> Result<(Value, Vec<String>), String> {
    let mut derived = service
        .clone()
        .as_object()
        .cloned()
        .ok_or("invalid Kernel model service")?;
    let mut changed = Vec::new();
    for (key, value) in parameters
        .as_object()
        .ok_or("invalid length-recovery parameters")?
    {
        if !matches!(key.as_str(), "thinkingLevel" | "reasoning") {
            return Err("length recovery may only lower reasoning parameters".into());
        }
        if derived.get(key) == Some(value) {
            continue;
        }
        derived.insert(key.clone(), value.clone());
        changed.push(key.clone());
    }
    Ok((Value::Object(derived), changed))
}

/// Recompute the recovery dispatch's model configuration from the frozen one and
/// the durable record.
///
/// Pure and deterministic, so the parameters that reached the wire can be
/// re-derived from durable facts at any time — and so a dispatch can never be
/// handed a configuration the record does not authorize. The record's
/// `recoveryParameters` are accepted only when they are exactly what the recorded
/// capability decision allowed: an empty set for an instruction-only recovery, or
/// the intended lowering when the parameter was proven effective.
pub(crate) fn derive_length_recovery_config(
    frozen: &crate::kernel_model_config::KernelModelConfig,
    record: &Value,
) -> Result<crate::kernel_model_config::KernelModelConfig, String> {
    let parameters = record
        .get("recoveryParameters")
        .ok_or("length-recovery record has no parameters")?;
    let effect = record
        .get("parameterEffect")
        .ok_or("length-recovery record has no parameter effect")?;
    let effective = effect.get("effective").and_then(Value::as_bool)
        .ok_or("length-recovery record has no effectiveness")?;
    let intended = effect
        .get("locallyIntendedParameters")
        .ok_or("length-recovery record has no intended parameters")?;
    if effective != (parameters == intended && !parameters.as_object().is_some_and(|map| map.is_empty()))
    {
        return Err("length-recovery parameters disagree with the recorded effect".into());
    }
    let (service, changed) = apply_recovery_parameters(&frozen.model_service, parameters)?;
    if !effective && !changed.is_empty() {
        return Err("an ineffective parameter must not change the request".into());
    }
    let mut derived = frozen.clone();
    derived.model_service = service;
    derived.hash()?;
    Ok(derived)
}

pub(crate) struct KernelCoordinator<'a> {
    database: &'a Database,
    clock: &'a dyn Clock,
    cancellation: &'a CancellationRegistry,
    binding: RunControlBinding,
    controller: Mutex<RunController>,
    preview: Option<&'a super::kernel_model_worker::PreviewSink>,
}

impl<'a> KernelCoordinator<'a> {
    fn unfinished_compute_wait_facts(&self) -> Result<Vec<kernel::WaitingJobFact>, String> {
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(Vec::new());
        }
        self.database.kernel_waiting_job_facts(
            &self.binding.conversation_id, &self.binding.run_id)
    }

    fn pending_host_job_notices(
        &self, history: &[Value],
    ) -> Result<Vec<fox_engine_protocol::HostJobNotice>, String> {
        self.database.validate_host_job_notice_history(&self.binding.conversation_id,
            &self.binding.run_id, history)?;
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(Vec::new());
        }
        let historical = fox_engine_protocol::historical_host_job_notice_bytes(history)?;
        self.database.pending_host_job_notices(&self.binding.conversation_id,
            &self.binding.run_id,(16 * 1024usize).saturating_sub(historical))
    }

    /// Both direct completion and a durable wake resume from the assistant
    /// response the model actually produced. Notices remain a typed Host lane,
    /// never a fabricated user turn or a second tool result.
    fn job_notice_followup_input(
        &self, pre_history: &[Value], assistant: &Value,
    ) -> Result<Option<(fox_engine_protocol::KernelInitialModelInput,
        Vec<fox_engine_protocol::HostJobNotice>)>, String> {
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(None);
        }
        let mut messages = pre_history.to_vec();
        messages.push(assistant.clone());
        // The response lease is still open here. Markers delivered by this
        // very request remain `bound` until the same response write-set
        // acknowledges them, so the read-only historical validator (which
        // requires `acknowledged`) cannot run before that transaction.
        // checked_job_notice_input revalidates the complete history and facts
        // after acknowledgement inside the response transaction.
        let historical=fox_engine_protocol::historical_host_job_notice_bytes(&messages)?;
        let notices=self.database.pending_host_job_notices(
            &self.binding.conversation_id,&self.binding.run_id,
            (16*1024usize).saturating_sub(historical))?;
        if notices.is_empty() { return Ok(None); }
        let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages = messages;
        input.validate_job_notice()?;
        Ok(Some((input, notices)))
    }

    pub(crate) fn start_prepared(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        let (input, config, hash) = database.kernel_initial_start_state(run_id)?;
        let (controller, effects) =
            RunController::start_with_initial_input(run_id, &input.turn_id, config, &hash, clock)
                .map_err(|error| error.to_string())?;
        database.kernel_commit_decision(
            run_id,
            clock.now_wall_ms(),
            &controller.persist_command(&effects),
        )?;
        Self::reopen(database, clock, run_id, cancellation)
    }

    pub(crate) fn reopen(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        Self::reopen_with_scope(database, clock, run_id, cancellation, false)
    }

    /// Rehydrate solely to consume a durable parked cancel. The already
    /// cancelled scope must exist; no usable execution token is registered or
    /// revived, and the caller may only settle the persisted cancel command.
    pub(crate) fn reopen_cancelled_wait(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        Self::reopen_with_scope(database, clock, run_id, cancellation, true)
    }

    fn reopen_with_scope(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
        cancelled_wait_only: bool,
    ) -> Result<Self, String> {
        let binding = database
            .run_control_binding(run_id)?
            .ok_or("authoritative Run has no frozen control binding")?;
        let data = database
            .kernel_rehydrate(run_id)?
            .ok_or("authoritative Run has no durable Kernel state")?;
        let config = &data.config;
        if binding.authority != ExecutionAuthority::Authoritative
            || config.kernel_mode != "authoritative"
            || binding.engine_id != config.engine_id
            || binding.permission_snapshot_id != config.permission_snapshot_id
            || binding.execution_profile_id != config.execution_profile_id
            || binding.budgets.approval_wait_ms != config.approval_wait_timeout_ms
            || binding.budgets.model_request_ms != config.model_request_timeout_ms
            || binding.budgets.model_first_response_ms != config.model_first_response_ms
            || binding.budgets.model_idle_ms != config.model_idle_ms
            || binding.budgets.tool_execution_ms != config.tool_execution_timeout_ms
            || binding.budgets.run_execution_ms != config.run_execution_budget_ms
            || binding.budgets.run_execution_limited != config.run_execution_limited
        {
            return Err(
                "Kernel and resource control bindings disagree; no authority fallback is allowed"
                    .into(),
            );
        }
        let controller = RunController::rehydrate(data).map_err(|error| error.to_string())?;
        if cancelled_wait_only {
            if controller.state() != kernel::RunState::WaitingJobs
                || !database.pending_kernel_host_commands(run_id)?
                    .iter().any(|command| command.kind == "cancel")
                || !cancellation.run_token(run_id)?.is_cancelled() {
                return Err("cancelled wait reconciliation has no matching scope and command".into());
            }
        } else {
            cancellation.register_run(run_id)?;
        }
        if controller.is_terminal() || controller.state() == kernel::RunState::Cancelling {
            cancellation.request_run_cancel(run_id);
        }
        Ok(Self {
            database,
            clock,
            cancellation,
            binding,
            controller: Mutex::new(controller),
            preview: None,
        })
    }

    pub(super) fn with_preview(
        mut self,
        preview: &'a super::kernel_model_worker::PreviewSink,
    ) -> Self {
        self.preview = Some(preview);
        self
    }

    /// Evaluate the task's promised deliverables for a round that would finish the
    /// Run, and stage the verdicts under a mark the completing decision commits.
    ///
    /// The verdicts come from durable facts and the filesystem; the decision comes
    /// from a different write-set a moment later. Staging first (and gating the
    /// final write on the committed mark) is what makes the pair crash-safe
    /// without ever finalizing a verdict no decision authorized.
    fn stage_completion_delivery(
        &self,
        history: &[Value],
        output: &Value,
    ) -> Result<CompletionDelivery, String> {
        let stop = super::delivery::evaluate_stop(
            &self.database,
            self.binding.permission.project_root.as_deref(),
            &self.binding.run_id,
        )?;
        match &stop {
            super::delivery::DeliveryStop::NoChecklist => {
                Ok(CompletionDelivery::Complete { mark: None })
            }
            super::delivery::DeliveryStop::Passed { .. }
            | super::delivery::DeliveryStop::Exhausted { .. } => {
                let mark = format!("delivery:completion:{}", uuid::Uuid::new_v4());
                super::delivery::stage_outcome(
                    &self.database,
                    &self.binding.run_id,
                    &mark,
                    &stop,
                    crate::database::now_ms(),
                )?;
                Ok(CompletionDelivery::Complete { mark: Some(mark) })
            }
            super::delivery::DeliveryStop::Repair { prompt, .. } => {
                let mark = format!("delivery:completion:{}", uuid::Uuid::new_v4());
                super::delivery::stage_outcome(
                    &self.database,
                    &self.binding.run_id,
                    &mark,
                    &stop,
                    crate::database::now_ms(),
                )?;
                let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
                input.messages = history.to_vec();
                input.messages.push(output.clone());
                input.messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": prompt}],
                    "timestamp": 0
                }));
                input.validate()?;
                Ok(CompletionDelivery::Repair {
                    mark,
                    prompt: prompt.clone(),
                    input,
                })
            }
        }
    }

    /// Make the staged verdicts of a decision that just committed final.
    fn finish_completion_delivery(
        &self,
        completion: Option<&CompletionDelivery>,
    ) -> Result<(), String> {
        let Some(mark) = completion.and_then(CompletionDelivery::mark) else {
            return Ok(());
        };
        super::delivery::finalize_outcome(
            &self.database,
            &self.binding.run_id,
            mark,
            crate::database::now_ms(),
        )?;
        // The mark was written by the commit taken immediately above, so it must
        // be here; if it is not, the ledger would describe work no committed
        // decision authorized.
        if !self.database.decision_mark_committed(&self.binding.run_id, mark)? {
            return Err("delivery stage lost its committed decision mark".into());
        }
        Ok(())
    }

    /// Drop a refused attempt's stage: its decision never committed, so its
    /// verdicts must never become the ledger's history.
    fn abandon_completion_delivery(
        &self,
        completion: Option<&CompletionDelivery>,
    ) -> Result<(), String> {
        let Some(mark) = completion.and_then(CompletionDelivery::mark) else {
            return Ok(());
        };
        super::delivery::withdraw_outcome(&self.database, &self.binding.run_id, mark)
    }

    /// A failed decision transaction never leaves speculative state in memory.
    fn apply(
        &self,
        lease: Option<DecisionLease<'_>>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
    ) -> Result<(), String> {
        self.apply_decision(lease, action, None, None)
    }

    /// Same durable decision as [`Self::apply`], additionally transitioning
    /// mid-run steering rows inside the same write-set (the round response
    /// answers delivered rows; the directive built with it arms new ones), and
    /// optionally stamping an opaque Host mark that is committed atomically with
    /// the decision (see [`kernel::KernelPersistCommand::delivery_decision_mark`]).
    fn apply_decision(
        &self,
        lease: Option<DecisionLease<'_>>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
        steering: Option<&crate::database::SteeringDecision>,
        decision_mark: Option<&str>,
    ) -> Result<(), String> {
        // Re-read the immutable, hash-verified resource binding on each entry.
        if self
            .database
            .run_control_binding(&self.binding.run_id)?
            .as_ref()
            != Some(&self.binding)
        {
            return Err("authoritative Run control binding changed".into());
        }
        let mut guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let mut candidate = guard.clone();
        let now = self.clock.read();
        let effects = action(&mut candidate, now).map_err(|error| error.to_string())?;
        let mut command = candidate.persist_command(&effects);
        // The mark travels with the write-set: the repository writes it inside
        // the decision's own transaction, so a refused or failed commit leaves
        // no mark behind and the staged work it belongs to can never be
        // finalized as if the decision had happened.
        command.delivery_decision_mark = decision_mark.map(str::to_owned);
        match lease {
            Some(DecisionLease::Approval(version)) => self.database.kernel_commit_decision_with_approval_version(
                &self.binding.run_id, now.wall_ms, &command, version)?,
            Some(DecisionLease::WaitingWakePolicy(version)) => self.database.kernel_commit_waiting_wake(
                &self.binding.run_id, now.wall_ms, &command, version)?,
            Some(DecisionLease::WaitingAccount) => self.database.kernel_commit_waiting_account(
                &self.binding.run_id, now.wall_ms, &command)?,
            Some(DecisionLease::ModelRetry(effect_key, owner)) => {
                self.database.kernel_commit_model_retry(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    effect_key,
                    owner,
                )?
            }
            Some(DecisionLease::Continuation(effect_key, owner)) => self.database.kernel_commit_continuation_model(
                &self.binding.run_id, now.wall_ms, &command, effect_key, owner, true, steering, None)?,
            Some(DecisionLease::ContinuationDispatch(effect_key, owner, input)) => self.database.kernel_commit_continuation_model(
                &self.binding.run_id, now.wall_ms, &command, effect_key, owner, false, None, input)?,
            Some(DecisionLease::Initial(owner)) => self.database.kernel_commit_initial_model(
                &self.binding.run_id,
                now.wall_ms,
                &command,
                owner,
                true,
                steering,
                None,
            )?,
            Some(DecisionLease::Tool(tool_call_id, owner)) => {
                self.database.kernel_commit_tool_result(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    tool_call_id,
                    owner,
                )?
            }
            Some(DecisionLease::Batch(batch_id, owner)) => {
                self.database.kernel_commit_batch_response(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    batch_id,
                    owner,
                    steering,
                )?
            }
            Some(DecisionLease::BatchDispatch(batch_id, owner, input)) => {
                self.database.kernel_commit_batch_dispatch(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    batch_id,
                    owner,
                    input,
                )?
            }
            None => {
                self.database
                    .kernel_commit_decision(&self.binding.run_id, now.wall_ms, &command)?
            }
        }
        // Persist cancellation first, then signal every issued execution token.
        for effect in &effects {
            if let Effect::CancelToolCall { tool_call_id } = effect {
                self.cancellation
                    .request_tool_cancel(&self.binding.run_id, tool_call_id);
            }
        }
        if candidate.is_terminal() || candidate.state() == kernel::RunState::Cancelling {
            self.cancellation.request_run_cancel(&self.binding.run_id);
        }
        *guard = candidate;
        Ok(())
    }

    pub(crate) fn tick(&self) -> Result<(), String> {
        self.tick_budgets(false)
    }

    pub(crate) fn account_waiting_jobs_now(&self) -> Result<(), String> {
        let park_seq = self.database.kernel_waiting_park_seq(&self.binding.run_id)?;
        self.apply(Some(DecisionLease::WaitingAccount), |controller, now| {
            controller.account_waiting_jobs(park_seq, now.wall_ms)
        })
    }

    /// Controlled B2b-2 wake. The caller owns the Run OS lock; B2b-3 will
    /// supply the automatic trigger. This method only creates the one durable
    /// continuation intent after all original Jobs have terminal notices.
    pub(crate) fn wake_waiting_jobs_now(&self) -> Result<bool,String> {
        self.wake_waiting_jobs_with_policy_version(None)
    }

    pub(crate) fn wake_waiting_jobs_at_policy_version(&self, version: u64) -> Result<bool,String> {
        self.wake_waiting_jobs_with_policy_version(Some(version))
    }

    fn wake_waiting_jobs_with_policy_version(&self, version: Option<u64>) -> Result<bool,String> {
        if self.snapshot()?.state!="waiting_jobs" {return Ok(false);}
        if !self.unfinished_compute_wait_facts()?.is_empty() {return Ok(false);}
        let (park_seq,parked)=self.database.kernel_waiting_park(&self.binding.run_id)?;
        let mut input=self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages=serde_json::from_value(parked["history"].clone())
            .map_err(|_|"invalid frozen job wait history")?;
        input.messages.push(parked["response"]["assistantMessage"].clone());
        input.validate_job_notice()?;
        let notices=self.pending_host_job_notices(&input.messages)?;
        if notices.is_empty() {
            return Err("kernel.job_notice_capacity_blocked: no bounded pending typed fact".into());
        }
        self.apply(version.map(DecisionLease::WaitingWakePolicy),|controller,now| {
            let accounted=controller.wait_accounted_until_wall_ms()
                .ok_or_else(||KernelError::FailClosed("job wait cursor is missing".into()))?;
            let effect_key=format!("continuation:{}",controller.last_event_seq()
                +u64::from(now.wall_ms>accounted)+1);
            let payload=serde_json::json!({"turnId":input.turn_id,
                "effectKey":effect_key,"lane":"job_notice","prompt":"",
                "input":input,"hostJobNotices":notices});
            controller.wake_waiting_jobs(park_seq,now.wall_ms,&payload.to_string())
        })?;
        Ok(true)
    }

    pub(crate) fn cancel_waiting_jobs_now(&self) -> Result<(), String> {
        let park_seq = self.database.kernel_waiting_park_seq(&self.binding.run_id)?;
        self.apply(None, |controller, now| {
            controller.cancel_waiting_jobs(park_seq, now.wall_ms)
        })
    }

    fn tick_settled_model(&self) -> Result<(), String> {
        self.tick_budgets(true)
    }

    fn tick_budgets(&self, model_settled: bool) -> Result<(), String> {
        // Fold pending worker-observed output progress into the controller so
        // the first-response/idle bounds measure real output. The registry is
        // consume-once: every preview counts exactly once, and ticks without
        // fresh output grow idleness. The wall-anchored signal is bridged into
        // the controller's monotonic domain by subtracting the observed idle
        // span from the current monotonic reading (production clocks advance
        // both domains together; unit tests drive note_model_progress
        // directly and never depend on this conversion).
        let progress = self
            .database
            .consume_kernel_model_progress(&self.binding.run_id);
        self.apply(None, |controller, now| {
            if let Some(progress_wall) = progress {
                let idle = now.wall_ms.saturating_sub(progress_wall).max(0);
                let mono = now.monotonic_ms.saturating_sub(idle).max(0);
                controller.note_model_progress(mono, progress_wall);
            }
            Ok(if model_settled { controller.tick_settled_model(now.monotonic_ms, now.wall_ms) }
               else { controller.tick(now.monotonic_ms, now.wall_ms) })
        })?;
        if self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .is_terminal()
        {
            self.database.clear_kernel_model_progress(&self.binding.run_id);
        }
        Ok(())
    }

    pub(crate) fn propose_tools(
        &self,
        batch_id: &str,
        calls: Vec<ToolCallRequest>,
        policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.propose_tool_batch(batch_id, calls, policy, now.monotonic_ms, now.wall_ms)
        })
    }

    pub(crate) fn propose_engine_batch(
        &self,
        checkpoint: fox_engine_protocol::KernelEngineBatchCheckpoint,
        policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        checkpoint.validate()?;
        if self
            .database
            .run_control_binding(&self.binding.run_id)?
            .as_ref()
            != Some(&self.binding)
        {
            return Err("authoritative Run control binding changed".into());
        }
        let value = serde_json::to_value(&checkpoint).map_err(|error| error.to_string())?;
        let stored = serde_json::json!({"value":value,"hash":checkpoint_hash(&value)});
        if let Some(existing) = self
            .database
            .kernel_engine_batch_checkpoint(&self.binding.run_id, &checkpoint.batch_id)?
        {
            if existing["checkpoint"] == stored
                && existing["engineId"].as_str() == Some(self.binding.engine_id.as_str())
            {
                return Ok(());
            }
            return Err("immutable engine checkpoint replay conflict".into());
        }
        let calls = checkpoint.assistant_message["content"]
            .as_array()
            .ok_or("missing engine proposal")?
            .iter()
            .filter(|block| block["type"] == "toolCall")
            .enumerate()
            .map(|(source_order, call)| ToolCallRequest {
                tool_call_id: call["id"].as_str().unwrap().into(),
                tool: call["name"].as_str().unwrap().into(),
                canonical_input_json: call["arguments"].to_string(),
                source_order,
            })
            .collect();
        self.tick()?;
        self.apply(None, |controller, now| {
            let mut effects = controller.propose_tool_batch(
                &checkpoint.batch_id,
                calls,
                policy,
                now.monotonic_ms,
                now.wall_ms,
            )?;
            effects
                .push(controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?);
            Ok(effects)
        })
    }

    pub(crate) fn prepare_stored_batch_resume(
        &self,
        batch_id: &str,
    ) -> Result<fox_engine_protocol::KernelBatchResumeFrame, String> {
        let event = self
            .database
            .kernel_engine_batch_checkpoint(&self.binding.run_id, batch_id)?
            .ok_or("original engine checkpoint is missing; it must not be regenerated")?;
        let value = &event["checkpoint"]["value"];
        if event["checkpoint"]["hash"].as_str() != Some(checkpoint_hash(value).as_str())
            || event["engineId"].as_str() != Some(self.binding.engine_id.as_str())
        {
            return Err("persisted engine checkpoint hash or engine identity mismatch".into());
        }
        let checkpoint: fox_engine_protocol::KernelEngineBatchCheckpoint =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        checkpoint.validate()?;
        if checkpoint.batch_id != batch_id {
            return Err("persisted engine checkpoint batch mismatch".into());
        }
        let history = self.model_retry_context(batch_id, self.context_view(batch_id, &checkpoint.history)?)?;
        // Replacement transport for a dead/compacting live session: re-collect
        // every active steering row (new rows plus rows already delivered to
        // the failed live attempt) under the batch delivery identity, so the
        // rebuilt dispatch carries each row exactly once.
        let steering_rows = self.database.deliver_steering_for_dispatch(
            &self.binding.run_id,
            &kernel::batch_delivery_idempotency_key(batch_id),
        )?;
        let notices = steering::steering_notices(&steering_rows);
        let frame = self.prepare_batch_resume(batch_id, history, checkpoint.assistant_message, notices)?;
        if event["turnId"].as_str() != Some(frame.turn_id.as_str()) {
            return Err("persisted engine checkpoint turn mismatch".into());
        }
        Ok(frame)
    }

    /// Reevaluate without a preliminary mutation transaction: superseding the
    /// old approval and installing the new decision are one durable write-set.
    /// Rehydration affects only apply()'s candidate; failure leaves memory intact.
    pub(crate) fn reevaluate_tool(
        &self, tool_call_id: &str, policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        let version = self.database.execution_policy(&self.binding.conversation_id)?.version;
        self.apply(Some(DecisionLease::Approval(version)), |controller, now| {
            let data = self.database.kernel_rehydrate(&self.binding.run_id)
                .map_err(KernelError::FailClosed)?
                .ok_or_else(|| KernelError::FailClosed("missing durable run for reevaluation".into()))?;
            *controller = RunController::rehydrate(data)?;
            controller.reevaluate_tool(policy, tool_call_id, version, now.monotonic_ms, now.wall_ms)
        })
    }

    pub(crate) fn resolve_approval(
        &self,
        tool_call_id: &str,
        decision: kernel::ApprovalDecision,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.resolve_approval(tool_call_id, decision, now.monotonic_ms)
        })
    }

    pub(crate) fn resolve_approval_at_version(
        &self, tool_call_id: &str, decision: kernel::ApprovalDecision, version: u64,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(Some(DecisionLease::Approval(version)), |controller, now| {
            controller.resolve_approval(tool_call_id, decision, now.monotonic_ms)
        })
    }

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.request_cancel()))
    }

    /// Host calls this only after its owned model/tool executors have stopped.
    pub(crate) fn settle_cancellation(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.settle_cancellation()))
    }

    /// Terminate the Run as failed, after recording a real delivery verdict for
    /// whatever the task promised.
    ///
    /// The verification is the ordinary deterministic gate, and it is written only
    /// through this failing decision's own committed mark. The Run is still failed:
    /// a verdict is evidence about files, never a claim that the task succeeded, and
    /// no repair round is armed for a Run that is already over. An evaluation error
    /// never blocks the terminal (a stuck Run is worse than a missing verdict), but
    /// it is reported to the caller's caller through the same failure.
    pub(crate) fn fail(&self, code: &str, message: &str) -> Result<(), String> {
        let mark = self.stage_failure_delivery().unwrap_or(None);
        self.apply_decision(
            None,
            |controller, _| {
                Ok(controller.terminate(kernel::RunOutcome::Failed {
                    code: code.into(),
                    message: message.into(),
                }))
            },
            None,
            mark.as_deref(),
        )?;
        if let Some(mark) = mark.as_deref() {
            super::delivery::finalize_outcome(
                &self.database,
                &self.binding.run_id,
                mark,
                crate::database::now_ms(),
            )?;
            // The mark was written by the commit taken immediately above, so it must
            // be here; otherwise the ledger would carry verdicts no decision owned.
            if !self.database.decision_mark_committed(&self.binding.run_id, mark)? {
                return Err("delivery failure stage lost its committed decision mark".into());
            }
        }
        Ok(())
    }

    /// Verify what this Run promised, staged for the failing decision to own.
    fn stage_failure_delivery(&self) -> Result<Option<String>, String> {
        let stop = super::delivery::evaluate_stop(
            &self.database,
            self.binding.permission.project_root.as_deref(),
            &self.binding.run_id,
        )?;
        if matches!(stop, super::delivery::DeliveryStop::NoChecklist) {
            return Ok(None);
        }
        let mark = format!("delivery:failure:{}", uuid::Uuid::new_v4());
        super::delivery::stage_failure_outcome(
            &self.database,
            &self.binding.run_id,
            &mark,
            &stop,
            crate::database::now_ms(),
        )?;
        Ok(Some(mark))
    }

    pub(crate) fn snapshot(&self) -> Result<kernel::KernelSnapshot, String> {
        self.database
            .kernel_build_full_snapshot(&self.binding.run_id)
    }

    // -----------------------------------------------------------------------
    // The one controlled length-truncation recovery
    // -----------------------------------------------------------------------

    /// The durable delivery facts the recovery instruction is generated from.
    ///
    /// Nothing here is invented and nothing is hard-coded per task: the Host's own
    /// checklist statuses decide whether the promised artifacts were verified,
    /// are unverified, or are missing, and only bounded display names of the
    /// outstanding items are quoted back to the model.
    fn length_recovery_delivery(&self) -> Result<LengthRecoveryDelivery, String> {
        let stop = super::delivery::evaluate_stop(
            &self.database,
            self.binding.permission.project_root.as_deref(),
            &self.binding.run_id,
        )?;
        let items = self.database.delivery_checklist(&self.binding.run_id)?;
        let passed = items.iter().filter(|item| item.status == "passed").count();
        let failed = items.iter().filter(|item| item.status == "failed").count();
        let pending = items.iter().filter(|item| item.status == "pending").count();
        let kind = match &stop {
            super::delivery::DeliveryStop::NoChecklist => LengthRecoveryDeliveryKind::NoChecklist,
            super::delivery::DeliveryStop::Passed { .. } => {
                LengthRecoveryDeliveryKind::VerifiedDeliverables
            }
            // A promise that failed its checks is a different fact from a promise
            // whose content simply was never verified, and the model is told which
            // one it is.
            super::delivery::DeliveryStop::Repair { .. }
            | super::delivery::DeliveryStop::Exhausted { .. } => {
                if failed > 0 {
                    LengthRecoveryDeliveryKind::IncompleteDeliverables
                } else {
                    LengthRecoveryDeliveryKind::UnverifiedArtifacts
                }
            }
        };
        let outstanding = items
            .iter()
            .filter(|item| item.status != "passed")
            .filter_map(|item| bound_item_name(&item.display_name))
            .take(LENGTH_RECOVERY_MAX_NAMED_ITEMS)
            .collect();
        Ok(LengthRecoveryDelivery { kind, passed, failed, pending, outstanding })
    }

    /// The adapter's observed serialization ability for this Run.
    ///
    /// Read-only, bounded, and deliberately taken from the runtime's own request
    /// boundary record (`usage.request` → `config.sent`) instead of from any local
    /// configuration or from the `resolved` layer. Absence of a record is reported
    /// as absence, so effectiveness can never be claimed without a wire fact.
    fn adapter_reasoning_evidence(&self) -> Result<AdapterReasoningEvidence, String> {
        let sent = self.database.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT event_json FROM run_events
                  WHERE run_id=?1 AND event_type='usage.request'
                  ORDER BY seq DESC LIMIT 16",
            )?;
            let rows = query.query_map([&self.binding.run_id], |row| row.get::<_, String>(0))?;
            for row in rows {
                let event: Value = match serde_json::from_str(&row?) {
                    Ok(event) => event,
                    Err(_) => continue,
                };
                let sent = &event["record"]["config"]["sent"];
                if sent.is_object() {
                    return Ok(Some(sent.clone()));
                }
            }
            Ok(None)
        })?;
        Ok(AdapterReasoningEvidence::from_sent(sent))
    }

    /// Build the durable audit record for the one recovery.
    ///
    /// It records what was actually observed (the trigger form and the counters
    /// the worker reported — absent facts stay absent), the three parameter layers
    /// (locally intended / really applied / the wire evidence they rest on) and
    /// whether the instruction is really delivered, the request identity of the
    /// dispatch that will carry it, and the exact instruction text.
    /// `appliedAtWallMs` stays null until the commit that arms the request, which
    /// is also what makes the allowance single-use.
    fn length_recovery_record(
        &self,
        failure: &fox_engine_protocol::KernelModelFailure,
        class: &str,
        effect_key: &str,
        owner: &str,
        turn_id: &str,
        checkpoint_seq: u64,
        scheduled_at_wall_ms: i64,
        due_wall_ms: i64,
        delivery: &LengthRecoveryDelivery,
        lever: &LengthRecoveryLever,
    ) -> Result<String, String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        let original = original_model_parameters(&config.model_service);
        let recovery = lever.applied.clone();
        let (derived_service, changed) =
            apply_recovery_parameters(&config.model_service, &recovery)?;
        let diagnostic = failure.diagnostic.as_ref();
        let mut evidence = serde_json::Map::new();
        evidence.insert(
            "form".into(),
            json!(length_truncation_evidence(failure).unwrap_or("unknown")),
        );
        if let Some(value) = diagnostic.and_then(|item| item.stop_reason.as_ref()) {
            evidence.insert("stopReason".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.reason.as_ref()) {
            evidence.insert("reason".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.text_chars) {
            evidence.insert("textChars".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.thinking_chars) {
            evidence.insert("thinkingChars".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.tool_call_count) {
            evidence.insert("toolCallCount".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.completion_required) {
            evidence.insert("completionRequired".into(), json!(value));
        }
        if let Some(value) = diagnostic.and_then(|item| item.dispatch_id.as_ref()) {
            evidence.insert("workerDispatchId".into(), json!(value));
        }
        evidence.insert("truncatedShape".into(), json!(length_truncation_shape(failure).token()));
        let instruction = delivery.instruction();
        // The recovery instruction is a Host-authored model-view message. A Run
        // with the typed Host-job-notice lane validates its retried frames against
        // the durable frozen sources, so a Host message cannot be added there: the
        // instruction is then reported as undeliverable instead of silently
        // dropped. The parameter lever is independent of that lane, but only when
        // it is proven effective — otherwise there is no real change at all and
        // the caller refuses the recovery instead of dispatching it.
        let instruction_lane = !self.database.compute_job_notice_enabled(&self.binding.run_id)?;
        if !instruction_lane && !lever.effective {
            return Err("length recovery has no deliverable levers for this Run".into());
        }
        let record = json!({
            "schemaVersion": 1,
            "recoveryId": uuid::Uuid::new_v4().to_string(),
            "trigger": class,
            "evidence": Value::Object(evidence),
            "delivery": {
                "kind": delivery.kind.token(),
                "passed": delivery.passed,
                "failed": delivery.failed,
                "pending": delivery.pending,
                "outstanding": delivery.outstanding,
            },
            // Three layers, kept apart: what the Host intended locally, what is
            // really applied to the request, and the wire fact the decision rests on.
            "parametersMode": lever.lever(),
            "parameterEffect": {
                "effective": lever.effective,
                "lever": lever.lever(),
                "reason": lever.reason,
                "signalSource": AdapterReasoningEvidence::SOURCE,
                "signalNote": "the resolved layer is deliberately NOT evidence: only the request boundary record counts",
                "observedSentParameters": lever.evidence.observed_sent.clone().unwrap_or(Value::Null),
                "observedReasoningEffort": lever.evidence.serialized_effort.clone().map(Value::String).unwrap_or(Value::Null),
                "locallyIntendedParameters": lever.intended,
                "expectedSentParameters": lever.expected_sent,
                "expectedSentNote": "expected, derived from the same verified field; the authoritative check is the durable usage record of the recovery attempt itself",
            },
            "originalParameters": original,
            "recoveryParameters": recovery,
            "changedServiceFields": changed,
            "instructionIsSoftConstraint": true,
            "instructionKind": delivery.kind.token(),
            "instructionDelivered": instruction_lane,
            "instructionDelivery": if instruction_lane { "model_view_message" } else { "unavailable_notice_lane" },
            "instruction": instruction,
            "runId": self.binding.run_id,
            "turnId": turn_id,
            "effectKey": effect_key,
            "owner": owner,
            "checkpointSeq": checkpoint_seq,
            "scheduledAtWallMs": scheduled_at_wall_ms,
            "dueWallMs": due_wall_ms,
            "appliedAtWallMs": Value::Null,
        });
        // The derived configuration must be exactly what the record authorizes,
        // and it must still be a valid Kernel model configuration.
        let mut derived = config.clone();
        derived.model_service = derived_service;
        derived.hash()?;
        let encoded = serde_json::to_string(&record).map_err(|_| "invalid length-recovery record")?;
        if encoded.len() > LENGTH_RECOVERY_MAX_RECORD_BYTES {
            return Err("length-recovery record exceeds its durable bound".into());
        }
        Ok(encoded)
    }

    /// The recovery scheduled for the dispatch that is about to be built, if any.
    ///
    /// Pending means: the record exists, it has not been applied to a request yet,
    /// and it names this dispatch's effect key. A different lane can therefore
    /// never swallow another lane's recovery.
    fn pending_length_recovery(&self, effect_key: &str) -> Result<Option<Value>, String> {
        let guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        if !guard.length_recovery_pending() {
            return Ok(None);
        }
        let record = guard
            .length_recovery_record()
            .ok_or("length-recovery record disappeared")?;
        let value: Value =
            serde_json::from_str(record).map_err(|_| "invalid length-recovery record")?;
        if value.get("effectKey").and_then(Value::as_str) != Some(effect_key) {
            return Ok(None);
        }
        Ok(Some(value))
    }

    /// The model configuration a dispatch must use, and the pending recovery that
    /// authorized a parameter change.
    ///
    /// The frozen configuration is returned unchanged unless the durable record
    /// names exactly this effect key; the derived configuration is recomputed from
    /// the frozen service plus the recorded parameters, so what reaches the wire is
    /// always re-derivable from durable facts and a stale or tampered record cannot
    /// smuggle in a different provider request.
    fn dispatch_model_config(
        &self,
        effect_key: &str,
    ) -> Result<(crate::kernel_model_config::KernelModelConfig, Option<Value>), String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        let recovery = self.pending_length_recovery(effect_key)?;
        let Some(record) = recovery else {
            return Ok((config, None));
        };
        let derived = derive_length_recovery_config(&config, &record)?;
        Ok((derived, Some(record)))
    }

    /// The instruction to append to the dispatch named by `effect_key`, when the
    /// pending recovery is that dispatch's.
    pub(super) fn length_recovery_instruction(&self, effect_key: &str) -> Result<Option<String>, String> {
        let Some(record) = self.pending_length_recovery(effect_key)? else {
            return Ok(None);
        };
        let instruction = record
            .get("instruction")
            .and_then(Value::as_str)
            .ok_or("length-recovery record has no instruction")?
            .to_owned();
        if instruction.len() > LENGTH_RECOVERY_MAX_INSTRUCTION_BYTES {
            return Err("length-recovery instruction exceeds its bound".into());
        }
        Ok(Some(instruction))
    }

    /// The model-configuration hash this dispatch is allowed to present: the
    /// frozen one, or — for the one controlled length recovery — the hash of the
    /// configuration re-derived from the frozen configuration plus the recorded
    /// parameters. Rejecting anything else keeps the freeze meaningful while still
    /// letting that single recovery change the request.
    fn expected_dispatch_config_hash(
        &self,
        effect_key: &str,
        frozen_hash: &str,
    ) -> Result<String, String> {
        let Some(record) = self.pending_length_recovery(effect_key)? else {
            return Ok(frozen_hash.to_owned());
        };
        let frozen = self.database.kernel_model_config(&self.binding.run_id)?;
        Ok(derive_length_recovery_config(&frozen, &record)?.hash()?)
    }

    /// The Host's honest Chinese account of a Run that could not finish its answer
    /// because the provider's output limit was reached.
    ///
    /// It is written on the Run's own failure, from durable facts, and never
    /// claims completion unless the content checks actually passed; it is a Host
    /// status, never a fabricated model reply.
    fn length_truncated_terminal_message(
        &self,
        delivery: Option<&LengthRecoveryDelivery>,
        recovery_spent: bool,
    ) -> String {
        let mut message = LENGTH_TRUNCATED_TERMINAL.to_owned();
        if let Some(delivery) = delivery {
            match delivery.kind {
                LengthRecoveryDeliveryKind::VerifiedDeliverables => {
                    message.push_str(&format!(
                        "已核验通过的交付物 {} 项。",
                        delivery.passed
                    ));
                }
                LengthRecoveryDeliveryKind::UnverifiedArtifacts => {
                    message.push_str(&format!(
                        "已通过核验 {} 项，内容尚未核验 {} 项。",
                        delivery.passed,
                        delivery.pending + delivery.failed
                    ));
                }
                LengthRecoveryDeliveryKind::IncompleteDeliverables => {
                    message.push_str(&format!(
                        "已通过核验 {} 项，缺失或不合格 {} 项。",
                        delivery.passed, delivery.failed
                    ));
                }
                LengthRecoveryDeliveryKind::NoChecklist => {
                    message.push_str("没有可用于内容核验的交付物清单。");
                }
            }
            if !delivery.outstanding.is_empty() {
                message.push_str(&format!(
                    "未完成项：{}。",
                    delivery.outstanding.join("、")
                ));
            }
        }
        message.push_str(if recovery_spent {
            "本次运行的受控长度恢复已使用过一次，不会再重复同一请求。"
        } else {
            "剩余运行预算不足以再发起一次受控恢复。"
        });
        message.push_str("已完成的工具结果已保留，可继续完成剩余工作。");
        message
    }

    /// Project a committed result barrier for a replacement engine session.
    /// The caller supplies the original engine checkpoint, never fresh model
    /// output. This read does not claim or complete the batch-delivery outbox.
    fn prepare_batch_resume(
        &self,
        batch_id: &str,
        history: Vec<Value>,
        assistant_message: Value,
        steering: Vec<fox_engine_protocol::KernelSteeringNotice>,
    ) -> Result<fox_engine_protocol::KernelBatchResumeFrame, String> {
        use fox_engine_protocol::KernelBatchResumeFrame;
        self.tick()?;
        let guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let snapshot = self.snapshot()?;
        if guard.state() != kernel::RunState::Running
            || snapshot.last_event_seq != guard.last_event_seq()
        {
            return Err("batch resume requires a current running Kernel snapshot".into());
        }
        let data = guard.shadow_checkpoint(self.clock.now_monotonic_ms());
        let batch = data
            .batches
            .iter()
            .find(|batch| batch.batch_id == batch_id && batch.barrier_emitted)
            .ok_or("batch result barrier has not committed")?;
        let effect = snapshot.pending_effects.iter().find(|effect| effect.effect_key == kernel::batch_delivery_effect_key(batch_id)
            && effect.kind == kernel::OutboxEffectKind::DeliverToolBatch && effect.status == kernel::OutboxStatus::Pending)
            .ok_or("batch delivery is missing, claimed or already completed; reconcile instead of replaying")?;
        let payload: Value =
            serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
        if effect.batch_id.as_deref() != Some(batch_id)
            || effect.idempotency_key != kernel::batch_delivery_idempotency_key(batch_id)
            || payload["orderedToolCallIds"] != serde_json::json!(batch.ordered)
        {
            return Err("durable batch delivery identity mismatch".into());
        }
        let tools = self.project_settled_tools(batch_id, &batch.ordered)?;
        let frame = KernelBatchResumeFrame {
            schema_version: 1,
            turn_id: data.turn_id,
            batch_id: batch_id.into(),
            idempotency_key: effect.idempotency_key.clone(),
            checkpoint_seq: snapshot.last_event_seq,
            history,
            assistant_message,
            tools,
            steering,
            host_job_notices: Vec::new(),
        };
        frame.validate()?;
        Ok(frame)
    }

    /// Durable result projection shared by the replacement-session resume frame
    /// and live-session directives. Includes the execution receipt so the model
    /// sees Host-confirmed approval and execution facts either way.
    fn project_settled_tools(&self, batch_id: &str, ordered: &[String])
        -> Result<Vec<fox_engine_protocol::KernelSettledToolResult>, String> {
        self.database.kernel_project_settled_tools(&self.binding.run_id,batch_id,ordered)
    }

    pub(super) fn dispatch_initial_with_worker(
        &self,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand,
        api_key: &str,
    ) -> Result<(), String> {
        let prepare_attempt = uuid::Uuid::new_v4().to_string();
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_started", observed_at,
        );
        self.ensure_context_with_worker("initial", 4096, owner, runtime, api_key)?;
        let (config, _) = self.dispatch_model_config(kernel::INITIAL_MODEL_EFFECT_KEY)?;
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_finished", observed_at,
        );
        self.dispatch_initial(owner, policy, |binding, frame, token| {
            let now = self.clock.read();
            let facts = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?
                .shadow_checkpoint(now.monotonic_ms);
            let since = facts
                .model_request_since_wall_ms
                .ok_or("initial model deadline is missing")?;
            if now.wall_ms < since {
                return Err("initial model clock moved backwards".into());
            }
            let remaining = binding.budgets.limit_operation_ms(
                binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)),
                facts.running_elapsed_ms,
            );
            super::kernel_model_worker::deliver_initial_with_preview(
                runtime,
                &config,
                api_key,
                binding,
                frame,
                token,
                remaining,
                self.preview,
                Some(self.database),
                Some(&prepare_attempt),
            )
        })
    }

    pub(super) fn dispatch_continuation_with_worker(
        &self,
        effect_key: &str,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand,
        api_key: &str,
    ) -> Result<(), String> {
        let prepare_attempt = uuid::Uuid::new_v4().to_string();
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_started", observed_at,
        );
        let (_,lane,_) = self.database.kernel_continuation_input_with_lane(
            &self.binding.run_id,effect_key)?;
        if lane.as_deref() != Some("job_notice") {
            self.ensure_context_with_worker(effect_key,4096,owner,runtime,api_key)?;
        }
        // The one controlled length recovery changes the request itself, so the
        // derived configuration is what this dispatch must actually send.
        let (config, _) = self.dispatch_model_config(effect_key)?;
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_finished", observed_at,
        );
        self.dispatch_initial_request(owner,policy,Some(effect_key),|binding,frame,token| {
            let now=self.clock.read();
            let facts=self.controller.lock().map_err(|_|"Kernel coordinator lock poisoned")?
                .shadow_checkpoint(now.monotonic_ms);
            let since=facts.model_request_since_wall_ms
                .ok_or("continuation model deadline is missing")?;
            if now.wall_ms<since {return Err("continuation model clock moved backwards".into());}
            let remaining=binding.budgets.limit_operation_ms(
                binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)),
                facts.running_elapsed_ms);
            super::kernel_model_worker::deliver_initial_with_preview(
                runtime,&config,api_key,binding,frame,token,remaining,self.preview,Some(self.database),Some(&prepare_attempt))
        })
    }

    fn model_request_window_expired(&self) -> Result<bool, String> {
        let now = self.clock.read();
        let facts = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(now.monotonic_ms);
        Ok(facts.model_request_since_wall_ms.is_some_and(|since|
            now.wall_ms.saturating_sub(since) >= self.binding.budgets.model_request_ms))
    }

    /// Classify and schedule the retry of a settled model failure.
    ///
    /// Timeouts first: this is called only after the owning transport has
    /// dropped/reaped its worker (`deliver_*` returned the error, or the Host's
    /// own model window expired), and the durable decision below refuses unless
    /// the same dispatch lease still exclusively owns the model request **and
    /// every tool is already settled**. So a first-response / stream-stall /
    /// whole-round timeout confirms the old request, its worker and its tool
    /// results before another request is admitted — and it is never turned into a
    /// 1-second retry (see the classification table above).
    ///
    /// A missing terminal frame is recoverable while those conditions hold.
    fn retry_settled_model(
        &self,
        effect_key: &str,
        owner: &str,
        cursor: u64,
        error: String,
    ) -> Result<(), String> {
        let failure = match super::kernel_model_worker::settled_failure(&error) {
            Some(failure) => failure,
            None if super::kernel_model_worker::is_reaped_transport_failure(&error) => {
                let turn_id = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
                    .shadow_checkpoint(self.clock.now_monotonic_ms()).turn_id;
                fox_engine_protocol::KernelModelFailure {
                    schema_version: 1, run_id: self.binding.run_id.clone(), turn_id,
                    checkpoint_seq: cursor, category: if error == "Kernel worker disconnected; reconcile delivery" || error == "Kernel worker write failed" {
                        "model_transport_failure".into()
                    } else { "model_timeout".into() }, http_status: None,
                    retry_after_ms: None, telemetry: None,
                    // The worker was reaped, so no round facts were ever observed:
                    // the diagnostic stays absent rather than being reported as zeros.
                    diagnostic: None,
                }
            }
            None => return Err(error),
        };
        if failure.run_id != self.binding.run_id || failure.checkpoint_seq != cursor {
            return Err("model failure belongs to another dispatch".into());
        }
        self.tick_settled_model()?;
        self.cancellation.run_token(&self.binding.run_id)?.check()?;
        let failure_json = serde_json::to_string(&failure).map_err(|_| "invalid model failure")?;
        // A length truncation needs the Host's own delivery facts to build its one
        // recovery instruction, and the adapter's own wire record to decide whether
        // a lowered parameter can change the request at all. Both are read before
        // the decision, so no filesystem/SQLite work happens under the controller
        // lock, and the classification decides whether that work is needed.
        let length_class = classify_model_failure(&failure);
        let (recovery_delivery, recovery_lever, instruction_lane) =
            if is_length_truncation(length_class) {
                let config = self.database.kernel_model_config(&self.binding.run_id)?;
                let evidence = self.adapter_reasoning_evidence()?;
                (
                    Some(self.length_recovery_delivery()?),
                    Some(plan_length_recovery_lever(&config.model_service, evidence)),
                    !self.database.compute_job_notice_enabled(&self.binding.run_id)?,
                )
            } else {
                (None, None, false)
            };
        let turn_id = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(self.clock.now_monotonic_ms())
            .turn_id;
        self.apply(
            Some(DecisionLease::ModelRetry(effect_key, owner)),
            |controller, now| {
                let facts = controller.shadow_checkpoint(now.monotonic_ms);
                let mut plan = plan_model_retry(
                    &failure,
                    &facts.retry,
                    &self.binding.run_id,
                    effect_key,
                );
                // The whole-Run elapsed bound is checked here as well as in the
                // Kernel, so the durable reason names the real cause instead of
                // being reported as exhausted retries. Measured from the Run's own
                // accumulated running time: an effect-key change cannot bypass it.
                if let Some(delay) = plan.delay_ms {
                    if self.binding.budgets.run_execution_limited
                        && delay
                            >= self.binding.budgets.run_execution_ms
                                .saturating_sub(facts.running_elapsed_ms)
                    {
                        plan = ModelRetryPlan::refused(RETRY_CLASS_RUN_BUDGET);
                    }
                }
                // The one recovery may only be dispatched when it really changes the
                // request: a parameter the adapter is proven to serialize, or an
                // instruction this Run can actually deliver. Neither → refuse, and
                // end honestly instead of spending a request with no effect.
                let lever_available = recovery_lever
                    .as_ref()
                    .is_some_and(|lever| lever.effective || instruction_lane);
                if plan.needs_recovery_record() && !lever_available {
                    plan = ModelRetryPlan::refused(RETRY_CLASS_LENGTH_RECOVERY_NO_LEVER);
                }
                let provider = failure.category == "provider_unavailable";
                let refusal_message = match (plan.delay_ms, recovery_delivery.as_ref()) {
                    // A truncated Run ends with the Host's own honest account of
                    // what its durable facts say — never a completion claim and
                    // never a fabricated model reply.
                    (None, _) if is_length_truncation(length_class) => {
                        self.length_truncated_terminal_message(
                            recovery_delivery.as_ref(),
                            facts.retry.length_recovery_attempts
                                >= MODEL_LENGTH_RECOVERY_MAX_ATTEMPTS,
                        )
                    }
                    _ => plan.refusal.to_owned(),
                };
                let admission = match plan.delay_ms {
                    // The controlled recovery changes the request itself, so it is
                    // admitted separately from every re-send lane.
                    Some(_) if plan.needs_recovery_record() => {
                        let delivery = recovery_delivery
                            .as_ref()
                            .ok_or_else(|| KernelError::FailClosed(
                                "length recovery has no delivery facts".into(),
                            ))?;
                        let lever = recovery_lever
                            .as_ref()
                            .ok_or_else(|| KernelError::FailClosed(
                                "length recovery has no capability decision".into(),
                            ))?;
                        let record = self
                            .length_recovery_record(
                                &failure,
                                plan.class,
                                effect_key,
                                owner,
                                &turn_id,
                                cursor,
                                now.wall_ms,
                                now.wall_ms.saturating_add(plan.delay_ms.unwrap_or(0)),
                                delivery,
                                lever,
                            )
                            .map_err(KernelError::FailClosed)?;
                        kernel::ModelRetryAdmission::RecoverLength {
                            class: plan.class.to_owned(),
                            record_json: record,
                        }
                    }
                    Some(_) => kernel::ModelRetryAdmission::Retry {
                        class: plan.class.to_owned(),
                    },
                    None => kernel::ModelRetryAdmission::Terminal {
                        class: plan.class.to_owned(),
                        message: refusal_message,
                    },
                };
                controller.schedule_model_retry_admitted(
                    now.monotonic_ms,
                    now.wall_ms,
                    effect_key,
                    &failure_json,
                    provider,
                    plan.delay_ms.unwrap_or(0),
                    admission,
                )
            },
        )
    }

    pub(crate) fn dispatch_initial(
        &self,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelInitialModelFrame,
            &kernel::CancellationToken,
        ) -> Result<fox_engine_protocol::KernelInitialModelResponse, String>,
    ) -> Result<(), String> {
        self.dispatch_initial_request(owner,policy,None,deliver)
    }

    fn dispatch_initial_request(
        &self,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        continuation_key: Option<&str>,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelInitialModelFrame,
            &kernel::CancellationToken,
        ) -> Result<fox_engine_protocol::KernelInitialModelResponse, String>,
    ) -> Result<(), String> {
        self.tick()?;
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let (mut input,continuation_lane,frozen_notices)=if let Some(key)=continuation_key {
            self.database.kernel_continuation_input_with_lane(&self.binding.run_id,key)?
        } else {(self.database.kernel_initial_input(&self.binding.run_id)?,None,Vec::new())};
        // Same steering boundary as the live transport: collect all active
        // rows into this frozen initial dispatch before it is armed.
        let dispatch_key=continuation_key.map(|key|format!("continuation-delivery:{key}"))
            .unwrap_or_else(||kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into());
        let _steering_rows=if continuation_lane.as_deref()==Some("job_notice") {Vec::new()} else {
            self.database.deliver_steering_for_dispatch(&self.binding.run_id,&dispatch_key)?
        };
        input.messages=self.database.kernel_bound_initial_frame_messages(
            &self.binding.run_id,continuation_key,&dispatch_key)?;
        // A retried initial/continuation request may carry the Host's retry
        // instruction — the one controlled length recovery's recorded text, or the
        // ordinary completion prompt — but only when this Run has no typed
        // Host-job-notice lane: that lane's commit validates the frame's messages
        // against the durable frozen sources, so appending a Host message there
        // would fail closed. Without the lane the append is the same Host-owned
        // model view the batch retry already uses.
        let notice_lane = self.database.compute_job_notice_enabled(&self.binding.run_id)?;
        if !notice_lane {
            let retry_target = continuation_key.unwrap_or("initial");
            input.messages = self.model_retry_context(retry_target, input.messages.clone())?;
        }
        let notice_mode = notice_lane;
        let historical_notice_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&input.messages)?;
        let new_notices = self.pending_host_job_notices(&input.messages)?;
        if continuation_lane.as_deref()==Some("job_notice") && new_notices!=frozen_notices {
            return Err("job_notice facts changed after durable continuation".into());
        }
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        let frame = {
            let mut guard = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?;
            if self
                .database
                .run_control_binding(&self.binding.run_id)?
                .as_ref()
                != Some(&self.binding)
            {
                return Err("initial control binding changed".into());
            }
            let frame = fox_engine_protocol::KernelInitialModelFrame {
                schema_version: 1,
                input,
                idempotency_key: dispatch_key.clone(),
                continuation_key: continuation_key.map(str::to_owned),
                continuation_lane: continuation_lane.clone(),
                checkpoint_seq: guard.last_event_seq(),
                host_job_notices: new_notices,
            };
            frame.validate()?;
            let payload = serde_json::to_value(&frame).map_err(|_|"invalid initial model input")?;
            let mut delivered_history = frame.input.messages.clone();
            delivered_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
            let notice_binding = crate::database::ModelNoticeInput {
                payload: &payload, delivered_history: &delivered_history,
                history_start: frame.input.messages.len(),
                historical_bytes: historical_notice_bytes, live_history: None,
                checkpoint_seq: frame.checkpoint_seq,
            };
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = if let Some(key)=continuation_key {
                candidate.begin_continuation_model_request(key,now.monotonic_ms,now.wall_ms)
            } else { candidate.begin_initial_model_request(now.monotonic_ms,now.wall_ms) }
                .map_err(|error| error.to_string())?;
            #[cfg(test)]
            notice_lease_test_barrier::fire(&self.binding.run_id);
            if let Some(key)=continuation_key {
                self.database.kernel_commit_continuation_model(&self.binding.run_id,now.wall_ms,
                    &candidate.persist_command(&effects),key,owner,false,None,
                    notice_mode.then_some(&notice_binding))?;
            } else {
                self.database.kernel_commit_initial_model(&self.binding.run_id,now.wall_ms,
                    &candidate.persist_command(&effects),owner,false,None,
                    notice_mode.then_some(&notice_binding))?;
            }
            *guard = candidate;
            frame
        };
        token.check()?;
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => {
                return self.retry_settled_model(
                    continuation_key.unwrap_or(kernel::INITIAL_MODEL_EFFECT_KEY),
                    owner,
                    frame.checkpoint_seq,
                    error,
                )
            }
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.input.turn_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("initial response belongs to another request".into());
        }
        self.tick_settled_model()?;
        if self.model_request_window_expired()? {
            return self.retry_settled_model(continuation_key.unwrap_or(kernel::INITIAL_MODEL_EFFECT_KEY), owner, frame.checkpoint_seq,
                super::kernel_model_worker::MODEL_WINDOW_EXPIRED.into());
        }
        token.check()?;
        let encoded = serde_json::to_string(&response).map_err(|_| "invalid initial response")?;
        let mut initial_history = frame.input.messages.clone();
        initial_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1,
                batch_id: format!(
                    "initial-batch:{}:{}",
                    self.binding.run_id, frame.checkpoint_seq
                ),
                history: initial_history.clone(),
                assistant_message: response.assistant_message.clone(),
            };
            checkpoint.validate()?;
            Some(checkpoint)
        } else { None };
        // A non-tool stop may not abandon input the Host already accepted: when
        // the user's additional request arrived while this round was frozen, the
        // round is answered by its own `steering` lane instead of completing.
        // The next request carries everything this round saw — the frozen
        // initial history plus this assistant turn — never a rebuild from the
        // Run's initial input, which would drop the work already done. The reply
        // is appended by the helper, exactly once.
        let steering_input = if next.is_none() {
            steering::steering_followup_input(
                &self.database,
                &self.binding,
                initial_history.clone(),
                response.assistant_message.clone(),
            )?
        } else {
            None
        };
        let mut wait_jobs = if next.is_none() && steering_input.is_none() {
            self.unfinished_compute_wait_facts()?
        } else { Vec::new() };
        let notice_followup = if next.is_none() && steering_input.is_none() && wait_jobs.is_empty() {
            self.job_notice_followup_input(&initial_history,&response.assistant_message)?
        } else { None };
        let parked_history = serde_json::to_string(&initial_history)
            .map_err(|_| "invalid parked initial history")?;
        // A request accepted between the queue read above and this write-set can
        // make a **Final** decision refuse. The model result is still in hand and
        // the write-set rolled back, so the safe recovery is to answer the new
        // request in this round and commit again — never to fail the Run (or
        // leave the model request leased) over a race the user cannot see.
        //
        // One retry is enough by construction: the re-planned decision arms a
        // `steering` continuation, which is not terminal, so the guard that
        // rejected the completion cannot apply to it.
        let mut steering_input = steering_input;
        let mut notice_followup = notice_followup;
        let mut attempts = 0u8;
        // A round that finishes the Run stages its delivery verdicts before the
        // completing decision commits, and finalizes them after — the same
        // two-phase write the live loop runs. See `stage_completion_delivery`.
        let mut completion: Option<CompletionDelivery> = None;
        loop {
            #[cfg(test)]
            live::test_barrier::fire(&self.binding.run_id);
            // A refused attempt's stage was never authorized by a commit: drop it
            // before this attempt stages its own verdicts.
            if let Some(refused) = completion.take() {
                self.abandon_completion_delivery(Some(&refused))?;
            }
            if next.is_none()
                && steering_input.is_none()
                && wait_jobs.is_empty()
                && notice_followup.is_none()
            {
                completion = Some(self.stage_completion_delivery(
                    &initial_history,
                    &response.assistant_message,
                )?);
            }
            let lease=continuation_key.map(|key|DecisionLease::Continuation(key,owner))
                .unwrap_or(DecisionLease::Initial(owner));
            let outcome = self.apply_decision(
                Some(lease),
                |controller, now| {
                let mut effects=vec![if continuation_key.is_some() {
                    controller.record_continuation_model_response(&encoded)?
                } else { controller.record_initial_model_response(&encoded)? }];
                if let Some(checkpoint) = next.clone() {
                    let value = serde_json::to_value(&checkpoint).map_err(|_| KernelError::FailClosed("invalid initial checkpoint".into()))?;
                    let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                    let calls = checkpoint.assistant_message["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|block| block["type"] == "toolCall")
                        .enumerate()
                        .map(|(source_order, call)| ToolCallRequest {
                            tool_call_id: call["id"].as_str().unwrap().into(),
                            tool: call["name"].as_str().unwrap().into(),
                            canonical_input_json: call["arguments"].to_string(),
                            source_order,
                        })
                        .collect();
                    effects.extend(controller.propose_tool_batch(
                        &checkpoint.batch_id,
                        calls,
                        policy,
                        now.monotonic_ms,
                        now.wall_ms,
                    )?);
                    effects.push(
                        controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?,
                    );
                } else if let Some(input) = &steering_input {
                    let stored = serde_json::to_string(input)
                        .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                    effects.extend(
                        controller.request_steering_followup(steering::STEERING_PROMPT, &stored)?,
                    );
                } else if !wait_jobs.is_empty() {
                    let response_seq = controller.last_event_seq();
                    effects.extend(controller.park_waiting_jobs(
                        now.monotonic_ms, now.wall_ms, self.database.kernel_data_root_id(),
                        response_seq, &encoded, &parked_history, &wait_jobs,
                    )?);
                } else if let Some((input,notices)) = &notice_followup {
                    effects.extend(controller.request_job_notice_followup(
                        &serde_json::to_string(input)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                        &serde_json::to_string(notices)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                    )?);
                } else if let Some(CompletionDelivery::Repair { prompt, input, .. }) = &completion {
                    // A promised artifact failed: this round owes the bounded
                    // repair round, not a completion.
                    effects.extend(controller.request_delivery_repair(
                        prompt,
                        &serde_json::to_string(input)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                    )?);
                } else {
                    effects.extend(controller.terminate(kernel::RunOutcome::Completed));
                }
                Ok(effects)
                },
                None,
                completion.as_ref().and_then(CompletionDelivery::mark),
            );
            match outcome {
                Ok(()) => {
                    self.finish_completion_delivery(completion.as_ref())?;
                    return Ok(());
                }

                Err(error) if error.starts_with(kernel::JOB_NOTICE_COMPETITION)
                    && attempts < live::STEERING_COMPETITION_ATTEMPTS => {
                    attempts+=1;
                    steering_input=steering::steering_followup_input(&self.database,&self.binding,
                        initial_history.clone(),response.assistant_message.clone())?;
                    if steering_input.is_some() {
                        wait_jobs.clear();
                        notice_followup=None;
                    } else {
                        wait_jobs=self.unfinished_compute_wait_facts()?;
                        notice_followup=if wait_jobs.is_empty() {
                            self.job_notice_followup_input(&initial_history,&response.assistant_message)?
                        } else {None};
                    }
                }
                Err(error)
                    if error.starts_with(kernel::STEERING_COMPETITION)
                        && attempts < live::STEERING_COMPETITION_ATTEMPTS =>
                {
                    attempts += 1;
                    if steering_input.is_none() {
                        steering_input = steering::steering_followup_input(
                            &self.database,
                            &self.binding,
                            initial_history.clone(),
                            response.assistant_message.clone(),
                        )?;
                    }
                    notice_followup = None;
                    if steering_input.is_none() {
                        // The queue emptied again (an explicit cancel raced us):
                        // there is nothing to plan for, so this is a real refusal.
                        return Err(replan_or_error(error));
                    }
                }
                Err(error) => return Err(replan_or_error(error)),
            }
        }
    }

    /// Only the durable snapshot can configure a formal worker dispatch.
    pub(super) fn dispatch_stored_batch_with_worker(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, api_key: &str,
    ) -> Result<(), String> {
        let prepare_attempt = uuid::Uuid::new_v4().to_string();
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_started", observed_at,
        );
        // The one controlled length recovery changes the request itself, so the
        // derived configuration is what this dispatch must actually send. The
        // frame built below carries the matching recorded instruction.
        let (config, _) = self.dispatch_model_config(&kernel::batch_delivery_effect_key(batch_id))?;
        let frame = self.prepare_stored_batch_resume(batch_id)?;
        let extra = crate::kernel_compaction::bytes(&frame.assistant_message)?
            .saturating_add(crate::kernel_compaction::bytes(&frame.tools)?)
            .saturating_add(4096);
        self.ensure_context_with_worker(batch_id, extra, owner, runtime, api_key)?;
        let observed_at = crate::database::now_ms();
        let _ = self.database.record_host_stage_point(
            &self.binding.run_id, &prepare_attempt, "context_prepare_finished", observed_at,
        );
        self.dispatch_batch_with_worker_with_attempt(batch_id, owner, policy, runtime, &config, api_key, &prepare_attempt)
    }

    #[cfg(test)]
    fn dispatch_batch_with_worker(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, config: &super::kernel_model_worker::KernelModelConfig,
        api_key: &str,
    ) -> Result<(), String> {
        self.dispatch_batch_with_worker_with_attempt(
            batch_id, owner, policy, runtime, config, api_key, &uuid::Uuid::new_v4().to_string(),
        )
    }

    /// Internal transport, with configuration checked before claiming.
    fn dispatch_batch_with_worker_with_attempt(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, config: &super::kernel_model_worker::KernelModelConfig,
        api_key: &str, prepare_attempt: &str,
    ) -> Result<(), String> {
        let stored = self.database.kernel_rehydrate(&self.binding.run_id)?
            .ok_or("Kernel Run is missing")?;
        // The identity guard is narrowed, never removed: a dispatch may carry the
        // frozen configuration, or — only for the one controlled length recovery —
        // the configuration re-derived from the frozen one plus the parameters the
        // durable record authorizes. `derive_length_recovery_config` recomputes
        // that derivation, so a stale or tampered record cannot smuggle in a
        // different provider request.
        let actual = config.hash()?;
        let expected = self.expected_dispatch_config_hash(
            &kernel::batch_delivery_effect_key(batch_id),
            &stored.config.prompt_config_hash,
        )?;
        if actual != expected
            || config.execution_profile_id != self.binding.execution_profile_id {
            return Err("Kernel model configuration differs from the frozen Run".into());
        }
        self.dispatch_batch(batch_id, owner, policy, |binding, frame, token| {
            let now = self.clock.read();
            let facts = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
                .shadow_checkpoint(now.monotonic_ms);
            let since = facts
                .model_request_since_wall_ms
                .ok_or("Kernel model request has no durable deadline")?;
            if now.wall_ms < since {
                return Err("Kernel model clock moved backwards".into());
            }
            let remaining = binding.budgets.limit_operation_ms(
                binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)),
                facts.running_elapsed_ms,
            );
            super::kernel_model_worker::deliver_with_preview(
                runtime,
                config,
                api_key,
                binding,
                frame,
                token,
                remaining,
                self.preview,
                Some(self.database),
                Some(&prepare_attempt),
            )
        })
    }

    /// Deliver a committed barrier, then atomically persist the original model
    /// response with its terminal or next-batch decision and lease completion.
    pub(crate) fn dispatch_batch(
        &self, batch_id: &str, owner: &str,
        policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelBatchResumeFrame,
            &kernel::CancellationToken,
        ) -> Result<fox_engine_protocol::KernelModelResponse, String>,
    ) -> Result<(), String> {
        let mut frame = self.prepare_stored_batch_resume(batch_id)?;
        let notice_mode = self.database.compute_job_notice_enabled(&self.binding.run_id)?;
        let historical_notice_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&frame.history)?;
        frame.host_job_notices = self.pending_host_job_notices(&frame.history)?;
        frame.validate()?;
        let notice_payload = serde_json::to_value(&frame).map_err(|_|"invalid batch model input")?;
        let mut delivered_history=bound_batch_live_history(&self.binding.run_id,&frame.history,
            &frame.assistant_message,&frame.tools,&frame.steering);
        let history_start = delivered_history.len();
        delivered_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let notice_binding = crate::database::ModelNoticeInput {
            payload: &notice_payload, delivered_history: &delivered_history,
            history_start,
            historical_bytes: historical_notice_bytes, live_history: None,
            checkpoint_seq: frame.checkpoint_seq,
        };
        self.database.kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        {
            let mut guard = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?;
            if guard.last_event_seq() != frame.checkpoint_seq
                || self.database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding) {
                return Err("Kernel changed before batch dispatch; rebuild the frame".into());
            }
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = candidate.begin_batch_model_request(batch_id, now.monotonic_ms, now.wall_ms)
                .map_err(|error| error.to_string())?;
            self.database.kernel_commit_batch_dispatch(
                &self.binding.run_id,
                now.wall_ms,
                &candidate.persist_command(&effects),
                batch_id,
                owner,
                notice_mode.then_some(&notice_binding),
            )?;
            *guard = candidate;
        }
        token.check()?;
        // No lock is held across engine I/O. Uncertain failures retain the lease.
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => {
                return self.retry_settled_model(
                    &kernel::batch_delivery_effect_key(batch_id),
                    owner,
                    frame.checkpoint_seq,
                    error,
                )
            }
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.turn_id
            || response.batch_id != frame.batch_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("model response belongs to another batch delivery".into());
        }
        self.tick_settled_model()?;
        if self.model_request_window_expired()? {
            return self.retry_settled_model(&kernel::batch_delivery_effect_key(batch_id), owner, frame.checkpoint_seq,
                super::kernel_model_worker::MODEL_WINDOW_EXPIRED.into());
        }
        token.check()?;
        let response_json = serde_json::to_string(&response).map_err(|error| error.to_string())?;
        // Node supplies only the new assistant message. Preserve the original
        // history and reconstruct prior results exclusively from durable facts.
        // The batch's own history is needed for both branches: the next tool
        // proposal and the steering follow-up must see the same rounds.
        let mut batch_history=bound_batch_live_history(&self.binding.run_id,&frame.history,
            &frame.assistant_message,&frame.tools,&frame.steering);
        batch_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1, batch_id: format!("model-batch:{}:{}", self.binding.run_id, frame.checkpoint_seq),
                history: batch_history.clone(), assistant_message: response.assistant_message.clone(),
            };
            checkpoint.validate()?;
            Some(checkpoint)
        } else {
            None
        };
        // Same guarantee as the live round loop: accepted-but-unanswered user
        // input is answered by its own lane before the Run may complete, and the
        // follow-up round keeps the settled tool calls and results this round
        // produced instead of restarting from the Run's initial input. The reply
        // is appended by the helper, exactly once, after the batch history.
        let steering_input = if next.is_none() {
            steering::steering_followup_input(
                &self.database,
                &self.binding,
                batch_history.clone(),
                response.assistant_message.clone(),
            )?
        } else {
            None
        };
        let mut wait_jobs = if next.is_none() && steering_input.is_none() {
            self.unfinished_compute_wait_facts()?
        } else { Vec::new() };
        let notice_followup = if next.is_none() && steering_input.is_none() && wait_jobs.is_empty() {
            self.job_notice_followup_input(&batch_history,&response.assistant_message)?
        } else { None };
        let parked_history = serde_json::to_string(&batch_history)
            .map_err(|_| "invalid parked batch history")?;
        // Same recovery as the initial path: a Final decision that loses a race
        // with a freshly accepted request is re-planned into its own steering
        // round in this call, with the reply the Host already holds.
        let mut steering_input = steering_input;
        let mut notice_followup = notice_followup;
        let mut attempts = 0u8;
        // A round that finishes the Run stages its delivery verdicts before the
        // completing decision commits, and finalizes them after — the same
        // two-phase write the live loop runs. See `stage_completion_delivery`.
        let mut completion: Option<CompletionDelivery> = None;
        loop {
            #[cfg(test)]
            live::test_barrier::fire(&self.binding.run_id);
            // A refused attempt's stage was never authorized by a commit: drop it
            // before this attempt stages its own verdicts.
            if let Some(refused) = completion.take() {
                self.abandon_completion_delivery(Some(&refused))?;
            }
            if next.is_none()
                && steering_input.is_none()
                && wait_jobs.is_empty()
                && notice_followup.is_none()
            {
                completion = Some(self.stage_completion_delivery(
                    &batch_history,
                    &response.assistant_message,
                )?);
            }
            let outcome = self.apply_decision(
                Some(DecisionLease::Batch(batch_id, owner)),
                |controller, now| {
                    let mut effects =
                        vec![controller.record_batch_model_response(batch_id, &response_json)?];
                    if let Some(checkpoint) = next.clone() {
                        let value = serde_json::to_value(&checkpoint)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                        let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                        let calls = checkpoint.assistant_message["content"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|block| block["type"] == "toolCall")
                            .enumerate()
                            .map(|(source_order, call)| ToolCallRequest {
                                tool_call_id: call["id"].as_str().unwrap().into(),
                                tool: call["name"].as_str().unwrap().into(),
                                canonical_input_json: call["arguments"].to_string(),
                                source_order,
                            })
                            .collect();
                        effects.extend(controller.propose_tool_batch(
                            &checkpoint.batch_id,
                            calls,
                            policy,
                            now.monotonic_ms,
                            now.wall_ms,
                        )?);
                        effects.push(
                            controller
                                .checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?,
                        );
                    } else if let Some(input) = &steering_input {
                        let stored = serde_json::to_string(input)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                        effects.extend(controller.request_steering_followup(
                            steering::STEERING_PROMPT,
                            &stored,
                        )?);
                    } else if !wait_jobs.is_empty() {
                        let response_seq = controller.last_event_seq();
                        effects.extend(controller.park_waiting_jobs(
                            now.monotonic_ms, now.wall_ms, self.database.kernel_data_root_id(),
                            response_seq, &response_json, &parked_history, &wait_jobs,
                        )?);
                    } else if let Some((input,notices)) = &notice_followup {
                        effects.extend(controller.request_job_notice_followup(
                            &serde_json::to_string(input)
                                .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                            &serde_json::to_string(notices)
                                .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                        )?);
                    } else if let Some(CompletionDelivery::Repair { prompt, input, .. }) = &completion {
                        // A promised artifact failed: this round owes the bounded
                        // repair round, not a completion.
                        effects.extend(controller.request_delivery_repair(
                            prompt,
                            &serde_json::to_string(input)
                                .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                        )?);
                    } else {
                        effects.extend(controller.terminate(kernel::RunOutcome::Completed));
                    }
                    Ok(effects)
                },
                None,
                completion.as_ref().and_then(CompletionDelivery::mark),
            );
            match outcome {
                Ok(()) => {
                    self.finish_completion_delivery(completion.as_ref())?;
                    return Ok(());
                }

                Err(error) if error.starts_with(kernel::JOB_NOTICE_COMPETITION)
                    && attempts < live::STEERING_COMPETITION_ATTEMPTS => {
                    attempts+=1;
                    steering_input=steering::steering_followup_input(&self.database,&self.binding,
                        batch_history.clone(),response.assistant_message.clone())?;
                    if steering_input.is_some() {
                        wait_jobs.clear();
                        notice_followup=None;
                    } else {
                        wait_jobs=self.unfinished_compute_wait_facts()?;
                        notice_followup=if wait_jobs.is_empty() {
                            self.job_notice_followup_input(&batch_history,&response.assistant_message)?
                        } else {None};
                    }
                }
                Err(error)
                    if error.starts_with(kernel::STEERING_COMPETITION)
                        && attempts < live::STEERING_COMPETITION_ATTEMPTS =>
                {
                    attempts += 1;
                    if steering_input.is_none() {
                        steering_input = steering::steering_followup_input(
                            &self.database,
                            &self.binding,
                            batch_history.clone(),
                            response.assistant_message.clone(),
                        )?;
                    }
                    notice_followup = None;
                    if steering_input.is_none() {
                        return Err(replan_or_error(error));
                    }
                }
                Err(error) => return Err(replan_or_error(error)),
            }
        }
    }

    /// Lease one durable dispatch and verify it still matches the approved
    /// tool identity. `None` means the call is not dispatchable right now
    /// (unknown tool, non-running/terminal state, or a lost pending CAS); the
    /// caller retries through the durable loop instead of replaying it.
    fn lease_dispatch_effect(
        &self,
        tool_call_id: &str,
        owner: &str,
    ) -> Result<Option<kernel::OutboxEffect>, String> {
        let effect_key = kernel::dispatch_effect_key(tool_call_id);
        let guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let data = guard.shadow_checkpoint(self.clock.now_monotonic_ms());
        let tool = data
            .tools
            .iter()
            .find(|tool| tool.tool_call_id == tool_call_id)
            .ok_or("unknown Kernel tool")?;
        if data.state.is_terminal()
            || data.state == kernel::RunState::Cancelling
            || tool.state != kernel::ToolCallState::Running
        {
            return Ok(None);
        }
        let Some(effect) = self.database.kernel_outbox_lease_effect(
            &self.binding.run_id,
            &effect_key,
            owner,
        )?
        else {
            return Ok(None);
        };
        let payload: Value =
            serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
        let expected_input: Value =
            serde_json::from_str(&tool.input_json).map_err(|error| error.to_string())?;
        if effect.kind != kernel::OutboxEffectKind::DispatchTool
            || effect.tool_call_id.as_deref() != Some(tool_call_id)
            || effect.idempotency_key != kernel::dispatch_idempotency_key(&self.binding.run_id, tool_call_id)
            || payload.get("tool").and_then(Value::as_str) != Some(tool.tool.as_str())
            || payload.get("input") != Some(&expected_input)
        {
            return Err("durable dispatch differs from the approved tool identity".into());
        }
        Ok(Some(effect))
    }

    /// Commit one certain tool result inside its own CAS-guarded decision.
    fn settle_dispatch_result(
        &self,
        tool_call_id: &str,
        owner: &str,
        succeeded: bool,
        result: &Value,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(
            Some(DecisionLease::Tool(tool_call_id, owner)),
            |controller, _| {
                controller.tool_settled(tool_call_id, succeeded, &result.to_string())
            },
        )
    }

    /// Dispatch exactly one durable intent. Err after lease is uncertain and is
    /// deliberately left leased; neither this method nor reopen retries it.
    /// The callback runs outside the controller lock, permitting cancellation and
    /// parallel independent calls. Results are committed in source-order batches.
    pub(crate) fn dispatch_tool(
        &self,
        tool_call_id: &str,
        owner: &str,
        execute: impl FnOnce(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>,
    ) -> Result<bool, String> {
        self.tick()?;
        let Some(leased) = self.lease_dispatch_effect(tool_call_id, owner)? else {
            return Ok(false);
        };
        let token = self
            .cancellation
            .tool_token(&self.binding.run_id, tool_call_id)?;
        token.check()?;
        let (succeeded, result) = execute(&self.binding, &leased, &token)?;
        self.settle_dispatch_result(tool_call_id, owner, succeeded, &result)?;
        Ok(true)
    }

    /// Run a Host-classified group of independent, read-only tool calls with
    /// bounded concurrency, then settle every certain result in source order.
    ///
    /// Safety/durability shape:
    ///  * each call acquires its own `pending → leased` CAS under its own
    ///    idempotency key, so no call is executed twice and a crashed owner's
    ///    lease is never stolen here;
    ///  * worker threads touch only owned copies of the frozen binding, the
    ///    leased effect and a per-tool cancellation token — never coordinator
    ///    or database state. All lease/settle decisions stay on this thread;
    ///  * each thread's result is persisted as its own decision in source
    ///    order, regardless of which thread finished first;
    ///  * an `Err`/panic means uncertain execution: that effect stays leased
    ///    (never replayed), the other completed reads still settle, and the
    ///    error propagates exactly as on the serial path so the live loop's
    ///    leased-effect guard detaches the session;
    ///  * cancellation is per tool (`tool_token`); a user cancel trips every
    ///    in-flight token, and the pre-execution check prevents starting any
    ///    remaining call.
    pub(crate) fn dispatch_read_only_group(
        &self,
        effects: &[kernel::OutboxEffect],
        owner: &str,
        execute: &(dyn Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
             + Sync),
    ) -> Result<(), String> {
        if effects.is_empty() {
            return Ok(());
        }
        if effects.len() == 1 {
            let tool_call_id = effects[0]
                .tool_call_id
                .clone()
                .ok_or("missing dispatch tool identity")?;
            return self
                .dispatch_tool(&tool_call_id, owner, |binding, leased, token| {
                    execute(binding, leased, token)
                })
                .map(|_| ());
        }
        self.tick()?;
        // Lease phase: every chosen call gets its own durable CAS before any of
        // them executes. A call that is no longer dispatchable ends the group;
        // what was already leased still has to run (its lease cannot dangle).
        let mut leased: Vec<(String, kernel::OutboxEffect, kernel::CancellationToken)> =
            Vec::with_capacity(effects.len());
        for effect in effects {
            let tool_call_id = effect
                .tool_call_id
                .clone()
                .ok_or("missing dispatch tool identity")?;
            let Some(leased_effect) = self.lease_dispatch_effect(&tool_call_id, owner)? else {
                break;
            };
            let token = self
                .cancellation
                .tool_token(&self.binding.run_id, &tool_call_id)?;
            token.check()?;
            leased.push((tool_call_id, leased_effect, token));
        }
        // Execution phase: owned per-thread copies keep the Send requirements
        // independent of coordinator internals (the clock trait object etc.).
        let jobs: Vec<(RunControlBinding, kernel::OutboxEffect, kernel::CancellationToken)> = leased
            .iter()
            .map(|(_tool_call_id, effect, token)| {
                (self.binding.clone(), effect.clone(), token.clone())
            })
            .collect();
        let outcomes: Vec<Result<(bool, Value), String>> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(jobs.len());
            for (binding, effect, token) in jobs {
                handles.push(scope.spawn(move || execute(&binding, &effect, &token)));
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        // A panicking reader has no durable result: treat it as
                        // uncertain execution; the leased effect is not settled.
                        Err("parallel read-only tool execution panicked without a durable result"
                            .to_owned())
                    })
                })
                .collect()
        });
        // Settlement phase: source order, one CAS-guarded decision per tool.
        let mut uncertain: Option<String> = None;
        for ((tool_call_id, _effect, _token), outcome) in leased.into_iter().zip(outcomes) {
            match outcome {
                Ok((succeeded, result)) => {
                    self.settle_dispatch_result(&tool_call_id, owner, succeeded, &result)?;
                }
                Err(error) => {
                    // This effect stays leased; keep the first failure to
                    // propagate after the other certain results settle.
                    uncertain.get_or_insert(error);
                }
            }
        }
        if let Some(error) = uncertain {
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

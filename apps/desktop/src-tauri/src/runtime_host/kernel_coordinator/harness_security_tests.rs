//! Role F independent negative cases: S01 authorization/scope boundaries and
//! B04 bounded result views + historical read-back honesty.
//!
//! O-REVIEW-01 / O-F-01 restructure. Every case is tagged in its name so a
//! defect reproduction can never be read as a security pass:
//!
//!   `_acc_`   — acceptance: verifies the frozen CONTRACTS v1.2 behaviour. A
//!               pass means the contract holds at this layer.
//!   `_gate_`  — acceptance gate: verifies the post-fix contract that the
//!               owning role (A) must satisfy. These FAIL on the current
//!               implementation by design; they are the flip condition for the
//!               defect, not a green check.
//!   `_baseline_` (#[ignore]) — reproduces the CURRENT defective behaviour.
//!               Run with `cargo test -- --ignored`. A pass means the defect is
//!               still present; it is evidence for the owner, never a pass on
//!               safety.
//!
//! Scope rules (see OWNERSHIP F-REGRESSION-01):
//!  - This module only calls public Host entry points (`tool_host::prepare` /
//!    `execute`, `tool_result_read::prepare`) and asserts observable behaviour.
//!  - Implementation defects are reported back to the owning role via F-REQ;
//!    they are NOT fixed here.
//!  - Fixtures are synthetic canaries inside an isolated temp directory. No
//!    user secrets, no real credentials, no network egress.
//!
//! Evidence tier: contract / pure-logic + Rust directed regression against the
//! real Host functions. Not: real-provider, GUI, or full desktop integration.

use crate::tool_host;
use crate::runtime_host::tool_result_read;
use serde_json::{json, Value};
use std::path::PathBuf;

/// A synthetic canary the untrusted path must never be able to reach.
const OUTSIDE_CANARY: &str = "OUTSIDE-CANARY-DATA";

/// A marker a fixture command writes to stdout only, so a non-zero exit that
/// loses stdout diagnostics can be detected.
const STDOUT_MARKER: &str = "STDOUT-MARKER";

/// Every argument `read_tool_result` must refuse, because honouring it would
/// let the model choose its own authorization scope.
const FORBIDDEN_READ_ARGS: &[&str] = &[
    "conversationId",
    "conversation",
    "authorizedConversationId",
    "runId",
    "toolCallId",
];

// ---------------------------------------------------------------------------
// B04 — `read_tool_result` argument boundary (acceptance)
// ---------------------------------------------------------------------------

#[test]
fn b04_acc_read_tool_result_refuses_every_scope_choosing_argument() {
    // Structural refusal only: per CONTRACTS v1.2 the failure may surface as
    // Err or as a structured business failure, and no message wording is
    // frozen, so we assert the refusal itself, not an English string.
    for &field in FORBIDDEN_READ_ARGS {
        let input = json!({ "reference": "fox-result://run-1/call-1", field: "attacker-supplied" });
        assert!(
            tool_result_read::prepare(&input).is_err(),
            "{field}: honouring it would let the model choose its own authorization scope"
        );
    }
}

#[test]
fn b04_acc_read_tool_result_rejects_malformed_and_foreign_references() {
    // Not a Fox reference at all.
    assert!(tool_result_read::prepare(&json!({ "reference": "not-a-reference" })).is_err());
    // A bare URL must not be accepted as an authorization fact.
    assert!(tool_result_read::prepare(&json!({ "reference": "https://example.com/leak" })).is_err());
    // A file path is not a result reference.
    assert!(tool_result_read::prepare(&json!({ "reference": "/etc/passwd" })).is_err());
    // Empty / missing reference.
    assert!(tool_result_read::prepare(&json!({ "reference": "" })).is_err());
    assert!(tool_result_read::prepare(&json!({})).is_err());
}

#[test]
fn b04_acc_read_tool_result_accepts_only_byte_range_arguments() {
    // Well-formed input resolves; offset/limit are the only numeric knobs.
    let request = tool_result_read::prepare(&json!({
        "reference": "fox-result://run-1/call-1",
        "offset": 100,
        "limit": 2048,
    }))
    .expect("a well-formed reference with a byte range must resolve");
    assert_eq!(request.reference, "fox-result://run-1/call-1");
    assert_eq!(request.offset, 100);
    assert_eq!(request.limit, 2048);

    // A non-numeric offset is refused rather than silently coerced to 0.
    assert!(tool_result_read::prepare(&json!({
        "reference": "fox-result://run-1/call-1",
        "offset": "big",
    }))
    .is_err());
    assert!(tool_result_read::prepare(&json!({
        "reference": "fox-result://run-1/call-1",
        "limit": -1,
    }))
    .is_err());
}

#[test]
fn b04_acc_read_tool_result_clamps_one_range_and_defaults_the_rest() {
    let default = tool_result_read::prepare(&json!({ "reference": "fox-result://run-1/call-1" }))
        .expect("reference alone must resolve");
    assert_eq!(default.offset, 0);
    assert_eq!(default.limit, tool_result_read::DEFAULT_LIMIT_BYTES);

    let huge = tool_result_read::prepare(&json!({
        "reference": "fox-result://run-1/call-1",
        "limit": 1_000_000_000,
    }))
    .expect("an oversized limit is clamped, not rejected");
    assert_eq!(huge.limit, tool_result_read::MAX_LIMIT_BYTES);
}

// ---------------------------------------------------------------------------
// S01 — a Host-executed command inherits the full parent environment
// ---------------------------------------------------------------------------

/// Evidence for the S01 boundary (not a fix). `tool_host` spawns the child with
/// a bare `Command::new` and never filters the environment, so any variable
/// present in the Fox process is readable by a tool the model requested. The
/// command runs inside the authorized project root and still sees the canary:
/// cwd, approval and cancellation are therefore not an OS sandbox.
#[cfg(windows)]
#[test]
fn s01_evidence_command_child_inherits_parent_environment_canary() {
    let dir = isolated_dir("s01-env");
    // A synthetic secret that exists only for this test.
    std::env::set_var("FOX_HARNESS_S1_CANARY", OUTSIDE_CANARY);

    let action = tool_host::prepare(
        "run_command",
        &json!({ "command": "echo %FOX_HARNESS_S1_CANARY%" }),
        dir.to_str().unwrap(),
    )
    .expect("run_command inside the authorized root must prepare");
    assert_eq!(tool_host::PreparedToolAction::preview(&action).tool, "run_command");

    let result = tool_host::execute(action);
    std::env::remove_var("FOX_HARNESS_S1_CANARY");

    let result = result.expect("echo must exit zero");
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        text.contains(OUTSIDE_CANARY),
        "the child inherited the parent environment: a model-requested command inside the authorized root saw the canary; got: {text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The same inheritance holds on non-Windows shells, so this is not a cmd.exe
/// quirk. Runs only where the platform shell is `sh -lc`.
#[cfg(not(windows))]
#[test]
fn s01_evidence_command_child_inherits_environment_nonwindows() {
    let dir = isolated_dir("s01-env2");
    std::env::set_var("FOX_HARNESS_S1_CANARY", OUTSIDE_CANARY);

    let action = tool_host::prepare(
        "run_command",
        &json!({ "command": format!("printf '%s' \"${{FOX_HARNESS_S1_CANARY}}\"") }),
        dir.to_str().unwrap(),
    )
    .expect("run_command must prepare");
    let result = tool_host::execute(action);
    std::env::remove_var("FOX_HARNESS_S1_CANARY");

    let result = result.expect("printf must exit zero");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(OUTSIDE_CANARY), "posix shell also inherited the canary");
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// EX-v1 §6 — a non-zero business exit must not become a success and must not
// drop the diagnostics the model needs. Two explicit phases.
// ---------------------------------------------------------------------------

/// BASELINE: the current non-zero branch returns `Err(String)` built from
/// stderr only, so a command that wrote its output to stdout loses it.
///
/// CONTRACTS v1.2 §6 allows the post-fix form to be either `Err` or
/// `Ok(Value)` with a top-level `isError: true`; this baseline only records
/// what the code does today. Run with `--ignored`.
#[test]
#[ignore]
fn ex_baseline_nonzero_exit_currently_returns_string_err() {
    let dir = isolated_dir("ex-baseline");
    let command = stdout_marker_then_exit_3();

    let action = tool_host::prepare("run_command", &json!({ "command": command }), dir.to_str().unwrap())
        .expect("run_command must prepare");

    let error = tool_host::execute(action).expect_err("the current form is Err");

    // The current form names the exit code ...
    assert!(error.contains('3'), "current form reports the code in the message: {error}");
    // ... and drops the stdout marker (the A01 defect).
    assert!(
        !error.contains(STDOUT_MARKER),
        "baseline: stdout-only output is currently dropped from the Err string"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// ACCEPTANCE GATE for the EX-v1 §6 contract. Whichever carrier the executor
/// uses after the fix — `Err` or `Ok(Value)` — these invariants must hold:
///
/// 1. the known exit code 3 is reported (never silently 0 and never absent);
/// 2. the stdout the command produced stays visible to the model;
/// 3. no branch may read as a plain completed/success result.
///
/// This deliberately does NOT lock the assertion to the old English `Err`
/// string. Fails on the current implementation (invariant 2 is violated) —
/// that is the flip condition for A01/A02.
#[test]
fn ex_gate_nonzero_exit_reports_failure_without_losing_diagnostics() {
    let dir = isolated_dir("ex-gate");
    let command = stdout_marker_then_exit_3();

    let action = tool_host::prepare("run_command", &json!({ "command": command }), dir.to_str().unwrap())
        .expect("run_command must prepare");

    match tool_host::execute(action) {
        // Form A: structured failure delivered as an error.
        Err(message) => {
            assert!(!message.is_empty(), "a business failure must carry a diagnosis");
            assert!(
                message.contains(STDOUT_MARKER),
                "invariant 2 (stdout diagnostics visible) is violated in the Err form: {message}"
            );
            // The exit code must be identifiable; a bare "3" substring is too
            // weak alone, so the marker plus a non-empty diagnosis is the
            // minimum, and the structured Ok form below carries the exact code.
        }
        // Form B: structured business failure delivered as a value.
        Ok(value) => {
            assert_eq!(
                value["isError"].as_bool(),
                Some(true),
                "an Ok business failure must set isError=true; a plain Ok is not a success receipt: {value}"
            );
            assert_eq!(
                value["details"]["exitCode"].as_u64(),
                Some(3),
                "the known exit code must be reported, not defaulted: {value}"
            );
            let text = value["content"][0]["text"].as_str().unwrap_or_default();
            assert!(
                text.contains(STDOUT_MARKER),
                "invariant 2 (stdout diagnostics visible) is violated in the Ok form: {text}"
            );
        }
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// S01 — the write/edit boundary still refuses out-of-root targets (acceptance)
// ---------------------------------------------------------------------------

#[test]
fn s01_acc_write_refuses_a_target_outside_the_authorized_root() {
    let inside = isolated_dir("s01-w");
    let outside = isolated_dir("s01-out");

    let escape = PathBuf::from(&outside).join("escaped.txt");
    let escaped = escape.to_str().unwrap().replace('\\', "/");
    let result = tool_host::prepare(
        "write_file",
        &json!({ "path": escaped, "content": OUTSIDE_CANARY }),
        inside.to_str().unwrap(),
    );
    assert!(
        result.is_err(),
        "an absolute path outside the authorized root must be refused at prepare time"
    );
    assert!(!escape.exists(), "no byte may be written outside the root");

    std::fs::remove_dir_all(&inside).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn s01_acc_write_refuses_parent_traversal_target() {
    let inside = isolated_dir("s01-t");

    let result = tool_host::prepare(
        "write_file",
        &json!({ "path": "../traversal.txt", "content": OUTSIDE_CANARY }),
        inside.to_str().unwrap(),
    );
    assert!(result.is_err(), "`..` traversal must be refused at prepare time");

    let parent_marker = inside.parent().unwrap().join("traversal.txt");
    assert!(!parent_marker.exists(), "traversal must not have written outside the root");

    std::fs::remove_dir_all(&inside).ok();
}

#[test]
fn s01_acc_empty_path_and_empty_command_are_rejected() {
    let inside = isolated_dir("s01-e");

    assert!(tool_host::prepare("write_file", &json!({ "path": "", "content": "x" }), inside.to_str().unwrap()).is_err());
    assert!(tool_host::prepare("run_command", &json!({ "command": "" }), inside.to_str().unwrap()).is_err());

    std::fs::remove_dir_all(&inside).ok();
}

#[test]
fn s01_acc_unknown_host_tool_is_refused() {
    let inside = isolated_dir("s01-unk");
    // Structural fail-closed check; no message wording is frozen.
    assert!(
        tool_host::prepare("definitely_not_a_tool", &json!({}), inside.to_str().unwrap()).is_err(),
        "unknown tool names must fail closed"
    );
    std::fs::remove_dir_all(&inside).ok();
}

// ---------------------------------------------------------------------------
// A03 — legitimate empty content must not be rejected. Baseline + gate.
// ---------------------------------------------------------------------------

/// BASELINE: `required_string` filters any value whose trimmed form is empty
/// (`tool_host.rs:424-431`), so an empty `content` — which CONTRACTS v1.2 §1.5
/// declares legitimate — is refused as "must contain a non-empty content".
/// Run with `--ignored`. A pass means the defect is still present.
#[test]
#[ignore]
fn a03_baseline_empty_content_is_currently_rejected() {
    let inside = isolated_dir("a03-baseline");
    let result = tool_host::prepare(
        "write_file",
        &json!({ "path": "empty.txt", "content": "" }),
        inside.to_str().unwrap(),
    );
    assert!(
        result.is_err(),
        "baseline: empty content is currently rejected, contrary to CONTRACTS v1.2 §1.5"
    );
    assert!(!inside.join("empty.txt").exists(), "baseline: nothing was written");
    std::fs::remove_dir_all(&inside).ok();
}

/// ACCEPTANCE GATE for CONTRACTS v1.2 §1.5: `content` may be empty or pure
/// whitespace; only `path`/`command`/`oldText` must be non-empty. Fails on the
/// current implementation — that is the flip condition for A03.
#[test]
fn a03_gate_empty_content_is_a_valid_write() {
    let inside = isolated_dir("a03-gate");
    let target = inside.join("empty.txt");

    let action = tool_host::prepare(
        "write_file",
        &json!({ "path": target.to_str().unwrap().replace('\\', "/"), "content": "" }),
        inside.to_str().unwrap(),
    )
    .expect("an empty file is legitimate content, not an invalid request");
    let result = tool_host::execute(action).expect("empty content must write successfully");
    assert_eq!(result["details"]["bytes"].as_u64(), Some(0));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "");

    std::fs::remove_dir_all(&inside).ok();
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn isolated_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fox-harness-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A command that writes the marker to stdout only, then exits 3. On Windows
/// `echo` writes to stdout and `exit /b 3` sets the code without touching stderr.
fn stdout_marker_then_exit_3() -> String {
    if cfg!(windows) {
        format!("echo {STDOUT_MARKER} & exit /b 3")
    } else {
        format!("echo {STDOUT_MARKER}; exit 3")
    }
}

/// Keep a Value-typed anchor so a future fixture helper still compiles; the
/// canary constant is shared across the cases above.
#[allow(dead_code)]
fn canary_value() -> Value {
    json!(OUTSIDE_CANARY)
}

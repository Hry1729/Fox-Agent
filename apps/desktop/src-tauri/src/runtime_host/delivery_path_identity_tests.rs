//! Windows project-path identity and policy-denial detail: real-gateway coverage.
//!
//! The frozen project root the product stores is the *canonical* folder spelling
//! (on Windows `std::fs::canonicalize` answers in the extended-length namespace,
//! `\\?\D:\...`), while a target the model names is usually plain (`D:\...`) and
//! a target the Host resolves comes back canonical. Containment therefore has to
//! be decided in one folded spelling space: deciding it on two of those
//! spellings refused *every* ordinary write inside the project and reported the
//! refusal as an unclassified policy denial.
//!
//! The end-to-end cases drive the real chain — `GatewayPolicy::decide` → host
//! admission/claim → managed execute → durable receipt — through the same
//! `KernelCoordinator` transport the product uses, rather than a helper alone.

use super::*;
use crate::kernel::{self, CancellationRegistry, PolicyDecision, PolicyDecisionPort, TestClock};
use crate::runtime_host::{kernel_coordinator::KernelCoordinator, kernel_gateway::GatewayPolicy};
use fox_engine_protocol::{
    ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
    TimeBudgets,
};
use rusqlite::OptionalExtension;

/// A merge task that makes the Host freeze a read-only `Concat` input set: the
/// inputs are discovered from the directory, never named one by one.
const MERGE_TASK: &str = "在初始目录的 .txt 文件中，按字节数从小到大选出 2 个最小文件，\
     大小相同按文件名升序选取。再按文件名升序合并为 joined.txt。每段先写完整文件名，下一行保留全部内容；\
     两段之间恰好一个空行。不要修改原文件。";

/// The same frozen-material boundary, with an Office document as the task's own
/// input: `zeta.xlsx` is read-only material and the declared CSV/Markdown are the
/// outputs.
const OFFICE_TASK: &str = "读取 zeta.xlsx 中的数据，生成 metrics.csv，并把统计口径写入 analysis_method.md。\
     不要修改原文件。";

// ---------------------------------------------------------------------------
// One spelling space for a Windows path
// ---------------------------------------------------------------------------

#[test]
fn windows_project_path_identity_folds_every_spelling_of_one_file() {
    // The frozen root carries the extended-length prefix, exactly as the folder
    // picker stores it.
    let verbatim_root = r"\\?\D:\work\proj";
    for named in [
        r"D:\work\proj\summary.json",
        r"\\?\D:\work\proj\summary.json",
        r"D:/work/proj/summary.json",
        r"\\?\D:/work/proj/summary.json",
        r"d:\WORK\Proj\summary.json",
        r"\\?\d:\work\proj\sub\..\summary.json",
    ] {
        assert_eq!(
            project_relative_path(named, Some(verbatim_root)).as_deref(),
            Some("summary.json"),
            "{named} names one file inside the frozen project"
        );
    }
    assert_eq!(
        project_relative_path(r"D:\work\proj\sub\deep.txt", Some(verbatim_root)).as_deref(),
        Some("sub/deep.txt"),
        "the relative form keeps its directory segments in the original case"
    );
    // A share and its verbatim spelling are the same location, in both
    // directions.
    assert_eq!(
        project_relative_path(r"\\?\UNC\srv\share\proj\a.txt", Some(r"\\srv\share\proj"))
            .as_deref(),
        Some("a.txt")
    );
    assert_eq!(
        project_relative_path(r"\\srv\share\proj\a.txt", Some(r"\\?\UNC\srv\share\proj"))
            .as_deref(),
        Some("a.txt")
    );
    // POSIX spellings still resolve, and a relative name is already relative to
    // the frozen project.
    assert_eq!(
        project_relative_path("/tmp/proj/out.txt", Some("/tmp/proj")).as_deref(),
        Some("out.txt")
    );
    for relative in ["out.txt", "./out.txt"] {
        assert_eq!(
            project_relative_path(relative, Some(verbatim_root)).as_deref(),
            Some("out.txt"),
            "{relative}"
        );
    }
    // A real relative folder that happens to be called `UNC` keeps its name: the
    // verbatim-UNC marker is a namespace, not a directory.
    assert_eq!(
        project_relative_path("UNC/notes.txt", Some(verbatim_root)).as_deref(),
        Some("UNC/notes.txt")
    );
    // The equality space folds the same spellings.
    assert_eq!(
        normalize_path_text(r"\\?\UNC\Srv\Share\A"),
        normalize_path_text(r"\\srv\share\a")
    );
    assert_eq!(
        normalize_path_text(r"\\?\D:\Work\Proj\A"),
        normalize_path_text("D:/work/proj/a")
    );
}

#[test]
fn windows_project_path_identity_still_refuses_real_escapes() {
    let root = r"\\?\D:\work\proj";
    for outside in [
        r"D:\work\elsewhere\summary.json",
        // `..` is resolved, so this lands outside the project.
        r"D:\work\proj\..\summary.json",
        r"D:\work\proj\sub\..\..\summary.json",
        r"C:\work\proj\summary.json",
        r"\\srv\other\summary.json",
        // The root itself is not a managed target.
        r"D:\work\proj",
    ] {
        assert_eq!(
            project_relative_path(outside, Some(root)),
            None,
            "{outside} is not inside the frozen project"
        );
    }
    assert_eq!(
        project_relative_path(r"\\srv\share2\a.txt", Some(r"\\srv\share")),
        None,
        "a sibling share is not inside the share"
    );
    // A POSIX path stays case-sensitive, so folding cannot widen containment.
    assert_eq!(
        project_relative_path("/tmp/proj/out.txt", Some("/tmp/Proj")),
        None
    );
    assert!(path_identity(r"D:\work\proj\a.txt").within(&path_identity(r"D:\work\proj")));
    assert!(!path_identity(r"D:\work\projx\a.txt").within(&path_identity(r"D:\work\proj")));
    assert!(!path_identity(r"D:\work\proj").within(&path_identity(r"D:\work\projx")));
}

#[cfg(windows)]
#[test]
fn normalize_folds_the_extended_length_namespace_in_a_resolved_path() {
    assert_eq!(
        normalize(Path::new(r"\\?\D:\work\proj\sub\..\out.txt")),
        PathBuf::from(r"D:\work\proj\out.txt")
    );
}

// ---------------------------------------------------------------------------
// The write gate over a canonical (extended-length) project root
// ---------------------------------------------------------------------------

#[test]
fn ordinary_writes_are_not_bound_by_a_read_only_merge_contract_they_never_froze() {
    let gate = Gate::canonical("allow", "读取 data.csv，生成 summary.json。");
    let task = "读取 data.csv，生成 summary.json。";
    // The canonical (Host-resolved) spelling and the plain spelling the model
    // uses are the same target; neither may be refused by a gate that has no
    // read-only input set to protect.
    let canonical = gate.root.join("summary.json");
    let plain = PathBuf::from(fold_windows_spelling(&canonical.to_string_lossy()));
    assert!(plain.is_absolute() || cfg!(not(windows)));
    for target in [&canonical, &plain] {
        assert!(
            ensure_frozen_content_writable(
                &gate.db,
                &gate.run,
                Some(task),
                gate.root.to_str().unwrap(),
                target
            )
            .is_ok(),
            "{} must be writable: no read-only merge contract exists",
            target.display()
        );
    }
    // The gate keeps its name: an ordinary task with no checklist at all is the
    // same case.
    assert!(gate.db.delivery_checklist(&gate.run).unwrap().is_empty());
}

#[test]
fn a_frozen_read_only_merge_input_is_protected_in_every_spelling_it_can_carry() {
    let gate = Gate::concat_project();
    let protected = gate.root.join("zeta.txt");
    let plain = PathBuf::from(fold_windows_spelling(&protected.to_string_lossy()));
    for target in [&protected, &plain] {
        let error = ensure_frozen_content_writable(
            &gate.db,
            &gate.run,
            Some(MERGE_TASK),
            gate.root.to_str().unwrap(),
            target,
        )
        .expect_err("a Host-frozen merge input must stay read-only");
        assert!(error.contains("初始合并输入"), "{error}");
        assert!(error.contains("tool.read_only_input"), "{error}");
    }
    // The output the task itself asked for is not an input: it stays writable.
    assert!(ensure_frozen_content_writable(
        &gate.db,
        &gate.run,
        Some(MERGE_TASK),
        gate.root.to_str().unwrap(),
        &gate.root.join("joined.txt")
    )
    .is_ok());
    // A target this project cannot place fails closed *with its real reason*
    // rather than as an unclassified policy denial.
    let outside = std::env::temp_dir().join("fox-outside-none.txt");
    let error = ensure_frozen_content_writable(
        &gate.db,
        &gate.run,
        Some(MERGE_TASK),
        gate.root.to_str().unwrap(),
        &outside,
    )
    .expect_err("an unresolvable target must not silently pass the gate");
    assert!(error.contains("tool.permission_denied"), "{error}");
}

// ---------------------------------------------------------------------------
// End-to-end: decide → admission/claim → managed execute → durable receipt
// ---------------------------------------------------------------------------

/// `Ok(())` when a *positive* policy case may accept this decision: the call is
/// permitted to proceed — immediately, or after the approval the frozen policy
/// requires.
///
/// `Reject` (a malformed call the model must fix) and `Deny` (a refusal that
/// happened) both mean the call cannot execute, so neither may be read as a
/// positive result. A new `PolicyDecision` variant makes this match fail to
/// compile rather than silently pass.
fn may_proceed(decision: &PolicyDecision) -> Result<(), String> {
    match decision {
        PolicyDecision::Allow | PolicyDecision::RequireApproval => Ok(()),
        PolicyDecision::Reject { code, message } => Err(format!("Reject ({code}: {message})")),
        PolicyDecision::Deny { reason } => Err(format!("Deny ({reason})")),
    }
}

fn assert_may_proceed(decision: &PolicyDecision, context: &str) {
    if let Err(why) = may_proceed(decision) {
        panic!("{context}: {why} is not a positive decision");
    }
}

#[test]
fn a_deliberate_write_is_decided_and_committed_under_a_canonical_project_root() {
    let task = "读取 data.csv，生成 summary.json。";
    for mode in ["allow", "ask"] {
        let gate = Gate::canonical(mode, task);
        let decision = gate.policy.decide(
            &gate.run,
            "tc",
            "write_file",
            &json!({"path":"summary.json","content":"{}"}).to_string(),
        );
        assert_may_proceed(&decision, &format!("mode={mode} write_file"));
        // The exact positive outcome, so an unexpected Reject can never hide
        // behind "not a Deny": allow executes now, ask waits for a human.
        if mode == "allow" {
            assert!(matches!(decision, PolicyDecision::Allow), "mode={mode}: {decision:?}");
        } else {
            assert!(
                matches!(decision, PolicyDecision::RequireApproval),
                "mode={mode}: {decision:?}"
            );
            // Decision layer only: this test stops here for `ask`. Nothing was
            // approved, dispatched or committed for that mode.
            continue;
        }
        let result = gate
            .call(
                "write",
                "write_file",
                json!({"path":"summary.json","content":"{\"ok\":true}"}),
            )
            .expect("a declared new output must be written");
        assert_eq!(result["details"]["operation"], "created", "{result}");
        gate.assert_committed_write("write", "summary.json");
        assert_eq!(
            fs::read_to_string(gate.root.join("summary.json")).unwrap(),
            "{\"ok\":true}"
        );
    }
}

#[test]
fn positive_write_decisions_never_accept_a_reject_or_a_denial() {
    // A precise edit the Run cannot commit yet is a `Reject` (an unobserved or
    // stale target, or a fragment that cannot be matched): the model must fix
    // the call, so it is not a positive outcome.
    let gate = Gate::canonical("allow", "读取 data.csv，生成 summary.json。");
    fs::write(gate.root.join("summary.json"), "{}").unwrap();
    let invalid = gate.policy.decide(
        &gate.run,
        "tc",
        "edit_file",
        &json!({"path":"summary.json","oldText":"absent","newText":"x"}).to_string(),
    );
    assert!(
        matches!(invalid, PolicyDecision::Reject { .. }),
        "an uncommittable precise edit must be a Reject: {invalid:?}"
    );
    assert!(
        may_proceed(&invalid).is_err(),
        "the positive rule must refuse a Reject: {invalid:?}"
    );

    // A target outside the frozen project is a `Deny`: also not positive.
    let outside = std::env::temp_dir().join("fox-outside-positive.txt");
    let denied = gate.policy.decide(
        &gate.run,
        "tc",
        "write_file",
        &json!({"path": outside.to_string_lossy(), "content":"x"}).to_string(),
    );
    assert!(
        matches!(denied, PolicyDecision::Deny { .. }),
        "a write outside the project must be denied: {denied:?}"
    );
    assert!(
        may_proceed(&denied).is_err(),
        "the positive rule must refuse a Deny: {denied:?}"
    );
}

#[test]
fn a_precise_edit_is_committed_under_a_canonical_project_root() {
    // The path is named twice: read first, then saved in place, which is what
    // makes the in-place edit writable (a read-only mention alone is refused).
    let gate = Gate::canonical("allow", "读取 note.txt，把 alpha 改成 ALPHA，保存 note.txt。");
    fs::write(gate.root.join("note.txt"), "alpha beta\n").unwrap();
    let read = gate
        .call("read-1", "read", json!({"path":"note.txt"}))
        .expect("the edit target must be observable first");
    let version = read["details"]["readVersion"]
        .as_str()
        .expect("readVersion")
        .to_owned();
    let edit = gate.call(
        "edit-1",
        "edit_file",
        json!({"path":"note.txt","oldText":"alpha","newText":"ALPHA","expectedVersion":version}),
    );
    assert!(
        edit.is_ok(),
        "the precise edit must be committed: {edit:?} (persisted: {})",
        gate.persisted_result_text("edit-1")
    );
    gate.assert_committed_write("edit-1", "note.txt");
    assert_eq!(
        fs::read_to_string(gate.root.join("note.txt")).unwrap(),
        "ALPHA beta\n"
    );
}

#[test]
fn a_published_artifact_is_committed_under_a_canonical_project_root() {
    let gate = Gate::canonical("allow", "读取 source.csv，生成 copy.csv。");
    fs::write(gate.root.join("source.csv"), "Category,Units\nA,3\nB,4\n").unwrap();
    gate.call(
        "compute",
        "attachment_compute",
        json!({"projectPaths":["source.csv"],
            "code":"const a=attachments[0];saveFile('copy.csv','synthetic publication');return 1;"}),
    )
    .expect("the computed artifact must be produced");
    let artifact = gate
        .db
        .load_conversation(&gate.conversation)
        .unwrap()
        .artifacts
        .into_iter()
        .find(|artifact| artifact.display_name == "copy.csv")
        .expect("the computed artifact is registered on the conversation");
    gate.call(
        "publish",
        "write_file",
        json!({"path":"copy.csv","artifactId":artifact.id}),
    )
    .expect("publishing a computed artifact is an ordinary managed write");
    gate.assert_committed_write("publish", "copy.csv");
    assert_eq!(
        fs::read_to_string(gate.root.join("copy.csv")).unwrap(),
        "synthetic publication"
    );
}

#[test]
fn a_mutating_office_write_is_not_refused_by_the_merge_gate() {
    let task = "读取 source.xlsx，生成 报告.docx。";
    let gate = Gate::canonical_with_office("ask", task);
    fs::write(gate.root.join("source.xlsx"), b"placeholder").unwrap();

    // A declared output is not the task's read-only input: the call reaches the
    // ordinary approval decision instead of an unclassified policy denial. This
    // is the *decision layer only* — nothing was approved, dispatched or written.
    let decision = gate.policy.decide(
        &gate.run,
        "tc",
        "call_mcp_tool",
        &json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "报告.docx"}})
        .to_string(),
    );
    assert_may_proceed(&decision, "mutating Office on a declared output");
    assert!(
        matches!(decision, PolicyDecision::RequireApproval),
        "ask mode must wait for the human, not execute: {decision:?}"
    );

    // Writing the task's own input back keeps its actionable refusal.
    let decision = gate.policy.decide(
        &gate.run,
        "tc",
        "call_mcp_tool",
        &json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "source.xlsx", "overwrite": true}})
        .to_string(),
    );
    let PolicyDecision::Deny { reason } = decision else {
        panic!("a mutating Office call on the task's input must be denied: {decision:?}");
    };
    assert!(reason.contains("tool.read_only_input"), "{reason}");
    assert!(
        !reason.contains("not permitted by the frozen resource policy"),
        "the refusal must keep its own actionable reason: {reason}"
    );
}

/// The refusal facts one denied call left in the durable record.
struct DeniedCall {
    /// The refusal sentence the Host produced. This is a durable diagnostic: the
    /// interface never renders it, it only maps the structured category.
    reason: String,
    /// The tool result exactly as the controller wrote it.
    result: Value,
}

#[test]
fn every_denied_write_keeps_the_controller_result_shape_and_its_category() {
    // The four real refusal paths that reach the persisted tool call: a plain
    // write, a precise edit, a mutating Office call — each refused because the
    // target is the task's read-only merge input — and a call outside the frozen
    // project, which the Host refuses without a category tag of its own.
    let merge = Gate::concat_project();
    let write = merge.expect_policy_denial(
        "deny-write",
        "write_file",
        json!({"path":"zeta.txt","content":"changed"}),
    );
    assert_eq!(write.result["details"]["reasonCode"], "tool.read_only_input", "{:?}", write.result);
    assert!(write.reason.contains("初始合并输入"), "{}", write.reason);
    // The class the Host gated travels with the refusal, together with the call's
    // own operation identity and the Host provenance mark, so the interface can
    // word it without reading the sentence or the model's arguments.
    assert_eq!(write.result["details"]["operationKind"], "file_write", "{:?}", write.result);
    assert_eq!(write.result["details"]["operation"], "write_file", "{:?}", write.result);
    assert_eq!(write.result["details"]["diagnosticSource"], "host", "{:?}", write.result);
    assert!(
        write.reason.contains("[operation_kind:file_write]"),
        "{}",
        write.reason
    );

    let edit = merge.expect_policy_denial(
        "deny-edit",
        "edit_file",
        json!({"path":"zeta.txt","oldText":"z","newText":"Z"}),
    );
    assert_eq!(edit.result["details"]["reasonCode"], "tool.read_only_input", "{:?}", edit.result);
    assert_eq!(edit.result["details"]["operationKind"], "file_write", "{:?}", edit.result);
    assert_eq!(edit.result["details"]["operation"], "edit_file", "{:?}", edit.result);

    // The Office variant needs a task whose own material *is* an Office document:
    // a mutating Office write is gated by the frozen-material boundary only after
    // `prepare_office` accepted the target, and that preparation rejects a `.txt`
    // target (and any out-of-project path) with `tool.invalid_input` before any
    // policy gate runs. `zeta.xlsx` below is named as the task's read-only input,
    // which is the shape the Office gate can actually refuse.
    let office = Gate::canonical_with_office("allow", OFFICE_TASK);
    fs::write(office.root.join("zeta.xlsx"), b"placeholder").unwrap();
    office.freeze(OFFICE_TASK);
    let office_denial = office.expect_policy_denial(
        "deny-office",
        "call_mcp_tool",
        json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "zeta.xlsx", "overwrite": true}}),
    );
    assert_eq!(office_denial.result["details"]["reasonCode"], "tool.read_only_input", "{:?}", office_denial.result);
    // A mutating Office write is refused through the connector tool, so without
    // the Host-named class the interface could only see `call_mcp_tool`.
    assert_eq!(
        office_denial.result["details"]["operationKind"], "office_write",
        "{:?}", office_denial.result
    );
    assert_eq!(office_denial.result["details"]["operation"], "office_create", "{:?}", office_denial.result);
    assert_eq!(office_denial.result["details"]["diagnosticSource"], "host", "{:?}", office_denial.result);
    assert!(
        office_denial.reason.contains("[operation_kind:office_write]"),
        "{}",
        office_denial.reason
    );

    // A target outside the frozen project is refused by the Host's containment
    // check, which tags its own category: the class (a file write) travels too,
    // and the actionable containment reason is kept rather than flattened.
    let outside = std::env::temp_dir().join("fox-outside-denied.txt");
    let outside_denial = merge.expect_policy_denial(
        "deny-outside",
        "write_file",
        json!({"path": outside.to_string_lossy(), "content": "x"}),
    );
    assert_eq!(
        outside_denial.result["details"]["reasonCode"],
        "tool.permission_denied",
        "{:?}",
        outside_denial.result
    );
    assert_eq!(
        outside_denial.result["details"]["operationKind"], "file_write",
        "{:?}", outside_denial.result
    );
    assert_eq!(outside_denial.result["details"]["operation"], "write_file", "{:?}", outside_denial.result);
    assert_eq!(
        outside_denial.result["details"]["diagnosticSource"], "host",
        "{:?}", outside_denial.result
    );
    assert!(
        outside_denial.reason.contains("outside the authorized project folder"),
        "{}",
        outside_denial.reason
    );
    assert!(
        !outside_denial.reason.contains("not permitted by the frozen resource policy"),
        "a classified refusal keeps its own reason: {}",
        outside_denial.reason
    );

    // A refusal the Host did not classify any further stays honestly empty: no
    // category and no operation class are invented, while the provenance mark is
    // still the Host's.
    let frozen = Gate::canonical("read_only", "读取 data.csv，生成 summary.json。");
    let unclassified = frozen.expect_policy_denial(
        "deny-unclassified",
        "write_file",
        json!({"path":"summary.json","content":"x"}),
    );
    assert_eq!(
        unclassified.result["details"]["reasonCode"], Value::Null,
        "{:?}", unclassified.result
    );
    assert_eq!(
        unclassified.result["details"]["operationKind"], Value::Null,
        "{:?}", unclassified.result
    );
    assert_eq!(
        unclassified.result["details"]["diagnosticSource"], "host",
        "{:?}", unclassified.result
    );
    assert!(
        unclassified.reason.contains("not permitted by the frozen resource policy"),
        "{}",
        unclassified.reason
    );

    // A refusal never poisons the write path: the declared output still commits.
    merge
        .call(
            "write-after-denials",
            "write_file",
            json!({"path":"joined.txt","content":"alpha.txt\naa\n\nzeta.txt\nz"}),
        )
        .expect("the declared output must still be writable after refusals");
    merge.assert_committed_write("write-after-denials", "joined.txt");
}

#[test]
fn a_denied_write_persists_its_reason_and_never_fakes_a_result() {
    let gate = Gate::concat_project();
    // The merge sources were discovered from the directory, so this is the
    // read-only merge gate refusing a path the task never named.
    let denied = gate.expect_policy_denial(
        "input-write",
        "write_file",
        json!({"path":"zeta.txt","content":"changed"}),
    );
    assert!(denied.reason.contains("初始合并输入"), "{}", denied.reason);
    assert_eq!(
        denied.result["details"]["reasonCode"], "tool.read_only_input",
        "{:?}", denied.result
    );

    // The durable event keeps the same real reason and the same category, and
    // the legacy projection carries the reason too, so a failed call is not a
    // failure with no explanation.
    let event = gate.tool_failed_message("input-write");
    assert!(event.contains("初始合并输入"), "{event}");
    assert_eq!(
        gate.tool_failed_reason_code("input-write").as_deref(),
        Some("tool.read_only_input")
    );
    let projected: Option<String> = gate
        .db
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT result_json FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",
                    rusqlite::params![&gate.run, "input-write"],
                    |row| row.get(0),
                )
                .optional()
        })
        .unwrap();
    let projected: Value = serde_json::from_str(&projected.expect("projected tool call")).unwrap();
    assert_eq!(projected["isError"], Value::Bool(true));
    assert!(
        projected["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("初始合并输入"),
        "{projected}"
    );

    // The output the task asked for still commits in the same Run: one refusal
    // does not poison the write path.
    gate.call(
        "write",
        "write_file",
        json!({"path":"joined.txt","content":"alpha.txt\naa\n\nzeta.txt\nz"}),
    )
    .expect("the declared output must still be writable");
    gate.assert_committed_write("write", "joined.txt");
}

// ---------------------------------------------------------------------------
// Fixture: a real Run over a canonical project root
// ---------------------------------------------------------------------------

struct Gate {
    db: Database,
    root: PathBuf,
    conversation: String,
    run: String,
    clock: TestClock,
    cancellation: CancellationRegistry,
    policy: GatewayPolicy,
}

impl Gate {
    /// A project whose frozen root is the canonical spelling the product stores.
    fn canonical(mode: &str, task: &str) -> Gate {
        Self::build(mode, task, false)
    }

    fn canonical_with_office(mode: &str, task: &str) -> Gate {
        Self::build(mode, task, true)
    }

    /// The merge task from the audit: the Host freezes a read-only `Concat`
    /// contract whose inputs are discovered from the project directory.
    fn concat_project() -> Gate {
        let gate = Self::canonical("allow", MERGE_TASK);
        for (name, body) in [("alpha.txt", "aa"), ("zeta.txt", "z"), ("middle.txt", "mmm")] {
            fs::write(gate.root.join(name), body).unwrap();
        }
        gate.freeze(MERGE_TASK);
        gate
    }

    fn build(mode: &str, task: &str, office: bool) -> Gate {
        let plain = std::env::temp_dir().join(format!("fox-pathid-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&plain).unwrap();
        // The product freezes the *canonical* folder spelling. On Windows that
        // is the extended-length namespace, which is exactly the spelling the
        // write gate used to compare against a stripped model path.
        let root = plain.canonicalize().unwrap();
        let db = Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(
                db.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some(mode),
            )
            .unwrap();
        let run = db.create_run(&conversation.id, task, None).unwrap().run.id;
        let clock = TestClock::new(crate::database::now_ms());
        let budgets = TimeBudgets::default();
        let permission = FrozenPermission {
            mode: match mode {
                "ask" => PermissionMode::Ask,
                "read_only" => PermissionMode::ReadOnly,
                _ => PermissionMode::Allow,
            },
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
            approval_epoch: None,
        };
        let binding = RunControlBinding {
            schema_version: 1,
            run_id: run.clone(),
            conversation_id: conversation.id.clone(),
            engine_id: "pi".into(),
            execution_profile_id: "legacy".into(),
            authority: ExecutionAuthority::Authoritative,
            read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
            permission,
            budgets: budgets.clone(),
        };
        db.freeze_run_control(&binding).unwrap();
        // The frozen Host scope and the model's declared tool catalog must agree
        // exactly; both are derived from one list here.
        let mut tool_names: Vec<String> = ["read", "write_file", "edit_file", "attachment_compute"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        if office {
            tool_names.push("call_mcp_tool".to_owned());
        }
        let model = crate::kernel_model_config::KernelModelConfig {
            engine_id: "pi".into(),
            native_adapter: None,
            execution_profile_id: "legacy".into(),
            model_service: json!({"apiType":"faux","modelId":"pathid-fixture","baseUrl":"http://localhost",
                "fauxResponses":["unused synthetic response"]}),
            system_prompt: "Synthetic path-identity fixture.".into(),
            proposal_tools: tool_names
                .iter()
                .map(|name| json!({"name":name,"description":"synthetic","parameters":{"type":"object","properties":{}}}))
                .collect(),
        };
        let prompt_hash = model.hash().unwrap();
        let config = kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "pathid-synthetic-manifest".into(),
            permission_snapshot_id: binding.permission_snapshot_id.clone(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: prompt_hash.clone(),
            model_request_timeout_ms: budgets.model_request_ms,
            model_first_response_ms: budgets.model_first_response_ms,
            model_idle_ms: budgets.model_idle_ms,
            tool_execution_timeout_ms: budgets.tool_execution_ms,
            run_execution_budget_ms: budgets.run_execution_ms,
            run_execution_limited: budgets.run_execution_limited,
            approval_wait_timeout_ms: budgets.approval_wait_ms,
            provider_max_retries: 0,
            turn_max_retries: 0,
            experimental_compute_job_notice: false,
        };
        db.kernel_create_run(
            &run,
            "pi",
            "authoritative",
            2,
            &binding.permission_snapshot_id,
            "legacy",
            &prompt_hash,
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        db.freeze_kernel_model_config(&run, &model).unwrap();
        db.freeze_kernel_initial_input(&fox_engine_protocol::KernelInitialModelInput {
            schema_version: 1,
            run_id: run.clone(),
            turn_id: format!("kernel-turn:{run}"),
            prompt_config_hash: prompt_hash,
            messages: vec![json!({"role":"user","content":task})],
        })
        .unwrap();
        let mut scope = crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: tool_names.iter().cloned().collect(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: Default::default(),
            lifecycle_hooks: vec![],
        };
        if office {
            scope.mcp_server_hashes.insert(
                crate::office::SERVER_ID.to_owned(),
                format!("sha256:{}", hex::encode(Sha256::digest(b"office-server"))),
            );
            scope.office_tools = ["office_create".to_owned()].into_iter().collect();
        }
        db.freeze_kernel_host_scope(&run, &scope).unwrap();
        let cancellation = CancellationRegistry::default();
        KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(root.join("storage")).unwrap();
        let policy = GatewayPolicy {
            binding,
            scope,
            database: Some(db.clone()),
            sessions_dir: Some(sessions),
            artifacts_dir: None,
        };
        Gate {
            db,
            root,
            conversation: conversation.id,
            run,
            clock,
            cancellation,
            policy,
        }
    }

    fn context(&self) -> Value {
        json!({"deliverableRoot":"fox/session-results","deliverableFolder":"session-results"})
    }

    /// Freeze the task's own delivery checklist through the production entry.
    fn freeze(&self, task: &str) {
        freeze_task_delivery(
            &self.db,
            &self.run,
            task,
            &self.context(),
            self.root.to_str(),
        )
        .unwrap();
    }

    fn call(&self, id: &str, tool: &str, input: Value) -> Result<Value, String> {
        let coordinator =
            KernelCoordinator::reopen(&self.db, &self.clock, &self.run, &self.cancellation)?;
        if !coordinator
            .snapshot()?
            .tool_calls
            .iter()
            .any(|call| call.tool_call_id == id)
        {
            coordinator.propose_engine_batch(
                fox_engine_protocol::KernelEngineBatchCheckpoint {
                    schema_version: 1,
                    batch_id: format!("batch-{id}"),
                    history: self.db.kernel_initial_input(&self.run)?.messages,
                    assistant_message: json!({"role":"assistant","stopReason":"toolUse",
                        "content":[{"type":"toolCall","id":id,"name":tool,"arguments":input}]}),
                },
                &self.policy,
            )?;
        }
        let mut output = None;
        coordinator.dispatch_tool(id, "pathid-owned-executor", |binding, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let result = crate::runtime_host::kernel_host::execute_claimed_dispatch(
                &self.db,
                binding,
                effect,
                tool,
                &payload,
                |durable, claim| {
                    let input: Value = serde_json::from_str(durable).unwrap();
                    let (result, evidence) = if matches!(tool, "write_file" | "edit_file") {
                        crate::runtime_host::managed_files::execute_admitted_file(
                            crate::runtime_host::managed_files::ManagedExecutionContext {
                                database: &self.db,
                                backups_dir: &self.root.join("versions"),
                                conversation_id: &binding.conversation_id,
                                run_id: &self.run,
                                project_root: binding.permission.project_root.as_deref(),
                                permission_mode: binding.permission.mode.as_str(),
                                scope: &self.policy.scope,
                                sessions_dir: self.policy.sessions_dir.as_deref(),
                            },
                            tool,
                            &input,
                            id,
                            claim.credential(),
                            Some(token),
                            &mut || {
                                self.db
                                    .revalidate_execution_credential(claim.credential())?;
                                self.policy.validate(tool, &input)
                            },
                        )
                    } else if tool == "read" {
                        (
                            crate::runtime_host::managed_files::execute_observed_reader(
                                &self.db,
                                binding,
                                tool,
                                &input,
                                id,
                                token,
                                std::time::Duration::from_secs(10),
                            ),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    } else {
                        (
                            self.policy.execute_context_resource(
                                &self.db,
                                &self.root.join("storage"),
                                self.policy.sessions_dir.as_ref().unwrap(),
                                &self.root,
                                tool,
                                &input,
                                token,
                            ),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    };
                    let outcome = crate::runtime_host::kernel_host::call_outcome(&result);
                    (result, evidence, outcome)
                },
            );
            match result {
                Ok(value) => {
                    let succeeded = matches!(
                        crate::runtime_host::kernel_host::call_outcome(&Ok(value.clone())),
                        fox_engine_protocol::CallOutcome::Completed
                    );
                    output = Some(value.clone());
                    Ok((succeeded, value))
                }
                Err(error) => {
                    output = Some(json!({"error":error}));
                    Ok((false, json!({"error":error})))
                }
            }
        })?;
        let result = output.ok_or_else(|| "proposal was denied before execution".to_string())?;
        if result["isError"] == true || result.get("error").is_some() {
            return Err(format!("{id}: {result}"));
        }
        Ok(result)
    }

    /// Propose one call that must be refused by policy, and return the refusal
    /// facts exactly as the controller persisted them.
    ///
    /// The shape asserted here is the real one the controller writes — the same
    /// one the user interface consumes: state `failed`, `isError`, the policy
    /// code and stage, `executionStarted:false`, an optional structured
    /// `reasonCode` copied from the Host's own tag, and no execution or write
    /// receipt that would suggest the call ran.
    fn expect_policy_denial(&self, id: &str, tool: &str, input: Value) -> DeniedCall {
        assert!(
            self.call(id, tool, input).is_err(),
            "{id}: a denied call must never dispatch"
        );
        let (state, result): (String, Option<String>) = self
            .db
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT state, result_json FROM kernel_tool_calls
                      WHERE run_id=?1 AND tool_call_id=?2",
                    rusqlite::params![&self.run, id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(state, "failed", "{id}: a policy denial is a failed call");
        let result: Value = serde_json::from_str(
            &result.expect("a policy denial must persist the reason it was refused for"),
        )
        .unwrap();
        assert_eq!(result["isError"], Value::Bool(true), "{result}");
        assert_eq!(
            result["details"]["executionStarted"],
            Value::Bool(false),
            "a refusal never started execution: {result}"
        );
        assert_eq!(result["details"]["errorCode"], "kernel.policy_denied", "{id}: {result}");
        assert_eq!(result["details"]["stage"], "policy", "{id}: {result}");
        // The category is either a bounded Host tag or honestly absent (null):
        // never prose, never an invented code.
        match &result["details"]["reasonCode"] {
            Value::Null => {}
            Value::String(code) => assert!(
                !code.is_empty()
                    && code.len() <= 64
                    && code.chars().all(|character| character.is_ascii_alphanumeric()
                        || character == '.'
                        || character == '_'),
                "a stored reason code must be a bounded contract tag: {code}"
            ),
            other => panic!("unexpected reasonCode shape: {other}"),
        }
        assert!(
            result["details"]["executionReceipt"].is_null(),
            "a refused call must not carry an execution receipt: {result}"
        );
        assert!(
            self.db
                .run_write_receipts(&self.run)
                .unwrap()
                .iter()
                .all(|receipt| receipt.tool_call_id.as_deref() != Some(id)),
            "{id}: a refused write must not leave a durable write receipt"
        );
        let reason = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        DeniedCall { reason, result }
    }

    /// The refusal text the durable tool-call record kept for one call.
    fn persisted_result_text(&self, id: &str) -> String {
        let result: Option<String> = self
            .db
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT result_json FROM kernel_tool_calls
                          WHERE run_id=?1 AND tool_call_id=?2",
                        rusqlite::params![&self.run, id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
            })
            .unwrap()
            .flatten();
        result.unwrap_or_else(|| "<no persisted result>".to_owned())
    }

    fn tool_failed_message(&self, id: &str) -> String {
        self.db
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT json_extract(payload_json,'$.message') FROM kernel_events
                          WHERE run_id=?1 AND event_type='tool.failed'
                            AND json_extract(payload_json,'$.toolCallId')=?2
                          ORDER BY seq DESC LIMIT 1",
                        rusqlite::params![&self.run, id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
            })
            .unwrap()
            .flatten()
            .unwrap_or_default()
    }

    /// The structured category the durable `tool.failed` event kept for one call.
    fn tool_failed_reason_code(&self, id: &str) -> Option<String> {
        self.db
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT json_extract(payload_json,'$.reasonCode') FROM kernel_events
                          WHERE run_id=?1 AND event_type='tool.failed'
                            AND json_extract(payload_json,'$.toolCallId')=?2
                          ORDER BY seq DESC LIMIT 1",
                        rusqlite::params![&self.run, id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
            })
            .unwrap()
            .flatten()
    }

    fn assert_committed_write(&self, id: &str, path: &str) {
        let dispatch = fox_engine_protocol::encode_dispatch_id(&self.run, id).unwrap();
        let execution = self
            .db
            .execution_receipt(&self.run, &dispatch)
            .unwrap()
            .unwrap();
        assert_eq!(
            execution.stage,
            fox_engine_protocol::ExecutionStage::FileCommitted
        );
        assert_eq!(
            execution.external_effect,
            fox_engine_protocol::SideEffectState::Committed
        );
        let receipts = self.db.run_write_receipts(&self.run).unwrap();
        let receipt = receipts
            .iter()
            .find(|receipt| receipt.tool_call_id.as_deref() == Some(id))
            .unwrap();
        assert!(same_managed_path(
            &receipt.storage_path,
            &self.root.join(path)
        ));
        assert!(compare_receipt_hash(receipt, &self.root.join(path)).unwrap());
    }
}

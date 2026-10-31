//! Compatibility read models for the existing desktop. These are committed in
//! the same transaction as Kernel facts and are never an execution authority.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::Digest;

/// Projection only: hook policy was evaluated from this immutable scope before
/// dispatch. Deterministic audit keys prevent duplicates on any later commit.
/// Compatibility tickets do not grant authority once their owning work settled.
/// Includes replacement tickets whose Kernel approval row has already gone away.
pub(super) fn expire_inactive_approvals(
    tx: &Transaction<'_>,
    run_id: Option<&str>,
    now: i64,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE approvals SET status='expired',
            decision_json=json_object('approved',json('false'),'reason','owner_settled','authority','kernel'),
            resolved_at=COALESCE(resolved_at,?2)
         WHERE status='pending' AND EXISTS (
             SELECT 1 FROM tool_calls t JOIN kernel_runs k ON k.run_id=t.run_id
             JOIN run_control_bindings b ON b.run_id=t.run_id
             WHERE t.id=approvals.tool_call_id AND (?1 IS NULL OR t.run_id=?1)
                 AND k.kernel_mode='authoritative' AND b.authority='authoritative'
                 AND (k.terminal_written=1 OR t.status IN ('completed','failed','cancelled','interrupted','denied','expired')))",
        params![run_id, now],
    )
}

fn project_usage(
    tx: &Transaction<'_>,
    run_id: &str,
    seq: i64,
    usage: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    if !usage.is_object() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let display_seq:i64=tx.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM run_events WHERE run_id=?1",[run_id],|r|r.get(0))?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT event_json FROM run_events WHERE run_id=?1
        AND event_type='usage.updated' ORDER BY seq DESC LIMIT 1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()?;
    let previous: Value = previous
        .map(|body| serde_json::from_str(&body))
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
        .unwrap_or(json!({}));
    let mut payload = json!({"type":"usage.updated"});
    let mut step_total = 0_i64;
    for (source, target) in [
        ("input", "inputTokens"),
        ("output", "outputTokens"),
        ("cacheRead", "cacheReadTokens"),
        ("cacheWrite", "cacheWriteTokens"),
    ] {
        let step = match usage.get(source) {
            None => 0,
            Some(value) => value
                .as_i64()
                .filter(|value| (0..=super::MAX_RUNTIME_USAGE_COUNTER).contains(value))
                .ok_or(rusqlite::Error::InvalidQuery)?,
        };
        step_total = step_total
            .checked_add(step)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        payload[target] = json!(previous[target]
            .as_i64()
            .unwrap_or(0)
            .checked_add(step)
            .ok_or(rusqlite::Error::InvalidQuery)?);
    }
    let reported = match usage.get("totalTokens") {
        None => 0,
        Some(value) => value
            .as_i64()
            .filter(|value| (0..=super::MAX_RUNTIME_USAGE_COUNTER).contains(value))
            .ok_or(rusqlite::Error::InvalidQuery)?,
    };
    payload["totalTokens"] = json!(previous["totalTokens"]
        .as_i64()
        .unwrap_or(0)
        .checked_add(step_total.max(reported))
        .ok_or(rusqlite::Error::InvalidQuery)?);
    super::validate_runtime_usage_update(tx, run_id, &payload, display_seq)?;
    tx.execute(
        "INSERT INTO run_events(id,run_id,seq,event_type,event_json,created_at)
        VALUES(?1,?2,?3,'usage.updated',?4,?5)",
        params![
            format!("kernel-usage:{run_id}:{seq}"),
            run_id,
            display_seq,
            payload.canonical_string(),
            now
        ],
    )?;
    super::child_runs::project_child_run_event(tx, run_id, "usage.updated", &payload, now)?;
    super::digital_colleagues::project_digital_colleague_event(
        tx,
        run_id,
        "usage.updated",
        &payload,
        now,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_usage_is_cumulative_bounded_and_transactional() {
        let root = std::env::temp_dir().join(format!("fox-kernel-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let db = crate::database::Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap();
        let run = db
            .create_run(&conversation.id, "usage", None)
            .unwrap()
            .run
            .id;
        db.with_connection(|connection| {
            let tx = connection.transaction()?;
            project_usage(
                &tx,
                &run,
                2,
                &json!({"input":10,"output":3,"cacheRead":2,"totalTokens":12}),
                10,
            )?;
            project_usage(
                &tx,
                &run,
                4,
                &json!({"input":8,"output":4,"cacheWrite":1,"totalTokens":20}),
                11,
            )?;
            let body: String = tx.query_row(
                "SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.updated' ORDER BY seq DESC LIMIT 1",
                [&run],
                |row| row.get(0),
            )?;
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap(),
                json!({"type":"usage.updated","inputTokens":18,
                "outputTokens":7,"cacheReadTokens":2,"cacheWriteTokens":1,"totalTokens":35})
            );
            for invalid in [
                json!({"output":-1}),
                json!({"input":1.5}),
                json!({"input":1_000_000_000_001_i64}),
            ] {
                assert!(project_usage(&tx, &run, 6, &invalid, 12).is_err());
            }
            tx.rollback()?;
            let count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM run_events WHERE run_id=?1",
                [&run],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    }
}

/// Whether the Kernel will accept a reusable ("本次对话允许") decision for one
/// approval.
///
/// A reusable decision is stored as an exact `(tool, scope)` grant, so it can
/// only be accepted when the Host can NAME that scope: an unnamed scope would
/// either authorize more than the user chose or leave an authorization that no
/// later call can ever match. This is the SINGLE predicate behind both the
/// declared decision surface and the acceptance check in
/// `queue_kernel_host_approval`, so the button the user is shown and the
/// permission the Kernel grants can never drift apart.
pub(crate) fn kernel_approval_reusable(
    tool: &str,
    input: &Value,
    replacement_bound: bool,
    project_root: Option<&str>,
) -> bool {
    // A repair budget override is explicitly one-time, and a purpose-specific
    // replacement ticket is bound to one dispatch: neither may become reusable.
    tool != "task_repair_escalate_start"
        && !replacement_bound
        && crate::runtime_host::tool_operation_scope(tool, input, project_root)
            .is_some()
}

/// The decisions the Kernel accepts for one approval, as the card must declare
/// them. Absence is not a declaration: the renderer refuses to invent a
/// conversation-wide option the Host never offered.
pub(crate) fn kernel_approval_available_decisions(
    tool: &str,
    input: &Value,
    replacement_bound: bool,
    project_root: Option<&str>,
) -> Value {
    if kernel_approval_reusable(tool, input, replacement_bound, project_root) {
        serde_json::json!(["allow_once", "allow_conversation", "deny"])
    } else {
        serde_json::json!(["allow_once", "deny"])
    }
}

#[cfg(test)]
mod item5_decision_surface_tests {
    use super::{kernel_approval_available_decisions, kernel_approval_reusable};
    use serde_json::json;

    /// 写入与执行分别判定：写入目标能被精确命名，所以它可以被一条会话级授权
    /// 覆盖；执行动作不可命名，所以任何写入授权都无法冒充执行授权，执行脚本
    /// 永远保留它自己的人工审批（项5 第4条）。
    #[test]
    fn item5_writing_a_script_never_authorizes_running_it() {
        let root = std::env::temp_dir()
            .join(format!("fox-item5-decision-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).expect("temp root");
        let root = root.to_string_lossy().into_owned();
        let write = json!({"path":"deploy.ps1","content":"Write-Host ok\n"});
        let execute = json!({"action":"sync","command":"pwsh -File deploy.ps1"});

        assert!(
            kernel_approval_reusable("write_file", &write, false, Some(&root)),
            "写入目标必须能被精确命名，否则会话级授权无法落地"
        );
        assert!(
            !kernel_approval_reusable("run_command", &execute, false, Some(&root)),
            "执行不能被同一个可命名范围覆盖：允许写文件不得自动允许执行"
        );
        assert_eq!(
            kernel_approval_available_decisions("write_file", &write, false, Some(&root)),
            json!(["allow_once", "allow_conversation", "deny"])
        );
        assert_eq!(
            kernel_approval_available_decisions("run_command", &execute, false, Some(&root)),
            json!(["allow_once", "deny"]),
            "执行脚本只能一次性批准，不得提供会话级授权"
        );
        // 一次性替换凭据与 repair override 都不得升级成可复用授权。
        assert!(!kernel_approval_reusable(
            "write_file",
            &write,
            true,
            Some(&root)
        ));
        assert!(!kernel_approval_reusable(
            "task_repair_escalate_start",
            &json!({}),
            false,
            Some(&root)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有可授权的项目目录时，写入同样无法被精确命名 → 不得声明会话级决策。
    #[test]
    fn item5_a_write_without_a_named_directory_cannot_be_authorized_for_the_conversation() {
        let write = json!({"path":"deploy.ps1","content":"x"});
        assert!(!kernel_approval_reusable("write_file", &write, false, None));
        assert_eq!(
            kernel_approval_available_decisions("write_file", &write, false, None),
            json!(["allow_once", "deny"])
        );
    }
}

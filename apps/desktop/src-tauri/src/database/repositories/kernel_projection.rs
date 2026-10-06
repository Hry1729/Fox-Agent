//! Compatibility read models for the existing desktop. These are committed in
//! the same transaction as Kernel facts and are never an execution authority.
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::Digest;

pub(super) fn project(
    tx: &Transaction<'_>,
    run_id: &str,
    now: i64,
    previous_seq: i64,
) -> rusqlite::Result<()> {
    let conversation: Option<String> = tx.query_row(
        "SELECT b.conversation_id FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
         WHERE r.run_id=?1 AND r.kernel_mode='authoritative' AND b.authority='authoritative'",
        [run_id], |row| row.get(0)).optional()?;
    let Some(conversation) = conversation else {
        return Ok(());
    };
    // The approval window elapsed with no human decision. The legacy status
    // vocabulary has no "paused", so the honest projection is 'interrupted': the
    // work stopped and is offered again, it is not a failed verdict and must not
    // silently render as still running.
    tx.execute("UPDATE runs SET
        status=(SELECT CASE r.state WHEN 'created' THEN 'queued' WHEN 'completed' THEN 'completed'
            WHEN 'cancelled' THEN 'cancelled' WHEN 'failed' THEN 'failed' WHEN 'budget_exhausted' THEN 'failed'
            WHEN 'approval_expired' THEN 'interrupted'
            ELSE 'running' END
            FROM kernel_runs r WHERE r.run_id=?1),
        started_at=COALESCE(started_at,?2),
        finished_at=(SELECT terminal_at FROM kernel_runs WHERE run_id=?1),
        last_seq=(SELECT last_event_seq FROM kernel_runs WHERE run_id=?1),
        error_code=(SELECT json_extract(payload_json,'$.code') FROM kernel_events
            WHERE run_id=?1 AND event_type IN ('run.failed','run.budget_exhausted','run.approval_expired') ORDER BY seq DESC LIMIT 1),
        error_message=(SELECT json_extract(payload_json,'$.message') FROM kernel_events
            WHERE run_id=?1 AND event_type IN ('run.failed','run.budget_exhausted','run.approval_expired') ORDER BY seq DESC LIMIT 1)
        WHERE id=?1", params![run_id,now])?;

    tx.execute("INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,
        result_json,execution_location,requires_approval,started_at,completed_at,updated_at)
        SELECT 'kernel-tool:'||t.run_id||':'||t.tool_call_id,t.tool_call_id,t.run_id,?2,t.tool,t.canonical_input_json,
            CASE t.state WHEN 'waiting_approval' THEN 'pending' ELSE t.state END,t.result_json,'host',EXISTS(SELECT 1 FROM kernel_approvals a WHERE a.run_id=t.run_id AND a.tool_call_id=t.tool_call_id),
            t.created_at,t.settled_at,?3 FROM kernel_tool_calls t WHERE t.run_id=?1
        ON CONFLICT(run_id,runtime_tool_call_id) DO UPDATE SET status=excluded.status,result_json=excluded.result_json,
            requires_approval=excluded.requires_approval,completed_at=excluded.completed_at,updated_at=excluded.updated_at",
        params![run_id,conversation,now])?;
    tx.execute("INSERT INTO approvals(id,tool_call_id,status,requested_action,request_json,decision_json,requested_at,resolved_at,category)
        SELECT 'kernel-approval:'||a.run_id||':'||a.tool_call_id||CASE WHEN a.policy_version IS NULL THEN '' ELSE ':v'||a.policy_version END,'kernel-tool:'||a.run_id||':'||a.tool_call_id,
            CASE a.state WHEN 'allow_once' THEN 'approved' WHEN 'allow_conversation' THEN 'approved' ELSE a.state END,
            t.tool,CASE WHEN t.tool='task_repair_escalate_start' THEN json_object(
                'authority','kernel','runId',a.run_id,'toolCallId',a.tool_call_id,'tool',t.tool,'policyVersion',a.policy_version,
                'category','task_repair_budget_override','title','Task Repair 预算人工升级',
                'target',json_extract(t.canonical_input_json,'$.taskId'),
                'summary',json_extract(t.canonical_input_json,'$.escalationReason'),
                'arguments',json(t.canonical_input_json),'input',json(t.canonical_input_json),
                'policyReason','普通 Repair 预算已耗尽','availableDecisions',json('[\"allow_once\",\"deny\"]'))
                ELSE json_object('authority','kernel','runId',a.run_id,'toolCallId',a.tool_call_id,'tool',t.tool,'policyVersion',a.policy_version,
                'input',json(t.canonical_input_json)) END,
            CASE WHEN a.state='pending' THEN NULL ELSE json_object('decision',a.state,'authority','kernel',
                'approved',json(CASE WHEN a.state IN ('allow_once','allow_conversation') THEN 'true' ELSE 'false' END),
                'scope',CASE WHEN a.state='allow_once' THEN 'once' WHEN a.state='allow_conversation' THEN 'conversation' ELSE NULL END,
                'category',CASE WHEN t.tool='task_repair_escalate_start' THEN 'task_repair_budget_override' ELSE 'tool_execution' END) END,
            a.created_at,a.decided_at,CASE WHEN t.tool='task_repair_escalate_start' THEN 'task_repair_budget_override' ELSE 'tool_execution' END
            FROM kernel_approvals a JOIN kernel_tool_calls t ON t.run_id=a.run_id AND t.tool_call_id=a.tool_call_id
        WHERE a.run_id=?1 ON CONFLICT(tool_call_id) DO UPDATE SET id=excluded.id,request_json=excluded.request_json,requested_at=excluded.requested_at,status=excluded.status,
            decision_json=excluded.decision_json,resolved_at=excluded.resolved_at", [run_id])?;

    // A whole-file replacement carries its OWN, purpose-specific request. The
    // binding is written into the projected approval record HERE, in the same
    // transaction as the Kernel facts, so the identity the user approves is the
    // identity the later claim requires. An ordinary write approval gets no such
    // block and can therefore never be read as a replacement authorization.
    project_whole_file_replacement_requests(tx, run_id, &conversation)?;

    // Item 5: the card must declare exactly the decisions the Kernel will accept.
    // The binding above decides whether "本次对话允许" is available, so the
    // declaration is written after it.
    declare_kernel_approval_decisions(tx, run_id)?;
    expire_inactive_approvals(tx, Some(run_id), now)?;

    super::kernel_display::activity(tx, run_id, now)?;
    super::kernel_display::artifacts(tx, run_id, previous_seq, now)?;

    let mut query = tx.prepare(
        "SELECT seq,payload_json,event_type FROM kernel_events WHERE run_id=?1 AND seq>?2
        AND event_type IN ('engine.initial_response','engine.batch_response','engine.continuation_response','context.compaction.result') ORDER BY seq",
    )?;
    let responses: Vec<(i64, String, String)> = query
        .query_map(params![run_id, previous_seq], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    for (seq, body, kind) in responses {
        let payload: Value = serde_json::from_str(&body)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if kind == "context.compaction.result" {
            project_usage(
                tx,
                run_id,
                seq,
                &payload["result"]["response"]["usage"],
                now,
            )?;
            continue; // A summary is a model view, never a visible assistant answer.
        }
        let response = &payload["response"];
        if let Some(usage) = response["assistantMessage"].get("usage") {
            project_usage(tx, run_id, seq, usage, now)?;
        }
        // Supersede the durable response that prompted this continuation.
        // The latest display row may already be this round's streamed preview;
        // use the preceding model event's cursor, never display ordering.
        if kind == "engine.continuation_response" {
            tx.execute(
                "UPDATE messages SET status='superseded', updated_at=?2
                 WHERE run_id=?1 AND role='assistant' AND status IN ('completed','streaming')
                   AND id='kernel-message:'||?1||':'||(
                    SELECT json_extract(payload_json,'$.response.checkpointSeq')
                    FROM kernel_events WHERE run_id=?1 AND seq<?3
                      AND event_type IN ('engine.initial_response','engine.batch_response','engine.continuation_response')
                    ORDER BY seq DESC LIMIT 1)",
                params![run_id, now, seq],
            )?;
        }
        let content = response["assistantMessage"]["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let checkpoint = response["checkpointSeq"].as_u64().ok_or(rusqlite::Error::InvalidQuery)?;
        let thinking = response["assistantMessage"]["content"].as_array().map(|blocks| blocks.iter()
            .filter(|block| block["type"] == "thinking").filter_map(|block| block["thinking"].as_str()).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
        super::kernel_display::reasoning(tx,run_id,checkpoint,&thinking,now)?;
        // Tool arguments and empty final bubbles are not chat text.
        if content.is_empty() {
            tx.execute("UPDATE messages SET content='',status='completed',updated_at=?2 WHERE id=?1 AND status='streaming'",
                params![format!("kernel-message:{run_id}:{checkpoint}"),now])?;
            continue;
        }
        let id = format!("kernel-message:{run_id}:{checkpoint}");
        let metadata = json!({"authority":"kernel","checkpointSeq":checkpoint}).to_string();
        tx.execute("INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,runtime_message_id,metadata_json,created_at,updated_at)
            VALUES(?1,?2,?3,'assistant','text',?4,'completed',
                (SELECT COALESCE(MAX(ordinal),0)+1 FROM messages WHERE conversation_id=?2),?1,?5,?6,?6)
            ON CONFLICT(id) DO UPDATE SET content=excluded.content,status=excluded.status,metadata_json=excluded.metadata_json,updated_at=excluded.updated_at",
            params![id,conversation,run_id,content,metadata,now])?;
        tx.execute(
            "UPDATE conversations SET updated_at=?2,last_message_at=?2 WHERE id=?1",
            params![conversation, now],
        )?;
    }
    // Child/colleague terminal summaries must observe the messages and usage
    // above in this same commit, never an earlier partially projected response.
    let mut events = tx.prepare("SELECT event_type,payload_json FROM kernel_events WHERE run_id=?1 AND seq>?2
        AND event_type IN ('run.started','run.completed','run.cancelled','run.failed','run.budget_exhausted','run.approval_expired') ORDER BY seq")?;
    let events = events
        .query_map(params![run_id, previous_seq], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (kind, body) in events {
        let payload: Value = serde_json::from_str(&body)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let kind = if kind == "run.budget_exhausted" {
            "run.failed"
        } else if kind == "run.approval_expired" {
            // A pause of the work, not a verdict about it. Downstream owners see
            // an interrupted turn so the user can continue instead of restarting.
            "run.interrupted"
        } else {
            kind.as_str()
        };
        super::child_runs::project_child_run_event(tx, run_id, kind, &payload, now)?;
        super::digital_colleagues::project_digital_colleague_event(
            tx, run_id, kind, &payload, now,
        )?;
        if matches!(
            kind,
            "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted"
        ) {
            let mut display_payload=payload.clone();
            display_payload["type"]=json!(kind);
            super::kernel_display::event(tx,run_id,"terminal",&display_payload,now)?;
            tx.execute("UPDATE messages SET status=CASE WHEN ?2='run.completed' THEN 'completed' ELSE 'interrupted' END,updated_at=?3
                WHERE run_id=?1 AND role='assistant' AND status='streaming'",params![run_id,kind,now])?;
            // Child owners have already returned before a parent terminal.
            // Settle any uncollected/incomplete team without changing child
            // execution facts or leaving a running team behind a terminal Run.
            tx.execute("UPDATE expert_team_runs SET status=CASE
                WHEN ?2='run.cancelled' THEN 'cancelled'
                WHEN ?2='run.completed'
                  AND (SELECT COUNT(*) FROM child_run_delegations d WHERE d.team_run_id=expert_team_runs.id)>=json_array_length(team_json,'$.members')
                  AND NOT EXISTS(SELECT 1 FROM child_run_delegations d WHERE d.team_run_id=expert_team_runs.id AND d.status!='completed')
                    THEN 'completed' ELSE 'failed' END,
                result_json=json_object('members',json((SELECT json_group_array(json_object(
                    'memberId',d.team_member_id,'childRunId',d.child_run_id,'status',d.status,'result',d.result_text,
                    'inputTokens',d.input_tokens,'outputTokens',d.output_tokens,'totalTokens',d.total_tokens,
                    'toolCallCount',d.tool_call_count,'errorCode',d.error_code,'errorMessage',d.error_message))
                    FROM child_run_delegations d WHERE d.team_run_id=expert_team_runs.id))),
                updated_at=?3,completed_at=?3 WHERE parent_run_id=?1 AND status='running'",params![run_id,kind,now])?;
        }
    }
    project_hooks(tx, run_id, now)?;
    Ok(())
}

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

fn project_hooks(tx: &Transaction<'_>, run_id: &str, now: i64) -> rusqlite::Result<()> {
    let scope: Option<String> = tx
        .query_row(
            "SELECT scope_json FROM kernel_host_runs WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(scope) = scope else {
        return Ok(());
    };
    let scope: super::kernel_host::KernelHostScope = serde_json::from_str(&scope)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    if scope.lifecycle_hooks.is_empty() {
        return Ok(());
    }
    let mut tools = tx.prepare("SELECT tool_call_id,tool,state,canonical_input_json FROM kernel_tool_calls WHERE run_id=?1")?;
    let tools = tools
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let (state,kind): (String,String) = tx.query_row("SELECT k.state,CASE
        WHEN EXISTS(SELECT 1 FROM digital_colleague_triggers d WHERE d.run_id=k.run_id) THEN 'digital_colleague'
        WHEN r.parent_run_id IS NOT NULL THEN 'child' ELSE 'primary' END
        FROM kernel_runs k JOIN runs r ON r.id=k.run_id WHERE k.run_id=?1",[run_id],|row|Ok((row.get(0)?,row.get(1)?)))?;
    let after_event = match state.as_str() {
        "completed" => Some("run.completed"),
        "cancelled" => Some("run.cancelled"),
        "failed" | "budget_exhausted" => Some("run.failed"),
        _ => None,
    };
    for hook in scope.lifecycle_hooks {
        let write = |tool_id: Option<&str>,
                     name: Option<&str>,
                     details: Value|
         -> rusqlite::Result<()> {
            let audit_key = hex::encode(sha2::Sha256::digest(
                json!([run_id, hook.id, hook.event, tool_id])
                    .to_string()
                    .as_bytes(),
            ));
            // Deleted hook definitions retain their frozen policy in scope, but
            // the existing audit view has an intentional cascading foreign key.
            tx.execute("INSERT INTO lifecycle_hook_executions(id,hook_id,run_id,tool_call_id,event,tool_name,action,outcome,details_json,created_at)
                SELECT ?1,?2,?3,?4,?5,?6,?7,'applied',?8,?9 WHERE EXISTS(SELECT 1 FROM lifecycle_hooks WHERE id=?2)
                ON CONFLICT(id) DO NOTHING",params![format!("kernel-hook:{audit_key}"),hook.id,
                run_id,tool_id,hook.event,name,hook.action,details.to_string(),now])?;
            Ok(())
        };
        if matches!(hook.event.as_str(), "before_tool" | "after_tool") {
            for (id, name, status, input) in &tools {
                if !crate::lifecycle_hooks::matcher_matches(&hook.matcher, name)
                    || (hook.event == "after_tool"
                        && !matches!(status.as_str(), "completed" | "failed" | "cancelled"))
                {
                    continue;
                }
                let input: Value = serde_json::from_str(input)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                let keys = input
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                write(
                    Some(id),
                    Some(name),
                    json!({"authority":"kernel","frozen":true,"reason":hook.reason,
                    "inputKeys":keys,"succeeded":status=="completed"}),
                )?;
            }
        } else if hook.event == "before_run"
            && state != "created"
            && crate::lifecycle_hooks::matcher_matches(&hook.matcher, &kind)
        {
            write(
                None,
                None,
                json!({"authority":"kernel","frozen":true,"runKind":kind,"reason":hook.reason}),
            )?;
        } else if hook.event == "after_run"
            && after_event
                .is_some_and(|event| crate::lifecycle_hooks::matcher_matches(&hook.matcher, event))
        {
            write(
                None,
                None,
                json!({"authority":"kernel","frozen":true,"type":after_event,"reason":hook.reason}),
            )?;
        }
    }
    Ok(())
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
            payload.to_string(),
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

/// Bind the purpose-specific replacement request into every `write_file`
/// approval projection of this Run that needs one, and propose the request.
///
/// The four-tuple is derived from durable Host facts only: the frozen project
/// root, the canonical target, the Host observation the model's declared
/// `expectedVersion` selects, and the candidate content. When the bound
/// observation already delivered the whole content there is nothing to bind.
fn project_whole_file_replacement_requests(
    tx: &Transaction<'_>,
    run_id: &str,
    conversation_id: &str,
) -> rusqlite::Result<()> {
    let project_root: Option<String> = tx
        .query_row(
            "SELECT json_extract(b.binding_json,'$.permission.projectRoot')
               FROM run_control_bindings b WHERE b.run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(project_root) = project_root.filter(|root| !root.trim().is_empty()) else {
        return Ok(());
    };
    let rows: Vec<(String, String)> = tx
        .prepare(
            "SELECT t.tool_call_id, t.canonical_input_json
               FROM kernel_tool_calls t
              WHERE t.run_id=?1 AND t.tool='write_file'",
        )?
        .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    for (tool_call_id, canonical_input_json) in rows {
        let input: Value = match serde_json::from_str(&canonical_input_json) {
            Ok(input) => input,
            Err(_) => continue,
        };
        let Some(path) = input.get("path").and_then(Value::as_str) else {
            continue;
        };
        let content = input.get("content").and_then(Value::as_str).unwrap_or_default();
        let target = match crate::tool_host::canonical_file_identity(
            std::path::Path::new(&project_root),
            path,
        ) {
            Ok(target) => target,
            Err(_) => continue,
        };
        let declared = input
            .get("expectedVersion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let baseline = match super::kernel_execution_admission::host_observation_for_version_in_tx(
            tx,
            run_id,
            &target,
            declared,
        ) {
            Ok(Some(baseline)) => baseline,
            _ => continue,
        };
        // REV-05 / F1: eligibility is the SOURCE observation AND the model
        // delivery. A whole-file read whose model view was bounded never
        // reached the model whole, so it still needs the purpose-specific
        // replacement confirmation. An unreadable/missing delivery fact is
        // treated as "still required", which proposes the confirmation rather
        // than silently authorizing a replacement.
        //
        // A create is not a replacement at all (item 5): its bound observation is
        // the Host's own verified-absent target, so there is no existing version
        // to bind and the approval keeps the ordinary decision surface.
        let replacement_required =
            super::kernel_execution_admission::replace_authorization_required_in_tx(
                tx, run_id, &baseline,
            )
            // Fail closed toward asking: an unknown delivery is not eligibility.
            .unwrap_or(true);
        if !replacement_required {
            continue;
        }
        let binding = super::kernel_execution_admission::replace_request_binding(
            conversation_id,
            run_id,
            &tool_call_id,
            &target,
            &baseline.version,
            content,
        );
        // Propose the request (insert-only: never rewrites or revives a row).
        if let Err(error) = super::kernel_execution_admission::propose_replace_grant_in_tx(
            tx,
            &format!("replace:{}", binding.request_digest),
            conversation_id,
            run_id,
            &fox_engine_protocol::encode_dispatch_id(run_id, &tool_call_id)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
            &binding.target_identity,
            &binding.version,
            &binding.candidate_digest,
            &binding.request_digest,
            crate::database::now_ms(),
        ) {
            return Err(rusqlite::Error::InvalidParameterName(error));
        }
        // Bind it into the projected approval record's immutable request.
        let approval_id = format!(
            "kernel-approval:{run_id}:{tool_call_id}",
        );
        let updated = tx.execute(
            "UPDATE approvals
                SET request_json = json_set(request_json, '$.wholeFileReplacement',
                        json_object(
                            'purpose','整文件替换',
                            'requestDigest',?3,
                            'targetIdentity',?4,
                            'baselineVersion',?5,
                            'candidateDigest',?6,
                            'availableDecisions', json_array('allow_once','deny')))
              WHERE id=?1 AND json_type(request_json,'$.wholeFileReplacement') IS NULL",
            params![
                approval_id,
                run_id,
                binding.request_digest,
                binding.target_identity,
                binding.version,
                binding.candidate_digest,
            ],
        )?;
        if updated == 0 {
            // The projection may name the approval with a policy-version suffix;
            // fall back to the tool-call join so the binding is never lost.
            tx.execute(
                "UPDATE approvals
                    SET request_json = json_set(request_json, '$.wholeFileReplacement',
                            json_object(
                                'purpose','整文件替换',
                                'requestDigest',?3,
                                'targetIdentity',?4,
                                'baselineVersion',?5,
                                'candidateDigest',?6,
                                'availableDecisions', json_array('allow_once','deny')))
                  WHERE tool_call_id = (SELECT id FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2)
                    AND json_type(request_json,'$.wholeFileReplacement') IS NULL",
                params![
                    run_id,
                    tool_call_id,
                    binding.request_digest,
                    binding.target_identity,
                    binding.version,
                    binding.candidate_digest,
                ],
            )?;
        }
    }
    Ok(())
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
        && crate::runtime_host::shadow_reconcile::tool_operation_scope(tool, input, project_root)
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

/// Declare the accepted decision surface on every pending Kernel approval card
/// of this Run, in the same transaction that projects it.
///
/// A card that omits its declaration falls back to a one-shot-only surface, so
/// an ordinary in-directory write would never offer the conversation-scoped
/// decision the backend fully supports (item 5). Only cards that carry no
/// declaration yet and belong to the Kernel authority are touched, so an
/// explicit one-time-only surface (the repair override) is never widened.
fn declare_kernel_approval_decisions(tx: &Transaction<'_>, run_id: &str) -> rusqlite::Result<()> {
    let project_root: Option<String> = tx
        .query_row(
            "SELECT json_extract(b.binding_json,'$.permission.projectRoot')
               FROM run_control_bindings b WHERE b.run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let rows: Vec<(String, String, String, i64)> = tx
        .prepare(
            "SELECT a.id, t.tool_name, t.input_json,
                    CASE WHEN json_type(a.request_json,'$.wholeFileReplacement') IS NULL
                         THEN 0 ELSE 1 END
               FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
              WHERE t.run_id=?1 AND a.status='pending'
                AND json_extract(a.request_json,'$.authority')='kernel'
                AND json_type(a.request_json,'$.availableDecisions') IS NULL",
        )?
        .query_map([run_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    for (approval_id, tool, input_json, replacement_bound) in rows {
        let input: Value = serde_json::from_str(&input_json).unwrap_or(Value::Null);
        let decisions = kernel_approval_available_decisions(
            &tool,
            &input,
            replacement_bound != 0,
            project_root.as_deref(),
        );
        tx.execute(
            "UPDATE approvals
                SET request_json = json_set(request_json,'$.availableDecisions',json(?2))
              WHERE id=?1 AND json_type(request_json,'$.availableDecisions') IS NULL",
            params![approval_id, decisions.to_string()],
        )?;
    }
    Ok(())
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

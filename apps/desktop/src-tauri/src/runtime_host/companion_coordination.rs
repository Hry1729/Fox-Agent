// Ported from OpenMausBot @ 1a4e4da9f380804ad98c7fd1b141bb0dbfafced4: server/runtime-coordinator.ts and server/peer-delivery.ts (Apache-2.0).
use super::*;
use crate::database::companion_coordination::{Assignment, CoordinationRequest};
use serde::Deserialize;

impl RuntimeHost {
    pub(crate) fn notify_companion_assignments(&self, assignments: &[Assignment]) {
        emit_updates(&self.app, &self.database, assignments);
    }

    pub(crate) fn tick_companion_coordination(&self) -> Result<(), String> {
        let active = self.database.with_connection(|connection| {
            let mut query = connection.prepare("SELECT r.id FROM companion_coordination_runs c JOIN runs r ON r.id=c.run_id WHERE r.status IN ('queued','running','awaiting_confirmation')")?;
            let rows = query.query_map([], |row| row.get::<_,String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        for run in active {
            let budget_error = self
                .database
                .enforce_digital_colleague_tool_budget(&run)
                .err();
            if self.database.digital_colleague_budget_exceeded(&run)? || budget_error.is_some() {
                let _ = self.cancel_run(&run);
                self.database.mark_run_failed(
                    &run,
                    "digital_colleague.budget_exceeded",
                    budget_error
                        .as_deref()
                        .unwrap_or("协作已达到原定时任务的用量上限。"),
                )?;
            }
        }
        let reconciled = self
            .database
            .reconcile_companion_assignments(crate::database::now_ms())?;
        for run in reconciled.cancel_run_ids {
            let _ = self.cancel_run(&run);
        }
        emit_updates(&self.app, &self.database, &reconciled.changed);
        for assignment in self.database.pending_companion_assignments()? {
            match self.database.claim_companion_assignment(&assignment.id) {
                Ok(Some(dispatch)) => {
                    self.dispatch_companion_work(dispatch.started)?;
                    emit_updates(&self.app, &self.database, &[assignment]);
                }
                Err(error) => {
                    self.database
                        .fail_companion_assignment(&assignment.id, &error)?;
                    emit_updates(&self.app, &self.database, &[assignment]);
                }
                Ok(None) => {}
            }
        }
        for group in self.database.pending_companion_resumes()? {
            let ids = group
                .iter()
                .map(|assignment| assignment.id.clone())
                .collect::<Vec<_>>();
            if let Some(dispatch) = self.database.claim_companion_resume(&ids)? {
                self.dispatch_companion_work(dispatch.started)?;
                emit_updates(&self.app, &self.database, &group);
            }
        }
        Ok(())
    }

    fn dispatch_companion_work(&self, started: StartRunResult) -> Result<(), String> {
        if self
            .database
            .digital_colleague_budget_exceeded(&started.run.id)?
        {
            self.database.mark_run_failed(
                &started.run.id,
                "digital_colleague.token_budget_exceeded",
                "协作已达到原定时任务的用量上限。",
            )?;
        } else {
            let backend = self.database.conversation_agent_backend(&started.run.conversation_id)?;
            let backend = backend.as_deref().and_then(fox_agent_backend::port::AgentBackend::parse);
            if let Some(backend) = backend.filter(|backend| *backend != fox_agent_backend::port::AgentBackend::Fox) {
                use tauri::Manager;
                let run_id = started.run.id.clone();
                let external = self.app.state::<crate::external_backend::ExternalBackendHost>();
                if let Some(error) = external.start_companion_run(started, backend).error {
                    self.database.mark_run_failed(&run_id, &error.code, &error.message)?;
                }
                return Ok(());
            }
            let text = started.user_message.content.clone();
            self.start_run_detached(started, text, Vec::new());
        }
        Ok(())
    }
}

pub(super) fn emit_updates(app: &AppHandle, database: &Database, assignments: &[Assignment]) {
    if assignments.is_empty() {
        return;
    }
    let mut conversations = std::collections::BTreeSet::new();
    for assignment in assignments {
        conversations.insert(assignment.source_conversation_id.clone());
        conversations.insert(assignment.recipient_conversation_id.clone());
        for companion in [
            Some(assignment.recipient_companion_id.as_str()),
            assignment.source_companion_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(profile) = database.companion_profile(companion) {
                conversations.insert(profile.conversation_id);
            }
        }
    }
    let _ = app.emit(
        "companion-assignments-changed",
        json!({"conversationIds":conversations}),
    );
    let _ = database.sync_app_notifications();
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusInput {
    id: Option<String>,
}

pub(super) fn execute(
    app: &AppHandle,
    database: &Database,
    conversation: &str,
    run: &str,
    tool: &str,
    input: Value,
) -> Result<Value, String> {
    match tool {
        "companion_list" => {
            if input.as_object().is_none_or(|fields| !fields.is_empty()) {
                return Err("查看伙伴名单不接受参数。".into());
            }
            let owner = database
                .run_companion_scope(run)?
                .map(|scope| scope.companion_id)
                .unwrap_or_default();
            let peers = crate::companion_team::roster(database, &owner)
                .map_err(|error| error.response().1.to_owned())?;
            Ok(json!({"companions":peers}))
        }
        "companion_coordinate" => {
            let input: CoordinationRequest =
                serde_json::from_value(input).map_err(|error| error.to_string())?;
            let assignments = database.enqueue_companion_coordination(conversation, run, input)?;
            emit_updates(app, database, &assignments);
            let message = if assignments
                .iter()
                .any(|assignment| assignment.reply == "resume")
            {
                "已排队。请结束本回合，结果会自动返回；不要轮询或提前报告完成。"
            } else {
                "已排队。本次不自动唤醒发起方，可通过协作记录查看结果；不要提前报告完成。"
            };
            Ok(json!({"assignments":assignments,"message":message}))
        }
        "companion_assignment_status" => {
            let input: StatusInput =
                serde_json::from_value(input).map_err(|error| error.to_string())?;
            Ok(
                json!({"assignments":database.companion_assignment_status(conversation, run, input.id.as_deref())?}),
            )
        }
        _ => Err("未知的伙伴协作工具。".into()),
    }
}

pub(super) fn context(
    database: &Database,
    run: &str,
    conversation: &str,
    text: &str,
) -> Result<Value, String> {
    let scope = database.run_companion_scope(run)?;
    let child =
        scope.as_ref().is_some_and(|scope| scope.is_child) || database.child_run(run)?.is_some();
    if child {
        return Ok(json!({"allowedTools":[]}));
    }
    let owner = scope
        .as_ref()
        .map(|scope| scope.companion_id.as_str())
        .unwrap_or("");
    let roster = crate::companion_team::roster(database, owner)
        .map_err(|error| error.response().1.to_owned())?;
    let chief = !owner.is_empty()
        && database
            .companion_profile(owner)
            .map_err(|error| error.response().1.to_owned())?
            .profile
            .is_default;
    let execution = database.companion_coordination_context(run)?;
    let consult = execution.as_ref().is_some_and(|context| context.consult);
    let tools: Vec<&str> = fox_engine_protocol::TOOL_CONTRACTS
        .iter()
        .map(|item| item.0)
        .filter(|tool| {
            if *tool == "companion_coordinate" && scope.is_none() {
                return false;
            }
            if consult {
                crate::companion_workspace::is_consultation_tool(tool)
            } else {
                crate::companion_workspace::is_companion_peer_tool(tool)
                    || chief
                        && matches!(
                            *tool,
                            "companion_list_unread_notifications"
                                | "companion_mark_notifications_read"
                                | "companion_retry_failed_task"
                        )
            }
        })
        .collect();
    let mut result = json!({"allowedTools":tools,"teamRoster":roster});
    if let Some(execution) = execution {
        result["rootRunId"] = json!(execution.root_run_id);
        if let Some(incoming) = execution.incoming {
            result["incoming"] = json!({
                "assignmentId":incoming.id,"fromName":incoming.source_name,"fromId":incoming.source_companion_id,
                "message":incoming.instructions,"doneWhen":incoming.done_when,"consult":incoming.consult,"reply":incoming.reply,
                "answer":execution.answer,
            });
        }
        result["returned"] = json!(execution.returned.into_iter().map(|assignment| json!({
            "id":assignment.id,"fromName":assignment.recipient_name,"status":assignment.status,"result":assignment.result.unwrap_or_default(),"question":assignment.question
        })).collect::<Vec<_>>());
    } else if scope.is_none() {
        result["mentions"] = json!(mentions(text, &result["teamRoster"]));
    }
    let _ = conversation;
    Ok(result)
}

fn mentions(text: &str, roster: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    let Some(peers) = roster.as_array() else {
        return found;
    };
    let mut peers: Vec<_> = peers.iter().collect();
    peers.sort_by_key(|peer| std::cmp::Reverse(peer["name"].as_str().unwrap_or("").len()));
    for (index, _) in text.match_indices('@') {
        if index > 0
            && text[..index]
                .chars()
                .last()
                .is_some_and(|ch| !ch.is_whitespace() && !matches!(ch, '，' | '。' | '（' | '('))
        {
            continue;
        }
        let suffix = &text[index + 1..];
        if let Some(peer) = peers.iter().find(|peer| {
            let name = peer["name"].as_str().unwrap_or("");
            !name.is_empty()
                && suffix.strip_prefix(name).is_some_and(|rest| {
                    rest.is_empty()
                        || rest.starts_with(|ch: char| {
                            ch.is_whitespace()
                                || matches!(
                                    ch,
                                    '，' | '。' | '：' | ':' | '、' | '!' | '！' | '?' | '？'
                                )
                        })
                })
        }) {
            let item = json!({"id":peer["id"],"name":peer["name"]});
            if !found.contains(&item) {
                found.push(item);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_match_whole_names_without_treating_file_paths_or_emails_as_people() {
        let roster = json!([{"id":"a","name":"数据"},{"id":"b","name":"数据助手"},{"id":"c","name":"周报.md"}]);
        assert_eq!(
            mentions(
                "请 @数据助手 分析；@数据/报告.csv 和 mail@数据助手 都不是提及",
                &roster
            ),
            vec![json!({"id":"b","name":"数据助手"})]
        );
        assert!(mentions("@周报.md/附件", &roster).is_empty());
    }
}

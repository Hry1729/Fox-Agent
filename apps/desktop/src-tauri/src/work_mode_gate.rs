use crate::database::{CreateGoalInput, Database, GoalRecord, GoalStatus, WorkEventRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkModeDecision {
    AutoActivate,
    RequestConfirmation,
    StayConversation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluation {
    pub decision: WorkModeDecision,
    pub reasons: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct WorkModeGateInput<'a> {
    pub request: &'a str,
    pub runtime_proposed: bool,
}

impl<'a> WorkModeGateInput<'a> {
    pub fn user_request(request: &'a str) -> Self {
        Self {
            request,
            runtime_proposed: false,
        }
    }

    pub fn runtime_proposal(request: &'a str) -> Self {
        Self {
            request,
            runtime_proposed: true,
            ..Self::user_request(request)
        }
    }
}

#[derive(Debug)]
pub struct AppliedGate {
    pub evaluation: GateEvaluation,
    pub goal: Option<GoalRecord>,
    pub event: Option<WorkEventRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveWorkModeConfirmationRequest {
    pub conversation_id: String,
    pub goal_id: String,
    pub expected_version: i64,
    pub approved: bool,
}

const NO_CHANGE_PHRASES: &[&str] = &[
    "暂不修改",
    "不要修改",
    "不修改代码",
    "先别改",
    "只讨论",
    "只解释",
    "只给方案",
    "do not change",
    "don't change",
    "no changes",
    "discussion only",
    "explain only",
];

const EXPLICIT_GOAL_PREFIXES: &[&str] = &[
    "/目标",
    "请创建一个目标",
    "请帮我创建一个目标",
    "帮我创建一个目标",
    "创建一个目标",
    "请新建一个目标",
    "新建一个目标",
    "请建立一个目标",
    "建立一个目标",
    "列个目标",
    "列一个目标",
    "请列个目标",
    "请列一个目标",
    "请把这项工作设为目标",
    "把这项工作设为目标",
    "请把这个任务作为目标跟踪",
    "把这个任务作为目标跟踪",
    "进入工作模式",
    "请进入工作模式",
    "/goal",
    "create a goal",
    "new goal",
    "set a goal",
    "track as a goal",
    "enter work mode",
];

pub fn evaluate(input: WorkModeGateInput<'_>) -> GateEvaluation {
    let normalized = input.request.trim().to_lowercase();
    if contains_any(&normalized, NO_CHANGE_PHRASES) {
        return evaluation(
            WorkModeDecision::StayConversation,
            "user_explicitly_requested_no_changes",
        );
    }
    if input.runtime_proposed {
        return evaluation(
            WorkModeDecision::RequestConfirmation,
            "runtime_proposal_passed_explicit_user_gate",
        );
    }
    if EXPLICIT_GOAL_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return evaluation(
            WorkModeDecision::RequestConfirmation,
            "user_explicitly_requested_goal",
        );
    }
    evaluation(
        WorkModeDecision::StayConversation,
        "user_did_not_explicitly_request_goal",
    )
}

pub fn apply_user_request(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    request: &str,
) -> Result<AppliedGate, String> {
    apply_user_request_with_intent(database, conversation_id, run_id, request, request)
}

pub fn apply_user_request_with_intent(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    intent_request: &str,
    user_request: &str,
) -> Result<AppliedGate, String> {
    apply(
        database,
        conversation_id,
        Some(run_id),
        WorkModeGateInput::user_request(intent_request),
        goal_title(user_request),
        user_request.trim().to_owned(),
        None,
        "host:work-mode-gate".to_owned(),
    )
}

pub fn apply_runtime_proposal(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    title: String,
    objective: String,
    acceptance_summary: Option<String>,
) -> Result<AppliedGate, String> {
    let proposal_text = format!(
        "{}\n{}\n{}",
        title,
        objective,
        acceptance_summary.as_deref().unwrap_or_default()
    );
    apply(
        database,
        conversation_id,
        Some(run_id),
        WorkModeGateInput::runtime_proposal(&proposal_text),
        title,
        objective,
        acceptance_summary,
        format!("runtime:{run_id}"),
    )
}

pub fn resolve_confirmation(
    database: &Database,
    request: &ResolveWorkModeConfirmationRequest,
) -> Result<(GoalRecord, WorkEventRecord), String> {
    let goal = database
        .goals()
        .get(&request.goal_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("goal '{}' was not found", request.goal_id))?;
    if goal.conversation_id != request.conversation_id {
        return Err("goal belongs to a different conversation".to_owned());
    }
    if goal.status != GoalStatus::Proposed {
        return Err("only a proposed goal can resolve work mode confirmation".to_owned());
    }
    if request.expected_version < 1 {
        return Err("expectedVersion must be an integer of at least 1".to_owned());
    }

    let (goal, event_type) = if request.approved {
        (
            database
                .goals()
                .activate(&goal.id, request.expected_version)
                .map_err(|error| error.to_string())?,
            "goal.activated",
        )
    } else {
        (
            database
                .goals()
                .cancel(&goal.id, request.expected_version)
                .map_err(|error| error.to_string())?,
            "goal.cancelled",
        )
    };
    let event = database.append_work_event(
        event_type,
        &goal.conversation_id,
        Some(&goal.id),
        None,
        None,
        json!({ "goal": goal, "source": "user_confirmation" }),
    )?;
    Ok((goal, event))
}

fn apply(
    database: &Database,
    conversation_id: &str,
    run_id: Option<&str>,
    input: WorkModeGateInput<'_>,
    title: String,
    objective: String,
    acceptance_summary: Option<String>,
    created_by: String,
) -> Result<AppliedGate, String> {
    let initial_evaluation = evaluate(input);
    if initial_evaluation.decision == WorkModeDecision::StayConversation {
        return Ok(AppliedGate {
            evaluation: initial_evaluation,
            goal: None,
            event: None,
        });
    }
    if let Some(goal) = database
        .goals()
        .get_by_conversation(conversation_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|goal| {
            matches!(
                goal.status,
                GoalStatus::Proposed | GoalStatus::Active | GoalStatus::Blocked
            )
        })
    {
        if input.runtime_proposed {
            if let Some(refined) = database
                .goals()
                .refine_host_placeholder(&goal.id, title, objective, acceptance_summary)
                .map_err(|error| error.to_string())?
            {
                let (decision, reason, event_type) = if refined.status == GoalStatus::Proposed {
                    (
                        WorkModeDecision::RequestConfirmation,
                        "runtime_refined_proposed_goal",
                        "goal.proposed",
                    )
                } else {
                    (
                        WorkModeDecision::AutoActivate,
                        "runtime_refined_host_goal",
                        "goal.activated",
                    )
                };
                let evaluation = evaluation(decision, reason);
                let event = database.append_work_event(
                    event_type,
                    conversation_id,
                    Some(&refined.id),
                    None,
                    run_id,
                    json!({
                        "goal": refined,
                        "gateDecision": evaluation.decision,
                        "gateReasons": evaluation.reasons,
                        "source": "runtime_refinement",
                    }),
                )?;
                return Ok(AppliedGate {
                    evaluation,
                    goal: Some(refined),
                    event: Some(event),
                });
            }
        }
        let evaluation = if goal.status == GoalStatus::Proposed {
            evaluation(
                WorkModeDecision::RequestConfirmation,
                "existing_proposed_goal_reused",
            )
        } else {
            evaluation(
                WorkModeDecision::AutoActivate,
                "existing_active_goal_reused",
            )
        };
        let event = if goal.status == GoalStatus::Proposed {
            Some(database.append_work_event(
                "goal.proposed",
                conversation_id,
                Some(&goal.id),
                None,
                run_id,
                json!({
                    "goal": goal,
                    "gateDecision": evaluation.decision,
                    "gateReasons": evaluation.reasons,
                    "source": "runtime_reuse",
                }),
            )?)
        } else {
            None
        };
        return Ok(AppliedGate {
            evaluation,
            goal: Some(goal),
            event,
        });
    }

    let status = match initial_evaluation.decision {
        WorkModeDecision::AutoActivate => GoalStatus::Active,
        WorkModeDecision::RequestConfirmation => GoalStatus::Proposed,
        WorkModeDecision::StayConversation => unreachable!(),
    };
    let event_type = if status == GoalStatus::Active {
        "goal.activated"
    } else {
        "goal.proposed"
    };
    let goal = database
        .goals()
        .create(CreateGoalInput {
            id: None,
            conversation_id: conversation_id.to_owned(),
            title,
            objective,
            acceptance_summary,
            status,
            created_by,
        })
        .map_err(|error| error.to_string())?;
    let event = database.append_work_event(
        event_type,
        conversation_id,
        Some(&goal.id),
        None,
        run_id,
        json!({
            "goal": goal,
            "gateDecision": initial_evaluation.decision,
            "gateReasons": initial_evaluation.reasons,
        }),
    )?;
    Ok(AppliedGate {
        evaluation: initial_evaluation,
        goal: Some(goal),
        event: Some(event),
    })
}

fn evaluation(decision: WorkModeDecision, reason: &'static str) -> GateEvaluation {
    GateEvaluation {
        decision,
        reasons: vec![reason],
    }
}

fn contains_any(value: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| value.contains(phrase))
}

fn goal_title(request: &str) -> String {
    let clean = request.trim();
    let mut title = clean.chars().take(80).collect::<String>();
    if clean.chars().count() > 80 {
        title.push('…');
    }
    if title.is_empty() {
        "工作模式任务".to_owned()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn setup() -> (Database, std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("fox-work-mode-gate-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("gate"), None, None)
            .expect("create conversation");
        (database, path, conversation.id)
    }

    #[test]
    fn cross_file_fix_stays_in_conversation_without_explicit_goal_intent() {
        let (database, path, conversation_id) = setup();
        let request = "修复跨文件登录问题并运行测试";
        let run = database
            .create_run(&conversation_id, request, None)
            .expect("create run")
            .run;
        let applied =
            apply_user_request(&database, &conversation_id, &run.id, request).expect("apply gate");
        assert_eq!(
            applied.evaluation.decision,
            WorkModeDecision::StayConversation
        );
        assert!(applied.goal.is_none());
        assert!(database
            .goals()
            .get_by_conversation(&conversation_id)
            .unwrap()
            .is_empty());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn explanation_and_one_shot_read_only_requests_stay_in_chat() {
        let (database, path, conversation_id) = setup();
        for request in [
            "解释这个函数做什么",
            "列出这个目录的文件",
            "现在几点？",
            "### 3. 什么是数据泄漏\n\n如果训练过程看到了本不该获得的信息，离线分数会虚高。\n\n- 根据测试集分数反复选择参数；\n- 先做全数据特征选择，再交叉验证。\n\n正确顺序通常是：先划分，再只在训练集上 fit 预处理器；对验证/测试只调用 transform。",
        ] {
            let applied = apply_user_request(&database, &conversation_id, "unused-run", request)
                .expect("apply gate");
            assert_eq!(
                applied.evaluation.decision,
                WorkModeDecision::StayConversation
            );
            assert!(applied.goal.is_none(), "request: {request}");
        }
        assert!(database
            .goals()
            .get_by_conversation(&conversation_id)
            .unwrap()
            .is_empty());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn explicit_no_change_request_overrides_other_signals() {
        let (database, path, conversation_id) = setup();
        let request = "先只讨论方案，暂不修改代码，也不要运行测试";
        assert_eq!(
            evaluate(WorkModeGateInput::runtime_proposal(request)).decision,
            WorkModeDecision::StayConversation
        );
        let run = database
            .create_run(&conversation_id, request, None)
            .expect("create run")
            .run;
        let applied =
            apply_user_request(&database, &conversation_id, &run.id, request).expect("apply gate");
        assert!(applied.goal.is_none());
        assert!(database
            .goals()
            .get_by_conversation(&conversation_id)
            .unwrap()
            .is_empty());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn changes_and_high_risk_requests_do_not_imply_goal_intent() {
        for request in ["优化一下登录逻辑", "删除所有生产环境数据"] {
            assert_eq!(
                evaluate(WorkModeGateInput::user_request(request)).decision,
                WorkModeDecision::StayConversation,
                "request: {request}"
            );
        }
    }

    #[test]
    fn delegated_subtask_seed_does_not_create_a_goal() {
        for request in [
            "请把下面这个独立子任务委派给合适的子 Agent，并在完成后汇总、验证它的结果：\n\n优化下前面生成的代码",
            "目标生成窗口的按钮太矮了，请优化一下",
        ] {
            assert_eq!(
                evaluate(WorkModeGateInput::user_request(request)).decision,
                WorkModeDecision::StayConversation,
                "request: {request}"
            );
        }
    }

    #[test]
    fn explicit_goal_language_and_slash_command_require_confirmation() {
        for request in [
            "/目标 优化前面生成的代码",
            "请创建一个目标来持续跟踪这个改造",
            "列个目标，再制定计划，依次修改这几个问题",
        ] {
            assert_eq!(
                evaluate(WorkModeGateInput::user_request(request)).decision,
                WorkModeDecision::RequestConfirmation,
                "request: {request}"
            );
        }
    }

    #[test]
    fn hidden_goal_mode_uses_the_visible_prompt_for_goal_copy() {
        let (database, path, conversation_id) = setup();
        let visible_request = "优化前面生成的代码";
        let run = database
            .create_run(&conversation_id, visible_request, None)
            .expect("create run")
            .run;
        let applied = apply_user_request_with_intent(
            &database,
            &conversation_id,
            &run.id,
            "/目标 优化前面生成的代码",
            visible_request,
        )
        .expect("apply hidden goal mode");
        assert_eq!(
            applied.evaluation.decision,
            WorkModeDecision::RequestConfirmation
        );
        let goal = applied.goal.expect("proposed goal");
        assert_eq!(goal.title, visible_request);
        assert_eq!(goal.objective, visible_request);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_proposal_is_intercepted_and_user_confirmation_controls_activation() {
        let (database, path, conversation_id) = setup();
        let run = database
            .create_run(&conversation_id, "考虑优化", None)
            .expect("create run")
            .run;
        let applied = apply_runtime_proposal(
            &database,
            &conversation_id,
            &run.id,
            "优化登录".to_owned(),
            "优化一下登录逻辑".to_owned(),
            None,
        )
        .expect("apply proposal");
        assert_eq!(
            applied.evaluation.decision,
            WorkModeDecision::RequestConfirmation
        );
        let proposed = applied.goal.expect("proposed goal");
        assert_eq!(proposed.status, GoalStatus::Proposed);

        let (active, event) = resolve_confirmation(
            &database,
            &ResolveWorkModeConfirmationRequest {
                conversation_id,
                goal_id: proposed.id,
                expected_version: proposed.version,
                approved: true,
            },
        )
        .expect("approve proposal");
        assert_eq!(active.status, GoalStatus::Active);
        assert_eq!(event.event_type, "goal.activated");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_proposal_refines_tracked_work_goal_before_confirmation() {
        let (database, path, conversation_id) = setup();
        let request = "列个目标，再制定计划，依次修改这几个问题";
        let run = database
            .create_run(&conversation_id, request, None)
            .expect("create run")
            .run;
        let host_applied =
            apply_user_request(&database, &conversation_id, &run.id, request).expect("apply gate");
        let placeholder = host_applied.goal.expect("host placeholder goal");
        assert_eq!(placeholder.status, GoalStatus::Proposed);
        assert_eq!(placeholder.title, request);

        let refined = apply_runtime_proposal(
            &database,
            &conversation_id,
            &run.id,
            "修复 AI Essentials 文档问题".to_owned(),
            "按优先级修正 README 与源码中的六项文档问题".to_owned(),
            Some("逐项修改并验证引用与说明一致".to_owned()),
        )
        .expect("refine host goal");

        assert_eq!(
            refined.evaluation.decision,
            WorkModeDecision::RequestConfirmation
        );
        assert_eq!(
            refined.evaluation.reasons,
            vec!["runtime_refined_proposed_goal"]
        );
        let goal = refined.goal.expect("refined goal");
        assert_eq!(goal.id, placeholder.id);
        assert_eq!(goal.status, GoalStatus::Proposed);
        assert_eq!(goal.title, "修复 AI Essentials 文档问题");
        assert_eq!(goal.version, placeholder.version + 1);
        assert_eq!(
            refined.event.expect("refinement event").event_type,
            "goal.proposed"
        );
        assert_eq!(
            database
                .goals()
                .get_by_conversation(&conversation_id)
                .unwrap()
                .len(),
            1
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }
}

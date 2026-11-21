use super::*;
use crate::database::CreateDigitalColleagueInput;

#[test]
fn companion_team_background_scope_keeps_management_with_default_root() {
    let root = std::env::temp_dir().join(format!("fox-chief-background-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let db = Database::open(root.join("fox.db")).unwrap();
    let chief = db.seed_default_companion(&root).unwrap();
    let peer = db.create_blank_companion(&root).unwrap();
    for (owner, expected) in [(&chief, true), (&peer, false)] {
        let task = db
            .create_digital_colleague(&CreateDigitalColleagueInput {
                name: "整理伙伴进展".into(),
                expert_id: String::new(),
                objective: "查看伙伴进展".into(),
                project_id: None,
                project_root: None,
                knowledge_references: Vec::new(),
                max_runs_per_day: 8,
                max_tokens_per_day: 10_000,
                max_duration_ms: 60_000,
                max_output_tokens: 2_000,
                max_tool_calls: 20,
                companion_id: Some(owner.profile.id.clone()),
            })
            .unwrap();
        let run = db
            .create_run(&task.conversation_id, "查看伙伴进展", None)
            .unwrap();
        let (before, servers_before) =
            effective_conversation_delegation_scope(&db, &task.conversation_id).unwrap();
        let (after, servers_after) =
            effective_background_delegation_scope(&db, &run.run.id, &task.conversation_id).unwrap();
        let after = after.expect("内置助手有明确的工具范围");
        assert_eq!(servers_before, servers_after);
        for contract in fox_engine_protocol::TOOL_CONTRACTS {
            if crate::companion_workspace::is_companion_team_tool(contract.0) {
                assert_eq!(
                    after.contains(contract.0),
                    expected || crate::companion_workspace::is_companion_peer_tool(contract.0),
                    "{}",
                    contract.0
                );
            }
        }
        assert!(before.unwrap().is_subset(&after));
    }
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

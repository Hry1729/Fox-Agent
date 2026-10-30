//! The agent `search_knowledge` tool on a local knowledge base goes through
//! `LocalKnowledgeStore::search_for_agent` (vector/hybrid retrieval when the
//! index is ready, lexical fallback with the real reason otherwise) while the
//! conversation binding gate still applies.

use super::execute_bound_knowledge_tool;
use crate::database::{KnowledgeReference, KnowledgeReferenceBindingRecord};
use crate::local_knowledge::agent_search_test_support::{base_with_document, temp_store};
use serde_json::{json, Value};

fn binding(base_id: &str) -> KnowledgeReferenceBindingRecord {
    KnowledgeReferenceBindingRecord {
        conversation_id: "conversation-a".to_owned(),
        reference: KnowledgeReference::local(base_id),
        knowledge_base_name: Some("A".to_owned()),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

fn search(
    store: &crate::local_knowledge::LocalKnowledgeStore,
    bindings: &[KnowledgeReferenceBindingRecord],
    input: Value,
) -> Result<Value, String> {
    let client = crate::yuxi::YuxiClient::new().unwrap();
    tauri::async_runtime::block_on(execute_bound_knowledge_tool(
        None,
        &client,
        store,
        bindings,
        "search_knowledge",
        &input,
        crate::local_knowledge_vector::RetrievalControl::default(),
    ))
}

#[test]
fn search_knowledge_tool_reports_index_not_ready_and_keeps_binding_isolation() {
    let (store, root) = temp_store("tool");
    let (base_a, document_a, _) =
        base_with_document(&store, "A", "a.md", &["A 项目私有知识 第一段", "A 项目私有知识 第二段"]);
    let (base_b, _, _) = base_with_document(&store, "B", "b.md", &["B 项目私有知识"]);
    let bindings = [binding(&base_a)];

    let own = search(
        &store,
        &bindings,
        json!({ "target": { "source": "local", "id": base_a }, "query": "私有知识" }),
    )
    .unwrap();
    let details = &own["details"];
    assert_eq!(details["source"], "local");
    assert_eq!(details["knowledgeBaseId"], base_a.as_str());
    assert_eq!(details["fallback"]["kind"], "sqlite_lexical");
    assert_eq!(details["fallback"]["reason"], "index_not_ready");
    assert_eq!(details["fallback"]["detail"], "未配置向量模型");
    assert_eq!(details["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(details["items"][0]["documentId"], document_a.as_str());
    assert!(!own.to_string().contains("B 项目私有知识"));

    let limited = search(
        &store,
        &bindings,
        json!({ "target": { "source": "local", "id": base_a }, "query": "私有知识", "topK": 1, "maxChars": 4 }),
    )
    .unwrap();
    let items = limited["details"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["content"].as_str().unwrap().chars().count(), 4);
    assert_eq!(limited["details"]["truncated"], true);

    let foreign = search(
        &store,
        &bindings,
        json!({ "target": { "source": "local", "id": base_b }, "query": "私有知识" }),
    )
    .unwrap_err();
    assert!(foreign.contains("local_knowledge.binding_invalid"), "{foreign}");
    assert!(!foreign.contains("B 项目私有知识"));

    let mixed = search(
        &store,
        &bindings,
        json!({
            "targets": [{ "source": "local", "id": base_a }, { "source": "local", "id": base_b }],
            "query": "私有知识"
        }),
    )
    .unwrap_err();
    assert!(mixed.contains("local_knowledge.binding_invalid"), "{mixed}");

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

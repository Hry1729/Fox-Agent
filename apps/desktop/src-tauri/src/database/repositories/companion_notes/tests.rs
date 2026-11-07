use super::*;
use std::path::Path;

fn with_notes(f: impl FnOnce(&Database, &Path, &str, &str)) {
    let root = std::env::temp_dir().join(format!("fox-companion-notes-{}", Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let db = Database::open(root.join("fox.db")).unwrap();
    let contact = db.add_companion(&root, "fox-security").unwrap();
    let id = &contact.profile.id;
    let chat = &contact.conversation_id;
    crate::companion_workspace::repair(&db, &root, id).unwrap();
    db.with_connection(|c|{
        c.execute("INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES ('notes-run',?,'completed','fox',1)",[chat])?;
        Ok(())
    }).unwrap();
    f(&db, &root, id, chat);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

fn write(db: &Database, chat: &str, body: Value) -> Value {
    db.companion_notes_tool(chat, "notes-run", "memory_write", &body)
        .unwrap()
}

fn message(db: &Database, chat: &str, id: &str, text: &str, ordinal: i64) {
    db.with_connection(|c|{
        c.execute("INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at) VALUES (?1,?2,'notes-run','user','text',?3,'completed',?4,1,1)",
            params![id,chat,text,ordinal])?;
        Ok(())
    }).unwrap()
}

fn shared(key: &str, text: &str, id: &str) -> Value {
    json!({"target":"shared","action":"add","basis":"user_stated","statement":"self_stable","kind":"preference",
        "canonicalKey":key,"content":text,"sourceMessageId":id,"evidenceExcerpt":text})
}

#[test]
fn private_notes_are_owner_scoped_and_user_confirmations_check_versions() {
    with_notes(|db, root, owner, chat| {
        let user = db
            .companion_note_save_user(owner, None, None, "私人报价", Some("quote"), None)
            .unwrap();
        let mid = user["memoryId"].as_str().unwrap();
        let patch = json!({"target":"notes","action":"patch","memoryId":mid,"content":"新报价"});
        let pending = write(db, chat, patch.clone());
        assert_eq!(pending["status"], "pending");
        assert_eq!(db.get_memory(mid).unwrap().unwrap().content, "私人报价");
        db.companion_note_request_resolve(owner, pending["requestId"].as_str().unwrap(), false)
            .unwrap();
        let second = write(db, chat, patch.clone());
        db.companion_note_save_user(owner, Some(mid), Some(1), "刚刚修改", None, None)
            .unwrap();
        let stale = db
            .companion_note_request_resolve(owner, second["requestId"].as_str().unwrap(), true)
            .unwrap();
        assert_eq!(stale["status"], "stale");
        let third = write(db, chat, patch);
        db.companion_note_request_resolve(owner, third["requestId"].as_str().unwrap(), true)
            .unwrap();
        assert_eq!(db.get_memory(mid).unwrap().unwrap().content, "新报价");
        let reopened = Database::open(root.join("fox.db")).unwrap();
        let saved = reopened.companion_notes_get(owner).unwrap();
        assert_eq!(saved["requests"], json!([]));
        for (request, status, original) in [(&pending, "rejected", "私人报价"), (&second, "stale", "私人报价"), (&third, "approved", "刚刚修改")] {
            let receipt = saved["requestHistory"].as_array().unwrap().iter()
                .find(|row| row["id"] == request["requestId"]).unwrap();
            assert_eq!(receipt["status"], status);
            assert_eq!(receipt["content"], original);
            assert!(receipt["resolvedAt"].as_i64().is_some());
        }
        drop(reopened);
        let other = db.seed_default_companion(root).unwrap();
        assert!(db
            .companion_note_delete(&other.profile.id, Some(mid), Some(3))
            .is_err());
        assert!(db
            .delete_memory(mid)
            .unwrap_err()
            .contains("companion.notes_dedicated_entry"));
        assert!(db
            .confirm_memory(mid, 3)
            .unwrap_err()
            .contains("companion.notes_dedicated_entry"));
        let note = json!({"target":"notes","action":"add","canonicalKey":"private","content":"测试单独撤销"});
        let created = write(db, chat, note.clone());
        assert_eq!(created["status"], "created");
        assert_eq!(write(db, chat, note.clone())["status"], "existing");
        let new_id = created["memoryId"].as_str().unwrap();
        assert_eq!(db.companion_notes_get(owner).unwrap()["writeReceipts"][0]["status"], "created");
        assert!(db
            .companion_memory_undo(owner, new_id, "another-run")
            .is_err());
        db.companion_memory_undo(owner, new_id, "notes-run")
            .unwrap();
        let reopened = Database::open(root.join("fox.db")).unwrap();
        let saved = reopened.companion_notes_get(owner).unwrap();
        assert_eq!(saved["writeReceipts"][0]["memoryId"], new_id);
        assert_eq!(saved["writeReceipts"][0]["status"], "undone");
        drop(reopened);
        assert_eq!(write(db, chat, note)["code"], "memory.user_deleted");
        let another = write(
            db,
            chat,
            json!({"target":"notes","action":"add","canonicalKey":"other","content":"另一条"}),
        );
        let revived = write(
            db,
            chat,
            json!({"target":"notes","action":"patch","memoryId":another["memoryId"],"canonicalKey":"private","content":"测试单独撤销"}),
        );
        assert_eq!(revived["code"], "memory.user_deleted");
    })
}

#[test]
fn shared_confirmation_can_keep_both_reject_partial_evidence_and_replace_only_one() {
    with_notes(|db, root, owner, chat| {
        let a = "我喜欢报告先写结论";
        let b = "我喜欢报告附原始数据";
        let c = "我喜欢报告先列预算";
        message(db, chat, "m1", a, 1);
        message(db, chat, "m2", b, 2);
        message(db, chat, "m3", c, 3);
        let first = write(db, chat, shared("lead", a, "m1"));
        assert_eq!(first["status"], "created");
        assert_eq!(
            write(db, chat, shared("lead", a, "m1"))["status"],
            "existing"
        );
        let mut mismatch = shared("second", b, "m1");
        assert!(db
            .companion_notes_tool(chat, "notes-run", "memory_write", &mismatch)
            .unwrap_err()
            .contains("memory.evidence_mismatch"));
        mismatch["sourceMessageId"] = json!("m2");
        mismatch["evidenceExcerpt"] = json!(b);
        mismatch["content"] = json!(c);
        assert_eq!(write(db, chat, mismatch)["status"], "pending");
        let second = write(db, chat, shared("data", b, "m2"));
        assert_eq!(second["status"], "pending");
        let cards = db.companion_notes_get(owner).unwrap()["sharedRequests"]
            .as_array()
            .unwrap()
            .clone();
        let card = cards
            .iter()
            .find(|c| c["id"] == second["requestId"])
            .unwrap();
        assert_eq!(card["matches"][0]["id"], first["memoryId"]);
        let added = db
            .companion_shared_request_resolve(owner, card["id"].as_str().unwrap(), "add", None)
            .unwrap();
        let old = first["memoryId"].as_str().unwrap();
        assert!(db.get_memory(old).unwrap().unwrap().enabled);
        assert!(
            db.get_memory(added["memoryId"].as_str().unwrap())
                .unwrap()
                .unwrap()
                .enabled
        );
        let pending = cards.iter().find(|item| item["content"] == c).unwrap();
        assert!(db
            .companion_shared_request_resolve(
                owner,
                pending["id"].as_str().unwrap(),
                "replace",
                Some("invalid")
            )
            .is_err());
        db.companion_shared_request_resolve(
            owner,
            pending["id"].as_str().unwrap(),
            "replace",
            Some(old),
        )
        .unwrap();
        assert!(!db.get_memory(old).unwrap().unwrap().enabled);
        let reopened = Database::open(root.join("fox.db")).unwrap();
        let saved = reopened.companion_notes_get(owner).unwrap();
        assert_eq!(saved["sharedRequests"], json!([]));
        for (id, status) in [(&card["id"], "added"), (&pending["id"], "replaced")] {
            let receipt = saved["sharedRequestHistory"].as_array().unwrap().iter()
                .find(|row| &row["id"] == id).unwrap();
            assert_eq!(receipt["status"], status);
            assert!(receipt["resolvedAt"].as_i64().is_some());
        }
        drop(reopened);
        assert!(
            db.get_memory(added["memoryId"].as_str().unwrap())
                .unwrap()
                .unwrap()
                .enabled
        );
        assert_eq!(
            write(db, chat, shared("over-quota", c, "m3"))["code"],
            "memory.run_limit"
        );
    })
}

#[test]
fn reported_negative_temporary_and_rephrased_statements_require_confirmation() {
    for (message, excerpt, text) in [
        ("客户喜欢长报告，我不喜欢", "喜欢长报告", "我喜欢长报告"),
        (
            "客户说，我喜欢先看结论。",
            "我喜欢先看结论",
            "我喜欢先看结论",
        ),
        ("我不喜欢长报告", "我不喜欢长报告", "我不喜欢长报告"),
        (
            "这次周报我喜欢用三段文字",
            "我喜欢用三段文字",
            "我喜欢用三段文字",
        ),
        ("我老板说我喜欢先看结论", "我喜欢先看结论", "我喜欢先看结论"),
        ("我喜欢先看结论", "我喜欢先看结论", "我喜欢先看预算"),
    ] {
        assert!(!shared::direct_statement(message, excerpt, text));
    }
    assert!(shared::direct_statement("我喜欢先看结论。请记住这条长期偏好。", "我喜欢先看结论。", "我喜欢先看结论。"));
    assert!(!shared::direct_statement("我喜欢先看结论。请记住这条长期偏好。", "我喜欢先看结论。", "我喜欢先看结论，再展开细节。"));
    assert!(shared::direct_statement(
        "我喜欢先看结论。",
        "我喜欢先看结论",
        "我喜欢先看结论"
    ));
}

#[test]
fn undoing_a_move_preserves_all_rows_at_limit_and_after_a_topic_delete() {
    with_notes(|db, root, owner, chat| {
        let add = |k: &str, t: &str| {
            write(
                db,
                chat,
                json!({"target":"notes","action":"add","canonicalKey":k,"content":t}),
            )
        };
        add("base", &"文".repeat(3400));
        let moving = add("moving", &"😀".repeat(500));
        add("rest", &"字".repeat(100));
        let result=db.companion_notes_tool(chat,"notes-run","note_organize",&json!({"memoryIds":[moving["memoryId"]],"topic":"报价资料","aliases":["原始报价"]})).unwrap();
        let mid = result["moveId"].as_str().unwrap();
        let topic = result["topicId"].as_str().unwrap();
        add("new", &"a".repeat(400));
        let undo = db.companion_note_move_undo(owner, mid).unwrap();
        assert_eq!(undo["overBy"], 400);
        let original = format!("\t{}\r\n{}\r\n  ", "多".repeat(99), "少".repeat(100));
        let too_long = db
            .companion_note_save_user(owner, None, None, &original, Some("draft"), None)
            .unwrap();
        assert_eq!(too_long["overBy"], 100);
        let notes = db.companion_notes_get(owner).unwrap();
        assert_eq!(notes["usedChars"], 3900);
        assert_eq!(notes["draft"].as_str(), Some(original.as_str()));
        let reopened = Database::open(root.join("fox.db")).unwrap();
        assert_eq!(
            reopened.companion_notes_get(owner).unwrap()["draft"].as_str(),
            Some(original.as_str())
        );
        drop(reopened);
        assert_eq!(notes["moves"][0]["state"], "applied");
        let topic_read = db.companion_topic_read(owner, topic).unwrap();
        assert!(topic_read["content"]
            .as_str()
            .unwrap()
            .contains(&"😀".repeat(500)));
        db.companion_topic_delete(owner, topic, topic_read["contentSha256"].as_str().unwrap())
            .unwrap();
        assert!(db.companion_topic_read(owner, topic).is_err());
        assert!(db.companion_note_move_undo(owner, mid).is_err());
        assert_eq!(
            db.companion_notes_get(owner).unwrap()["moves"][0]["state"],
            "topic_deleted"
        );
        db.companion_note_draft_to_topic(owner, "草稿专题", &[])
            .unwrap();
        assert!(db.companion_notes_get(owner).unwrap()["draft"].is_null());
    })
}

#[test]
fn project_reader_requires_both_grant_and_active_expert_and_cannot_write() {
    with_notes(|db, _, owner, chat| {
        db.companion_note_save_user(owner, None, None, "报价编号Q9854", None, None)
            .unwrap();
        let stored = write(
            db,
            chat,
            json!({"target":"topic","action":"add","topic":"采购","content":"专题独有编号 QTopic9854"}),
        );
        db.with_connection(|c|{
            c.execute_batch("INSERT INTO projects(id,name,root_path,created_at,updated_at) VALUES ('reader-project','reader','reader-root',1,1);
              INSERT INTO conversations(id,agent_id,title,project_id,status,created_at,updated_at)
              VALUES ('reader-chat','fox-general','reader','reader-project','active',1,1);
              INSERT INTO runs(id,conversation_id,status,model,created_at) VALUES ('reader-run','reader-chat','completed','fox',1);")?;
            Ok(())
        }).unwrap();
        assert!(db
            .companion_runtime_notes("reader-chat", "QTopic9854")
            .unwrap()
            .is_none());
        db.companion_grant_project(owner, "reader-project", true)
            .unwrap();
        assert!(db.companion_note_access("reader-chat").unwrap().is_none());
        db.with_connection(|c|{
            c.execute("INSERT INTO conversation_expert_bindings
               (id,conversation_id,expert_id,state,activation_source,expert_version,package_hash,display_snapshot_json,activated_at)
               VALUES ('reader-binding','reader-chat','fox-security','active','user','1','hash','{}',1)",[])?;
            Ok(())
        }).unwrap();
        let view = db
            .companion_runtime_notes("reader-chat", "QTopic9854")
            .unwrap()
            .unwrap();
        assert_eq!(view["role"], "reader");
        assert_eq!(view["notes"][0]["content"], "报价编号Q9854");
        assert!(view["snippets"][0]["text"]
            .as_str()
            .unwrap()
            .contains("QTopic9854"));
        assert!(db
            .companion_notes_tool(
                "reader-chat",
                "reader-run",
                "memory_write",
                &json!({"target":"notes","action":"add","content":"非法"})
            )
            .is_err());
        db.archive_companion(owner).unwrap();
        assert!(db.companion_note_access("reader-chat").unwrap().is_none());
        db.restore_companion(owner).unwrap();
        assert!(db.companion_note_access("reader-chat").unwrap().is_some());
        db.companion_grant_project(owner, "reader-project", false)
            .unwrap();
        assert!(db
            .companion_notes_tool(
                "reader-chat",
                "reader-run",
                "note_search",
                &json!({"query":"Q9854"})
            )
            .is_err());

        let topic = stored["topicId"].as_str().unwrap();
        let filename = format!("{topic}.md");
        for key in [topic, "采购", filename.as_str()] {
            assert!(db
                .companion_notes_tool(
                    "reader-chat",
                    "reader-run",
                    "note_read",
                    &json!({"topic":key})
                )
                .is_err());
        }
        assert!(db
            .companion_runtime_notes("reader-chat", "QTopic9854")
            .unwrap()
            .is_none());
        db.companion_grant_project(owner, "reader-project", true)
            .unwrap();
        for (conv, run) in [(chat, "notes-run"), ("reader-chat", "reader-run")] {
            let search = db
                .companion_notes_tool(conv, run, "note_search", &json!({"query":"QTopic9854"}))
                .unwrap();
            assert_eq!(search["items"][0]["topicId"], topic);
            for key in [topic, "采购", filename.as_str()] {
                let result = db
                    .companion_notes_tool(conv, run, "note_read", &json!({"topic":key}))
                    .unwrap();
                assert!(result["content"].as_str().unwrap().contains("QTopic9854"));
            }
        }
        db.companion_topic_delete(owner, topic, stored["contentSha256"].as_str().unwrap())
            .unwrap();
        for (conv, run) in [(chat, "notes-run"), ("reader-chat", "reader-run")] {
            let view = db
                .companion_runtime_notes(conv, "QTopic9854")
                .unwrap()
                .unwrap();
            assert!(!view.to_string().contains("QTopic9854"));
            assert_eq!(view["topics"], json!([]));
            assert_eq!(view["snippets"], json!([]));
            let search = db
                .companion_notes_tool(conv, run, "note_search", &json!({"query":"QTopic9854"}))
                .unwrap();
            assert_eq!(search["items"], json!([]));
            for key in [topic, "采购", filename.as_str()] {
                assert!(db
                    .companion_notes_tool(conv, run, "note_read", &json!({"topic":key}))
                    .is_err());
            }
        }
    })
}

#[test]
fn topic_hash_conflicts_keep_user_draft_and_refresh_external_edits() {
    with_notes(|db, root, owner, chat| {
        let topic = db
            .companion_topic_save(owner, None, "报告", &["报告书".into()], "第一版", None)
            .unwrap();
        let id = topic["topicId"].as_str().unwrap();
        let path = crate::data_paths::companion_notes_dir(root, owner)
            .join("topics")
            .join(format!("{id}.md"));
        let contents =
            crate::companion_notes_files::render("报告", &["报告书".into()], "外部修订 ABC900");
        std::fs::write(&path, contents).unwrap();
        let conflict = db
            .companion_topic_save(
                owner,
                Some(id),
                "报告",
                &[],
                "我的修改",
                topic["contentSha256"].as_str(),
            )
            .unwrap();
        assert_eq!(conflict["status"], "conflict");
        assert_eq!(conflict["draft"], "我的修改");
        assert_eq!(conflict["current"]["content"], "外部修订 ABC900");
        let hits = db
            .companion_notes_tool(chat, "notes-run", "note_search", &json!({"query":"ABC900"}))
            .unwrap();
        assert_eq!(hits["items"][0]["topicId"], id);
        assert!(db.companion_topic_read(owner, "../../outside.md").is_err());
    })
}

#[test]
fn every_user_transition_shares_the_same_resident_budget() {
    with_notes(|db, _, id, chat| {
        let large = db
            .companion_note_save_user(id, None, None, &"😀".repeat(3750), Some("large"), None)
            .unwrap();
        let main_id = large["memoryId"].as_str().unwrap();
        let second = db
            .companion_note_save_user(id, None, None, &"字".repeat(200), Some("disable"), None)
            .unwrap();
        let second_id = second["memoryId"].as_str().unwrap();
        db.companion_note_set_enabled(id, second_id, 1, false)
            .unwrap();
        db.companion_note_save_user(id, None, None, &"甲".repeat(150), Some("add"), None)
            .unwrap();
        assert_eq!(
            db.companion_note_set_enabled(id, second_id, 2, true)
                .unwrap()["overBy"],
            100
        );
        assert!(!db.get_memory(second_id).unwrap().unwrap().enabled);
        let pending = write(
            db,
            chat,
            json!({"target":"notes","action":"patch","memoryId":main_id,"content":"😀".repeat(3950)}),
        );
        let rid = pending["requestId"].as_str().unwrap();
        assert_eq!(
            db.companion_note_request_resolve(id, rid, true).unwrap()["overBy"],
            100
        );
        assert_eq!(
            db.companion_notes_get(id).unwrap()["requests"][0]["id"],
            rid
        );
        db.with_connection(|c|{
            for (mid,state,text) in [("candidate","candidate","字".repeat(200)),("competition","conflict","字".repeat(4000))]{
                c.execute("INSERT INTO memory_entities(id,scope,scope_key,kind,canonical_key,content,state,created_by,created_at,updated_at)
                    VALUES (?1,'agent',?2,'fact',?1,?3,?4,'agent',1,1)",params![mid,id,text,state])?;
            }
            c.execute("INSERT INTO memory_conflicts(id,existing_memory_id,competing_memory_id,created_at) VALUES ('replace',?,'competition',1)",[main_id])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            db.companion_note_confirm(id, "candidate", 1).unwrap()["overBy"],
            100
        );
        assert_eq!(
            db.companion_note_conflict_resolve(id, "replace", true)
                .unwrap()["overBy"],
            150
        );
        assert_eq!(
            db.get_memory("competition").unwrap().unwrap().state,
            "conflict"
        );
        db.companion_note_conflict_resolve(id, "replace", false)
            .unwrap();
        db.companion_note_delete(id, Some(main_id), Some(1))
            .unwrap();
        assert_eq!(
            db.companion_note_confirm(id, "candidate", 1).unwrap()["status"],
            "updated"
        );
    })
}

#[test]
fn deleted_topic_files_left_after_interruption_are_cleaned_at_startup() {
    with_notes(|db, root, id, _| {
        let record = db
            .companion_topic_save(id, None, "cleanup", &[], "details", None)
            .unwrap();
        let tid = record["topicId"].as_str().unwrap();
        let dir = crate::data_paths::companion_notes_dir(root, id).join("topics");
        let path = dir.join(format!("{tid}.md"));
        let keep = dir.join("user.md");
        std::fs::write(&keep, "unmanaged").unwrap();
        db.with_connection(|c| {
            c.execute(
                "UPDATE companion_note_topics SET deleted_at=1 WHERE id=?",
                [tid],
            )?;
            Ok(())
        })
        .unwrap();
        db.cleanup_companion_topic_files(root).unwrap();
        assert!(!path.exists());
        assert!(keep.exists());
    })
}

#[test]
fn foreign_topic_identifiers_cannot_be_used_to_create_or_organize_topics() {
    with_notes(|db, root, owner, chat| {
        let other = db.seed_default_companion(root).unwrap();
        crate::companion_workspace::repair(db, root, &other.profile.id).unwrap();
        let topic = db
            .companion_topic_save(
                &other.profile.id,
                None,
                "另一只联系人的专题",
                &[],
                "其他内容",
                None,
            )
            .unwrap();
        let foreign = topic["topicId"].as_str().unwrap();
        let own = write(
            db,
            chat,
            json!({"target":"notes","action":"add","canonicalKey":"own","content":"自己的内容"}),
        );
        let user = db
            .companion_note_save_user(owner, None, None, "自己的手写内容", None, None)
            .unwrap();
        for target in [foreign.to_owned(), format!("{foreign}.md")] {
            for (tool, input) in [
                (
                    "memory_write",
                    json!({"target":"topic","action":"add","topic":target,"content":"追加"}),
                ),
                (
                    "note_organize",
                    json!({"memoryIds":[own["memoryId"]],"topic":target}),
                ),
                (
                    "note_organize",
                    json!({"memoryIds":[user["memoryId"]],"topic":target}),
                ),
            ] {
                let error = db
                    .companion_notes_tool(chat, "notes-run", tool, &input)
                    .expect_err(tool);
                assert!(error.contains("companion.topic_not_found"), "{error}");
            }
        }
        let data = db.companion_notes_get(owner).unwrap();
        assert_eq!(data["topics"], json!([]));
        assert_eq!(data["requests"], json!([]));
        assert_eq!(
            db.companion_topic_read(&other.profile.id, foreign).unwrap()["content"],
            "其他内容"
        );
    });
}

#[test]
fn undo_does_not_reveal_whether_another_contact_owns_the_memory() {
    with_notes(|db, root, _, chat| {
        let other = db.seed_default_companion(root).unwrap();
        let created = write(
            db,
            chat,
            json!({"target":"notes","action":"add","content":"仅本人可见"}),
        );
        let mid = created["memoryId"].as_str().unwrap();
        let foreign = db
            .companion_memory_undo(&other.profile.id, mid, "notes-run")
            .unwrap_err();
        let missing = db
            .companion_memory_undo(&other.profile.id, "not-found", "notes-run")
            .unwrap_err();
        assert_eq!(foreign, missing);
        assert!(foreign.contains("memory.not_found"));
        assert!(db.get_memory(mid).unwrap().unwrap().enabled);
    });
}

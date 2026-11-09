use super::*;

const AGENTS: &str = "
    INSERT INTO agents(id, name, description, runtime_type, system_prompt, default_model, created_at, updated_at)
    VALUES ('tpl-a','A','','local','','',1,1), ('tpl-b','B','','local','','',1,1),
           ('tpl-c','C','','local','','',1,1);";
const PROFILES: &str = "
    INSERT INTO companion_profiles
        (id,template_agent_id,display_name,workspace_dir,notes_dir,is_default,created_at,updated_at)
    VALUES ('cmp_a','tpl-a','Fox','wa','na',1,1,1), ('cmp_b','tpl-b','Bear','wb','nb',0,1,1);
    INSERT INTO conversations
        (id,agent_id,title,project_root,status,companion_id,created_at,updated_at)
    VALUES ('private-a','tpl-a','Fox','wa','active','cmp_a',1,1),
           ('private-b','tpl-b','Bear','wb','active','cmp_b',1,1);
    INSERT INTO projects(id,name,root_path,created_at,updated_at) VALUES ('project','Project','work',1,1);";
const DELETE_B: &str = "
    INSERT INTO companion_deletions
        (companion_id,template_agent_id,display_name,plan_json,plan_hash,state,started_at)
    VALUES ('cmp_b','tpl-b','Bear','{}','hash','deleting',2);";

fn database() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    run(&mut db, 1).unwrap();
    db.execute_batch(AGENTS).unwrap();
    db.execute_batch(PROFILES).unwrap();
    db
}

fn denied(db: &Connection, sql: &str, reason: &str) {
    let error = db.execute_batch(sql).expect_err(sql);
    assert!(error.to_string().contains(reason), "{sql}: {error}");
}

fn number(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn fresh_and_populated_v105_reopen_without_rewriting_old_chats() {
    let mut db = Connection::open_in_memory().unwrap();
    run_with_target(&mut db, 1, MigrationTarget::V105).unwrap();
    assert_eq!(
        number(&db, "SELECT max(version) FROM schema_migrations"),
        105
    );
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM pragma_table_info('conversations') WHERE name='companion_id'"
        ),
        0
    );
    db.execute_batch(AGENTS).unwrap();
    db.execute_batch(
        "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                      VALUES ('old','tpl-a','old title','active',1,2)",
    )
    .unwrap();
    run(&mut db, 2).unwrap();
    db.execute_batch(PROFILES).unwrap();
    run(&mut db, 3).unwrap();
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM schema_migrations WHERE version=106 AND applied_at=2"
        ),
        1
    );
    assert_eq!(number(&db, "SELECT count(*) FROM conversations WHERE id='old' AND title='old title' AND updated_at=2 AND companion_id IS NULL"), 1);
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM conversations WHERE companion_id IS NOT NULL"
        ),
        2
    );
    check_foreign_keys_after_migration(&db).unwrap();
}

#[test]
fn identity_chat_shape_and_grants_are_enforced() {
    let db = database();
    denied(&db, "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
                 VALUES ('cmp_expert','C','','local','','',1,1)", "agent_id_reserved_prefix");
    denied(
        &db,
        "UPDATE companion_profiles SET template_agent_id='tpl-c' WHERE id='cmp_b'",
        "companion_template_immutable",
    );
    denied(
        &db,
        "UPDATE companion_profiles SET is_default=0 WHERE id='cmp_a'",
        "companion_default_flag_immutable",
    );
    denied(
        &db,
        "UPDATE companion_profiles SET display_name=' FOX ' WHERE id='cmp_b'",
        "UNIQUE",
    );
    denied(&db, "INSERT INTO companion_profiles(id,template_agent_id,display_name,workspace_dir,notes_dir,created_at,updated_at)
                 VALUES ('cmp_c','tpl-a','Third','w','n',1,1)", "UNIQUE");
    denied(&db, "INSERT INTO companion_profiles(id,template_agent_id,display_name,workspace_dir,notes_dir,is_default,created_at,updated_at)
                 VALUES ('cmp_c','tpl-c','Third','w','n',1,1,1)", "UNIQUE");
    denied(
        &db,
        "INSERT INTO conversations(id,agent_id,title,status,companion_id,created_at,updated_at)
                 VALUES ('two','tpl-a','Duplicate','active','cmp_a',1,1)",
        "UNIQUE",
    );
    denied(&db, "INSERT INTO conversations(id,agent_id,title,status,companion_id,archived,created_at,updated_at)
                 VALUES ('bad','tpl-a','Bad','active','cmp_a',1,1,1)", "companion_chat_invalid_shape");
    denied(
        &db,
        "UPDATE conversations SET companion_id=NULL WHERE id='private-a'",
        "companion_binding_immutable",
    );
    denied(
        &db,
        "UPDATE conversations SET trashed_at=2 WHERE id='private-a'",
        "companion_chat_lifecycle",
    );
    denied(
        &db,
        "UPDATE conversations SET archived=1 WHERE id='private-b'",
        "companion_chat_lifecycle",
    );
    denied(
        &db,
        "UPDATE conversations SET project_root='other' WHERE id='private-a'",
        "companion_chat_project_locked",
    );
    denied(
        &db,
        "UPDATE conversations SET project_id='project' WHERE id='private-a'",
        "companion_chat_project_locked",
    );
    denied(
        &db,
        "UPDATE conversations SET full_access=1 WHERE id='private-a'",
        "companion_chat_full_access_refused",
    );
    denied(
        &db,
        "INSERT INTO companion_project_grants(id,project_id,companion_id,granted_at)
                 VALUES ('g','project','cmp_a',1)",
        "companion_grant_invalid_target",
    );
    db.execute_batch(
        "INSERT INTO companion_project_grants(id,project_id,companion_id,granted_at)
                      VALUES ('g','project','cmp_b',1)",
    )
    .unwrap();
    denied(
        &db,
        "INSERT INTO companion_project_grants(id,project_id,companion_id,granted_at)
                 VALUES ('g2','project','cmp_b',2)",
        "UNIQUE",
    );
    db.execute_batch(
        "UPDATE companion_project_grants SET revoked_at=2 WHERE id='g';
                      INSERT INTO companion_project_grants(id,project_id,companion_id,granted_at)
                      VALUES ('g2','project','cmp_b',3);",
    )
    .unwrap();
}

#[test]
fn deletion_requires_a_durable_inventory_and_exclusive_backups_are_processed() {
    let db = database();
    db.execute_batch("INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                      VALUES ('m','private-b','user','text','quote','completed',1,1,1)").unwrap();
    denied(
        &db,
        "DELETE FROM messages WHERE id='m'",
        "companion_chat_append_only",
    );
    denied(
        &db,
        "DELETE FROM companion_profiles WHERE id='cmp_a'",
        "companion_delete_requires_flow",
    );
    denied(&db, "INSERT INTO companion_deletions(companion_id,template_agent_id,display_name,plan_json,plan_hash,state,started_at)
                 VALUES ('cmp_a','tpl-a','Fox','{}','hash','deleting',2)", "companion_default_not_deletable");
    db.execute_batch(DELETE_B).unwrap();
    denied(
        &db,
        "DELETE FROM conversations WHERE id='private-b'",
        "companion_chat_delete_refused",
    );
    denied(
        &db,
        "DELETE FROM companion_profiles WHERE id='cmp_b'",
        "companion_delete_requires_flow",
    );
    db.execute_batch("INSERT INTO companion_deletion_files(companion_id,path,kind,disposition) VALUES
                      ('cmp_b','before.foxbak','managed_backup','remove'),
                      ('cmp_b','after.foxbak','managed_backup','retain');
                      UPDATE companion_deletions SET files_collected_at=3 WHERE companion_id='cmp_b';").unwrap();
    denied(
        &db,
        "INSERT INTO companion_deletion_files(companion_id,path,kind,disposition)
                 VALUES ('cmp_b','late','attachment','remove')",
        "companion_deletion_files_too_late",
    );
    db.execute_batch(
        "DELETE FROM conversations WHERE id='private-b';
                      DELETE FROM companion_profiles WHERE id='cmp_b';",
    )
    .unwrap();
    denied(
        &db,
        "UPDATE companion_deletions SET state='done' WHERE companion_id='cmp_b'",
        "companion_deletion_incomplete",
    );
    db.execute_batch("UPDATE companion_deletions SET db_purged_at=4 WHERE companion_id='cmp_b'")
        .unwrap();
    denied(
        &db,
        "UPDATE companion_deletions SET state='done' WHERE companion_id='cmp_b'",
        "companion_deletion_incomplete",
    );
    db.execute_batch(
        "UPDATE companion_deletion_files SET state='removed' WHERE path='before.foxbak';
                      UPDATE companion_deletion_files SET state='kept' WHERE path='after.foxbak';
                      UPDATE companion_deletions SET state='done' WHERE companion_id='cmp_b';",
    )
    .unwrap();
    assert_eq!(number(&db, "SELECT count(*) FROM messages WHERE id='m'"), 0);
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM companion_deletion_files WHERE companion_id='cmp_b'"
        ),
        2
    );
    check_foreign_keys_after_migration(&db).unwrap();
}

#[test]
fn resident_notes_enforce_ownership_and_the_unicode_budget_on_insert_and_update() {
    let db = database();
    const NOTE: &str = "INSERT INTO memory_entities
        (id,scope,scope_key,kind,canonical_key,content,state,enabled,source_conversation_id,created_by,created_at,updated_at)
        VALUES (?1,'agent',?2,'fact',?1,?3,?4,1,?5,'user',1,1)";
    let large = "😀".repeat(3900);
    db.execute(
        NOTE,
        ("base", "cmp_a", large.as_str(), "confirmed", "private-a"),
    )
    .unwrap();
    let owner = db
        .execute(NOTE, ("foreign", "cmp_a", "a", "confirmed", "private-b"))
        .unwrap_err();
    assert!(owner
        .to_string()
        .contains("memory_companion_owner_mismatch"));
    let excess = db
        .execute(
            NOTE,
            (
                "excess",
                "cmp_a",
                "字".repeat(101),
                "confirmed",
                "private-a",
            ),
        )
        .unwrap_err();
    assert!(excess
        .to_string()
        .contains("companion_note_budget_exceeded"));
    db.execute(
        NOTE,
        (
            "moving",
            "cmp_a",
            "字".repeat(500),
            "candidate",
            "private-a",
        ),
    )
    .unwrap();
    db.execute_batch("UPDATE memory_entities SET moved_at=2, state='confirmed' WHERE id='moving'")
        .unwrap();
    denied(
        &db,
        "UPDATE memory_entities SET moved_at=NULL WHERE id='moving'",
        "companion_note_budget_exceeded",
    );
    denied(
        &db,
        "UPDATE memory_entities SET scope_key='cmp_b' WHERE id='base'",
        "memory_companion_scope_immutable",
    );
    denied(
        &db,
        "UPDATE memory_entities SET source_conversation_id='private-b' WHERE id='base'",
        "memory_companion_scope_immutable",
    );
    denied(
        &db,
        "DELETE FROM memory_entities WHERE id='base'",
        "memory_companion_hard_delete_refused",
    );
    db.execute(
        NOTE,
        (
            "candidate",
            "cmp_a",
            "字".repeat(200),
            "candidate",
            "private-a",
        ),
    )
    .unwrap();
    denied(
        &db,
        "UPDATE memory_entities SET state='confirmed' WHERE id='candidate'",
        "companion_note_budget_exceeded",
    );
    db.execute_batch(
        "UPDATE memory_entities SET content='合适',state='confirmed' WHERE id='candidate'",
    )
    .unwrap();
    db.execute(
        NOTE,
        ("other", "cmp_b", large.as_str(), "confirmed", "private-b"),
    )
    .unwrap();
    db.execute_batch(DELETE_B).unwrap();
    db.execute_batch("DELETE FROM memory_entities WHERE id='other'")
        .unwrap();
}

#[test]
fn soft_deleting_a_topic_removes_search_hits_and_invalidates_moves_atomically() {
    let db = database();
    db.execute_batch("INSERT INTO companion_note_topics
                      (id,companion_id,title,file_name,created_by,content_sha256,created_at,updated_at)
                      VALUES ('ntp_a','cmp_a','报价','a.md','user','hash',1,1);
                      INSERT INTO companion_note_chunks(topic_id,ordinal,text,bigrams,topic_revision)
                      VALUES ('ntp_a',0,'X123报价','X1',1);
                      INSERT INTO companion_note_moves(id,companion_id,topic_id,memory_ids_json,state,created_at)
                      VALUES ('move','cmp_a','ntp_a','[]','applied',1);
                      INSERT INTO memory_entities
                      (id,scope,scope_key,kind,canonical_key,content,state,enabled,moved_at,moved_topic_id,created_by,created_at,updated_at)
                      VALUES ('note','agent','cmp_a','fact','k','X123','confirmed',1,1,'ntp_a','user',1,1);
                      SAVEPOINT before_delete;
                      UPDATE companion_note_topics SET deleted_at=2 WHERE id='ntp_a';").unwrap();
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM companion_note_chunks WHERE text LIKE '%X123%'"
        ),
        0
    );
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM companion_note_moves WHERE state='topic_deleted'"
        ),
        1
    );
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM memory_entities WHERE id='note' AND enabled=0 AND deleted_at=2"
        ),
        1
    );
    denied(
        &db,
        "INSERT INTO companion_note_chunks(topic_id,ordinal,text,bigrams,topic_revision)
                 VALUES ('ntp_a',1,'p','p',1)",
        "companion_topic_deleted",
    );
    denied(
        &db,
        "UPDATE companion_note_topics SET deleted_at=NULL WHERE id='ntp_a'",
        "companion_topic_deleted_final",
    );
    db.execute_batch("ROLLBACK TO before_delete; RELEASE before_delete;")
        .unwrap();
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM companion_note_chunks WHERE topic_id='ntp_a'"
        ),
        1
    );
    assert_eq!(
        number(
            &db,
            "SELECT count(*) FROM companion_note_moves WHERE state='applied'"
        ),
        1
    );
}

#[test]
fn shared_preference_requests_allow_no_match_and_only_replace_a_displayed_choice() {
    let db = database();
    db.execute_batch("INSERT INTO memory_entities
        (id,scope,kind,canonical_key,content,state,created_by,created_at,updated_at) VALUES
        ('new','global','preference','a','A','candidate','user',1,1),
        ('old','global','preference','b','B','confirmed','user',1,1),
        ('another','global','preference','c','C','candidate','user',1,1);
        INSERT INTO companion_shared_requests(id,companion_id,candidate_memory_id,candidate_version,created_at)
        VALUES ('first','cmp_a','new',1,1), ('second','cmp_a','another',1,1);
        UPDATE companion_shared_requests SET status='added',resolved_at=2 WHERE id='first';
        INSERT INTO companion_shared_request_matches(request_id,memory_id,expected_version) VALUES ('second','old',1);").unwrap();
    denied(&db, "UPDATE companion_shared_requests SET status='replaced',replaced_memory_id='new',resolved_at=2 WHERE id='second'",
                "FOREIGN KEY");
    db.execute_batch("UPDATE companion_shared_requests SET status='replaced',replaced_memory_id='old',resolved_at=2 WHERE id='second'").unwrap();
    check_foreign_keys_after_migration(&db).unwrap();
}

#[test]
fn v109_model_columns_default_to_follow_global_and_migration_never_rewrites_a_choice() {
    let mut db = Connection::open_in_memory().unwrap();
    run(&mut db, 1).unwrap();
    db.execute_batch(AGENTS).unwrap();
    db.execute_batch(PROFILES).unwrap();
    assert_eq!(
        number(&db, "SELECT count(*) FROM companion_profiles WHERE model_provider_id IS NULL AND model_ref IS NULL"),
        2,
        "旧联系人升级后默认跟随全局"
    );
    assert_eq!(
        number(&db, "SELECT count(*) FROM pragma_table_info('runs') WHERE name='model_snapshot'"),
        1
    );
    db.execute_batch("UPDATE companion_profiles SET model_provider_id='prov', model_ref='prov-model' WHERE id='cmp_a'").unwrap();
    run(&mut db, 2).unwrap();
    assert_eq!(
        number(&db, "SELECT count(*) FROM companion_profiles WHERE id='cmp_a' AND model_provider_id='prov' AND model_ref='prov-model'"),
        1,
        "重复执行迁移不改写已保存的选择"
    );
    check_foreign_keys_after_migration(&db).unwrap();
}

#[test]
fn v110_grants_start_empty_and_the_snapshot_column_defaults_to_null() {
    let mut db = Connection::open_in_memory().unwrap();
    run(&mut db, 1).unwrap();
    db.execute_batch(AGENTS).unwrap();
    db.execute_batch(PROFILES).unwrap();
    assert_eq!(number(&db, "SELECT count(*) FROM companion_tool_grants"), 0, "旧联系人不自动获得新能力");
    assert_eq!(
        number(&db, "SELECT count(*) FROM pragma_table_info('runs') WHERE name='companion_tools_snapshot'"),
        1
    );
    db.execute_batch(
        "INSERT INTO companion_tool_grants(companion_id,server_id,tool,created_at) VALUES ('cmp_a','@web','web_search',1)",
    ).unwrap();
    run(&mut db, 2).unwrap();
    assert_eq!(
        number(&db, "SELECT count(*) FROM companion_tool_grants WHERE companion_id='cmp_a' AND server_id='@web' AND tool='web_search'"),
        1,
        "重复执行迁移保留已保存的授权"
    );
    denied(&db, "INSERT INTO companion_tool_grants(companion_id,server_id,tool,created_at) VALUES ('cmp_gone','@web','web_search',1)", "FOREIGN KEY");
    check_foreign_keys_after_migration(&db).unwrap();
}

#[test]
fn v112_reminders_default_to_global_and_legacy_read_converts_after_backfill() {
    let db = database();
    // 升级保留原有通知偏好：旧联系人默认沿用全局，游标尚未转换。
    assert_eq!(number(&db, "SELECT count(*) FROM companion_profiles WHERE reminder_mode='follow_global'"), 2);
    assert_eq!(number(&db, "SELECT count(*) FROM companion_profiles WHERE last_read_delivery_order IS NULL"), 2);
    denied(&db, "UPDATE companion_profiles SET reminder_mode='loud' WHERE id='cmp_a'", "CHECK");
    // 通知中心接受新的 message 提醒类型，未知类型仍被拒绝。
    db.execute_batch("INSERT INTO app_notifications(id,merge_key,kind,severity,title,body,source_type,source_id,status,created_at,updated_at)
        VALUES ('n1','companion-messages:cmp_a','message','normal','Fox','1 条新消息','companion_messages','cmp_a','bubble:1000',1,1)").unwrap();
    denied(&db, "INSERT INTO app_notifications(id,merge_key,kind,severity,title,body,source_type,source_id,status,created_at,updated_at)
        VALUES ('n2','x','bogus','normal','t','b','s','sid','s',1,1)", "CHECK");
    // 旧已读进度按 K6 转换为气泡阅读位置（回填完成后）。
    db.execute_batch("
        INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
        VALUES ('m1','private-a','assistant','text','一','completed',1,1,1),
               ('m2','private-a','assistant','text','二','completed',2,1,1),
               ('m3','private-a','assistant','text','三','completed',3,1,1);
        INSERT INTO contact_bubbles(bubble_id,conversation_id,run_id,source_message_id,source_ordinal,author,projection,segment_start,segment_end,delivery_order,delivered_at,content,fingerprint,created_at)
        VALUES ('b1','private-a',NULL,'m1',1,'companion','message',NULL,NULL,1000,1,'一','f',1),
               ('b2','private-a',NULL,'m2',2,'companion','message',NULL,NULL,2000,1,'二','f',1),
               ('b3','private-a',NULL,'m3',3,'companion','message',NULL,NULL,3000,1,'三','f',1);
        INSERT INTO contact_bubble_backfills(conversation_id,completed_at) VALUES ('private-a',1);
        UPDATE companion_profiles SET last_read_ordinal=2 WHERE id='cmp_a';").unwrap();
    super::super::repositories::companions::convert_pending_read_cursors(&db).unwrap();
    assert_eq!(number(&db, "SELECT last_read_delivery_order FROM companion_profiles WHERE id='cmp_a'"), 2000);
    // 未回填的联系人游标保持 NULL（VIEW 仍按旧 ordinal 计算有效位置）。
    assert_eq!(number(&db, "SELECT count(*) FROM companion_profiles WHERE id='cmp_b' AND last_read_delivery_order IS NULL"), 1);
    check_foreign_keys_after_migration(&db).unwrap();
}

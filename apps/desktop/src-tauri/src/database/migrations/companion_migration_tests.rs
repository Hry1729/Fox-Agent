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

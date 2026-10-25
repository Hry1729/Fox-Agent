#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::*;

impl Database {
    /// Avatar decoration is independent of immutable expert prompt snapshots.
    /// Apply this user's selected set once; preserve subsequent manual choices.
    pub(crate) fn seed_expert_avatars(&self) -> Result<(), String> {
        let avatars: Vec<Value> = serde_json::from_str(include_str!("../../../../public/avatars/experts/assignments.json")).map_err(|e|e.to_string())?;
        self.with_connection(|connection| {
            let tx=connection.transaction()?;
            for avatar in &avatars {
                let Some(id)=avatar["id"].as_str() else { continue };
                let Some(src)=avatar["src"].as_str() else { continue };
                let applied: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_runtime_config WHERE agent_id=?1 AND config_key='appearance.bundledAvatar.v1')",[id],|r|r.get(0))?;
                if applied {continue;}
                let changed=tx.execute("UPDATE agents SET icon=?2 WHERE id=?1",params![id,src])?;
                if changed>0 {
                    let now=now_ms();
                    tx.execute("INSERT INTO agent_runtime_config(agent_id,scope_type,scope_id,config_key,value_json,source,created_at,updated_at) VALUES (?1,'agent',?1,'appearance.bundledAvatar.v1','true','user',?2,?2)",params![id,now])?;
                }
            }
            tx.commit()
        })
    }
    pub(crate) fn ensure_office_connector(
        &self,
        binary: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let now = now_ms();
            connection.execute(
                "INSERT INTO mcp_servers(id,name,command,args_json,transport,enabled,status,last_error,created_at,updated_at)
                 VALUES (?1,'Office 文档',?2,'[\"mcp\"]','stdio',1,?3,?4,?5,?5)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, command=excluded.command,
                    args_json=excluded.args_json, transport=excluded.transport, endpoint_url=NULL,
                    definition=NULL, status=excluded.status, last_error=excluded.last_error,
                    updated_at=CASE WHEN command!=excluded.command THEN excluded.updated_at ELSE updated_at END",
                params![crate::office::SERVER_ID, binary, if error.is_some(){"unavailable"}else{"unknown"},error,now],
            )?;
            Ok(())
        })
    }

    pub(crate) fn seed_bundled_experts(&self) -> Result<(), String> {
        let catalog: Vec<Value> = serde_json::from_str(include_str!(
            "../../../resources/expert-library/bundled.json"
        ))
        .map_err(|error| error.to_string())?;
        // Parse and verify every package before opening the transaction: a bad
        // shipped package must not leave half of the library installed.
        let mut requests = Vec::new();
        for entry in &catalog {
            let mut request =
                crate::expert_packages::bundled_install_request(entry["package"].clone())?;
            request.expert_id = entry["expertId"]
                .as_str()
                .ok_or("缺少内置专家 ID")?
                .to_owned();
            request.package_manifest["officeTools"] = entry["officeTools"].clone();
            request.package_manifest["bundledLibrary"] = json!("agency-zh-fox-v1");
            // No invented knowledge IDs: inherit only the knowledge explicitly
            // bound to the conversation, using the existing runtime contract.
            if let Some(manifest) = request.package_manifest.as_object_mut() {
                manifest.remove("knowledge");
                manifest.remove("knowledgeReferences");
            }
            requests.push(request);
        }
        self.with_connection(|connection| {
            let tx=connection.transaction()?;
            let now=now_ms();
            for r in &requests {
                let current: Option<(String,Option<String>)>=tx.query_row(
                    "SELECT package_source,package_hash FROM agents WHERE id=?1",[&r.expert_id],
                    |row|Ok((row.get(0)?,row.get(1)?)),
                ).optional()?;
                // A user's local expert must never be silently replaced.
                if current.as_ref().is_some_and(|(source,_)|source=="local"||source=="remote") { continue; }
                let installed: bool=tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM expert_package_versions WHERE package_id=?1 AND version=?2)",
                    params![r.package_id,r.version],|row|row.get(0),
                )?;
                // Preserve user rollbacks and enabled-skill choices on every restart.
                // A bundled expert whose `agents` row is gone (removed outside Fox)
                // but whose package history remains is restored below instead of
                // being skipped forever (which left conversations, bindings and
                // the expert library pointing at a missing row).
                let missing=current.is_none();
                if installed && !missing { refresh_compute_bindings(&tx,&r.expert_id,&r.version,now)?; continue; }
                tx.execute(
                    "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at,
                       icon,category,opening_suggestions_json,is_builtin,package_version,package_manifest_json,
                       agent_kind,invocation_mode,visibility,package_source,package_id,package_hash)
                     VALUES (?1,?2,?3,'pi',?4,'configured-model',?5,?5,?6,?7,?8,0,?9,?10,
                       'expert','inline','expert_center','imported',?11,?12)
                     ON CONFLICT(id) DO UPDATE SET name=excluded.name,description=excluded.description,
                       system_prompt=excluded.system_prompt,category=excluded.category,
                       opening_suggestions_json=excluded.opening_suggestions_json,is_builtin=0,
                       package_version=excluded.package_version,package_manifest_json=excluded.package_manifest_json,
                       package_source='imported',package_id=excluded.package_id,package_hash=excluded.package_hash,updated_at=excluded.updated_at",
                    params![r.expert_id,r.name,r.description,r.system_prompt,now,r.icon,r.category,
                        json!(r.opening_suggestions).canonical_string(),r.version,r.package_manifest.canonical_string(),r.package_id,r.package_hash],
                )?;
                if installed {
                    // Row restored from the shipped catalog: keep the existing
                    // package history and make the shipped version the active one.
                    eprintln!("fox.database warn bundled_expert_restored id={} version={}",r.expert_id,r.version);
                    tx.execute("UPDATE expert_package_versions SET status='historical' WHERE expert_id=?1 AND status='active' AND NOT (package_id=?2 AND version=?3)",params![r.expert_id,r.package_id,r.version])?;
                    tx.execute("UPDATE expert_package_versions SET status='active',activated_at=?3 WHERE package_id=?1 AND version=?2 AND status<>'active'",params![r.package_id,r.version,now])?;
                } else {
                tx.execute("UPDATE expert_package_versions SET status='historical' WHERE expert_id=?1 AND status='active'",[&r.expert_id])?;
                tx.execute(
                    "INSERT INTO expert_package_versions(id,expert_id,package_id,version,package_hash,package_json,source,status,created_at,activated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,'builtin','active',?7,?7)",
                    params![format!("expert-package-version-{}",Uuid::new_v4().simple()),r.expert_id,r.package_id,r.version,r.package_hash,r.package.canonical_string(),now],
                )?;
                }
                tx.execute(
                    "INSERT INTO agent_runtime_config(agent_id,scope_type,scope_id,config_key,value_json,source,created_at,updated_at)
                     VALUES (?1,'agent',?1,'skills.enabled',?2,'package',?3,?3)
                     ON CONFLICT(agent_id,scope_type,scope_id,config_key) DO NOTHING",
                    params![r.expert_id,json!(r.enabled_skills).canonical_string(),now],
                )?;
                refresh_compute_bindings(&tx,&r.expert_id,&r.version,now)?;
            }
            tx.commit()
        })
    }
}


fn refresh_compute_bindings(tx: &rusqlite::Transaction<'_>, expert_id: &str, version: &str, now: i64) -> rusqlite::Result<()> {
    let Some(current)=query_agent_record(tx,expert_id)? else {return Ok(());};
    if current.package_version!=version || current.package_manifest["bundledLibrary"]!="agency-zh-fox-v1"
        || !current.package_manifest["allowedTools"].as_array().is_some_and(|tools|tools.iter().any(|tool|tool=="attachment_compute")) { return Ok(()); }
                // Upgrade idle conversations to the newly installed bundled capability.
                // Keep historical binding snapshots and all in-flight runs immutable.
                let mut query=tx.prepare("SELECT b.id FROM conversation_expert_bindings b WHERE b.expert_id=?1 AND b.state='active'
                    AND b.expert_version IN ('1.0.0','1.1.0','1.2.0') AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.conversation_id=b.conversation_id
                    AND r.status IN ('queued','running','waiting_approval'))")?;
                let bindings=query.query_map([expert_id],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                drop(query);
                if !bindings.is_empty() {
                    let expert=query_agent_record(&tx,expert_id)?.ok_or(rusqlite::Error::InvalidQuery)?;
                    let snapshot=agent_package_snapshot(&expert);
                    let hash=package_snapshot_hash(&snapshot);
                    for old in bindings {
                        tx.execute("UPDATE conversation_expert_bindings SET state='replaced',deactivated_at=?2 WHERE id=?1",params![old,now])?;
                        tx.execute("INSERT INTO conversation_expert_bindings(id,conversation_id,expert_id,state,activation_source,expert_version,
                            package_hash,package_snapshot_json,display_snapshot_json,activated_at,deactivated_at)
                            SELECT ?1,conversation_id,expert_id,'active','bundled-capability-upgrade',?2,?3,?4,
                            json_set(display_snapshot_json,'$.packageVersion',?2),?5,NULL FROM conversation_expert_bindings WHERE id=?6",
                            params![Uuid::new_v4().to_string(),version,hash,snapshot.canonical_string(),now,old])?;
                    }
                }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_is_complete_and_reopening_preserves_package_hashes() {
        let path = std::env::temp_dir().join(format!("fox-library-{}.db", Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let skills_dir =
            std::env::temp_dir().join(format!("fox-bundled-skills-{}", Uuid::new_v4()));
        crate::skills::install_bundled_office_skills(&skills_dir).unwrap();
        let skills = crate::skills::scan_skills(&skills_dir, &[]).unwrap();
        assert_eq!(skills.len(), 3);
        assert!(skills.iter().all(|s| s.record.valid));
        let experts = db.list_agents().unwrap();
        assert_eq!(
            experts
                .iter()
                .filter(|a| a.package_manifest["bundledLibrary"] == "agency-zh-fox-v1")
                .count(),
            20
        );
        let reviewer = db.get_agent("fox-reviewer").unwrap().unwrap();
        assert_eq!(reviewer.icon.as_deref(),Some("/avatars/experts/Fox_CodeReviewer.png"));
        db.execute_raw_sql("UPDATE agents SET icon='/custom-reviewer.png' WHERE id='fox-reviewer'").unwrap();
        db.seed_expert_avatars().unwrap();
        assert_eq!(db.get_agent("fox-reviewer").unwrap().unwrap().icon.as_deref(),Some("/custom-reviewer.png"));
        assert!(!reviewer.package_manifest["allowedTools"]
            .as_array()
            .unwrap()
            .contains(&json!("write_file")));
        let conversation = db
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let binding = db
            .bind_conversation_expert(&conversation.id, "fox-reviewer", "test")
            .unwrap();
        db.seed_bundled_experts().unwrap();
        assert_eq!(
            db.current_conversation_expert_binding(&conversation.id)
                .unwrap()
                .unwrap()
                .package_snapshot,
            binding.package_snapshot
        );
        assert_eq!(
            db.get_agent("fox-reviewer").unwrap().unwrap().package_hash,
            reviewer.package_hash
        );
        assert_eq!(db.list_agents().unwrap().len(), experts.len());
        drop(db);
        let reopened = Database::open(path.clone()).unwrap();
        assert_eq!(
            reopened
                .get_agent("fox-reviewer")
                .unwrap()
                .unwrap()
                .package_hash,
            reviewer.package_hash
        );
        drop(reopened);
        fs_cleanup(&path);
    }
    fn fs_cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod compute_upgrade_tests {
    use super::*;
    #[test]
    fn compute_upgrade_refreshes_idle_binding_preserves_active_run_and_user_choices() {
        let root=std::env::temp_dir().join(format!("fox-compute-upgrade-{}",Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let db=Database::open(root.join("test.db")).unwrap();
        let agent=db.get_agent("fox-data-analyst").unwrap().unwrap();
        let installed_version=agent.package_version.clone();
        let mut old=agent.package_manifest;
        old["allowedTools"].as_array_mut().unwrap().retain(|tool|tool!="attachment_compute");
        db.with_connection(|c| {
            c.execute("UPDATE agents SET package_version='1.1.0',package_manifest_json=?1,icon='custom-avatar.png' WHERE id='fox-data-analyst'",[old.canonical_string()])?;
            c.execute("UPDATE expert_package_versions SET version='1.1.0' WHERE expert_id='fox-data-analyst' AND version=?1",[&installed_version])?;
            c.execute("UPDATE agent_runtime_config SET value_json='[\"custom-skill\"]',source='user' WHERE agent_id='fox-data-analyst' AND config_key='skills.enabled'",[])?;
            Ok(())
        }).unwrap();
        let idle=db.create_conversation(db.default_agent_id(),None,None,Some("read_only")).unwrap();
        let running=db.create_conversation(db.default_agent_id(),None,None,Some("read_only")).unwrap();
        let idle_before=db.bind_conversation_expert(&idle.id,"fox-data-analyst","test").unwrap();
        let active_before=db.bind_conversation_expert(&running.id,"fox-data-analyst","test").unwrap();
        db.create_run(&running.id,"active test",None).unwrap();
        assert!(db.conversation_has_active_run(&running.id).unwrap());
        db.seed_bundled_experts().unwrap();
        let idle_after=db.current_conversation_expert_binding(&idle.id).unwrap().unwrap();
        assert_eq!(idle_after.expert_version,installed_version);
        for name in ["compute_job_start","compute_job_status","compute_job_cancel","compute_job_result"] {
            assert!(idle_after.package_snapshot["packageManifest"]["allowedTools"].as_array().unwrap().contains(&json!(name)));
        }
        assert!(idle_after.package_snapshot["packageManifest"]["officeTools"].as_array().unwrap().contains(&json!("office_import_data")));
        assert_ne!(idle_after.id,idle_before.id);
        assert!(idle_after.package_snapshot["packageManifest"]["allowedTools"].as_array().unwrap().iter().any(|tool|tool=="attachment_compute"));
        assert_eq!(db.current_conversation_expert_binding(&running.id).unwrap().unwrap().package_snapshot,active_before.package_snapshot);
        assert_eq!(db.get_agent("fox-data-analyst").unwrap().unwrap().icon.as_deref(),Some("custom-avatar.png"));
        db.with_connection(|c| {
            let prior:String=c.query_row("SELECT package_snapshot_json FROM conversation_expert_bindings WHERE id=?1",[&idle_before.id],|r|r.get(0))?;
            assert_eq!(serde_json::from_str::<Value>(&prior).unwrap(),idle_before.package_snapshot);
            let skills:String=c.query_row("SELECT value_json FROM agent_runtime_config WHERE agent_id='fox-data-analyst' AND config_key='skills.enabled'",[],|r|r.get(0))?;
            assert_eq!(skills,"[\"custom-skill\"]");
            Ok(())
        }).unwrap();
        db.seed_bundled_experts().unwrap();
        assert_eq!(db.current_conversation_expert_binding(&idle.id).unwrap().unwrap().id,idle_after.id);
    }
}

/// Windows retest 13.28: an `agents` row removed outside Fox (sqlite3 shell,
/// foreign keys off) used to abort startup with `Query is not read-only`
/// (`rusqlite::Error::InvalidQuery` from the migration FK gate).
#[cfg(test)]
mod missing_agent_row_tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("fox-missing-agent-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The same open path the setup hook uses.
    fn startup_open(root: &std::path::Path) -> Result<Database, String> {
        Database::open_with_migration_backup(root.join("fox.db"), &root.join("backups"))
    }

    fn create_local_expert(db: &Database) -> String {
        db.save_agent(&SaveAgentRequest {
            id: None,
            name: "本机测试专家".to_owned(),
            description: "用于缺行回归".to_owned(),
            icon: None,
            category: "engineering".to_owned(),
            system_prompt: "Answer briefly.".to_owned(),
            default_model: "configured-model".to_owned(),
            opening_suggestions: vec![],
            package_manifest: json!({"skills": ["project-review"], "allowedTools": ["read"]}),
        })
        .unwrap()
        .id
    }

    /// Mirrors `sqlite3 fox.db "delete from agents where id='…'"`.
    fn delete_agent_row_outside_fox(db: &Database, id: &str) {
        db.with_connection(|c| {
            c.pragma_update(None, "foreign_keys", "OFF")?;
            let deleted = c.execute("DELETE FROM agents WHERE id=?1", [id])?;
            c.pragma_update(None, "foreign_keys", "ON")?;
            assert_eq!(deleted, 1);
            Ok(())
        })
        .unwrap();
    }

    fn agent_fk_violations(db: &Database) -> i64 {
        db.query_count_raw(
            "SELECT COUNT(*) FROM pragma_foreign_key_check WHERE \"table\" IN
               ('conversations','agent_runtime_config','expert_package_versions')",
            &[],
        )
        .unwrap()
    }

    #[test]
    fn startup_tolerates_deleted_local_expert_row() {
        let root = temp_root("plain");
        let db = startup_open(&root).unwrap();
        let expert = create_local_expert(&db);
        assert!(db.query_count_raw("SELECT COUNT(*) FROM agent_runtime_config WHERE agent_id=?1", &[&expert as &dyn rusqlite::ToSql]).unwrap() > 0);
        let agents_before = db.list_agents().unwrap().len();
        delete_agent_row_outside_fox(&db, &expert);
        drop(db);

        let db = startup_open(&root).expect("startup must not fail on a missing local expert row");
        assert!(db.get_agent(&expert).unwrap().is_none(), "a deleted local expert is not resurrected");
        assert_eq!(db.list_agents().unwrap().len(), agents_before - 1);
        assert_eq!(agent_fk_violations(&db), 0);
        assert!(db.get_agent(DEFAULT_AGENT_ID).unwrap().is_some());
        drop(db);
        // A second restart is a no-op.
        let db = startup_open(&root).unwrap();
        assert!(db.repair_missing_agent_references().unwrap().missing_agent_ids.is_empty());
        drop(db);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn startup_repairs_conversations_and_bindings_of_deleted_local_expert() {
        let root = temp_root("bound");
        let db = startup_open(&root).unwrap();
        let expert = create_local_expert(&db);
        // A local primary agent (chat-selector assistant) used directly as a
        // conversation's agent …
        let assistant = create_local_expert(&db);
        db.with_connection(|c| {
            c.execute(
                "UPDATE agents SET agent_kind='assistant',invocation_mode='primary',visibility='chat_selector' WHERE id=?1",
                [&assistant],
            )
            .map(|_| ())
        })
        .unwrap();
        let direct = db.create_conversation(&assistant, Some("直接对话"), None, None).unwrap();
        // … and a general-assistant conversation with the local expert bound.
        let bound = db.create_conversation(DEFAULT_AGENT_ID, Some("绑定对话"), None, None).unwrap();
        let binding = db.bind_conversation_expert(&bound.id, &expert, "test").unwrap();
        delete_agent_row_outside_fox(&db, &assistant);
        delete_agent_row_outside_fox(&db, &expert);
        drop(db);

        let db = startup_open(&root).expect("startup must not fail when conversations still reference the expert");
        let repaired_direct = db.load_conversation(&direct.id).expect("conversation that used the expert opens");
        assert_eq!(repaired_direct.conversation.agent_id, DEFAULT_AGENT_ID);
        let repaired_bound = db.load_conversation(&bound.id).expect("bound conversation opens");
        assert_eq!(repaired_bound.conversation.agent_id, DEFAULT_AGENT_ID);
        assert!(db.current_conversation_expert_binding(&bound.id).unwrap().is_none(), "the dangling binding is no longer active");
        assert_eq!(
            db.query_count_raw("SELECT COUNT(*) FROM conversation_expert_bindings WHERE id=?1 AND state='removed' AND deactivated_at IS NOT NULL", &[&binding.id as &dyn rusqlite::ToSql]).unwrap(),
            1,
            "the binding row and its snapshot are kept as history"
        );
        let listed = db.list_conversations().unwrap();
        assert!(listed.iter().any(|c| c.id == direct.id) && listed.iter().any(|c| c.id == bound.id), "both conversations stay in the sidebar");
        assert_eq!(agent_fk_violations(&db), 0);
        // The conversation can take a new expert again.
        db.bind_conversation_expert(&bound.id, "fox-reviewer", "test").unwrap();
        drop(db);
        let db = startup_open(&root).unwrap();
        assert_eq!(db.current_conversation_expert_binding(&bound.id).unwrap().unwrap().expert_id, "fox-reviewer");
        drop(db);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn startup_reseeds_deleted_bundled_expert_row() {
        let root = temp_root("bundled");
        let db = startup_open(&root).unwrap();
        let before = db.get_agent("fox-reviewer").unwrap().unwrap();
        let conversation = db.create_conversation(DEFAULT_AGENT_ID, None, None, None).unwrap();
        db.bind_conversation_expert(&conversation.id, "fox-reviewer", "test").unwrap();
        delete_agent_row_outside_fox(&db, "fox-reviewer");
        // A built-in (is_builtin=1) agent too.
        delete_agent_row_outside_fox(&db, "fox-debugger");
        drop(db);

        let db = startup_open(&root).expect("startup must re-seed missing built-in/bundled experts");
        let after = db.get_agent("fox-reviewer").unwrap().expect("bundled expert re-seeded");
        assert_eq!(after.package_hash, before.package_hash);
        assert_eq!(after.package_version, before.package_version);
        assert!(db.get_agent("fox-debugger").unwrap().is_some(), "built-in agent re-seeded");
        assert_eq!(
            db.query_count_raw("SELECT COUNT(*) FROM expert_package_versions WHERE expert_id='fox-reviewer' AND status='active'", &[]).unwrap(),
            1
        );
        assert_eq!(
            db.current_conversation_expert_binding(&conversation.id).unwrap().unwrap().expert_id,
            "fox-reviewer",
            "a binding to a re-seeded expert stays active"
        );
        assert_eq!(agent_fk_violations(&db), 0);
        drop(db);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn non_agent_foreign_key_violation_fails_with_readable_message() {
        let root = temp_root("fk");
        let db = startup_open(&root).unwrap();
        db.with_connection(|c| {
            c.pragma_update(None, "foreign_keys", "OFF")?;
            c.execute(
                "INSERT INTO conversation_expert_bindings(id,conversation_id,expert_id,state,activation_source,
                   expert_version,package_hash,display_snapshot_json,activated_at,deactivated_at)
                 VALUES ('orphan-binding','missing-conversation','fox-reviewer','removed','test','1','h','{}',1,1)",
                [],
            )?;
            c.pragma_update(None, "foreign_keys", "ON")
        })
        .unwrap();
        drop(db);
        let error = match startup_open(&root) {
            Ok(_) => panic!("an unrelated FK violation still stops the migration"),
            Err(error) => error,
        };
        assert!(!error.contains("Query is not read-only"), "{error}");
        assert!(error.contains("外键完整性检查失败"), "{error}");
        assert!(error.contains("conversation_expert_bindings → conversations"), "{error}");
        let message = crate::startup_failure::database_open_message(&root.join("fox.db"), &root.join("backups"), &error);
        assert!(message.contains(&root.join("fox.db").display().to_string()), "{message}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

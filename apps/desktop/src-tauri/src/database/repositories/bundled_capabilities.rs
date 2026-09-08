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
                if installed { continue; }
                tx.execute(
                    "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at,
                       icon,category,opening_suggestions_json,is_builtin,package_version,package_manifest_json,
                       agent_kind,invocation_mode,visibility,package_source,package_id,package_hash)
                     VALUES (?1,?2,?3,'pi',?4,'configured-model',?5,?5,?6,?7,?8,0,?9,?10,
                       'expert','inline','expert_center','imported',?11,?12)
                     ON CONFLICT(id) DO UPDATE SET name=excluded.name,description=excluded.description,
                       system_prompt=excluded.system_prompt,icon=excluded.icon,category=excluded.category,
                       opening_suggestions_json=excluded.opening_suggestions_json,is_builtin=0,
                       package_version=excluded.package_version,package_manifest_json=excluded.package_manifest_json,
                       package_source='imported',package_id=excluded.package_id,package_hash=excluded.package_hash,updated_at=excluded.updated_at",
                    params![r.expert_id,r.name,r.description,r.system_prompt,now,r.icon,r.category,
                        json!(r.opening_suggestions).to_string(),r.version,r.package_manifest.to_string(),r.package_id,r.package_hash],
                )?;
                tx.execute("UPDATE expert_package_versions SET status='historical' WHERE expert_id=?1 AND status='active'",[&r.expert_id])?;
                tx.execute(
                    "INSERT INTO expert_package_versions(id,expert_id,package_id,version,package_hash,package_json,source,status,created_at,activated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,'builtin','active',?7,?7)",
                    params![format!("expert-package-version-{}",Uuid::new_v4().simple()),r.expert_id,r.package_id,r.version,r.package_hash,r.package.to_string(),now],
                )?;
                tx.execute(
                    "INSERT INTO agent_runtime_config(agent_id,scope_type,scope_id,config_key,value_json,source,created_at,updated_at)
                     VALUES (?1,'agent',?1,'skills.enabled',?2,'package',?3,?3)
                     ON CONFLICT(agent_id,scope_type,scope_id,config_key) DO UPDATE SET value_json=excluded.value_json,source='package',updated_at=excluded.updated_at",
                    params![r.expert_id,json!(r.enabled_skills).to_string(),now],
                )?;
            }
            tx.commit()
        })
    }
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

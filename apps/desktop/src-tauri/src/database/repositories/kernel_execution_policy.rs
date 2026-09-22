//! Durable live policy versions and full-request idempotent CAS.
use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPolicyState {
    pub version: u64,
    pub mode: String,
}

impl Database {
    pub fn execution_policy(&self, conversation_id: &str) -> Result<ExecutionPolicyState, String> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT version,mode FROM kernel_execution_policies WHERE conversation_id=?1",
                [conversation_id],
                |r| {
                    Ok(ExecutionPolicyState {
                        version: r.get(0)?,
                        mode: r.get(1)?,
                    })
                },
            )
        })
    }
    pub fn change_execution_policy(
        &self,
        conversation_id: &str,
        request_id: &str,
        expected_version: u64,
        desired_mode: &str,
    ) -> Result<ExecutionPolicyState, String> {
        if request_id.is_empty() || !matches!(desired_mode, "read_only" | "ask" | "allow") {
            return Err("invalid policy request".into());
        }
        let digest = fox_engine_protocol::canonical_digest(&[
            ("subject", conversation_id),
            ("expected", &expected_version.to_string()),
            ("desired", desired_mode),
        ]);
        self.with_connection(|c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let prior:Option<(String,u64,String)>=tx.query_row("SELECT request_digest,result_version,result_mode FROM kernel_policy_requests WHERE conversation_id=?1 AND request_id=?2",params![conversation_id,request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((old,version,mode))=prior {
                if old!=digest { return Err(rusqlite::Error::InvalidParameterName("policy_request_identity_conflict".into())); }
                return Ok(ExecutionPolicyState{version,mode});
            }
            let version:u64=tx.query_row("SELECT version FROM kernel_execution_policies WHERE conversation_id=?1",[conversation_id],|r|r.get(0))?;
            if version!=expected_version { return Err(rusqlite::Error::InvalidParameterName("policy_version_conflict".into())); }
            // Project-scoped mode remains shared by its conversations. SQL triggers
            // increment every affected policy and expire unused approval decisions.
            let project:Option<String>=tx.query_row("SELECT project_id FROM conversations WHERE id=?1",[conversation_id],|r|r.get(0))?;
            if let Some(project)=project { tx.execute("UPDATE projects SET permission_mode=?2,updated_at=?3 WHERE id=?1",params![project,desired_mode,now_ms()])?; }
            else { tx.execute("UPDATE conversations SET permission_mode=?2,updated_at=?3 WHERE id=?1",params![conversation_id,desired_mode,now_ms()])?; }
            let result=tx.query_row("SELECT version,mode FROM kernel_execution_policies WHERE conversation_id=?1",[conversation_id],|r|Ok(ExecutionPolicyState{version:r.get(0)?,mode:r.get(1)?}))?;
            tx.execute("INSERT INTO kernel_policy_requests VALUES(?1,?2,?3,?4,?5)",params![conversation_id,request_id,digest,result.version,result.mode])?;
            tx.commit()?; Ok(result)
        })
    }
    /// Reopen only an unconsumed intent. Dispatch identity, once issued, is terminal
    /// for execution purposes even if no process/job exists.
    pub fn reevaluate_execution_intent(
        &self,
        run_id: &str,
        tool_call_id: &str,
    ) -> Result<(), String> {
        self.with_connection(|c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let dispatched:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_effect_outbox WHERE run_id=?1 AND tool_call_id=?2 AND effect_type='dispatch_tool')",params![run_id,tool_call_id],|r|r.get(0))?;
            if dispatched {return Err(rusqlite::Error::InvalidParameterName("intent_consumed".into()));}
            // A policy-version ticket is unique only if a re-evaluation follows
            // an actual policy change. Re-prompting under the same version would
            // let the old UI id alias the replacement ticket.
            let same_version:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_approvals a JOIN runs r ON r.id=a.run_id JOIN kernel_execution_policies p ON p.conversation_id=r.conversation_id WHERE a.run_id=?1 AND a.tool_call_id=?2 AND a.policy_version=p.version)",params![run_id,tool_call_id],|r|r.get(0))?;
            if same_version {return Err(rusqlite::Error::InvalidParameterName("intent_policy_unchanged".into()));}
            let changed=tx.execute("UPDATE kernel_tool_calls SET state='pending' WHERE run_id=?1 AND tool_call_id=?2 AND state IN ('pending','waiting_approval')",params![run_id,tool_call_id])?;
            if changed!=1 {return Err(rusqlite::Error::InvalidParameterName("intent_not_reevaluable".into()));}
            tx.execute(
                "INSERT INTO kernel_approval_history(
                    run_id,tool_call_id,state,requested_at,resolved_at,created_at)
                 SELECT run_id,tool_call_id,state,created_at,decided_at,?3
                   FROM kernel_approvals
                  WHERE run_id=?1 AND tool_call_id=?2",
                params![run_id, tool_call_id, now_ms()],
            )?;
            tx.execute("DELETE FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",params![run_id,tool_call_id])?;
            tx.execute("UPDATE kernel_runs SET state='running' WHERE run_id=?1 AND state='waiting_approval'",[run_id])?;
            tx.commit()
        })
    }
    pub fn revoke_execution_parent(&self, run_id: &str) -> Result<u64, String> {
        self.with_connection(|c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",[run_id],|r|r.get(0))?;
            if !exists {return Err(rusqlite::Error::InvalidParameterName("unknown parent run".into()));}
            tx.execute("INSERT INTO kernel_parent_generations VALUES(?1,1) ON CONFLICT(run_id) DO UPDATE SET generation=generation+1",[run_id])?;
            let generation=tx.query_row("SELECT generation FROM kernel_parent_generations WHERE run_id=?1",[run_id],|r|r.get(0))?;
            tx.commit()?; Ok(generation)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Database, std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("fox-policy-{}.sqlite", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conv = db
            .create_conversation("fox-general", Some("policy test"), None, None)
            .unwrap();
        (db, path, conv.id)
    }
    #[test]
    fn execution_policy_retry_is_bound_to_full_request_before_cas() {
        let (db, _, id) = fixture();
        let old = db.execution_policy(&id).unwrap();
        let next = if old.mode == "allow" { "ask" } else { "allow" };
        let changed = db
            .change_execution_policy(&id, "request-1", old.version, next)
            .unwrap();
        assert_eq!(changed.version, old.version + 1);
        assert_eq!(
            db.change_execution_policy(&id, "request-1", old.version, next)
                .unwrap(),
            changed
        );
        assert!(db
            .change_execution_policy(&id, "request-1", 99, next)
            .is_err());
        assert!(db
            .change_execution_policy(&id, "request-2", old.version, "ask")
            .is_err());
    }
    #[test]
    fn execution_policy_two_connections_cas_one_winner() {
        let (db, path, id) = fixture();
        let initial = db.execution_policy(&id).unwrap();
        let mut alternatives = ["read_only", "ask", "allow"]
            .into_iter()
            .filter(|mode| *mode != initial.mode)
            .collect::<Vec<_>>();
        assert_eq!(alternatives.len(), 2);
        let ours_mode = alternatives.pop().unwrap();
        let theirs_mode = alternatives.pop().unwrap();
        let version = initial.version;
        let other = Database::open(path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b = barrier.clone();
        let i = id.clone();
        let handle = std::thread::spawn(move || {
            b.wait();
            other.change_execution_policy(&i, "other", version, theirs_mode)
        });
        barrier.wait();
        let ours = db.change_execution_policy(&id, "ours", version, ours_mode);
        let theirs = handle.join().unwrap();
        assert_ne!(ours.is_ok(), theirs.is_ok());
        let winner = ours.as_ref().or(theirs.as_ref()).unwrap();
        assert_eq!(winner.version, version + 1);
        assert!(winner.mode == ours_mode || winner.mode == theirs_mode);
    }
    #[test]
    fn execution_policy_legacy_mode_change_increments_generation() {
        let (db, _, id) = fixture();
        let before = db.execution_policy(&id).unwrap();
        db.update_conversation_permission_mode(&id, "allow")
            .unwrap();
        let after = db.execution_policy(&id).unwrap();
        assert_eq!(after.mode, "allow");
        assert_eq!(after.version, before.version + 1);
        db.update_conversation_permission_mode(&id, "ask").unwrap();
        assert_eq!(db.execution_policy(&id).unwrap().version, after.version + 1);
    }
    #[test]
    fn execution_policy_parent_cancellation_increments_durable_generation() {
        let (db, path, id) = fixture();
        let run = db.create_run(&id, "parent", None).unwrap().run.id;
        assert_eq!(db.revoke_execution_parent(&run).unwrap(), 1);
        let reopened = Database::open(path).unwrap();
        assert_eq!(reopened.revoke_execution_parent(&run).unwrap(), 2);
        assert!(reopened.revoke_execution_parent("missing").is_err());
    }
    fn waiting_intent(db: &Database, conversation: &str) -> String {
        let run = db.create_run(conversation, "intent", None).unwrap().run.id;
        db.with_connection(|c|{
   c.execute("INSERT INTO kernel_runs(run_id,engine_id,kernel_mode,capability_manifest_version,permission_snapshot_id,execution_profile_id,prompt_config_hash,frozen_config_json,state,last_event_seq,created_at,updated_at) VALUES(?1,'kernel','authoritative',1,'p','durable','h','{}','waiting_approval',0,1,1)",[&run])?;
   c.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,state,created_at) VALUES(?1,'same-call','batch','write_file',0,'{}','waiting_approval',1)",[&run])?;
   c.execute("INSERT INTO kernel_approvals(run_id,tool_call_id,state,created_at) VALUES(?1,'same-call','allow_once',1)",[&run])?;Ok(())
  }).unwrap();
        run
    }
    #[test]
    fn execution_policy_old_approved_ticket_stays_expired_after_roundtrip() {
        let (db, _, id) = fixture();
        let run = waiting_intent(&db, &id);
        db.update_conversation_permission_mode(&id, "allow")
            .unwrap();
        db.update_conversation_permission_mode(&id, "ask").unwrap();
        let state: String = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT state FROM kernel_approvals WHERE run_id=?1",
                    [&run],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(state, "expired");
        db.reevaluate_execution_intent(&run, "same-call").unwrap();
        let (calls,history):(i64,i64)=db.with_connection(|c|c.query_row("SELECT (SELECT count(*) FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='same-call'),(SELECT count(*) FROM kernel_approval_history WHERE run_id=?1 AND state='expired')",[&run],|r|Ok((r.get(0)?,r.get(1)?)))).unwrap();
        assert_eq!((calls, history), (1, 1));
    }
    #[test]
    fn execution_policy_consumed_intent_cannot_be_reopened() {
        let (db, _, id) = fixture();
        let run = waiting_intent(&db, &id);
        db.with_connection(|c|{c.execute("INSERT INTO kernel_effect_outbox(run_id,effect_key,effect_type,idempotency_key,tool_call_id,batch_id,payload_json,status,attempts,created_at,updated_at) VALUES(?1,'dispatch:same-call','dispatch_tool','dispatch','same-call','batch','{}','pending',0,1,1)",[&run])?;Ok(())}).unwrap();
        assert!(db
            .reevaluate_execution_intent(&run, "same-call")
            .unwrap_err()
            .contains("intent_consumed"));
    }
}

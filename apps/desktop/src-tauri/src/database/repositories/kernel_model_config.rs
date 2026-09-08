use super::{now_ms, Database};
use crate::kernel_model_config::{KernelModelConfig, KERNEL_MODEL_ADAPTER};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())))
}

impl Database {
    /// No automatic backfill: an old Run with only a hash cannot infer its body.
    pub(crate) fn freeze_kernel_model_config(&self, run_id: &str, config: &KernelModelConfig) -> Result<(), String> {
        let hash = config.hash()?;
        let encoded = serde_json::to_string(config).map_err(|_| "invalid Kernel model configuration")?;
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing: Option<(i64, String, String, String)> = transaction.query_row(
                "SELECT schema_version,adapter_version,config_json,config_hash FROM kernel_model_configs WHERE run_id=?1",
                [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
            if let Some((version, adapter, body, stored_hash)) = existing {
                if version != 1 || adapter != KERNEL_MODEL_ADAPTER || body != encoded || stored_hash != hash {
                    return Err(invalid("immutable Kernel model configuration conflict"));
                }
            } else {
                transaction.execute("INSERT INTO kernel_model_configs(run_id,schema_version,adapter_version,config_json,config_hash,created_at)
                    VALUES(?1,1,?2,?3,?4,?5)", params![run_id,KERNEL_MODEL_ADAPTER,encoded,hash,now_ms()])?;
            }
            transaction.commit()
        })
    }

    pub(crate) fn kernel_model_config(&self, run_id: &str) -> Result<KernelModelConfig, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let row: Option<(i64,String,String,String,String,String,String,String,String)> = transaction.query_row(
                "SELECT c.schema_version,c.adapter_version,c.config_json,c.config_hash,r.prompt_config_hash,
                    r.execution_profile_id,r.frozen_config_json,b.binding_json,b.binding_hash
                 FROM kernel_model_configs c JOIN kernel_runs r ON r.run_id=c.run_id
                 JOIN run_control_bindings b ON b.run_id=r.run_id
                 WHERE c.run_id=?1 AND r.kernel_mode='authoritative' AND r.engine_id='pi'
                   AND b.authority='authoritative' AND b.engine_id='pi'",
                [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).optional()?;
            let (version,adapter,body,hash,run_hash,profile,frozen,binding_json,binding_hash) = row.ok_or_else(|| invalid("Kernel model configuration is missing; no current-settings fallback"))?;
            if version != 1 || adapter != KERNEL_MODEL_ADAPTER || body.len() > 1_048_576 {
                return Err(invalid("unsupported Kernel model configuration version or size"));
            }
            let config: KernelModelConfig = serde_json::from_str(&body).map_err(|_| invalid("invalid stored Kernel model configuration"))?;
            let frozen: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen).map_err(|_| invalid("invalid frozen Kernel Run"))?;
            let binding: fox_engine_protocol::RunControlBinding = serde_json::from_str(&binding_json).map_err(|_| invalid("invalid frozen control binding"))?;
            binding.validate().map_err(invalid)?;
            crate::kernel::RunController::validate_config(&frozen).map_err(|_| invalid("invalid frozen Kernel Run"))?;
            if config.hash().map_err(invalid)? != hash || hash != run_hash || hash != frozen.prompt_config_hash
                || config.execution_profile_id != profile || profile != frozen.execution_profile_id
                || profile != binding.execution_profile_id || binding.run_id != run_id
                || frozen.engine_id != "pi" || frozen.kernel_mode != "authoritative"
                || binding.engine_id != "pi" || binding.authority != fox_engine_protocol::ExecutionAuthority::Authoritative
                || binding.permission_snapshot_id != Database::run_control_permission_hash(&binding.permission).map_err(invalid)?
                || binding_hash != format!("sha256:{}", hex::encode(Sha256::digest(binding_json.as_bytes()))) {
                return Err(invalid("stored Kernel model configuration identity/hash mismatch"));
            }
            transaction.commit()?;
            Ok(config)
        })
    }
}

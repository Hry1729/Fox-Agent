//! Frozen initial model context. This is not an Outbox or an engine dispatch.
use super::{kernel_model_config::read_model_config, now_ms, Database};
use fox_engine_protocol::KernelInitialModelInput;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

fn hash(body: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())))
}

impl Database {
    pub(crate) fn kernel_initial_start_state(&self, run_id: &str) -> Result<(KernelInitialModelInput, crate::kernel::RunFrozenConfig, String), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let input = read_input(&transaction, run_id)?;
            let (config, hash): (String, String) = transaction.query_row(
                "SELECT r.frozen_config_json,i.input_hash FROM kernel_runs r JOIN kernel_initial_inputs i ON i.run_id=r.run_id
                 WHERE r.run_id=?1 AND r.state='created' AND r.last_event_seq=0", [run_id], |row| Ok((row.get(0)?,row.get(1)?)))?;
            let config = serde_json::from_str(&config).map_err(|_| invalid("invalid initial Run configuration"))?;
            transaction.commit()?;
            Ok((input, config, hash))
        })
    }

    /// Created-only insertion; exact replays are read-only even after startup.
    /// Never infer missing historical inputs from today's conversation messages.
    pub(crate) fn freeze_kernel_initial_input(
        &self,
        input: &KernelInitialModelInput,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            freeze_initial_input_in_tx(&tx, input)?;
            tx.commit()
        })
    }

    pub(crate) fn kernel_initial_input(
        &self,
        run_id: &str,
    ) -> Result<KernelInitialModelInput, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let input = read_input(&transaction, run_id)?;
            transaction.commit()?;
            Ok(input)
        })
    }
}

pub(super) fn read_input(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
) -> rusqlite::Result<KernelInitialModelInput> {
    let config = read_model_config(transaction, run_id)?;
    let row: Option<(i64,String,String,String,String,Option<String>,String)> = transaction.query_row(
        "SELECT i.schema_version,i.turn_id,i.input_json,i.input_hash,i.prompt_config_hash,r.turn_id,r.state
         FROM kernel_initial_inputs i JOIN kernel_runs r ON r.run_id=i.run_id WHERE i.run_id=?1",
        [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
    ).optional()?;
    let (version, turn_id, body, stored_hash, prompt_hash, run_turn, state) =
        row.ok_or_else(|| invalid("Kernel initial input is missing; no conversation fallback"))?;
    if version != 1 || body.len() > 1_048_576 || hash(&body) != stored_hash {
        return Err(invalid(
            "invalid Kernel initial input version, size or hash",
        ));
    }
    let input: KernelInitialModelInput =
        serde_json::from_str(&body).map_err(|_| invalid("invalid stored Kernel initial input"))?;
    input.validate().map_err(invalid)?;
    if input.run_id != run_id
        || input.turn_id != turn_id
        || input.prompt_config_hash != prompt_hash
        || config.hash().map_err(invalid)? != prompt_hash
        || state != "created" && run_turn.as_deref() != Some(turn_id.as_str())
    {
        return Err(invalid("stored Kernel initial input identity mismatch"));
    }
    Ok(input)
}

pub(super) fn freeze_initial_input_in_tx(transaction: &rusqlite::Transaction<'_>, input: &KernelInitialModelInput) -> rusqlite::Result<()> {
        input.validate().map_err(invalid)?;
        let body = serde_json::to_string(input).map_err(|_| invalid("invalid Kernel initial input"))?;
            if read_model_config(&transaction, &input.run_id)?.hash().map_err(invalid)? != input.prompt_config_hash {
                return Err(invalid("Kernel initial input model configuration mismatch"));
            }
            let existing: Option<String> = transaction.query_row(
                "SELECT input_json FROM kernel_initial_inputs WHERE run_id=?1", [&input.run_id], |row| row.get(0),
            ).optional()?;
            if let Some(existing) = existing {
                if existing != body || read_input(&transaction, &input.run_id)? != *input {
                    return Err(invalid("immutable Kernel initial input conflict"));
                }
            } else {
                transaction.execute(
                    "INSERT INTO kernel_initial_inputs(run_id,schema_version,turn_id,input_json,input_hash,prompt_config_hash,created_at)
                     VALUES(?1,1,?2,?3,?4,?5,?6)",
                    params![input.run_id,input.turn_id,body,hash(&body),input.prompt_config_hash,now_ms()],
                )?;
            }
    Ok(())
}

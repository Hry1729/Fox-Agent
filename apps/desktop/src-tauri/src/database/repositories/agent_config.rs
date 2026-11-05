use super::*;
use fox_agent_backend::{agent_config::{AgentBinding, BindingRecord, BindingSource, LayerResult, VerificationLayer, VerificationReport, VerificationStatus}, port::AgentBackend};

impl Database {
    pub fn app_setting_updated_at(&self, key: &str) -> Result<Option<i64>, String> {
        self.with_connection(|connection| connection.query_row("SELECT updated_at FROM app_settings WHERE key=?1", [key], |row| row.get(0)).optional())
    }

    pub fn agent_binding(&self, backend: AgentBackend) -> Result<Option<BindingRecord>, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT enabled,source,provider_id,endpoint_id,model_id,updated_at FROM agent_bindings WHERE backend=?1",
                [backend.as_str()],
                |row| {
                    let source = match row.get::<_, Option<String>>(1)?.as_deref() {
                        Some("fox_service") => Some(BindingSource::FoxService { provider_id: row.get(2)?, endpoint_id: row.get::<_, Option<String>>(3)?.unwrap_or_default(), model_id: row.get::<_, Option<String>>(4)?.unwrap_or_default() }),
                        Some("native") => Some(BindingSource::Native),
                        _ => None,
                    };
                    let updated_at = row.get(5)?;
                    Ok(BindingRecord { backend, enabled: row.get::<_, i64>(0)? != 0, binding: source.map(|source| AgentBinding { source, updated_at }) })
                },
            ).optional()
        })
    }

    pub fn set_agent_enabled(&self, backend: AgentBackend, enabled: bool) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("INSERT INTO agent_bindings(backend,enabled,updated_at) VALUES(?1,?2,?3) ON CONFLICT(backend) DO UPDATE SET enabled=excluded.enabled", params![backend.as_str(), enabled, now_ms()])?;
            Ok(())
        })
    }

    pub fn save_agent_binding(&self, backend: AgentBackend, binding: &BindingSource) -> Result<(), String> {
        let (source, provider, endpoint, model) = match binding {
            BindingSource::FoxService { provider_id, endpoint_id, model_id } => ("fox_service", provider_id.as_deref(), Some(endpoint_id.as_str()), Some(model_id.as_str())),
            BindingSource::Native => ("native", None, None, None),
        };
        self.with_connection(|connection| {
            connection.execute("INSERT INTO agent_bindings(backend,source,provider_id,endpoint_id,model_id,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(backend) DO UPDATE SET source=excluded.source,provider_id=excluded.provider_id,endpoint_id=excluded.endpoint_id,model_id=excluded.model_id,updated_at=excluded.updated_at", params![backend.as_str(),source,provider,endpoint,model,now_ms()])?;
            Ok(())
        })
    }

    pub fn clear_agent_binding(&self, backend: AgentBackend) -> Result<(), String> {
        self.with_connection(|connection| {
            // The user's enable choice survives clearing a service or login binding.
            connection.execute("UPDATE agent_bindings SET source=NULL,provider_id=NULL,endpoint_id=NULL,model_id=NULL,updated_at=?2 WHERE backend=?1", params![backend.as_str(), now_ms()])?;
            connection.execute("DELETE FROM agent_verifications WHERE backend=?1", [backend.as_str()])?;
            Ok(())
        })
    }

    pub fn save_agent_verification(&self, backend: AgentBackend, report: &VerificationReport) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for layer in &report.layers {
                if layer.status == VerificationStatus::Running { continue; }
                transaction.execute("INSERT INTO agent_verifications(backend,identity,layer,status,latency_ms,code,checked_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(backend,identity,layer) DO UPDATE SET status=excluded.status,latency_ms=excluded.latency_ms,code=excluded.code,checked_at=excluded.checked_at", params![backend.as_str(), report.identity, layer.layer.as_str(), layer.status.as_str(), layer.latency_ms, layer.code, report.checked_at])?;
            }
            transaction.commit()
        })
    }

    pub fn agent_verification(&self, backend: AgentBackend, identity: &str) -> Result<Option<VerificationReport>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT layer,status,latency_ms,code,checked_at FROM agent_verifications WHERE backend=?1 AND identity=?2")?;
            let rows = statement.query_map(params![backend.as_str(),identity], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<i64>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,i64>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() { return Ok(None); }
            let checked_at = rows.iter().map(|row| row.4).max().unwrap_or_default();
            let layers = VerificationLayer::ALL.into_iter().filter_map(|layer| {
                let (_,status,latency,code,_) = rows.iter().find(|row| row.0 == layer.as_str())?;
                let status = match status.as_str() { "passed" => VerificationStatus::Passed, "failed" => VerificationStatus::Failed, _ => VerificationStatus::Skipped };
                Some(LayerResult::new(layer,status,code.as_deref(),*latency))
            }).collect();
            Ok(Some(VerificationReport { identity: identity.into(), checked_at, layers }))
        })
    }
}

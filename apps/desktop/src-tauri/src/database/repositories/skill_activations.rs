//! Persistent audit of which skill content (id/version/hash) a Run actually
//! received — full text injected into the frozen first prompt, or loaded
//! mid-run through the `skill_load` Host tool. Skill instructions never grant
//! tools; these rows only answer "which instruction version did the model see".

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct SkillActivationRecord {
    pub skill_id: String,
    pub version: String,
    pub content_sha256: String,
    pub source: String,
    pub required_tools: Vec<String>,
    pub missing_tools: Vec<String>,
    pub tools_available: bool,
    pub char_count: i64,
    pub byte_count: i64,
    pub created_at: i64,
}

impl Database {
    pub(crate) fn record_skill_activation(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        record: &SkillActivationRecord,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            record_in_connection(conn, run_id, conversation_id, record)?;
            Ok(())
        })
    }

    pub(crate) fn skill_activations(&self, run_id: &str) -> Result<Vec<SkillActivationRecord>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT skill_id, version, content_sha256, source,
                        required_tools_json, missing_tools_json, tools_available,
                        char_count, byte_count, created_at
                 FROM skill_activations WHERE run_id=?1 ORDER BY seq",
            )?;
            let rows = stmt
                .query_map(params![run_id], |row| {
                    let required: String = row.get(4)?;
                    let missing: String = row.get(5)?;
                    Ok(SkillActivationRecord {
                        skill_id: row.get(0)?,
                        version: row.get(1)?,
                        content_sha256: row.get(2)?,
                        source: row.get(3)?,
                        required_tools: serde_json::from_str(&required).unwrap_or_default(),
                        missing_tools: serde_json::from_str(&missing).unwrap_or_default(),
                        tools_available: row.get::<_, i64>(6)? != 0,
                        char_count: row.get(7)?,
                        byte_count: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}

pub(super) fn record_in_connection(conn: &rusqlite::Connection, run_id: &str,
    conversation_id: Option<&str>, record: &SkillActivationRecord) -> rusqlite::Result<()> {
            conn.execute(
                "INSERT OR IGNORE INTO skill_activations(
                    run_id, conversation_id, skill_id, version, content_sha256, source,
                    required_tools_json, missing_tools_json, tools_available,
                    char_count, byte_count, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    run_id,
                    conversation_id,
                    record.skill_id,
                    record.version,
                    record.content_sha256,
                    record.source,
                    serde_json::to_string(&record.required_tools).unwrap_or_else(|_| "[]".into()),
                    serde_json::to_string(&record.missing_tools).unwrap_or_else(|_| "[]".into()),
                    record.tools_available as i64,
                    record.char_count,
                    record.byte_count,
                    record.created_at,
                ],
            )?;
    Ok(())
}

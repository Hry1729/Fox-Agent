use super::{now_ms, Database};
use crate::database::{
    CreateMemoryRequest, MemoryConflictRecord, MemoryEntityRecord, MemoryListRequest,
    MemoryProposalInput, MemoryRecallBundle, MemoryRecallItem, MemoryRecallRecord,
    MemoryRevisionRecord, ResolveMemoryConflictRequest, SetMemoryEnabledRequest,
    UpdateMemoryRequest,
};
use rusqlite::{params, OptionalExtension, Row, Transaction};
use serde_json::Value;
use std::cmp::Ordering;
use uuid::Uuid;

const MEMORY_SELECT: &str =
    "SELECT m.id, m.scope, m.scope_key, m.kind, m.canonical_key, m.content, m.state,
            m.enabled, m.source_conversation_id, m.source_message_id, m.source_run_id,
            m.evidence_excerpt, m.created_by, m.confidence, m.version, m.created_at,
            m.updated_at, m.confirmed_at, m.disabled_at, m.deleted_at,
            (SELECT id FROM memory_conflicts c
             WHERE c.status = 'open'
               AND (c.existing_memory_id = m.id OR c.competing_memory_id = m.id)
             ORDER BY c.created_at DESC LIMIT 1),
            (SELECT COUNT(*) FROM memory_recalls r WHERE r.memory_id = m.id),
            (SELECT MAX(recalled_at) FROM memory_recalls r WHERE r.memory_id = m.id)
     FROM memory_entities m";

impl Database {
    pub fn list_memories(
        &self,
        request: &MemoryListRequest,
    ) -> Result<Vec<MemoryEntityRecord>, String> {
        let agent_id = request.agent_id.as_deref().unwrap_or("").trim().to_owned();
        let project_id = request
            .project_id
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_owned();
        let query = request.query.as_deref().unwrap_or("").trim().to_lowercase();
        self.with_connection(|connection| {
            let sql = format!(
                "{MEMORY_SELECT}
                 WHERE (?1 = 1 OR m.deleted_at IS NULL)
                   AND (?2 = '' OR m.scope = 'global' OR (m.scope = 'agent' AND m.scope_key = ?2))
                   AND (?3 = '' OR m.scope = 'global' OR (m.scope = 'project' AND m.scope_key = ?3))
                   AND (?4 = '' OR lower(m.canonical_key) LIKE '%' || ?4 || '%'
                                OR lower(m.content) LIKE '%' || ?4 || '%')
                 ORDER BY m.deleted_at IS NOT NULL, m.state = 'conflict' DESC,
                          m.updated_at DESC, m.id
                 LIMIT 500"
            );
            connection
                .prepare(&sql)?
                .query_map(
                    params![request.include_deleted, agent_id, project_id, query],
                    memory_from_row,
                )?
                .collect()
        })
    }

    pub fn get_memory(&self, memory_id: &str) -> Result<Option<MemoryEntityRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!("{MEMORY_SELECT} WHERE m.id = ?1"),
                    [memory_id],
                    memory_from_row,
                )
                .optional()
        })
    }

    pub fn create_user_memory(
        &self,
        request: &CreateMemoryRequest,
    ) -> Result<MemoryEntityRecord, String> {
        let scope_key = normalize_scope_key(&request.scope, request.scope_key.as_deref())?;
        let evidence = request.evidence_excerpt.as_deref().unwrap_or("");
        validate_memory_fields(
            &request.scope,
            &request.kind,
            &request.canonical_key,
            &request.content,
            evidence,
        )?;
        self.insert_memory(
            &request.scope,
            &scope_key,
            &request.kind,
            &request.canonical_key,
            &request.content,
            evidence,
            "user",
            1.0,
            None,
            None,
            None,
            true,
        )
    }

    pub fn propose_memory_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
        input: &MemoryProposalInput,
    ) -> Result<MemoryEntityRecord, String> {
        validate_memory_fields(
            &input.scope,
            &input.kind,
            &input.canonical_key,
            &input.content,
            &input.evidence_excerpt,
        )?;
        let (agent_id, project_id): (String, Option<String>) =
            self.with_connection(|connection| {
                connection.query_row(
                    "SELECT agent_id, project_id FROM conversations
                 WHERE id = ?1 AND trashed_at IS NULL",
                    [conversation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })?;
        let scope_key = match input.scope.as_str() {
            "global" => String::new(),
            "agent" => agent_id,
            "project" => project_id
                .ok_or_else(|| "memory.project_scope_requires_authorized_project".to_owned())?,
            _ => return Err("memory.invalid_scope".to_owned()),
        };
        if let Some(message_id) = input.source_message_id.as_deref() {
            let belongs = self.with_connection(|connection| {
                connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND conversation_id = ?2)",
                    params![message_id, conversation_id],
                    |row| row.get::<_, bool>(0),
                )
            })?;
            if !belongs {
                return Err("memory.source_message_outside_conversation".to_owned());
            }
        }
        self.insert_memory(
            &input.scope,
            &scope_key,
            &input.kind,
            &input.canonical_key,
            &input.content,
            &input.evidence_excerpt,
            "agent",
            input.confidence.clamp(0.0, 1.0),
            Some(conversation_id),
            input.source_message_id.as_deref(),
            Some(run_id),
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_memory(
        &self,
        scope: &str,
        scope_key: &str,
        kind: &str,
        canonical_key: &str,
        content: &str,
        evidence_excerpt: &str,
        actor: &str,
        confidence: f64,
        source_conversation_id: Option<&str>,
        source_message_id: Option<&str>,
        source_run_id: Option<&str>,
        user_confirmed: bool,
    ) -> Result<MemoryEntityRecord, String> {
        let canonical_key = canonical_key.trim();
        let content = content.trim();
        let evidence_excerpt = evidence_excerpt.trim();
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT id, content FROM memory_entities
                     WHERE scope = ?1 AND scope_key = ?2
                       AND canonical_key = ?3 COLLATE NOCASE
                       AND state = 'confirmed' AND enabled = 1 AND deleted_at IS NULL
                     ORDER BY updated_at DESC LIMIT 1",
                    params![scope, scope_key, canonical_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((existing_id, existing_content)) = existing.as_ref() {
                if existing_content.trim() == content {
                    let record = load_memory(&transaction, existing_id)?;
                    transaction.commit()?;
                    return Ok(record);
                }
            }

            let id = Uuid::new_v4().to_string();
            let has_conflict = existing.is_some();
            let state = if has_conflict {
                "conflict"
            } else if user_confirmed {
                "confirmed"
            } else {
                "candidate"
            };
            let enabled = user_confirmed && !has_conflict;
            transaction.execute(
                "INSERT INTO memory_entities(
                    id, scope, scope_key, kind, canonical_key, content, state, enabled,
                    source_conversation_id, source_message_id, source_run_id, evidence_excerpt,
                    created_by, confidence, version, created_at, updated_at, confirmed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           ?13, ?14, 1, ?15, ?15, ?16)",
                params![
                    id,
                    scope,
                    scope_key,
                    kind,
                    canonical_key,
                    content,
                    state,
                    enabled,
                    source_conversation_id,
                    source_message_id,
                    source_run_id,
                    evidence_excerpt,
                    actor,
                    confidence,
                    now,
                    if enabled { Some(now) } else { None },
                ],
            )?;
            if let Some((existing_id, _)) = existing {
                transaction.execute(
                    "INSERT INTO memory_conflicts(
                        id, existing_memory_id, competing_memory_id, status, created_at
                     ) VALUES (?1, ?2, ?3, 'open', ?4)",
                    params![Uuid::new_v4().to_string(), existing_id, id, now],
                )?;
            }
            let record = load_memory(&transaction, &id)?;
            insert_revision(
                &transaction,
                &record,
                actor,
                if actor == "agent" {
                    "proposed"
                } else {
                    "created"
                },
                None,
            )?;
            transaction.commit()?;
            Ok(record)
        })
    }

    pub fn confirm_memory(
        &self,
        memory_id: &str,
        expected_version: i64,
    ) -> Result<MemoryEntityRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let before = load_memory(&transaction, memory_id)?;
            if before.deleted_at.is_some() {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if before.open_conflict_id.is_some() {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.resolve_conflict_first".to_owned(),
                ));
            }
            if before.state != "candidate" || before.version != expected_version {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.stale_or_invalid_transition".to_owned(),
                ));
            }
            ensure_no_confirmed_collision(&transaction, &before, memory_id)?;
            transaction.execute(
                "UPDATE memory_entities
                 SET state = 'confirmed', enabled = 1, confirmed_at = ?2,
                     disabled_at = NULL, version = version + 1, updated_at = ?2
                 WHERE id = ?1 AND version = ?3",
                params![memory_id, now, expected_version],
            )?;
            let after = load_memory(&transaction, memory_id)?;
            insert_revision(&transaction, &after, "user", "confirmed", Some(&before))?;
            transaction.commit()?;
            Ok(after)
        })
        .map_err(normalize_memory_error)
    }

    pub fn update_memory(
        &self,
        request: &UpdateMemoryRequest,
    ) -> Result<MemoryEntityRecord, String> {
        let evidence = request.evidence_excerpt.as_deref().unwrap_or("");
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let before = load_memory(&transaction, &request.memory_id)?;
            validate_memory_fields(
                &before.scope,
                &request.kind,
                &request.canonical_key,
                &request.content,
                evidence,
            )
            .map_err(rusqlite::Error::InvalidParameterName)?;
            if before.deleted_at.is_some() || before.version != request.expected_version {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.stale_or_deleted".to_owned(),
                ));
            }
            let changed_identity = before.canonical_key.to_lowercase()
                != request.canonical_key.trim().to_lowercase()
                || before.content.trim() != request.content.trim();
            if before.state == "confirmed" && before.enabled && changed_identity {
                let candidate = MemoryEntityRecord {
                    canonical_key: request.canonical_key.trim().to_owned(),
                    content: request.content.trim().to_owned(),
                    ..before.clone()
                };
                ensure_no_confirmed_collision(&transaction, &candidate, &request.memory_id)?;
            }
            transaction.execute(
                "UPDATE memory_entities
                 SET kind = ?2, canonical_key = ?3, content = ?4, evidence_excerpt = ?5,
                     version = version + 1, updated_at = ?6
                 WHERE id = ?1 AND version = ?7",
                params![
                    request.memory_id,
                    request.kind.trim(),
                    request.canonical_key.trim(),
                    request.content.trim(),
                    evidence.trim(),
                    now,
                    request.expected_version,
                ],
            )?;
            let after = load_memory(&transaction, &request.memory_id)?;
            insert_revision(&transaction, &after, "user", "updated", Some(&before))?;
            transaction.commit()?;
            Ok(after)
        })
        .map_err(normalize_memory_error)
    }

    pub fn set_memory_enabled(
        &self,
        request: &SetMemoryEnabledRequest,
    ) -> Result<MemoryEntityRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let before = load_memory(&transaction, &request.memory_id)?;
            if before.deleted_at.is_some()
                || before.state != "confirmed"
                || before.version != request.expected_version
            {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.stale_or_not_confirmed".to_owned(),
                ));
            }
            if request.enabled {
                ensure_no_confirmed_collision(&transaction, &before, &request.memory_id)?;
            }
            transaction.execute(
                "UPDATE memory_entities
                 SET enabled = ?2, disabled_at = ?3, version = version + 1, updated_at = ?4
                 WHERE id = ?1 AND version = ?5",
                params![
                    request.memory_id,
                    request.enabled,
                    if request.enabled { None } else { Some(now) },
                    now,
                    request.expected_version,
                ],
            )?;
            let after = load_memory(&transaction, &request.memory_id)?;
            insert_revision(
                &transaction,
                &after,
                "user",
                if request.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                Some(&before),
            )?;
            transaction.commit()?;
            Ok(after)
        })
        .map_err(normalize_memory_error)
    }

    pub fn delete_memory(&self, memory_id: &str) -> Result<MemoryEntityRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let before = load_memory(&transaction, memory_id)?;
            if before.deleted_at.is_some() {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.already_deleted".to_owned(),
                ));
            }
            if before.open_conflict_id.is_some() {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.resolve_conflict_before_delete".to_owned(),
                ));
            }
            transaction.execute(
                "UPDATE memory_entities
                 SET enabled = 0, disabled_at = COALESCE(disabled_at, ?2), deleted_at = ?2,
                     version = version + 1, updated_at = ?2
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![memory_id, now],
            )?;
            let after = load_memory(&transaction, memory_id)?;
            insert_revision(&transaction, &after, "user", "deleted", Some(&before))?;
            transaction.commit()?;
            Ok(after)
        })
        .map_err(normalize_memory_error)
    }

    pub fn resolve_memory_conflict(
        &self,
        request: &ResolveMemoryConflictRequest,
    ) -> Result<MemoryEntityRecord, String> {
        if !matches!(
            request.decision.as_str(),
            "keep_existing" | "accept_competing"
        ) {
            return Err("memory.invalid_conflict_decision".to_owned());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let conflict = load_conflict(&transaction, &request.conflict_id)?;
            if conflict.status != "open" {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory.conflict_already_resolved".to_owned(),
                ));
            }
            let existing = load_memory(&transaction, &conflict.existing_memory_id)?;
            let competing = load_memory(&transaction, &conflict.competing_memory_id)?;
            let result_id = if request.decision == "keep_existing" {
                transaction.execute(
                    "UPDATE memory_entities
                     SET state = 'candidate', enabled = 0, disabled_at = ?2,
                         version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    params![competing.id, now],
                )?;
                conflict.existing_memory_id.clone()
            } else {
                transaction.execute(
                    "UPDATE memory_entities
                     SET enabled = 0, disabled_at = ?2, version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    params![existing.id, now],
                )?;
                transaction.execute(
                    "UPDATE memory_entities
                     SET state = 'confirmed', enabled = 1, confirmed_at = ?2,
                         disabled_at = NULL, version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    params![competing.id, now],
                )?;
                conflict.competing_memory_id.clone()
            };
            transaction.execute(
                "UPDATE memory_conflicts
                 SET status = 'resolved', resolution = ?2, resolved_at = ?3
                 WHERE id = ?1 AND status = 'open'",
                params![request.conflict_id, request.decision, now],
            )?;
            let existing_after = load_memory(&transaction, &conflict.existing_memory_id)?;
            let competing_after = load_memory(&transaction, &conflict.competing_memory_id)?;
            insert_revision(
                &transaction,
                &existing_after,
                "user",
                "conflict_resolved",
                Some(&existing),
            )?;
            insert_revision(
                &transaction,
                &competing_after,
                "user",
                "conflict_resolved",
                Some(&competing),
            )?;
            let result = load_memory(&transaction, &result_id)?;
            transaction.commit()?;
            Ok(result)
        })
        .map_err(normalize_memory_error)
    }

    pub fn list_memory_revisions(
        &self,
        memory_id: &str,
    ) -> Result<Vec<MemoryRevisionRecord>, String> {
        self.with_connection(|connection| {
            connection
                .prepare(
                    "SELECT id, memory_id, actor, action, before_json, after_json, created_at
                     FROM memory_revisions WHERE memory_id = ?1
                     ORDER BY created_at DESC, id LIMIT 100",
                )?
                .query_map([memory_id], |row| {
                    Ok(MemoryRevisionRecord {
                        id: row.get(0)?,
                        memory_id: row.get(1)?,
                        actor: row.get(2)?,
                        action: row.get(3)?,
                        before: parse_optional_json(row.get(4)?),
                        after: parse_optional_json(row.get(5)?),
                        created_at: row.get(6)?,
                    })
                })?
                .collect()
        })
    }

    pub fn list_memory_recalls(
        &self,
        memory_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecallRecord>, String> {
        let limit = limit.clamp(1, 200) as i64;
        self.with_connection(|connection| {
            connection
                .prepare(
                    "SELECT id, memory_id, conversation_id, run_id, query, reason, score,
                            rank, evidence_excerpt, recalled_at
                     FROM memory_recalls WHERE memory_id = ?1
                     ORDER BY recalled_at DESC, id LIMIT ?2",
                )?
                .query_map(params![memory_id, limit], |row| {
                    Ok(MemoryRecallRecord {
                        id: row.get(0)?,
                        memory_id: row.get(1)?,
                        conversation_id: row.get(2)?,
                        run_id: row.get(3)?,
                        query: row.get(4)?,
                        reason: row.get(5)?,
                        score: row.get(6)?,
                        rank: row.get(7)?,
                        evidence_excerpt: row.get(8)?,
                        recalled_at: row.get(9)?,
                    })
                })?
                .collect()
        })
    }

    pub fn recall_memories(
        &self,
        conversation_id: &str,
        run_id: Option<&str>,
        query: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<MemoryRecallBundle, String> {
        let query = truncate(query.trim(), 1_000);
        let limit = limit.clamp(1, 20);
        let max_chars = max_chars.clamp(512, 12_000);
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (agent_id, project_id): (String, Option<String>) = transaction.query_row(
                "SELECT agent_id, project_id FROM conversations
                 WHERE id = ?1 AND trashed_at IS NULL",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let candidates = transaction
                .prepare(&format!(
                    "{MEMORY_SELECT}
                     WHERE m.deleted_at IS NULL AND m.state = 'confirmed' AND m.enabled = 1
                       AND (m.scope = 'global'
                            OR (m.scope = 'agent' AND m.scope_key = ?1)
                            OR (m.scope = 'project' AND m.scope_key = COALESCE(?2, '')))
                     ORDER BY m.updated_at DESC LIMIT 500"
                ))?
                .query_map(params![agent_id, project_id], memory_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut ranked = candidates
                .into_iter()
                .filter_map(|memory| {
                    let (score, hits) = memory_score(&query, &memory);
                    if hits == 0 && !matches!(memory.kind.as_str(), "preference" | "identity") {
                        None
                    } else {
                        Some((memory, score, hits))
                    }
                })
                .collect::<Vec<_>>();
            ranked.sort_by(|left, right| {
                right
                    .1
                    .partial_cmp(&left.1)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| right.0.updated_at.cmp(&left.0.updated_at))
            });

            let now = now_ms();
            let mut total_chars = 0usize;
            let mut items = Vec::new();
            for (memory, score, hits) in ranked.into_iter().take(limit) {
                let overhead = memory.canonical_key.chars().count() + 80;
                if total_chars + overhead >= max_chars {
                    break;
                }
                let remaining = max_chars - total_chars - overhead;
                let content = truncate(&memory.content, remaining);
                let reason = format!(
                    "scope={}; kind={}; lexical_hits={hits}; state=confirmed; enabled=true",
                    memory.scope, memory.kind
                );
                let rank = items.len() as i64 + 1;
                transaction.execute(
                    "INSERT INTO memory_recalls(
                        id, memory_id, conversation_id, run_id, query, reason, score, rank,
                        evidence_excerpt, recalled_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        Uuid::new_v4().to_string(),
                        memory.id,
                        conversation_id,
                        run_id,
                        query,
                        reason,
                        score,
                        rank,
                        memory.evidence_excerpt,
                        now,
                    ],
                )?;
                total_chars += overhead + content.chars().count();
                items.push(MemoryRecallItem {
                    id: memory.id,
                    scope: memory.scope,
                    kind: memory.kind,
                    canonical_key: memory.canonical_key,
                    content,
                    evidence_excerpt: memory.evidence_excerpt,
                    reason,
                    score,
                });
            }
            transaction.commit()?;
            Ok(MemoryRecallBundle {
                status: if items.is_empty() {
                    "empty"
                } else {
                    "recalled"
                }
                .to_owned(),
                query,
                items,
                total_chars,
            })
        })
    }
}

fn memory_from_row(row: &Row<'_>) -> rusqlite::Result<MemoryEntityRecord> {
    Ok(MemoryEntityRecord {
        id: row.get(0)?,
        scope: row.get(1)?,
        scope_key: row.get(2)?,
        kind: row.get(3)?,
        canonical_key: row.get(4)?,
        content: row.get(5)?,
        state: row.get(6)?,
        enabled: row.get(7)?,
        source_conversation_id: row.get(8)?,
        source_message_id: row.get(9)?,
        source_run_id: row.get(10)?,
        evidence_excerpt: row.get(11)?,
        created_by: row.get(12)?,
        confidence: row.get(13)?,
        version: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        confirmed_at: row.get(17)?,
        disabled_at: row.get(18)?,
        deleted_at: row.get(19)?,
        open_conflict_id: row.get(20)?,
        recall_count: row.get(21)?,
        last_recalled_at: row.get(22)?,
    })
}

fn load_memory(
    transaction: &Transaction<'_>,
    memory_id: &str,
) -> rusqlite::Result<MemoryEntityRecord> {
    transaction.query_row(
        &format!("{MEMORY_SELECT} WHERE m.id = ?1"),
        [memory_id],
        memory_from_row,
    )
}

fn load_conflict(
    transaction: &Transaction<'_>,
    conflict_id: &str,
) -> rusqlite::Result<MemoryConflictRecord> {
    transaction.query_row(
        "SELECT id, existing_memory_id, competing_memory_id, status, resolution,
                created_at, resolved_at
         FROM memory_conflicts WHERE id = ?1",
        [conflict_id],
        |row| {
            Ok(MemoryConflictRecord {
                id: row.get(0)?,
                existing_memory_id: row.get(1)?,
                competing_memory_id: row.get(2)?,
                status: row.get(3)?,
                resolution: row.get(4)?,
                created_at: row.get(5)?,
                resolved_at: row.get(6)?,
            })
        },
    )
}

fn insert_revision(
    transaction: &Transaction<'_>,
    after: &MemoryEntityRecord,
    actor: &str,
    action: &str,
    before: Option<&MemoryEntityRecord>,
) -> rusqlite::Result<()> {
    let before_json = before
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let after_json = serde_json::to_string(after)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    transaction.execute(
        "INSERT INTO memory_revisions(
            id, memory_id, actor, action, before_json, after_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            Uuid::new_v4().to_string(),
            after.id,
            actor,
            action,
            before_json,
            after_json,
            now_ms(),
        ],
    )?;
    Ok(())
}

fn ensure_no_confirmed_collision(
    transaction: &Transaction<'_>,
    memory: &MemoryEntityRecord,
    except_id: &str,
) -> rusqlite::Result<()> {
    let collision = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM memory_entities
            WHERE id <> ?1 AND scope = ?2 AND scope_key = ?3
              AND canonical_key = ?4 COLLATE NOCASE
              AND state = 'confirmed' AND enabled = 1 AND deleted_at IS NULL
         )",
        params![
            except_id,
            memory.scope,
            memory.scope_key,
            memory.canonical_key
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if collision {
        return Err(rusqlite::Error::InvalidParameterName(
            "memory.confirmed_key_collision".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_scope_key(scope: &str, scope_key: Option<&str>) -> Result<String, String> {
    match scope {
        "global" => Ok(String::new()),
        "agent" | "project" => {
            let value = scope_key.unwrap_or("").trim();
            if value.is_empty() {
                Err("memory.scope_key_required".to_owned())
            } else {
                Ok(value.to_owned())
            }
        }
        _ => Err("memory.invalid_scope".to_owned()),
    }
}

fn validate_memory_fields(
    scope: &str,
    kind: &str,
    canonical_key: &str,
    content: &str,
    evidence_excerpt: &str,
) -> Result<(), String> {
    if !matches!(scope, "global" | "agent" | "project") {
        return Err("memory.invalid_scope".to_owned());
    }
    if !matches!(
        kind,
        "preference" | "identity" | "project" | "workflow" | "fact" | "other"
    ) {
        return Err("memory.invalid_kind".to_owned());
    }
    let key_len = canonical_key.trim().chars().count();
    let content_len = content.trim().chars().count();
    if key_len == 0 || key_len > 160 {
        return Err("memory.invalid_canonical_key".to_owned());
    }
    if content_len == 0 || content_len > 4_000 {
        return Err("memory.invalid_content".to_owned());
    }
    if evidence_excerpt.trim().chars().count() > 1_000 {
        return Err("memory.evidence_too_long".to_owned());
    }
    Ok(())
}

fn memory_score(query: &str, memory: &MemoryEntityRecord) -> (f64, usize) {
    let normalized = query.to_lowercase();
    let haystack = format!(
        "{} {}",
        memory.canonical_key.to_lowercase(),
        memory.content.to_lowercase()
    );
    let mut hits = 0usize;
    if normalized.is_empty() {
        hits = 1;
    } else {
        if haystack.contains(&normalized) || normalized.contains(&memory.content.to_lowercase()) {
            hits += 3;
        }
        for term in normalized
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| term.chars().count() >= 2)
        {
            if haystack.contains(term) {
                hits += 1;
            }
        }
    }
    let scope_bonus = match memory.scope.as_str() {
        "project" => 0.3,
        "agent" => 0.2,
        _ => 0.1,
    };
    let kind_bonus = match memory.kind.as_str() {
        "preference" | "identity" => 0.5,
        "project" | "workflow" => 0.2,
        _ => 0.0,
    };
    (
        hits as f64 + scope_bonus + kind_bonus + memory.confidence * 0.1,
        hits,
    )
}

fn parse_optional_json(value: Option<String>) -> Option<Value> {
    value.and_then(|text| serde_json::from_str(&text).ok())
}

fn normalize_memory_error(error: String) -> String {
    for code in [
        "memory.resolve_conflict_first",
        "memory.stale_or_invalid_transition",
        "memory.stale_or_deleted",
        "memory.stale_or_not_confirmed",
        "memory.resolve_conflict_before_delete",
        "memory.already_deleted",
        "memory.conflict_already_resolved",
        "memory.confirmed_key_collision",
    ] {
        if error.contains(code) {
            return code.to_owned();
        }
    }
    error
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let mut output = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        output.push_str("…");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_database() -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-memory-test-{}.db", Uuid::new_v4()));
        (
            Database::open(path.clone()).expect("open memory database"),
            path,
        )
    }

    #[test]
    fn governs_candidates_conflicts_recall_and_audit() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation("fox-general", Some("Memory"), None, None)
            .expect("create conversation");
        let existing = database
            .create_user_memory(&CreateMemoryRequest {
                scope: "agent".to_owned(),
                scope_key: Some("fox-general".to_owned()),
                kind: "preference".to_owned(),
                canonical_key: "editor".to_owned(),
                content: "The user prefers Vim".to_owned(),
                evidence_excerpt: Some("User explicitly selected Vim".to_owned()),
            })
            .expect("create confirmed memory");
        assert_eq!(existing.state, "confirmed");
        assert!(existing.enabled);

        let started = database
            .create_run(&conversation.id, "Please use my preferred editor", None)
            .expect("create run");
        let recalled = database
            .recall_memories(
                &conversation.id,
                Some(&started.run.id),
                "Please use my preferred editor",
                8,
                6_000,
            )
            .expect("recall memories");
        assert_eq!(recalled.status, "recalled");
        assert_eq!(recalled.items[0].id, existing.id);
        assert_eq!(
            database
                .list_memory_recalls(&existing.id, 20)
                .expect("list recalls")
                .len(),
            1
        );

        let conflicting = database
            .propose_memory_for_run(
                &conversation.id,
                &started.run.id,
                &MemoryProposalInput {
                    scope: "agent".to_owned(),
                    kind: "preference".to_owned(),
                    canonical_key: "editor".to_owned(),
                    content: "The user prefers VS Code".to_owned(),
                    evidence_excerpt: "A later user statement mentioned VS Code".to_owned(),
                    source_message_id: Some(started.user_message.id.clone()),
                    confidence: 0.8,
                },
            )
            .expect("propose conflicting memory");
        assert_eq!(conflicting.state, "conflict");
        assert!(!conflicting.enabled);
        let conflict_id = conflicting
            .open_conflict_id
            .clone()
            .expect("open conflict id");
        let accepted = database
            .resolve_memory_conflict(&ResolveMemoryConflictRequest {
                conflict_id,
                decision: "accept_competing".to_owned(),
            })
            .expect("accept competing memory");
        assert_eq!(accepted.id, conflicting.id);
        assert_eq!(accepted.state, "confirmed");
        assert!(accepted.enabled);
        assert!(!database.get_memory(&existing.id).unwrap().unwrap().enabled);

        let disabled = database
            .set_memory_enabled(&SetMemoryEnabledRequest {
                memory_id: accepted.id.clone(),
                enabled: false,
                expected_version: accepted.version,
            })
            .expect("disable memory");
        assert!(!disabled.enabled);
        let empty = database
            .recall_memories(&conversation.id, None, "editor", 8, 6_000)
            .expect("recall after disable");
        assert_eq!(empty.status, "empty");
        assert!(
            database
                .list_memory_revisions(&accepted.id)
                .expect("list revisions")
                .len()
                >= 3
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}

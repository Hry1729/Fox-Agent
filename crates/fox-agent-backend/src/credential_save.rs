//! Provider save with Key persistence (sandbox.md 13.20 H.3 / 13.21).
//!
//! A provider save touches two stores that cannot share one transaction: the
//! OS credential store (Keys) and the Fox database (the provider record). The
//! order and the undo rules live here, behind the [`KeyStore`] trait, so they
//! are unit tested on any host; the desktop wires the real keyring and
//! database into [`save_with_keys`].
//!
//! Rules:
//! 1. Before any Key is written, every Key the save will touch is read
//!    (snapshot). A store that cannot be read refuses the save: nothing is
//!    written.
//! 2. New / replaced Keys are written before the record (the record must
//!    never point at a Key that does not exist yet).
//! 3. If a Key write or the record save fails, every Key already written is
//!    put back to its snapshot: the previous value is restored, a Key that
//!    did not exist before is deleted. The old record is untouched because
//!    the database save itself failed. No residue is left behind.
//! 4. Clearing a Key happens only after the record is saved, so a failed
//!    save never leaves the old record with its Key already gone. A failed
//!    clear after a successful save is reported as such (the record stays
//!    saved).

use std::collections::BTreeMap;

/// The credential store, keyed by credential id (base URL or
/// `fox-protocol:<provider>:<profile>`).
pub trait KeyStore {
    /// `Ok(None)` when no Key is stored; `Err` only when the store cannot be read.
    fn read(&mut self, id: &str) -> Result<Option<String>, String>;
    fn write(&mut self, id: &str, key: &str) -> Result<(), String>;
    /// Deleting a Key that does not exist is `Ok`.
    fn delete(&mut self, id: &str) -> Result<(), String>;
}

/// What one save does to the Keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPlan {
    /// Written before the record, undone if the save fails. `None` deletes the
    /// Key (an explicit empty protocol Key); it is undone the same way.
    pub before_record: Vec<(String, Option<String>)>,
    /// Deleted only after the record is saved.
    pub after_record: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError<E> {
    /// A Key could not be read before the save; nothing was changed.
    KeySnapshot { id: String, error: String },
    /// A Key write failed; Keys already written were put back.
    KeyWrite { id: String, error: String, rollback: Vec<String> },
    /// The record save failed; Keys written for it were put back.
    Record { error: E, rollback: Vec<String> },
    /// The record is saved, but deleting a Key afterwards failed.
    KeyDeleteAfterSave { id: String, error: String },
}

impl<E> SaveError<E> {
    /// Rollback failures (empty when every Key was put back).
    pub fn rollback_failures(&self) -> &[String] {
        match self {
            SaveError::KeyWrite { rollback, .. } | SaveError::Record { rollback, .. } => rollback,
            _ => &[],
        }
    }
}

/// Put every touched Key back to its snapshot, newest first. Returns the
/// failures (id: error), which the caller surfaces; it never stops early.
fn rollback(store: &mut dyn KeyStore, applied: &[(String, Option<String>)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (id, previous) in applied.iter().rev() {
        let result = match previous {
            Some(value) => store.write(id, value),
            None => store.delete(id),
        };
        if let Err(error) = result {
            failures.push(format!("{id}: {error}"));
        }
    }
    failures
}

/// Run one provider save: Keys first (undoable), then the record, then the
/// Key deletions. See the module rules.
pub fn save_with_keys<T, E>(
    store: &mut dyn KeyStore,
    plan: &KeyPlan,
    save_record: impl FnOnce() -> Result<T, E>,
) -> Result<T, SaveError<E>> {
    // 1. Snapshot (first value wins when an id repeats).
    let mut snapshot: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (id, _) in &plan.before_record {
        if snapshot.contains_key(id) {
            continue;
        }
        match store.read(id) {
            Ok(value) => {
                snapshot.insert(id.clone(), value);
            }
            Err(error) => return Err(SaveError::KeySnapshot { id: id.clone(), error }),
        }
    }
    // 2. Writes, recording what has to be put back.
    let mut applied: Vec<(String, Option<String>)> = Vec::new();
    for (id, value) in &plan.before_record {
        let previous = snapshot.get(id).cloned().unwrap_or(None);
        let result = match value {
            Some(key) => store.write(id, key),
            None => store.delete(id),
        };
        // Recorded even on failure: a failed write may have half-applied.
        applied.push((id.clone(), previous));
        if let Err(error) = result {
            let rollback = rollback(store, &applied);
            return Err(SaveError::KeyWrite { id: id.clone(), error, rollback });
        }
    }
    // 3. The record.
    let saved = match save_record() {
        Ok(saved) => saved,
        Err(error) => {
            let rollback = rollback(store, &applied);
            return Err(SaveError::Record { error, rollback });
        }
    };
    // 4. Deletions after the record is saved.
    for id in &plan.after_record {
        if let Err(error) = store.delete(id) {
            return Err(SaveError::KeyDeleteAfterSave { id: id.clone(), error });
        }
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory store with injectable failures.
    #[derive(Default)]
    struct MemoryStore {
        keys: BTreeMap<String, String>,
        fail_read: Option<String>,
        fail_write: Option<String>,
        fail_delete: Option<String>,
        log: Vec<String>,
    }

    impl KeyStore for MemoryStore {
        fn read(&mut self, id: &str) -> Result<Option<String>, String> {
            if self.fail_read.as_deref() == Some(id) {
                return Err("store locked".into());
            }
            Ok(self.keys.get(id).cloned())
        }
        fn write(&mut self, id: &str, key: &str) -> Result<(), String> {
            self.log.push(format!("write {id}"));
            if self.fail_write.as_deref() == Some(id) {
                return Err("write denied".into());
            }
            self.keys.insert(id.to_owned(), key.to_owned());
            Ok(())
        }
        fn delete(&mut self, id: &str) -> Result<(), String> {
            self.log.push(format!("delete {id}"));
            if self.fail_delete.as_deref() == Some(id) {
                return Err("delete denied".into());
            }
            self.keys.remove(id);
            Ok(())
        }
    }

    /// The provider record in the database (a stand-in for the row).
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Record {
        base_url: String,
        model: String,
    }

    const OLD_URL: &str = "https://api.deepseek.com/v1";
    const NEW_URL: &str = "https://api.deepseek.example/v1";
    const PROTO: &str = "fox-protocol:p1:codex";

    fn store_with_old_keys() -> MemoryStore {
        let mut store = MemoryStore::default();
        store.keys.insert(OLD_URL.into(), "sk-old".into());
        store.keys.insert(PROTO.into(), "sk-proto-old".into());
        store
    }

    fn old_record() -> Record {
        Record { base_url: OLD_URL.into(), model: "deepseek-chat".into() }
    }

    /// Database save that fails (read-only file / locked database): the row
    /// in `db` stays as it was, the way SQLite leaves it on a failed write.
    fn failing_save(db: &Record, next: Record) -> impl FnOnce() -> Result<Record, String> {
        let _unchanged = db.clone();
        move || {
            let _ = next;
            Err("attempt to write a readonly database".to_owned())
        }
    }

    #[test]
    fn record_save_failure_keeps_the_old_key_and_config_without_residue() {
        let mut store = store_with_old_keys();
        let before = store.keys.clone();
        let db = old_record();
        // Replace the Key, change the address (new credential id), replace the
        // protocol Key; the database write then fails.
        let plan = KeyPlan {
            before_record: vec![
                (OLD_URL.into(), Some("sk-new".into())),
                (NEW_URL.into(), Some("sk-new-url".into())),
                (PROTO.into(), Some("sk-proto-new".into())),
            ],
            after_record: vec![],
        };
        let next = Record { base_url: NEW_URL.into(), model: "deepseek-reasoner".into() };
        let result = save_with_keys(&mut store, &plan, failing_save(&db, next));
        match result {
            Err(SaveError::Record { error, rollback }) => {
                assert!(error.contains("readonly"));
                assert!(rollback.is_empty(), "every Key put back: {rollback:?}");
            }
            other => panic!("expected a record failure, got {other:?}"),
        }
        // The old Keys are back, the Key written under the new address is gone.
        assert_eq!(store.keys, before);
        assert!(!store.keys.contains_key(NEW_URL));
        // The record was never changed.
        assert_eq!(db, old_record());
    }

    #[test]
    fn clearing_the_key_happens_only_after_the_record_is_saved() {
        let mut store = store_with_old_keys();
        let before = store.keys.clone();
        let db = old_record();
        let plan = KeyPlan { before_record: vec![], after_record: vec![OLD_URL.into()] };
        let result = save_with_keys(&mut store, &plan, failing_save(&db, old_record()));
        assert!(matches!(result, Err(SaveError::Record { .. })));
        assert_eq!(store.keys, before, "a failed save never deletes the Key");
        assert!(store.log.is_empty(), "the store was not touched: {:?}", store.log);

        // Same plan with a working save: the Key is deleted after it.
        let saved = save_with_keys(&mut store, &plan, || Ok::<_, String>(old_record())).unwrap();
        assert_eq!(saved, old_record());
        assert!(!store.keys.contains_key(OLD_URL));
        assert_eq!(store.keys.get(PROTO).map(String::as_str), Some("sk-proto-old"));
    }

    #[test]
    fn key_write_failure_puts_back_the_keys_already_written_and_skips_the_record() {
        let mut store = store_with_old_keys();
        store.fail_write = Some(PROTO.into());
        let before = store.keys.clone();
        let mut called = false;
        let plan = KeyPlan {
            before_record: vec![
                (OLD_URL.into(), Some("sk-new".into())),
                (PROTO.into(), Some("sk-proto-new".into())),
            ],
            after_record: vec![OLD_URL.into()],
        };
        let result = save_with_keys(&mut store, &plan, || {
            called = true;
            Ok::<_, String>(())
        });
        match result {
            Err(SaveError::KeyWrite { id, rollback, .. }) => {
                assert_eq!(id, PROTO);
                // Restoring PROTO fails too (the store refuses that id), which
                // is reported, never hidden.
                assert_eq!(rollback.len(), 1);
                assert!(rollback[0].starts_with(PROTO));
            }
            other => panic!("expected a Key write failure, got {other:?}"),
        }
        assert!(!called, "the record is not saved after a Key write failed");
        assert_eq!(store.keys, before, "the first Key was put back, PROTO was never changed");
    }

    #[test]
    fn unreadable_store_refuses_before_anything_is_written() {
        let mut store = store_with_old_keys();
        store.fail_read = Some(OLD_URL.into());
        let before = store.keys.clone();
        let plan = KeyPlan { before_record: vec![(OLD_URL.into(), Some("sk-new".into()))], after_record: vec![] };
        let result = save_with_keys(&mut store, &plan, || Ok::<_, String>(()));
        assert!(matches!(result, Err(SaveError::KeySnapshot { .. })));
        assert_eq!(store.keys, before);
        assert!(store.log.is_empty());
    }

    #[test]
    fn explicit_empty_protocol_key_is_restored_when_the_save_fails() {
        let mut store = store_with_old_keys();
        let before = store.keys.clone();
        let db = old_record();
        let plan = KeyPlan { before_record: vec![(PROTO.into(), None)], after_record: vec![] };
        let result = save_with_keys(&mut store, &plan, failing_save(&db, old_record()));
        assert!(matches!(result, Err(SaveError::Record { .. })));
        assert_eq!(store.keys, before);
    }

    #[test]
    fn delete_failure_after_save_reports_the_saved_record_state() {
        let mut store = store_with_old_keys();
        store.fail_delete = Some(OLD_URL.into());
        let plan = KeyPlan { before_record: vec![], after_record: vec![OLD_URL.into()] };
        let result = save_with_keys(&mut store, &plan, || Ok::<_, String>(old_record()));
        assert!(matches!(result, Err(SaveError::KeyDeleteAfterSave { ref id, .. }) if id == OLD_URL));
        assert_eq!(store.keys.get(OLD_URL).map(String::as_str), Some("sk-old"));
    }

    #[test]
    fn successful_save_writes_keys_then_record() {
        let mut store = MemoryStore::default();
        let plan = KeyPlan { before_record: vec![(NEW_URL.into(), Some("sk-new".into()))], after_record: vec![] };
        let saved = save_with_keys(&mut store, &plan, || Ok::<_, String>(7)).unwrap();
        assert_eq!(saved, 7);
        assert_eq!(store.keys.get(NEW_URL).map(String::as_str), Some("sk-new"));
    }
}

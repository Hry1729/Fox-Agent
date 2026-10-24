//! Provider delete with Key reclamation (sandbox.md 13.22).
//!
//! Before 13.22 deleting a provider removed its database record but left the
//! Key it had stored under its `base_url` in the OS credential store: an
//! orphan nobody could see in Fox, and nothing in the audit log said so.
//!
//! Fox is a personal Agent, so the user's decision is simple: **deleting a
//! provider deletes its Keys**. There is deliberately no check whether some
//! other entry happens to use the same address.
//!
//! Rules (tested below with an in-memory [`KeyStore`]; the desktop only wires
//! the real keyring, the real database delete and the real audit log):
//! 1. The record is deleted first. If that fails, or there was no record, no
//!    Key is touched.
//! 2. Only after the record is gone are the Keys deleted: the provider's
//!    `base_url` Key and every protocol Key (`fox-protocol:<provider>:<profile>`).
//!    Deleting a Key that does not exist is fine.
//! 3. A failed Key delete never brings the record back. Every Key is still
//!    attempted (no early stop), and each failure is reported so the caller
//!    writes one audit **warning** naming the Keys left behind.
//! 4. A successful delete writes one audit entry too. Audit details carry ids
//!    and counts only, never a Key value.

use crate::credential_save::KeyStore;

/// Audit event written when the provider and all of its Keys are gone.
pub const AUDIT_PROVIDER_DELETED: &str = "model.provider_deleted";
/// Audit event (warning) written when the record is gone but a Key could not
/// be deleted.
pub const AUDIT_PROVIDER_KEY_DELETE_FAILED: &str = "model.provider_key_delete_failed";

/// The Keys a provider owns: its `base_url` Key first, then one protocol Key
/// per profile, without duplicates. Profiles that never had a separate Key
/// are included on purpose: deleting a missing Key is a no-op, and it also
/// reclaims a Key left over from an older profile setting.
pub fn provider_key_ids<'a>(
    provider_id: &str,
    base_url: &str,
    profile_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut push = |id: String| {
        if !id.trim().is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    };
    push(base_url.to_owned());
    for profile_id in profile_ids {
        push(crate::provider_protocol::credential_id(provider_id, profile_id));
    }
    ids
}

/// One Key that could not be deleted after the record was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDeleteFailure {
    pub id: String,
    pub error: String,
}

/// What a provider delete did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// `false` when there was no record to delete (nothing else was touched).
    pub record_deleted: bool,
    /// Keys removed (or already absent) after the record was deleted.
    pub keys_deleted: Vec<String>,
    /// Keys still in the store because their delete failed.
    pub key_failures: Vec<KeyDeleteFailure>,
}

/// One audit entry for the caller to append.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub event: &'static str,
    /// `true` for the warning entry.
    pub warning: bool,
    pub details: serde_json::Value,
}

impl DeleteOutcome {
    /// The audit entry this delete owes, or `None` when nothing was deleted.
    pub fn audit(&self, provider_id: &str) -> Option<AuditEntry> {
        if !self.record_deleted {
            return None;
        }
        if self.key_failures.is_empty() {
            return Some(AuditEntry {
                event: AUDIT_PROVIDER_DELETED,
                warning: false,
                details: serde_json::json!({
                    "providerId": provider_id,
                    "keysDeleted": self.keys_deleted.len(),
                }),
            });
        }
        Some(AuditEntry {
            event: AUDIT_PROVIDER_KEY_DELETE_FAILED,
            warning: true,
            details: serde_json::json!({
                "providerId": provider_id,
                "recordDeleted": true,
                "keysDeleted": self.keys_deleted.len(),
                "keysLeft": self.key_failures.len(),
                // Credential ids (base URL / fox-protocol id) so the user can
                // find the orphan; never the Key itself.
                "leftIds": self.key_failures.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
                "errors": self.key_failures.iter().map(|f| f.error.as_str()).collect::<Vec<_>>(),
            }),
        })
    }
}

/// Delete the record, then its Keys. See the module rules.
///
/// `delete_record` returns `Ok(true)` when a record was removed, `Ok(false)`
/// when there was none. Its error is returned unchanged and no Key is touched.
pub fn delete_with_keys<E>(
    store: &mut dyn KeyStore,
    key_ids: &[String],
    delete_record: impl FnOnce() -> Result<bool, E>,
) -> Result<DeleteOutcome, E> {
    let record_deleted = delete_record()?;
    let mut outcome = DeleteOutcome { record_deleted, keys_deleted: Vec::new(), key_failures: Vec::new() };
    if !record_deleted {
        return Ok(outcome);
    }
    for id in key_ids {
        match store.delete(id) {
            Ok(()) => outcome.keys_deleted.push(id.clone()),
            Err(error) => outcome.key_failures.push(KeyDeleteFailure { id: id.clone(), error }),
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    #[derive(Default)]
    struct MemoryStore {
        keys: BTreeMap<String, String>,
        fail_delete: Option<String>,
        /// Shared with the record closure so the test sees one ordered log.
        log: Rc<RefCell<Vec<String>>>,
    }

    impl KeyStore for MemoryStore {
        fn read(&mut self, id: &str) -> Result<Option<String>, String> {
            Ok(self.keys.get(id).cloned())
        }
        fn write(&mut self, id: &str, key: &str) -> Result<(), String> {
            self.keys.insert(id.to_owned(), key.to_owned());
            Ok(())
        }
        fn delete(&mut self, id: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("delete {id}"));
            if self.fail_delete.as_deref() == Some(id) {
                return Err("delete denied".into());
            }
            self.keys.remove(id);
            Ok(())
        }
    }

    const URL: &str = "https://api.deepseek.com/v1";
    const OTHER_URL: &str = "https://api.other.example/v1";
    const PROVIDER: &str = "p1";

    fn ids() -> Vec<String> {
        provider_key_ids(PROVIDER, URL, ["codex", "pi"])
    }

    fn store() -> MemoryStore {
        let mut store = MemoryStore::default();
        store.keys.insert(URL.into(), "sk-url".into());
        store.keys.insert("fox-protocol:p1:codex".into(), "sk-codex".into());
        store.keys.insert(OTHER_URL.into(), "sk-other".into());
        store
    }

    #[test]
    fn key_ids_cover_base_url_and_every_profile_without_duplicates() {
        assert_eq!(
            provider_key_ids(PROVIDER, URL, ["codex", "pi", "codex"]),
            vec![URL.to_owned(), "fox-protocol:p1:codex".to_owned(), "fox-protocol:p1:pi".to_owned()]
        );
        assert_eq!(provider_key_ids(PROVIDER, URL, []), vec![URL.to_owned()]);
    }

    #[test]
    fn record_is_deleted_before_any_key() {
        let mut store = store();
        let log = Rc::clone(&store.log);
        let outcome = delete_with_keys(&mut store, &ids(), || {
            log.borrow_mut().push("record".into());
            Ok::<_, String>(true)
        })
        .unwrap();
        let mut expected = vec!["record".to_owned()];
        expected.extend(ids().iter().map(|id| format!("delete {id}")));
        assert_eq!(*store.log.borrow(), expected);
        assert!(outcome.record_deleted);
        assert_eq!(outcome.keys_deleted, ids());
        assert!(outcome.key_failures.is_empty());
        // No orphan: the provider's Keys are gone, another address is untouched.
        assert!(!store.keys.contains_key(URL));
        assert!(!store.keys.contains_key("fox-protocol:p1:codex"));
        assert_eq!(store.keys.get(OTHER_URL).map(String::as_str), Some("sk-other"));
    }

    #[test]
    fn record_delete_failure_touches_no_key() {
        let mut store = store();
        let before = store.keys.clone();
        let result = delete_with_keys(&mut store, &ids(), || Err::<bool, _>("database is locked".to_owned()));
        assert_eq!(result, Err("database is locked".to_owned()));
        assert_eq!(store.keys, before);
        assert!(store.log.borrow().is_empty(), "{:?}", store.log);
    }

    #[test]
    fn missing_record_touches_no_key_and_writes_no_audit() {
        let mut store = store();
        let before = store.keys.clone();
        let outcome = delete_with_keys(&mut store, &ids(), || Ok::<_, String>(false)).unwrap();
        assert!(!outcome.record_deleted);
        assert_eq!(store.keys, before);
        assert!(store.log.borrow().is_empty());
        assert_eq!(outcome.audit(PROVIDER), None);
    }

    #[test]
    fn key_delete_failure_keeps_the_record_deleted_tries_every_key_and_warns() {
        let mut store = store();
        store.fail_delete = Some(URL.into());
        let mut record_exists = true;
        let outcome = delete_with_keys(&mut store, &ids(), || {
            record_exists = false;
            Ok::<_, String>(true)
        })
        .unwrap();
        // No rollback of the record.
        assert!(!record_exists);
        assert!(outcome.record_deleted);
        // The failure did not stop the remaining Keys.
        assert_eq!(store.log.borrow().len(), 3, "{:?}", store.log);
        assert!(!store.keys.contains_key("fox-protocol:p1:codex"));
        assert_eq!(store.keys.get(URL).map(String::as_str), Some("sk-url"));
        assert_eq!(outcome.key_failures, vec![KeyDeleteFailure { id: URL.into(), error: "delete denied".into() }]);

        let audit = outcome.audit(PROVIDER).expect("a warning is owed");
        assert_eq!(audit.event, AUDIT_PROVIDER_KEY_DELETE_FAILED);
        assert!(audit.warning);
        assert_eq!(audit.details["providerId"], PROVIDER);
        assert_eq!(audit.details["recordDeleted"], true);
        assert_eq!(audit.details["keysLeft"], 1);
        assert_eq!(audit.details["leftIds"][0], URL);
    }

    #[test]
    fn successful_delete_writes_one_audit_without_key_values() {
        let mut store = store();
        let outcome = delete_with_keys(&mut store, &ids(), || Ok::<_, String>(true)).unwrap();
        let audit = outcome.audit(PROVIDER).expect("success is audited too");
        assert_eq!(audit.event, AUDIT_PROVIDER_DELETED);
        assert!(!audit.warning);
        assert_eq!(audit.details["providerId"], PROVIDER);
        assert_eq!(audit.details["keysDeleted"], 3);
        let text = audit.details.to_string();
        assert!(!text.contains("sk-"), "audit must not carry Key values: {text}");
    }

    #[test]
    fn shared_address_is_deleted_without_checking_other_entries() {
        // The user's decision for a personal Agent: no "is this address used
        // elsewhere" check. Two providers on one address share one Key, and
        // deleting either one removes it.
        let mut store = store();
        let outcome = delete_with_keys(&mut store, &provider_key_ids("p2", URL, []), || Ok::<_, String>(true)).unwrap();
        assert!(outcome.key_failures.is_empty());
        assert!(!store.keys.contains_key(URL));
    }
}

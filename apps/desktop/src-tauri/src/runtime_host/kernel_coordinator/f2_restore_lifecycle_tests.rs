//! A-F2: request identity and receipt must survive retries and later writes.
//! These cases enter through the real restore executor and use real databases.

use super::*;

fn prepared_versions(label: &str) -> (Database, PathBuf, String, String, String, String) {
    let (db, root, conversation, run) = rev_fixture(label, "allow");
    assert_ne!(
        host_write(&db, &root, &run, "a.txt", "first\n", "missing", "f2-write-v1")["isError"],
        true
    );
    let path = stored_path(&root, "a.txt");
    let first = managed_version_ids(&db, &conversation, &path)
        .first()
        .cloned()
        .expect("first managed version");
    assert_ne!(
        host_write(
            &db,
            &root,
            &run,
            "a.txt",
            "second\n",
            &tool_host::file_version(b"first\n"),
            "f2-write-v2",
        )["isError"],
        true
    );
    let second = managed_version_ids(&db, &conversation, &path)
        .last()
        .cloned()
        .expect("second managed version");
    (db, root, conversation, run, first, second)
}

#[test]
fn f2_same_request_cannot_replay_a_different_version_or_force() {
    let (db, root, conversation, _run, first, second) = prepared_versions("f2-intent");
    let backups = root.join("managed-files");
    let receipt = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &first,
        false,
        "f2-same-identity",
        &managed_files::NoRestoreFault,
    )
    .expect("first restore");

    for (version, force) in [(&second, false), (&first, true)] {
        let error = managed_files::restore_version_with_seam(
            &db,
            &backups,
            &conversation,
            version,
            force,
            "f2-same-identity",
            &managed_files::NoRestoreFault,
        )
        .expect_err("reusing an id with different intent must be rejected");
        assert!(error.contains("identity_conflict"), "got: {error}");
    }
    assert_eq!(
        db.restore_request(&conversation, "f2-same-identity")
            .expect("request")
            .expect("recorded")
            .result_version_id
            .as_deref(),
        Some(receipt.id.as_str())
    );
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
}

#[test]
fn f2_running_retry_does_not_claim_that_the_target_was_untouched() {
    let (db, root, conversation, _run, first, _second) = prepared_versions("f2-running");
    let version = db.managed_file_version(&first).unwrap().unwrap();
    // Simulate an old delivery lost after durable claim. The call below still
    // crosses the real restore entry; only its pre-existing request is seeded.
    db.record_restore_request(
        &conversation,
        "f2-running-request",
        &first,
        &version.storage_path,
        Some(&tool_host::file_version(b"second\n")),
        "restore:f2-running-request",
    )
    .expect("durable running request");
    let reopened = Database::open(root.join("facts.db")).expect("reopen database");
    let error = managed_files::restore_version_with_seam(
        &reopened,
        &root.join("managed-files"),
        &conversation,
        &first,
        false,
        "f2-running-request",
        &managed_files::NoRestoreFault,
    )
    .expect_err("running request is not replayed");
    assert!(error.contains("in_progress_or_uncertain"), "got: {error}");
    assert!(!error.contains("left untouched"), "got: {error}");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"second\n");
}

#[test]
fn f2_later_valid_host_write_does_not_revoke_restore_receipt() {
    struct WriteAfterRestore<'a> {
        db: &'a Database,
        root: &'a Path,
        run: &'a str,
    }
    impl managed_files::RestoreFaultSeam for WriteAfterRestore<'_> {
        fn after_replace(&self) -> Result<(), String> {
            let result = host_write(
                self.db,
                self.root,
                self.run,
                "a.txt",
                "third\n",
                &tool_host::file_version(b"first\n"),
                "f2-write-after-restore",
            );
            if result["isError"] == true {
                return Err(format!("later Host write failed: {result}"));
            }
            Ok(())
        }
    }

    let (db, root, conversation, run, first, _second) = prepared_versions("f2-later-write");
    let backups = root.join("managed-files");
    let receipt = managed_files::restore_version_with_seam(
        &db,
        &backups,
        &conversation,
        &first,
        false,
        "f2-later-write-request",
        &WriteAfterRestore {
            db: &db,
            root: &root,
            run: &run,
        },
    )
    .expect("restore committed before the later write");
    assert_eq!(receipt.change_kind, "restored");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"third\n");
    assert_eq!(
        db.restore_request(&conversation, "f2-later-write-request")
            .unwrap()
            .unwrap()
            .result_version_id
            .as_deref(),
        Some(receipt.id.as_str())
    );
}

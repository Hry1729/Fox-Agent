//! A-F2: request identity and receipt must survive retries and later writes.
//! These cases enter through the real restore executor and use real databases.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
    let recorded = db.restore_request(&conversation, "f2-same-identity")
        .expect("request").expect("recorded");
    assert_eq!(recorded.result_version_id.as_deref(), Some(receipt.id.as_str()));
    assert!(db.settle_restore_request(
        &conversation, "f2-same-identity", "another-owner", "recovery_required", None,
    ).is_err(), "a different owner cannot settle the request");
    assert!(db.settle_restore_request(
        &conversation,
        "f2-same-identity",
        recorded.attempt_owner.as_deref().expect("claimed owner"),
        "recovery_required",
        None,
    ).is_err(), "even the original owner cannot downgrade a terminal receipt");
    assert_eq!(
        db.restore_request(&conversation, "f2-same-identity")
            .unwrap().unwrap().result_version_id.as_deref(),
        Some(receipt.id.as_str()),
    );
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");

    // A v82 row survived migration, but v82 never stored `force`. The new
    // entry must not infer that its intent matches this delivery.
    let legacy_version = db.managed_file_version(&first).unwrap().unwrap();
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_restore_requests(
                conversation_id,request_id,version_id,target_identity,baseline_version,
                dispatch_id,state,created_at)
             VALUES (?1,'f2-legacy-request',?2,?3,NULL,'restore:f2-legacy-request','running',1)",
            rusqlite::params![conversation, first, legacy_version.storage_path],
        )?;
        Ok(())
    }).expect("historical request with unknown force");
    let legacy_error = managed_files::restore_version_with_seam(
        &db, &backups, &conversation, &first, false, "f2-legacy-request",
        &managed_files::NoRestoreFault,
    ).expect_err("v82 intent cannot be inferred");
    assert!(legacy_error.contains("identity_conflict"), "got: {legacy_error}");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
}

#[test]
fn f2_running_retry_does_not_claim_that_the_target_was_untouched() {
    struct Gate {
        calls: AtomicUsize,
        phase: Mutex<(bool, bool)>, // inside commit, release
        changed: Condvar,
    }
    impl managed_files::RestoreFaultSeam for Gate {
        fn before_capture(&self) -> Result<(), String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                let mut phase = self.phase.lock().unwrap();
                phase.0 = true;
                self.changed.notify_all();
                while !phase.1 {
                    let (next, timeout) = self.changed.wait_timeout(phase, Duration::from_secs(10)).unwrap();
                    phase = next;
                    if timeout.timed_out() {
                        return Err("timed out waiting to release restore commit".into());
                    }
                }
            }
            Ok(())
        }
    }
    let (db, root, conversation, _run, first, _second) = prepared_versions("f2-running");
    let retry_db = Database::open(root.join("facts.db")).expect("second Database connection");
    let gate = Arc::new(Gate {
        calls: AtomicUsize::new(0),
        phase: Mutex::new((false, false)),
        changed: Condvar::new(),
    });
    let (first_gate, first_root, first_conversation, first_version) =
        (gate.clone(), root.clone(), conversation.clone(), first.clone());
    let first_delivery = std::thread::spawn(move || {
        managed_files::restore_version_with_seam(
            &db,
            &first_root.join("managed-files"),
            &first_conversation,
            &first_version,
            false,
            "f2-running-request",
            first_gate.as_ref(),
        )
    });
    {
        let phase = gate.phase.lock().unwrap();
        let (phase, timeout) = gate.changed.wait_timeout_while(
            phase,
            Duration::from_secs(10),
            |phase| !phase.0,
        ).unwrap();
        assert!(!timeout.timed_out() && phase.0, "first restore reached claimed commit");
    }
    let retry = managed_files::restore_version_with_seam(
        &retry_db,
        &root.join("managed-files"),
        &conversation,
        &first,
        false,
        "f2-running-request",
        &managed_files::NoRestoreFault,
    );
    assert!(retry_db.settle_restore_request(
        &conversation, "f2-running-request", "another-owner", "recovery_required", None,
    ).is_err(), "a non-owner cannot settle an in-flight restore");
    {
        let mut phase = gate.phase.lock().unwrap();
        phase.1 = true;
        gate.changed.notify_all();
    }
    let error = retry.expect_err("running request is not replayed");
    assert!(error.contains("in_progress_or_uncertain"), "got: {error}");
    assert!(!error.contains("left untouched"), "got: {error}");
    first_delivery.join().expect("restore thread").expect("first restore commits");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
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
    let restored_count = db.managed_file_versions(&conversation, None)
        .unwrap()
        .into_iter()
        .filter(|row| row.change_kind == "restored")
        .count();
    drop(db);
    let reopened = Database::open(root.join("facts.db")).expect("reopen after response loss");
    let replay = managed_files::restore_version_with_seam(
        &reopened,
        &backups,
        &conversation,
        &first,
        false,
        "f2-later-write-request",
        &managed_files::NoRestoreFault,
    ).expect("same request replays its committed receipt");
    assert_eq!(replay.id, receipt.id);
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"third\n");
    assert_eq!(
        reopened.managed_file_versions(&conversation, None).unwrap().into_iter()
            .filter(|row| row.change_kind == "restored").count(),
        restored_count,
    );
}

#[test]
fn f2_failed_receipt_transaction_never_replays_an_uncertain_file_effect() {
    let (db, root, conversation, _run, first, _second) = prepared_versions("f2-settle-fail");
    db.execute_raw_sql(
        "CREATE TRIGGER f2_reject_restore_settlement
         BEFORE UPDATE OF state ON kernel_restore_requests
         BEGIN SELECT RAISE(ABORT, 'injected restore settlement failure'); END;",
    ).expect("install durable failure seam");
    let outcome = managed_files::restore_version_with_seam(
        &db,
        &root.join("managed-files"),
        &conversation,
        &first,
        false,
        "f2-settle-failure",
        &managed_files::NoRestoreFault,
    );
    assert!(outcome.is_err(), "receipt settlement must fail");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
    assert_eq!(
        db.restore_request(&conversation, "f2-settle-failure").unwrap().unwrap().state,
        "running", "failed settlement retains a claimed uncertain request",
    );
    assert_eq!(
        db.managed_file_versions(&conversation, None).unwrap().into_iter()
            .filter(|row| row.change_kind == "restored").count(),
        0, "the restored version and its receipt must roll back together",
    );
    drop(db);
    let reopened = Database::open(root.join("facts.db")).expect("reopen after settlement failure");
    let error = managed_files::restore_version_with_seam(
        &reopened,
        &root.join("managed-files"),
        &conversation,
        &first,
        false,
        "f2-settle-failure",
        &managed_files::NoRestoreFault,
    ).expect_err("uncertain request must never replay the file effect");
    assert!(error.contains("in_progress_or_uncertain"), "got: {error}");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
}

#[test]
fn f2_two_database_connections_claim_one_restore_identity() {
    struct RaceSeam {
        arrived: Arc<(Mutex<usize>, Condvar)>,
        calls: AtomicUsize,
        claimed: Arc<AtomicUsize>,
        phase: Arc<(Mutex<(bool, bool)>, Condvar)>, // loser ready, winner finished
    }
    impl managed_files::RestoreFaultSeam for RaceSeam {
        fn after_claim(&self) {
            self.claimed.fetch_add(1, Ordering::SeqCst);
        }
        fn before_capture(&self) -> Result<(), String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // Both calls passed the initial absence check before either
                // reaches the durable request claim.
                let (lock, changed) = self.arrived.as_ref();
                let mut arrived = lock.lock().unwrap();
                *arrived += 1;
                changed.notify_all();
                let (next, timeout) = changed.wait_timeout_while(
                    arrived, Duration::from_secs(10), |arrived| *arrived < 2,
                ).unwrap();
                arrived = next;
                if timeout.timed_out() && *arrived < 2 {
                    return Err("timed out waiting for competing restore".into());
                }
            }
            Ok(())
        }
        fn after_replace(&self) -> Result<(), String> {
            let (lock, changed) = self.phase.as_ref();
            let mut phase = lock.lock().unwrap();
            if self.calls.load(Ordering::SeqCst) == 1 {
                phase.0 = true;
                changed.notify_all();
                let (next, timeout) = changed.wait_timeout_while(
                    phase, Duration::from_secs(10), |phase| !phase.1,
                ).unwrap();
                phase = next;
                if timeout.timed_out() && !phase.1 {
                    return Err("timed out waiting for winner receipt".into());
                }
            } else {
                let (next, timeout) = changed.wait_timeout_while(
                    phase, Duration::from_secs(10), |phase| !phase.0,
                ).unwrap();
                phase = next;
                if timeout.timed_out() && !phase.0 {
                    return Err("timed out waiting for competing attempt".into());
                }
            }
            Ok(())
        }
    }

    let (db_one, root, conversation, _run, first, _second) = prepared_versions("f2-race");
    let db_two = Database::open(root.join("facts.db")).expect("independent connection");
    let arrived = Arc::new((Mutex::new(0_usize), Condvar::new()));
    let claimed = Arc::new(AtomicUsize::new(0));
    let phase = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let mut handles = Vec::new();
    for db in [db_one, db_two] {
        let (root, conversation, first) = (root.clone(), conversation.clone(), first.clone());
        let seam = Arc::new(RaceSeam {
            arrived: arrived.clone(),
            calls: AtomicUsize::new(0),
            claimed: claimed.clone(),
            phase: phase.clone(),
        });
        let thread_phase = phase.clone();
        handles.push(std::thread::spawn(move || {
            let result = managed_files::restore_version_with_seam(
                &db, &root.join("managed-files"), &conversation, &first, false,
                "f2-racing-id", seam.as_ref(),
            );
            let (lock, changed) = thread_phase.as_ref();
            let mut stage = lock.lock().unwrap();
            if seam.calls.load(Ordering::SeqCst) == 1 {
                stage.0 = true;
            } else {
                stage.1 = true;
            }
            changed.notify_all();
            result
        }));
    }
    let results: Vec<_> = handles.into_iter()
        .map(|handle| handle.join().expect("restore thread"))
        .collect();
    let db = Database::open(root.join("facts.db")).expect("reopen race result");
    let receipt = db.restore_request(&conversation, "f2-racing-id")
        .unwrap().expect("one durable request");
    assert_eq!(receipt.state, "committed", "a loser must not demote the winner");
    assert_eq!(claimed.load(Ordering::SeqCst), 1, "only the claimant may enter the file commit path");
    let result_id = receipt.result_version_id.expect("committed version identity");
    for result in results {
        match result {
            Ok(row) => assert_eq!(row.id, result_id),
            Err(error) => assert!(error.contains("in_progress_or_uncertain"), "got: {error}"),
        }
    }
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first\n");
    assert_eq!(
        db.managed_file_versions(&conversation, None).unwrap().into_iter()
            .filter(|row| row.change_kind == "restored").count(),
        1, "one request may register only one restored version",
    );
}

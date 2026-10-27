use super::*;

/// The native engine flows this file used to drive (Codex app-server and
/// DeepSeek Harness) are retired by the single Pi mainline
/// (`crate::kernel_model_config::RETIRED_KERNEL_ENGINES`).
///
/// A retired engine has no executable Kernel configuration, so no run, worker,
/// approval or restart path can be started from one. Their old rows stay
/// readable: the record deserializes with its frozen engine id and the
/// conversation-level mapping keeps mapping it to the Fox path (pinned by the
/// `legacy` and `host_routing` tests).
#[test]
fn a_retired_native_engine_never_becomes_a_runnable_configuration() {
    for engine in crate::kernel_model_config::RETIRED_KERNEL_ENGINES {
        let mut config = worker_configuration();
        config.engine_id = engine.into();
        let refusal = config.adapter_version().expect_err("a retired engine has no adapter version");
        assert!(
            refusal.starts_with(crate::kernel_model_config::KERNEL_ENGINE_RETIRED_CODE),
            "the refusal carries its stable code: {refusal}"
        );
        assert!(config.hash().is_err(), "a retired engine must never freeze a run");
        assert!(
            crate::kernel_model_config::capture_native_adapter(engine).is_err(),
            "no native binary is captured for a retired engine"
        );
        // Readability: the stored record keeps its identity and round-trips.
        let record: crate::kernel_model_config::KernelModelConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).expect("an old record still reads");
        assert_eq!(record.engine_id, engine);
    }
    assert!(worker_configuration().hash().is_ok(), "the Pi engine still freezes a run");
}

/// Retirement did not remove the identity check: a run frozen with one engine
/// still refuses a configuration that names another one, and its stored
/// configuration is unchanged afterwards.
#[test]
fn a_frozen_run_identity_still_refuses_another_engine() {
    let clock = TestClock::new(1000);
    let config = worker_configuration();
    let (db, root, run) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    for engine in crate::kernel_model_config::RETIRED_KERNEL_ENGINES {
        let mut wrong = config.clone();
        wrong.engine_id = engine.into();
        assert!(
            db.freeze_kernel_model_config(&run, &wrong).is_err(),
            "`{engine}` must not cross a frozen run identity"
        );
    }
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_model_config(&run).unwrap(), config);
}

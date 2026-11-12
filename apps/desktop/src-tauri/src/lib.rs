mod app_capabilities;
mod app_state;
mod artifact_gateway;
mod canonical_json;
mod commands;
mod database;
mod data_paths;
mod companion_notes_files;
mod companion_workspace;
mod data_compute;
mod report_pdf;
mod expert_packages;
mod expert_teams;
mod expert_workflows;
mod external_backend;
mod external_model;
mod agent_config;
mod search_service;
mod external_policy;
mod external_tools;
mod kernel;
mod kernel_compaction;
mod kernel_reconciliation;
mod kernel_model_config;
mod lifecycle_hooks;
#[cfg(feature = "local-embedding")]
mod local_embedding;
mod local_knowledge;
mod local_knowledge_import;
mod local_knowledge_picker;
mod local_knowledge_storage;
mod local_knowledge_vector;
mod maintenance;
mod mcp;
mod mcp_openapi;
mod office;
mod model_reachability;
mod model_service;
mod runtime_host;
mod resource_gateway;
mod script_encoding;
mod skills;
mod startup_failure;
mod system_proxy;
mod tool_guard;
mod tool_host;
mod html_preview;
mod process_jobs;
mod vector_store;
mod work_diagnostics;
mod work_mode_gate;
mod yuxi;
#[cfg(windows)]
mod window_resize;


use app_state::AppState;
use database::Database;
use local_knowledge::LocalKnowledgeStore;
use model_service::ModelServiceClient;
use runtime_host::RuntimeHost;
use tauri::Manager;
use yuxi::YuxiClient;

#[cfg(feature = "local-embedding")]
pub fn local_embedding_smoke_package_exit_code(package_path: &std::ffi::OsStr) -> i32 {
    match local_embedding::smoke_test_package(std::path::Path::new(&package_path)) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(not(feature = "local-embedding"))]
pub fn local_embedding_smoke_package_exit_code(_package_path: &std::ffi::OsStr) -> i32 {
    eprintln!("local embedding is not enabled in this desktop build");
    2
}

pub fn local_knowledge_retrieval_worker_exit_code(
    root: &std::ffi::OsStr,
    request_json: &std::ffi::OsStr,
    zvec_resource_dir: Option<&std::ffi::OsStr>,
) -> i32 {
    let root = std::path::PathBuf::from(root);
    let request_json = request_json.to_string_lossy().into_owned();
    let zvec_resource_dir = zvec_resource_dir.map(std::path::PathBuf::from);
    // KB-1: an orphaned worker (parent gone without a Job, or a hung native
    // call) ends itself instead of holding the index and knowledge.db.
    let _ = std::thread::Builder::new()
        .name("fox-retrieval-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(local_knowledge_vector::RETRIEVAL_WORKER_WATCHDOG);
            eprintln!("[local_knowledge.worker_watchdog] retrieval worker exceeded its lifetime and exits");
            std::process::exit(124);
        });
    let result = match std::thread::Builder::new()
        .name("fox-retrieval-native".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let request =
                serde_json::from_str::<local_knowledge_vector::KnowledgeRetrievalRequest>(
                    &request_json,
                )
                .map_err(|error| {
                    local_knowledge::LocalKnowledgeError::invalid(format!(
                        "无法解析检索隔离请求：{error}"
                    ))
                })?;
            let store = local_knowledge::LocalKnowledgeStore::open_retrieval_worker(
                root,
                zvec_resource_dir.as_deref(),
            )?;
            store.search_retrieval_in_process(request)
        }) {
        Ok(worker) => worker.join().unwrap_or_else(|_| {
            Err(local_knowledge::LocalKnowledgeError::storage(
                "向量检索原生工作线程异常终止",
            ))
        }),
        Err(error) => Err(local_knowledge::LocalKnowledgeError::storage(format!(
            "无法启动向量检索原生工作线程：{error}"
        ))),
    };
    match result {
        Ok(data) => {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "data": data,
                    "errorCode": null,
                    "errorMessage": null,
                })
            );
            0
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "data": null,
                    "errorCode": error.code(),
                    "errorMessage": error.to_string(),
                })
            );
            2
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();
    // An explicit data directory must isolate WebView storage as well as SQLite.
    // Otherwise two development/acceptance instances can share localStorage and
    // cached conversation selections despite using different databases.
    let isolated_windows = std::env::var_os("FOX_DATA_DIR").map(|_| {
        context.config_mut().app.windows.iter_mut().filter(|window|window.create).map(|window| {
            let config = window.clone();
            window.create = false;
            config
        }).collect::<Vec<_>>()
    });
    let app = tauri::Builder::default()
        .register_uri_scheme_protocol("fox-preview", |ctx,request| html_preview::serve(ctx.webview_label(),request))
        .setup(move |app| {
            // Any setup error used to reach tauri's `panic!("Failed to setup app: …")`:
            // with the release `windows` subsystem that is an exit with no window
            // and no visible text. Report it in Chinese (with the data path) and
            // exit instead.
            let mut startup_data_dir: Option<std::path::PathBuf> = None;
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let app_data_dir = match std::env::var_os("FOX_DATA_DIR") {
                Some(path) => std::path::PathBuf::from(path),
                None => app.path().app_data_dir().map_err(|error| {
                    format!("无法取得系统为 Fox 分配的数据目录。请检查当前用户的应用数据目录，或设置 FOX_DATA_DIR 后重试。原因：{error}")
                })?,
            };
            startup_data_dir = Some(app_data_dir.clone());
            std::fs::create_dir_all(&app_data_dir)?;
            let isolated_webview_dir = app_data_dir.join("webviews");
            maintenance::apply_pending_restore(&app_data_dir)?;

            // A pending schema migration is preceded by an automatic
            // consistent copy in `<data>/backups/` (newest five kept).
            let database_path = app_data_dir.join("fox.db");
            let database = Database::open_with_migration_backup(
                database_path.clone(),
                &app_data_dir.join("backups"),
            )
            .map_err(|error| {
                startup_failure::database_open_message(
                    &database_path,
                    &app_data_dir.join("backups"),
                    &error,
                )
            })?;
            database.repair_interrupted_runs()?;
            if let Err(error) = commands::companions::ensure_default(&database, &app_data_dir) {
                eprintln!("无法初始化默认联系人：{error}");
            }
            if let Err(error) = database.cleanup_companion_topic_files(&app_data_dir) {
                eprintln!("联系人专题文件清理未完成：{error}");
            }
            database.audit_interrupted_tasks()?;
            // K1: project the bubbles of every private chat in the background.
            // Each batch holds the database lock only for its own work and
            // yields between batches; a failure is logged and resumed on the
            // next launch, never blocking startup.
            let bubble_backfill_database = database.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("contact-bubble-backfill".to_owned())
                .spawn(move || loop {
                    match bubble_backfill_database.backfill_contact_bubbles_batch(8) {
                        Ok(0) => break,
                        Ok(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                        Err(error) => {
                            eprintln!("联系人气泡后台回填未完成（下次启动续跑）：{error}");
                            break;
                        }
                    }
                })
            {
                eprintln!("无法启动联系人气泡回填线程：{error}");
            }
            let runtime_sessions_dir = app_data_dir.join("runtime-sessions");
            std::fs::create_dir_all(&runtime_sessions_dir)?;
            let attachments_dir = app_data_dir.join("attachments");
            std::fs::create_dir_all(&attachments_dir)?;
            let skills_dir = app_data_dir.join("skills");
            std::fs::create_dir_all(&skills_dir)?;
            office::setup(&database, &app.path().resource_dir()?)?;
            skills::install_bundled_office_skills(&skills_dir)?;
            let yuxi_client = YuxiClient::new()?;
            let runtime_host = RuntimeHost::new(
                app.handle().clone(),
                database.clone(),
                runtime_sessions_dir,
                attachments_dir,
                skills_dir.clone(),
                yuxi_client.clone(),
            );
            runtime_host.recover_active_graph_node_reviews()?;
            runtime_host.recover_active_graph_node_cancellations()?;
            runtime_host.recover_pending_graph_acceptances()?;
            let yuxi_runtime = yuxi::runtime::YuxiRuntimeHost::new(
                app.handle().clone(),
                database.clone(),
                yuxi_client.clone(),
            );
            yuxi_runtime.recover_pending_runs_detached();
            // External Agent backends (pi / Codex / DeepSeek over ACP). Built
            // whatever FOX_EXTERNAL_BACKENDS says: with the administrator's
            // switch forcing them off no route ever reaches it; startup only interrupts runs a previous
            // process left active and recovers Windows ancestor ACEs.
            app.manage(external_backend::ExternalBackendHost::new(
                app.handle().clone(),
                database.clone(),
                &app_data_dir,
            ));
            let model_service_client = ModelServiceClient::new()?;
            let configured_local_knowledge_root =
                local_knowledge_storage::database_storage_path(&database)?;
            let local_knowledge_root = local_knowledge_storage::resolve_startup_root(
                &app_data_dir,
                configured_local_knowledge_root.as_deref(),
            );
            // zvec-rust links to zvec_c_api.dll at process load time on Windows, so the
            // bundled DLL must live in the resource root beside the executable. Keeping
            // this path identical to the bundle destination also makes the later health
            // probe report the same library that Windows loaded during startup.
            //
            // An unwritable storage path (read-only directory or database,
            // path is a file) must not abort setup: the store starts degraded
            // (read-only or empty), a warning is logged, and the library page
            // and data-maintenance diagnostics show 「知识库路径不可写：<路径>」
            // with a way to choose another path.
            // A configured path that cannot be resolved (e.g. write access
            // denied) also starts degraded at that path; only an unset
            // path uses the default root.
            let local_knowledge = LocalKnowledgeStore::open_for_startup_root(
                &local_knowledge_root,
                Some(app.path().resource_dir()?.as_path()),
            );
            if local_knowledge.startup_issue().is_none() {
                if let Err(error) = local_knowledge.ensure_default_fox_guide() {
                    eprintln!("fox.local_knowledge warn default_guide_failed error={error}");
                }
            }
            app.manage(AppState::new(
                database,
                runtime_host,
                yuxi_client,
                yuxi_runtime,
                model_service_client,
                local_knowledge,
                app_data_dir,
                skills_dir,
            ));
            // AppState 注册之后才能派发：调度器拉起的 Run 会调用 app.state::<AppState>()。
            commands::companion_deletions::resume_pending(&app.state::<AppState>());
            app.state::<AppState>().runtime_host.start_digital_colleague_scheduler();
            // Not fatal: a broken preview cache directory (e.g. a file at
            // cache/knowledge-preview) is logged here and reported with its
            // path only when a preview uses the cache.
            let _ = maintenance::recover_knowledge_preview_cache_at_startup(&app.state::<AppState>());
            let retired_commands = app.state::<AppState>().runtime_host.retire_native_approval_commands()?;
            if retired_commands > 0 {
                eprintln!("fox.approval_route retired {retired_commands} stale native Kernel approval command(s)");
            }
            app.state::<AppState>().runtime_host.recover_native_approvals_detached()?;
            if let Some(windows) = &isolated_windows {
                for config in windows {
                    tauri::WebviewWindowBuilder::from_config(app,config)?
                        .data_directory(isolated_webview_dir.join(&config.label)).build()?;
                }
            }
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                window_resize::install(&window)?;
            }
            Ok(())
            })();
            if let Err(error) = result {
                let message =
                    startup_failure::setup_failure_message(startup_data_dir.as_deref(), &error.to_string());
                startup_failure::report(&message);
                std::process::exit(1);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            html_preview::html_preview_create,
            html_preview::html_preview_release,
            commands::runtime_initialize,
            commands::runtime_status,
            commands::runtime_diagnostics,
            commands::diagnostics_export,
            commands::diagnose_work_state,
            commands::export_work_trace,
            commands::backup_create,
            commands::backup_restore,
            commands::data_cleanup,
            commands::knowledge_preview_cache_statistics,
            commands::knowledge_preview_cache_limit_set,
            commands::knowledge_preview_cache_clear,
            commands::knowledge_preview_cache_operation_register,
            commands::knowledge_preview_cache_operation_finish,
            commands::knowledge_preview_cache_cancel,
            commands::agents_list,
            commands::agent_save,
            commands::agent_copy,
            commands::companions::companions_list,
            commands::companions::companion_ensure_default,
            commands::companions::companion_add_from_expert,
            commands::companions::companion_create_blank,
            commands::companions::companion_open,
            commands::companions::companion_update_profile,
            commands::companions::companion_set_model,
            commands::companions::companion_capabilities_get,
            commands::companions::companion_capability_set,
            commands::companion_files::companion_files_list,
            commands::companion_files::companion_file_open,
            commands::companion_files::companion_file_save,
            commands::companions::companion_set_pinned,
            commands::companions::companion_mark_read,
            commands::companions::companion_mark_unread,
            commands::companions::companion_set_reminder_mode,
            commands::companions::companion_quote_preview,
            commands::companions::companion_bubble_favorite,
            commands::companions::companion_bubble_reaction,
            commands::companions::companion_favorites_list,
            commands::companions::companion_archive,
            commands::companions::companion_restore,
            commands::agent_delete,
            expert_packages::expert_package_inspect,
            expert_packages::expert_package_install,
            expert_packages::expert_package_export,
            expert_packages::expert_package_versions,
            expert_packages::expert_package_rollback,
            expert_workflows::expert_workflow_get,
            expert_workflows::expert_workflow_start,
            expert_workflows::expert_workflow_gate_resolve,
            expert_workflows::expert_workflow_cancel,
            expert_teams::expert_team_get,
            expert_teams::expert_team_cancel,
            commands::skills_list,
            commands::skill_set_enabled,
            commands::mcp_servers_list,
            commands::mcp_server_save,
            commands::mcp_server_test,
            commands::mcp_server_set_enabled,
            commands::mcp_server_delete,
            commands::lifecycle_hooks_list,
            commands::lifecycle_hook_save,
            commands::lifecycle_hook_set_enabled,
            commands::lifecycle_hook_delete,
            commands::conversations_list,
            commands::conversations_archived_list,
            commands::conversations_trashed_list,
            commands::usage_statistics,
            commands::observability_statistics,
            commands::run_ui_metric_record,
            commands::offline_evaluation_run,
            commands::user_profile_get,
            commands::user_profile_save,
            commands::companion_tasks::companion_task_create,
            commands::companion_tasks::companion_task_update,
            commands::companion_tasks::companion_task_list,
            commands::companion_tasks::companion_task_history,
            commands::companion_tasks::companion_task_pause,
            commands::companion_tasks::companion_task_resume,
            commands::companion_tasks::companion_task_run_now,
            commands::companion_tasks::companion_task_delete,
            commands::companion_notes::companion_notes_get,
            commands::companion_notes::companion_note_save_user,
            commands::companion_notes::companion_note_set_enabled,
            commands::companion_notes::companion_note_draft_save,
            commands::companion_notes::companion_note_draft_to_topic,
            commands::companion_notes::companion_topic_read,
            commands::companion_notes::companion_topic_save,
            commands::companion_notes::companion_topic_delete,
            commands::companion_notes::companion_note_request_resolve,
            commands::companion_notes::companion_shared_request_resolve,
            commands::companion_notes::companion_memory_undo,
            commands::companion_notes::companion_note_move_undo,
            commands::companion_notes::companion_note_delete,
            commands::companion_notes::companion_note_clear,
            commands::companion_notes::companion_note_confirm,
            commands::companion_notes::companion_note_conflict_resolve,
            commands::companion_notes::companion_grants_list,
            commands::companion_notes::companion_grant_project,
            commands::companion_notes::companion_revoke_project,
            commands::companion_notes::companion_project_note_options,
            commands::companion_notes::companion_project_note_reference_set,
            commands::memories_list,
            commands::memory_create,
            commands::memory_confirm,
            commands::memory_update,
            commands::memory_set_enabled,
            commands::memory_delete,
            commands::memory_conflict_resolve,
            commands::memory_revisions_list,
            commands::memory_recalls_list,
            commands::projects_list,
            commands::projects_set_pinned,
            app_capabilities::projects_management_list,
            app_capabilities::project_path_update,
            app_capabilities::project_archive,
            app_capabilities::project_root_open,
            companion_workspace::companion_workspace_repair,
            companion_workspace::companion_import_files,
            companion_workspace::companion_export_file,
            commands::companion_deletions::companion_delete_preview,
            commands::companion_deletions::companion_delete,
            commands::companion_deletions::companion_delete_resume,
            commands::companion_deletions::companion_retained_list,
            commands::companion_deletions::companion_retained_open,
            commands::companion_deletions::companion_retained_dismiss,
            app_capabilities::app_notifications_list,
            app_capabilities::app_notification_read,
            app_capabilities::app_notification_publish,
            app_capabilities::app_notifications_mark_all_read,
            app_capabilities::app_notifications_clear_read,
            app_capabilities::notification_preferences_get,
            app_capabilities::notification_preferences_save,
            app_capabilities::message_feedback_save,
            app_capabilities::app_global_search,
            commands::project_delete,
            commands::project_folder_pick,
            commands::project_folder_open,
            commands::external_url_open,
            commands::project_files_list,
            commands::project_file_read,
            commands::project_file_action,
            commands::artifact_inspect,
            commands::artifact_action,
            commands::project_permission_update,
            commands::conversation_permission_state,
            commands::conversation_permission_update,
            commands::permission_rules_list,
            commands::permission_rule_revoke,
            commands::approval_allow_project_rule,
            commands::permission_audit_list,
            commands::conversation_create,
            commands::conversation_expert_bind,
            commands::conversation_expert_remove,
            commands::conversation_load,
            commands::conversation_history,
            commands::conversations_search,
            commands::companion_chat_search,
            commands::conversation_rename,
            commands::conversation_change_project,
            commands::conversation_pin,
            commands::conversation_archive,
            commands::conversation_unarchive,
            commands::conversation_restore,
            commands::conversation_fork,
            commands::conversation_reset_context,
            commands::companion_context_repair,
            commands::conversation_delete,
            commands::conversation_purge,
            commands::attachments_save,
            commands::knowledge_bindings_set,
            commands::run_start,
            commands::run_rewind,
            commands::run_resume,
            commands::kernel_reconciliation_load,
            commands::kernel_reconciliation_options,
            commands::kernel_reconciliation_query,
            commands::kernel_reconciliation_confirm,
            commands::kernel_reconciliation_resume,
            commands::kernel_reliability_snapshot,
            commands::kernel_job_execute,
            commands::kernel_job_start,
            commands::kernel_job_status,
            commands::kernel_job_list,
            commands::kernel_job_cancel,
            commands::kernel_job_resume,
            commands::kernel_job_reconcile_orphans,
            commands::run_cancel,
            commands::child_run_cancel_by_user,
            commands::approval_resolve,
            // Package 2-D U1: accepting an unverified delivery. A separate command
            // from `approval_resolve` on purpose — that one grants tool authority,
            // this one records that a user took responsibility for content that is
            // not verified, and the two must never share a resolution path.
            commands::delivery_acceptance_resolve,
            commands::delivery_acceptance_state,
            commands::run_steering_enqueue,
            commands::run_steering_list,
            commands::run_steering_discard,
            commands::managed_file_versions_list,
            commands::managed_file_restore,
            commands::plan_revision_resolve,
            commands::work_mode_confirmation_resolve,
            commands::goal_delete,
            commands::goal_running_set,
            commands::yuxi_service_get,
            commands::yuxi_service_save,
            commands::yuxi_service_test,
            commands::yuxi_login,
            commands::yuxi_current_user,
            commands::yuxi_agents_sync,
            commands::yuxi_models_list,
            commands::knowledge_bases_list,
            commands::knowledge_detail,
            commands::knowledge_document_content,
            commands::knowledge_document_activity_get,
            commands::knowledge_document_reading_state_save,
            commands::knowledge_document_bookmark_save,
            commands::knowledge_document_bookmark_delete,
            commands::knowledge_document_annotation_save,
            commands::knowledge_document_annotation_delete,
            commands::knowledge_document_source_metadata,
            commands::knowledge_document_range,
            commands::knowledge_preview_cache_acquire,
            commands::knowledge_preview_cache_read,
            commands::knowledge_preview_cache_release,
            commands::knowledge_preview_cache_open,
            commands::knowledge_document_download,
            commands::knowledge_document_download_cancel,
            commands::downloaded_file_open,
            commands::knowledge_query,
            commands::knowledge_graph,
            commands::knowledge_graph_labels,
            commands::knowledge_graph_stats,
            commands::model_service_get,
            commands::model_service_save,
            commands::model_service_test,
            commands::model_providers_list,
            commands::model_provider_save,
            commands::model_provider_delete,
            search_service::search_service_get,
            search_service::search_service_save,
            agent_config::agent_config_overview,
            agent_config::agent_enable_set,
            agent_config::agent_binding_save,
            agent_config::agent_binding_clear,
            agent_config::agent_config_verify,
            agent_config::agent_config_verify_cancel,
            agent_config::verification_permissions::agent_config_verify_answer,
            agent_config::agent_import_preview,
            agent_config::agent_import_apply,
            // Four-agent plan v0.3.1, Host wiring first batch: backend picker state.
            commands::agent_backend_routing::agent_backends_status,
            // External backend wiring batch: effective capability dimensions.
            commands::agent_backend_routing::agent_backend_capabilities,
            commands::agent_backend_routing::agent_backend_network_status,
            commands::agent_backend_routing::agent_backend_preview_action,
            // UX batch ①②: login guidance / import, permission mode, revocation.
            commands::agent_backend_routing::agent_backend_credentials,
            commands::agent_backend_routing::agent_backend_open_private_home,
            commands::agent_backend_routing::agent_backend_permission_status,
            commands::agent_backend_routing::agent_backend_revoke_full_access,
            commands::agent_backend_routing::agent_backend_native_permissions,
            commands::agent_backend_routing::agent_backend_native_permission_set,
            commands::agent_backend_routing::agent_backend_approval_modes,
            commands::agent_backend_routing::agent_backend_preferences_get,
            commands::agent_backend_routing::agent_backend_preferences_save,
            commands::agent_backend_routing::agent_backend_probe_codex_responses,
            commands::plugin_center::plugin_catalog_list,
            commands::plugin_center::plugin_installations_list,
            commands::plugin_center::plugin_set_activation,
            local_knowledge::local_knowledge_bases_list,
            local_knowledge::local_knowledge_file_sources_list,
            local_knowledge::local_knowledge_file_source_add,
            local_knowledge::local_knowledge_file_source_rescan,
            local_knowledge::local_knowledge_file_source_update,
            local_knowledge::local_knowledge_file_source_remove,
            local_knowledge::local_knowledge_local_files_list,
            local_knowledge::local_knowledge_local_file_read,
            local_knowledge::local_knowledge_local_file_open,
            local_knowledge::local_knowledge_base_get,
            local_knowledge::local_knowledge_base_create,
            local_knowledge::local_knowledge_base_update,
            local_knowledge::local_knowledge_base_delete,
            local_knowledge::local_knowledge_documents_list,
            local_knowledge::local_knowledge_folders_list,
            local_knowledge::local_knowledge_folder_create,
            local_knowledge::local_knowledge_document_file_read,
            local_knowledge::local_knowledge_document_file_open,
            local_knowledge::local_knowledge_documents_import_start,
            local_knowledge_picker::local_knowledge_import_files_pick,
            local_knowledge_picker::local_knowledge_import_folder_pick,
            local_knowledge_picker::local_knowledge_source_folder_pick,
            local_knowledge::local_knowledge_jobs_list,
            local_knowledge::local_knowledge_job_get,
            local_knowledge::local_knowledge_job_cancel,
            local_knowledge::local_knowledge_job_retry,
            local_knowledge::local_knowledge_storage_status,
            local_knowledge::local_knowledge_storage_path_reset,
            local_knowledge_storage::local_knowledge_storage_directory_pick,
            local_knowledge::local_knowledge_storage_migrate_start,
            local_knowledge::local_knowledge_storage_migration_get,
            local_knowledge_vector::local_embedding_models_list,
            local_knowledge_vector::local_embedding_model_install_start,
            local_knowledge_vector::local_embedding_model_download_cancel,
            local_knowledge_vector::local_embedding_model_download_retry,
            local_knowledge_vector::local_embedding_model_test,
            local_knowledge_vector::local_embedding_model_set_default,
            local_knowledge_vector::local_embedding_model_delete,
            local_knowledge_vector::local_embedding_model_import,
            local_knowledge_vector::local_vector_backend_health,
            local_knowledge_vector::local_knowledge_index_start,
            local_knowledge_vector::local_knowledge_retrieval_test,
            local_knowledge_vector::local_knowledge_retrieval_cases_list,
            local_knowledge_vector::local_knowledge_retrieval_case_save,
            local_knowledge_vector::local_knowledge_retrieval_case_delete,
            local_knowledge_vector::local_knowledge_retrieval_cases_export,
            local_knowledge_vector::local_knowledge_document_chunks,
            local_knowledge_vector::local_knowledge_document_delete,
            local_knowledge_vector::local_knowledge_document_reparse,
        ])
        .build(context)
        .expect("error while building Fox");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            // CX-2: shutdown already ran every step; its summary is the
            // diagnostic record of what could not be cleaned up.
            if let Err(error) = app_handle.state::<AppState>().runtime_host.shutdown() {
                eprintln!("[fox.exit] runtime shutdown finished with errors: {error}");
            }
            mcp::shutdown_connections();
            app_handle.state::<external_backend::ExternalBackendHost>().shutdown_blocking();
        }
    });
}

// Single-Pi mainline acceptance fixture (2026-10-27), work package D2/D3.
//
// `src/bin/single-pi-acceptance-fixture.rs` seeds two experts with separate
// local knowledge bases in an isolated `FOX_DATA_DIR` and dumps what exists.
// It must go through the production APIs only (`LocalKnowledgeStore::create_base`
// / `start_import`, `Database::save_agent` / `bind_conversation_expert` /
// `set_knowledge_reference_bindings`), but every module in this crate is private,
// so a bin cannot reach them. This re-export block is the whole surface it needs;
// it exposes nothing new (every item is already `pub` inside a private module)
// and it is additive, with no change to existing behaviour.
#[doc(hidden)]
pub mod acceptance_api {
    pub use crate::database::{
        AgentRecord, ConversationExpertBinding, ConversationSummary, Database, KnowledgeReference,
        KnowledgeReferenceBindingInput, KnowledgeReferenceBindingRecord, RuntimeSessionRecord,
        SaveAgentRequest,
    };
    pub use crate::local_knowledge::{
        CreateLocalKnowledgeBaseRequest, LocalKnowledgeBase, LocalKnowledgeDocumentsRequest,
        LocalKnowledgeImportFileRequest, LocalKnowledgeImportRequest, LocalKnowledgeStore,
    };
    pub use crate::local_knowledge_storage::LOCAL_KNOWLEDGE_STORAGE_PATH_KEY;
}

#[cfg(test)]
mod rt3_startup_order_tests {
    /// 数字同事调度器必须在 `AppState` 注册之后才开始派发。
    /// 更早启动时，知识库或 Office 工具里的 `app.state::<AppState>()` 会 panic。
    #[test]
    fn rt3_scheduler_starts_only_after_app_state_is_managed() {
        let source = include_str!("lib.rs");
        let managed_at = source
            .find("app.manage(AppState::new(")
            .expect("setup 应注册 AppState");
        let scheduler_at = source
            .find("start_digital_colleague_scheduler")
            .expect("setup 应启动数字同事调度器");
        assert!(
            scheduler_at > managed_at,
            "start_digital_colleague_scheduler 第一次出现必须在 app.manage(AppState::new( 之后"
        );
    }
}

mod app_state;
mod commands;
mod database;
mod digital_colleagues;
mod expert_packages;
mod expert_teams;
mod expert_workflows;
mod lifecycle_hooks;
mod local_knowledge;
mod local_knowledge_import;
mod local_knowledge_picker;
mod local_knowledge_storage;
mod maintenance;
mod mcp;
mod mcp_openapi;
mod model_service;
mod runtime_host;
mod skills;
mod tool_guard;
mod tool_host;
mod vector_store;
mod work_diagnostics;
mod work_mode_gate;
mod yuxi;

use app_state::AppState;
use database::Database;
use local_knowledge::LocalKnowledgeStore;
use model_service::ModelServiceClient;
use runtime_host::RuntimeHost;
use tauri::Manager;
use yuxi::YuxiClient;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = std::env::var_os("FOX_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&app_data_dir)?;
            maintenance::apply_pending_restore(&app_data_dir)?;

            let database = Database::open(app_data_dir.join("fox.db"))?;
            database.repair_interrupted_runs()?;
            database.audit_interrupted_tasks()?;
            let runtime_sessions_dir = app_data_dir.join("runtime-sessions");
            std::fs::create_dir_all(&runtime_sessions_dir)?;
            let attachments_dir = app_data_dir.join("attachments");
            std::fs::create_dir_all(&attachments_dir)?;
            let skills_dir = app_data_dir.join("skills");
            std::fs::create_dir_all(&skills_dir)?;
            let yuxi_client = YuxiClient::new()?;
            let runtime_host = RuntimeHost::new(
                app.handle().clone(),
                database.clone(),
                runtime_sessions_dir,
                attachments_dir,
                skills_dir.clone(),
                yuxi_client.clone(),
            );
            runtime_host.start_digital_colleague_scheduler();
            let yuxi_runtime = yuxi::runtime::YuxiRuntimeHost::new(
                app.handle().clone(),
                database.clone(),
                yuxi_client.clone(),
            );
            yuxi_runtime.recover_pending_runs_detached();
            let model_service_client = ModelServiceClient::new()?;
            let configured_local_knowledge_root =
                local_knowledge_storage::database_storage_path(&database)?;
            let local_knowledge_root = local_knowledge_storage::resolve_startup_root(
                &app_data_dir,
                configured_local_knowledge_root.as_deref(),
            );
            let local_knowledge = LocalKnowledgeStore::open(local_knowledge_root)?;
            local_knowledge.ensure_default_fox_guide()?;
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
            maintenance::recover_knowledge_preview_cache(&app.state::<AppState>())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
            digital_colleagues::digital_colleagues_list,
            digital_colleagues::digital_colleague_create,
            digital_colleagues::digital_colleague_update,
            digital_colleagues::digital_colleague_set_paused,
            digital_colleagues::digital_colleague_revoke,
            digital_colleagues::digital_colleague_schedules_list,
            digital_colleagues::digital_colleague_schedule_save,
            digital_colleagues::digital_colleague_channels_list,
            digital_colleagues::digital_colleague_channel_create,
            digital_colleagues::digital_colleague_channel_revoke,
            digital_colleagues::digital_colleague_trigger_manual,
            digital_colleagues::digital_colleague_trigger_channel,
            digital_colleagues::digital_colleague_triggers_list,
            digital_colleagues::digital_colleague_audit_list,
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
            commands::project_delete,
            commands::project_folder_pick,
            commands::external_url_open,
            commands::project_files_list,
            commands::project_file_read,
            commands::project_permission_update,
            commands::conversation_create,
            commands::conversation_expert_bind,
            commands::conversation_expert_remove,
            commands::conversation_load,
            commands::conversation_history,
            commands::conversations_search,
            commands::conversation_rename,
            commands::conversation_pin,
            commands::conversation_archive,
            commands::conversation_unarchive,
            commands::conversation_restore,
            commands::conversation_fork,
            commands::conversation_delete,
            commands::conversation_purge,
            commands::attachments_save,
            commands::knowledge_bindings_set,
            commands::run_start,
            commands::run_rewind,
            commands::run_resume,
            commands::run_cancel,
            commands::approval_resolve,
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
            commands::plugin_center::plugin_catalog_list,
            commands::plugin_center::plugin_installations_list,
            commands::plugin_center::plugin_set_activation,
            local_knowledge::local_knowledge_bases_list,
            local_knowledge::local_knowledge_file_sources_list,
            local_knowledge::local_knowledge_file_source_add,
            local_knowledge::local_knowledge_file_source_rescan,
            local_knowledge::local_knowledge_file_source_remove,
            local_knowledge::local_knowledge_local_files_list,
            local_knowledge::local_knowledge_local_file_read,
            local_knowledge::local_knowledge_local_file_open,
            local_knowledge::local_knowledge_base_get,
            local_knowledge::local_knowledge_base_create,
            local_knowledge::local_knowledge_base_update,
            local_knowledge::local_knowledge_documents_list,
            local_knowledge::local_knowledge_document_file_read,
            local_knowledge::local_knowledge_document_file_open,
            local_knowledge::local_knowledge_documents_import_start,
            local_knowledge_picker::local_knowledge_import_files_pick,
            local_knowledge_picker::local_knowledge_source_folder_pick,
            local_knowledge::local_knowledge_jobs_list,
            local_knowledge::local_knowledge_job_get,
            local_knowledge::local_knowledge_job_cancel,
            local_knowledge::local_knowledge_job_retry,
            local_knowledge::local_knowledge_storage_status,
            local_knowledge_storage::local_knowledge_storage_directory_pick,
            local_knowledge::local_knowledge_storage_migrate_start,
            local_knowledge::local_knowledge_storage_migration_get,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Fox");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            let _ = app_handle.state::<AppState>().runtime_host.shutdown();
            mcp::shutdown_connections();
        }
    });
}

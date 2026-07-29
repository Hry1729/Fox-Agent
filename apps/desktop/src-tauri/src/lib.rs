mod app_state;
mod commands;
mod database;
mod maintenance;
mod mcp;
mod model_service;
mod runtime_host;
mod skills;
mod tool_guard;
mod tool_host;
mod yuxi;

use app_state::AppState;
use database::Database;
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
            let yuxi_runtime = yuxi::runtime::YuxiRuntimeHost::new(
                app.handle().clone(),
                database.clone(),
                yuxi_client.clone(),
            );
            yuxi_runtime.recover_pending_runs_detached();
            let model_service_client = ModelServiceClient::new()?;
            app.manage(AppState::new(
                database,
                runtime_host,
                yuxi_client,
                yuxi_runtime,
                model_service_client,
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
            commands::skills_list,
            commands::skill_set_enabled,
            commands::mcp_servers_list,
            commands::mcp_server_save,
            commands::mcp_server_test,
            commands::mcp_server_set_enabled,
            commands::mcp_server_delete,
            commands::conversations_list,
            commands::usage_statistics,
            commands::user_profile_get,
            commands::user_profile_save,
            commands::projects_list,
            commands::project_folder_pick,
            commands::project_files_list,
            commands::project_file_read,
            commands::project_permission_update,
            commands::conversation_create,
            commands::conversation_load,
            commands::conversation_history,
            commands::conversations_search,
            commands::conversation_rename,
            commands::conversation_delete,
            commands::attachments_save,
            commands::knowledge_bindings_set,
            commands::run_start,
            commands::run_resume,
            commands::run_cancel,
            commands::approval_resolve,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building Fox");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            let _ = app_handle.state::<AppState>().runtime_host.shutdown();
        }
    });
}

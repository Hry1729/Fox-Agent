#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(package_path) = std::env::var_os("FOX_EMBEDDING_SMOKE_PACKAGE") {
        std::process::exit(fox_desktop_lib::local_embedding_smoke_package_exit_code(
            &package_path,
        ));
    }
    if let (Some(root), Some(request)) = (
        std::env::var_os("FOX_KNOWLEDGE_RETRIEVAL_ROOT"),
        std::env::var_os("FOX_KNOWLEDGE_RETRIEVAL_REQUEST"),
    ) {
        let resource_dir = std::env::var_os("FOX_KNOWLEDGE_ZVEC_RESOURCE_DIR");
        std::process::exit(fox_desktop_lib::local_knowledge_retrieval_worker_exit_code(
            &root,
            &request,
            resource_dir.as_deref(),
        ));
    }
    fox_desktop_lib::run();
}

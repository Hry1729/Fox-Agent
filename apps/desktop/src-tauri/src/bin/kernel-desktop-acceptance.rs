//! Launch the real desktop against an explicitly isolated acceptance directory.
//! This is a binary target so Tauri embeds the Windows common-controls manifest.
//! No fallback to the user's normal data directory is permitted.
fn main() {
    assert!(
        cfg!(debug_assertions),
        "acceptance launcher is development-only"
    );
    let directory = std::path::PathBuf::from(
        std::env::var_os("FOX_DATA_DIR")
            .expect("FOX_DATA_DIR must name an isolated acceptance directory"),
    );
    assert!(
        directory.is_absolute(),
        "acceptance directory must be absolute"
    );
    let marker = directory.join(".fox-kernel-acceptance");
    assert!(
        !directory.join("fox.db").exists() || marker.is_file(),
        "refusing an existing database without the acceptance marker"
    );
    std::fs::create_dir_all(&directory).expect("create acceptance directory");
    std::fs::write(marker, "Isolated Fox Kernel desktop acceptance data.\n")
        .expect("write acceptance marker");
    std::env::set_var("FOX_KERNEL_MODE", "authoritative");
    fox_desktop_lib::run();
}

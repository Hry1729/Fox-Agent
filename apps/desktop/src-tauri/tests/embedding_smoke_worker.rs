use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn desktop_embedding_smoke_worker_does_not_start_the_app_window() {
    let missing_package = std::env::temp_dir().join(format!(
        "fox-missing-embedding-package-{}",
        std::process::id()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_fox-desktop"))
        .env("FOX_EMBEDDING_SMOKE_PACKAGE", &missing_package)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("desktop smoke worker should start");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("worker status should be readable") {
            assert_eq!(status.code(), Some(1));
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("desktop smoke worker started the app instead of exiting");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

//! Static guard: every process Fox starts in production code hides its
//! Windows console (see `src/console.rs`).
//!
//! Scans the Rust sources of the Host (`apps/desktop/src-tauri/src`) and of
//! the crates under `crates/*/src`. Test code is skipped: files named
//! `*_tests.rs` / `tests.rs`, and everything from a top-level
//! `#[cfg(test)] mod ...` to the end of the file.
//!
//! Rules, for the production part of each file:
//! 1. A `Command::new(` (std or tokio) must be followed, before the
//!    `.spawn(` / `.status(` / `.output(` that starts it, by a
//!    `hide_console` call, or carry a `console-policy:` comment (for example
//!    a Unix-only launch) in the three lines above it.
//! 2. `creation_flags(` is only called inside `console.rs`; anywhere else it
//!    needs a `console-policy:` comment in the three lines above it, because
//!    a second `creation_flags` call silently replaces the hidden-console flag.
//! 3. No production code passes `DETACHED_PROCESS` or `CREATE_NEW_CONSOLE`.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn production_sources() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("apps/desktop/src-tauri/src"), &mut files);
    for entry in std::fs::read_dir(root.join("crates")).expect("crates/").flatten() {
        rust_files(&entry.path().join("src"), &mut files);
    }
    files.retain(|path| {
        let name = path.file_name().unwrap().to_string_lossy();
        !(name.ends_with("_tests.rs") || name == "tests.rs")
    });
    files.sort();
    files
}

/// Lines before the first top-level `#[cfg(test)]` that introduces a module.
fn production_lines(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim_end() != "#[cfg(test)]" {
            continue;
        }
        let next = lines[index + 1..].iter().find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with("#[") && !trimmed.starts_with("//")
        });
        if next.is_some_and(|line| line.starts_with("mod ") || line.starts_with("pub mod ") || line.starts_with("pub(crate) mod ")) {
            return lines[..index].to_vec();
        }
    }
    lines
}

fn has_policy_comment(lines: &[&str], index: usize) -> bool {
    lines[index.saturating_sub(3)..=index].iter().any(|line| line.contains("console-policy:"))
}

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*")
}

#[test]
fn every_production_launch_point_hides_the_console() {
    let root = repo_root();
    let files = production_sources();
    assert!(files.len() > 50, "scan found too few sources: {}", files.len());
    let mut launch_points = 0;
    let mut problems = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let lines = production_lines(&text);
        let relative = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let is_console_module = relative == "crates/fox-agent-backend/src/console.rs";
        for (index, line) in lines.iter().enumerate() {
            if is_comment(line) {
                continue;
            }
            if line.contains("Command::new(") {
                launch_points += 1;
                let end = (index..lines.len().min(index + 40))
                    .find(|&i| [".spawn(", ".status(", ".output("].iter().any(|start| lines[i].contains(start)))
                    .unwrap_or_else(|| (index + 40).min(lines.len()) - 1);
                let hidden = lines[index..=end].iter().any(|line| line.contains("hide_console"));
                if !hidden && !has_policy_comment(&lines, index) {
                    problems.push(format!("{relative}:{}: Command::new without hide_console: {}", index + 1, line.trim()));
                }
            }
            if line.contains("creation_flags(") && !is_console_module && !line.trim_start().starts_with("pub fn") && !has_policy_comment(&lines, index) {
                problems.push(format!("{relative}:{}: raw creation_flags (use console::HideConsole::hide_console_with): {}", index + 1, line.trim()));
            }
            if !is_console_module && (line.contains("CREATE_NEW_CONSOLE") || (line.contains("DETACHED_PROCESS") && !line.contains("DETACHED_PROCESS_FLAG"))) {
                problems.push(format!("{relative}:{}: console-showing creation flag: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(launch_points >= 15, "the scan saw only {launch_points} launch points; did the layout change?");
    assert!(problems.is_empty(), "launch points that may show a console window on Windows:\n{}", problems.join("\n"));
}

#[test]
fn the_backend_launch_hides_the_console_and_keeps_the_job_suspension() {
    let acp = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acp.rs")).unwrap();
    assert!(
        acp.contains("hide_console_with(&mut command, win_job::CREATE_SUSPENDED)"),
        "the ACP backend launch (pi-acp / codex-acp / dsh) must start with CREATE_NO_WINDOW | CREATE_SUSPENDED"
    );
}

/// The lint itself: a launch without the helper is reported.
#[test]
fn the_lint_reports_an_unhidden_launch() {
    let source = "fn f() {\n    let mut c = std::process::Command::new(\"cmd\");\n    c.arg(\"/c\");\n    c.spawn().unwrap();\n}\n";
    let lines = production_lines(source);
    let index = lines.iter().position(|line| line.contains("Command::new(")).unwrap();
    assert!(!lines[index..].iter().take(4).any(|line| line.contains("hide_console")));
    assert!(!has_policy_comment(&lines, index));
    let test_only = "fn f() {}\n#[cfg(test)]\nmod tests {\n    fn g() { std::process::Command::new(\"x\").spawn(); }\n}\n";
    assert_eq!(production_lines(test_only), vec!["fn f() {}"]);
}

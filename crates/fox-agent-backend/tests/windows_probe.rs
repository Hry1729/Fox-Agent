#![cfg(windows)]
// Fox is a GUI process; a console test runner can conceal this regression.
#![windows_subsystem = "windows"]

use fox_agent_backend::sandbox::{RealProbe, SystemProbe};
use std::path::PathBuf;

#[test]
fn real_probe_from_gui_process_does_not_create_a_console() {
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system root"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let script = r#"$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class FoxConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'
[FoxConsoleProbe]::GetConsoleWindow().ToInt64()
"#;
    let output = RealProbe
        .run(
            &powershell,
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ],
        )
        .expect("real Windows probe succeeds and captures stdout");
    assert_eq!(
        output.trim(),
        "0",
        "background probes must not create a console window"
    );
    assert!(RealProbe
        .run(
            &powershell,
            &["-NoProfile", "-NonInteractive", "-Command", "exit 7"]
        )
        .is_none());
}

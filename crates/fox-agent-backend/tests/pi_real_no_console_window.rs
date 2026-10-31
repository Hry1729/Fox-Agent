//! fox-fix-console: the real pi chain shows no console window on Windows.
//!
//! The desktop Host is a GUI process without a console. This test makes its
//! own process look the same (`FreeConsole`), then drives the real chain
//! (`ExternalRuntime` -> `AcpConnection` -> `pi-acp.cmd` -> pi-acp ->
//! `pi-gated.cmd` -> `pi.cmd` -> pi 1.4.0 -> its bash tool and the Fox tool
//! extension; see `pi_real_fox_tools.rs`) through a plain turn, a command
//! tool turn (pi's bash) and a Fox tool turn, sampling the desktop's
//! top-level windows every 50 ms. It fails on any visible console window
//! (`ConsoleWindowClass`, conhost) owned by a process of this test's process
//! tree, and on any visible Windows Terminal window
//! (`CASCADIA_HOSTING_WINDOW_CLASS`) that was not there before the run (a
//! console opened through Windows Terminal belongs to WindowsTerminal.exe,
//! not to the tree).
//!
//! Positive control first: a `cmd.exe` started from this console-less
//! process without `CREATE_NO_WINDOW` must be seen by the same detector,
//! otherwise the session cannot show windows at all (no interactive desktop)
//! and a clean result would mean nothing. Then the test fails with
//! `FOX_REQUIRE_CONSOLE_WINDOW_CHECK=1` (CI) and prints SKIPPED otherwise.
//! The static side (every launch point uses `console::HideConsole`) is
//! `console_spawn_lint.rs`.
#![cfg(feature = "acp")]

#[cfg(windows)]
#[path = "support/pi_real.rs"]
mod pi_real;

#[cfg(windows)]
mod windows_check {
    use super::pi_real::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
    use windows_sys::Win32::System::Console::FreeConsole;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};

    const CONSOLE_CLASS: &str = "ConsoleWindowClass";
    const TERMINAL_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

    #[derive(Clone, Debug)]
    struct VisibleWindow {
        hwnd: usize,
        class: String,
        pid: u32,
    }

    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> i32 {
        let out = &mut *(data as *mut Vec<VisibleWindow>);
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut class = [0u16; 256];
        let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
        if class == CONSOLE_CLASS || class == TERMINAL_CLASS {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            out.push(VisibleWindow { hwnd: hwnd as usize, class, pid });
        }
        1
    }

    fn console_windows() -> Vec<VisibleWindow> {
        let mut out: Vec<VisibleWindow> = Vec::new();
        unsafe { EnumWindows(Some(collect), &mut out as *mut Vec<VisibleWindow> as LPARAM) };
        out
    }

    /// pid -> (parent pid, exe name) for every process.
    fn processes() -> HashMap<u32, (u32, String)> {
        let mut map = HashMap::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return map;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
                map.insert(entry.th32ProcessID, (entry.th32ParentProcessID, String::from_utf16_lossy(&entry.szExeFile[..end])));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        map
    }

    /// This process and all of its descendants (exe names by pid).
    fn tree() -> HashMap<u32, String> {
        let all = processes();
        let root = std::process::id();
        let mut tree: HashMap<u32, String> = HashMap::new();
        tree.insert(root, all.get(&root).map(|(_, name)| name.clone()).unwrap_or_default());
        loop {
            let before = tree.len();
            for (pid, (parent, name)) in &all {
                if tree.contains_key(parent) && *pid != *parent {
                    tree.entry(*pid).or_insert_with(|| name.clone());
                }
            }
            if tree.len() == before {
                return tree;
            }
        }
    }

    /// Windows that belong to this tree, or terminal windows not in `baseline`.
    fn offending(baseline: &HashSet<usize>) -> Vec<String> {
        let tree = tree();
        console_windows()
            .into_iter()
            .filter(|window| tree.contains_key(&window.pid) || (window.class == TERMINAL_CLASS && !baseline.contains(&window.hwnd)))
            .map(|window| format!("{} hwnd={:#x} pid={} ({})", window.class, window.hwnd, window.pid, tree.get(&window.pid).cloned().unwrap_or_else(|| "not in tree".into())))
            .collect()
    }

    /// The detector sees a console window from an unflagged cmd.exe.
    fn positive_control(baseline: &HashSet<usize>) -> Option<Vec<String>> {
        // Deliberately no CREATE_NO_WINDOW: this is the bug being detected.
        let mut control = std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"]).spawn().expect("start the control cmd.exe");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = None;
        while Instant::now() < deadline {
            let found = offending(baseline);
            if !found.is_empty() {
                seen = Some(found);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = fox_agent_backend::console::HideConsole::hide_console(&mut std::process::Command::new("taskkill")).args(["/PID", &control.id().to_string(), "/T", "/F"]).output();
        let _ = control.kill();
        let _ = control.wait();
        // Let the control's console go away before the real run is sampled.
        let gone = Instant::now() + Duration::from_secs(10);
        while Instant::now() < gone && !offending(baseline).is_empty() {
            std::thread::sleep(Duration::from_millis(100));
        }
        seen
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_pi_chain_opens_no_console_window() {
        let Some(pi_runtime) = runtime_dir("real_pi_chain_opens_no_console_window") else { return };
        // Like the GUI Host: no console of our own, so an unflagged console
        // child would get a new, visible one.
        unsafe { FreeConsole() };
        let baseline: HashSet<usize> = console_windows().into_iter().map(|window| window.hwnd).collect();
        match positive_control(&baseline) {
            Some(seen) => eprintln!("positive control: the detector saw the unflagged cmd.exe console: {seen:?}"),
            None if std::env::var("FOX_REQUIRE_CONSOLE_WINDOW_CHECK").as_deref() == Ok("1") => {
                panic!("FOX_REQUIRE_CONSOLE_WINDOW_CHECK=1 but an unflagged cmd.exe showed no visible console window: this session cannot show windows, so the check would prove nothing")
            }
            None => {
                eprintln!("SKIPPED real_pi_chain_opens_no_console_window: an unflagged cmd.exe showed no visible console window here (no interactive desktop); skip is not a pass");
                return;
            }
        }
        let leftover = offending(&baseline);
        assert!(leftover.is_empty(), "the control's console did not close: {leftover:?}");

        // Sample for the whole run.
        let stop = Arc::new(AtomicBool::new(false));
        let found: Arc<Mutex<Vec<String>>> = Arc::default();
        let processes_seen: Arc<Mutex<HashSet<String>>> = Arc::default();
        let sampler = {
            let (stop, found, processes_seen, baseline) = (Arc::clone(&stop), Arc::clone(&found), Arc::clone(&processes_seen), baseline.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let windows = offending(&baseline);
                    if !windows.is_empty() {
                        let mut found = found.lock().unwrap();
                        for window in windows {
                            if !found.contains(&window) {
                                found.push(window);
                            }
                        }
                    }
                    processes_seen.lock().unwrap().extend(tree().into_values().map(|name| name.to_ascii_lowercase()));
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        };

        let Setup { runtime, host, bridge: _bridge, sink, project, _state } = setup(pi_runtime, Wiring::Bridge);
        let conversation = "conv-no-console";
        runtime.start_turn(turn(conversation, "nc-hello", "hello", project.path())).unwrap();
        let terminal = sink.terminal("nc-hello", Duration::from_secs(150)).await;
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        // A command tool: pi's bash tool runs `echo` through the shell.
        runtime.start_turn(turn(conversation, "nc-bash", "GATE:noconsole", project.path())).unwrap();
        let terminal = sink.terminal("nc-bash", Duration::from_secs(90)).await;
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert!(project.path().join("fox-proof-noconsole.txt").is_file(), "the bash tool ran: {:?}", sink.events("nc-bash"));
        // A Fox tool (the bridge extension inside pi).
        runtime.start_turn(turn(conversation, "nc-tool", "FOXTOOL:present_files", project.path())).unwrap();
        let terminal = sink.terminal("nc-tool", Duration::from_secs(60)).await;
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert!(!host.calls.lock().unwrap().is_empty());
        runtime.close_conversation(conversation, "test done").await;
        runtime.shutdown().await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        stop.store(true, Ordering::SeqCst);
        sampler.join().unwrap();
        let seen = processes_seen.lock().unwrap().clone();
        eprintln!("processes seen in the tree: {seen:?}");
        // The chain really ran through cmd.exe and node (the .cmd shims).
        assert!(seen.contains("cmd.exe") && seen.contains("node.exe"), "expected cmd.exe and node.exe in the tree: {seen:?}");
        let found = found.lock().unwrap().clone();
        assert!(found.is_empty(), "visible console windows during the pi chain:\n{}", found.join("\n"));
    }
}

//! Windows console policy for every child process Fox starts.
//!
//! Fox is a GUI process with no console of its own. A console program it
//! starts without a creation flag (cmd.exe for a `.cmd` entry point, node,
//! git, taskkill, clip.exe, ...) gets a brand-new console, and Windows shows
//! that console as a window (conhost, or a Windows Terminal tab when
//! Windows Terminal is the default terminal). Everything the child starts
//! afterwards inherits that visible console: one unflagged launch of
//! `pi-acp.cmd` is a `C:\WINDOWS\system32\cmd.exe` window that pi then
//! retitles `pi`.
//!
//! [`CREATE_NO_WINDOW`] gives the child a console that has no window. The
//! whole process tree under it shares that hidden console, so descendants
//! that start without flags of their own stay hidden too, and console
//! semantics are kept: `.cmd` files, Ctrl+C handling and child consoles work
//! as before.
//!
//! The two flags that look similar are wrong here:
//! * `DETACHED_PROCESS` starts the child with no console at all. cmd.exe
//!   running a `.cmd` then has nothing to attach to, and every console
//!   program that child starts allocates a new, visible console.
//! * `CREATE_NEW_CONSOLE` explicitly opens a new console window.
//!
//! Extra flags combine by OR: the Job Object launches pass `CREATE_SUSPENDED`
//! (assign to the job, then resume) through [`HideConsole::hide_console_with`].
//! `CREATE_NO_WINDOW` does not interact with Job Object membership or
//! `CREATE_BREAKAWAY_FROM_JOB`.
//!
//! Every `Command::new` in production code calls [`HideConsole::hide_console`]
//! or [`HideConsole::hide_console_with`]; `tests/console_spawn_lint.rs` fails
//! the build's tests on any launch point that does not.

/// `CREATE_NO_WINDOW`: a console without a window, inherited by the tree.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Hide the console of a child process on Windows (a no-op elsewhere).
///
/// `creation_flags` replaces earlier values, so a launch that needs more
/// flags passes them here instead of calling `creation_flags` itself.
pub trait HideConsole {
    /// Start the child with [`CREATE_NO_WINDOW`] | `extra_flags`.
    fn hide_console_with(&mut self, extra_flags: u32) -> &mut Self;

    /// Start the child with [`CREATE_NO_WINDOW`].
    fn hide_console(&mut self) -> &mut Self {
        self.hide_console_with(0)
    }
}

impl HideConsole for std::process::Command {
    fn hide_console_with(&mut self, extra_flags: u32) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(creation_flags(extra_flags));
        }
        #[cfg(not(windows))]
        let _ = extra_flags;
        self
    }
}

#[cfg(feature = "acp")]
impl HideConsole for tokio::process::Command {
    fn hide_console_with(&mut self, extra_flags: u32) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(creation_flags(extra_flags));
        #[cfg(not(windows))]
        let _ = extra_flags;
        self
    }
}

/// The creation flags [`HideConsole::hide_console_with`] sets.
///
/// Refuses (panics in debug and release alike) the two console flags that
/// would bring a window back: this is a programming error, never input.
pub fn creation_flags(extra_flags: u32) -> u32 {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    assert!(
        extra_flags & (DETACHED_PROCESS | CREATE_NEW_CONSOLE) == 0,
        "hide_console_with: DETACHED_PROCESS / CREATE_NEW_CONSOLE would show a console window"
    );
    CREATE_NO_WINDOW | extra_flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_console_flags_combine_with_job_flags() {
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        assert_eq!(creation_flags(0), 0x0800_0000);
        assert_eq!(creation_flags(CREATE_SUSPENDED), 0x0800_0004);
        assert_eq!(creation_flags(CREATE_SUSPENDED | CREATE_BREAKAWAY_FROM_JOB), 0x0900_0004);
    }

    #[test]
    #[should_panic(expected = "DETACHED_PROCESS")]
    fn detached_process_is_refused() {
        creation_flags(0x0000_0008);
    }

    #[test]
    #[should_panic(expected = "CREATE_NEW_CONSOLE")]
    fn create_new_console_is_refused() {
        creation_flags(0x0000_0010);
    }

    #[cfg(windows)]
    #[test]
    fn windows_sys_agrees_on_the_flag_values() {
        use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW as SYS_NO_WINDOW, CREATE_SUSPENDED, DETACHED_PROCESS};
        assert_eq!(CREATE_NO_WINDOW, SYS_NO_WINDOW);
        assert_eq!(CREATE_SUSPENDED, 0x4);
        assert_eq!(DETACHED_PROCESS, 0x8);
        assert_eq!(CREATE_NEW_CONSOLE, 0x10);
    }
}

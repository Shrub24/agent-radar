//! What this machine can say about a process that the runtime cannot.
//!
//! Herdr reports which command holds a pane's foreground. The kernel reports
//! two further facts Radar presents: how long that process has been alive, and
//! whether it has taken the terminal over. Neither is in the Herdr API, so both
//! are read here and nowhere else — a process that is gone, or a platform whose
//! state cannot be read, yields unknown facts rather than a guess.

use crate::model::{LocalFacts, TerminalMode};

/// Reads what the operating system knows about `pid`.
pub fn facts(pid: i32) -> LocalFacts {
    platform::facts(pid)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::CString;
    use std::os::fd::RawFd;
    use std::time::Duration;

    use super::{LocalFacts, TerminalMode};

    pub fn facts(pid: i32) -> LocalFacts {
        LocalFacts {
            running_for: running_for(pid),
            terminal: terminal_mode(pid),
        }
    }

    /// How long the process has been alive.
    ///
    /// From its own start time, not from when Radar first saw it: a build that
    /// has been running for an hour when Radar starts has been running for an
    /// hour.
    fn running_for(pid: i32) -> Option<Duration> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let ticks = start_ticks(&stat)?;
        let hertz = clock_ticks()?;
        let uptime = uptime_seconds()?;
        let elapsed = uptime - ticks as f64 / hertz as f64;
        Some(Duration::from_secs_f64(elapsed.max(0.0)))
    }

    /// The `starttime` field of a `/proc/<pid>/stat` line, in clock ticks.
    ///
    /// The command name sits in parentheses and may itself contain spaces and
    /// parentheses, so the fields are counted from the *last* `)` rather than by
    /// splitting the whole line.
    fn start_ticks(stat: &str) -> Option<u64> {
        let fields = &stat[stat.rfind(')')? + 1..];
        // The remainder begins at `state` (field 3); `starttime` is field 22.
        fields.split_whitespace().nth(22 - 3)?.parse().ok()
    }

    fn clock_ticks() -> Option<i64> {
        let hertz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        (hertz > 0).then_some(hertz)
    }

    fn uptime_seconds() -> Option<f64> {
        let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
        uptime.split_whitespace().next()?.parse().ok()
    }

    /// Whether the foreground program has taken the terminal over.
    ///
    /// Read from the terminal the process itself holds, which is why the
    /// process's own `fd 0` is followed rather than Radar's own terminal: the
    /// two are different devices, and only one of them belongs to the pane.
    fn terminal_mode(pid: i32) -> TerminalMode {
        let Ok(device) = std::fs::read_link(format!("/proc/{pid}/fd/0")) else {
            return TerminalMode::Unknown;
        };
        // Only a pty: `/dev/tty` would be Radar's own controlling terminal, and
        // a virtual console or a file is not a pane at all.
        let path = device.to_string_lossy().into_owned();
        if !path.starts_with("/dev/pts/") {
            return TerminalMode::Unknown;
        }
        let Ok(fd) = open_terminal(&path) else {
            return TerminalMode::Unknown;
        };
        let mode = read_attributes(fd).map_or(TerminalMode::Unknown, mode_from_attributes);
        unsafe { libc::close(fd) };
        mode
    }

    /// Opens a terminal device for reading, without making it ours.
    fn open_terminal(path: &str) -> Result<RawFd, std::ffi::NulError> {
        let path = CString::new(path)?;
        // `O_NOCTTY` matters: opening a terminal without it can hand this
        // process a controlling terminal it did not ask for.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK,
            )
        };
        Ok(fd)
    }

    fn read_attributes(fd: RawFd) -> Option<libc::termios> {
        let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
        let read = unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) };
        // A negative fd lands here too, which is an unreadable terminal and not
        // a reason to fail.
        (read == 0).then(|| unsafe { attributes.assume_init() })
    }

    /// A program that has turned off canonical mode *and* echo is drawing the
    /// screen itself. One that has not is printing to a terminal the shell
    /// still owns.
    fn mode_from_attributes(attributes: libc::termios) -> TerminalMode {
        let line_discipline = attributes.c_lflag & (libc::ICANON | libc::ECHO);
        if line_discipline == 0 {
            TerminalMode::FullScreen
        } else {
            TerminalMode::Line
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn attributes(line_discipline: libc::tcflag_t) -> libc::termios {
            let mut attributes: libc::termios = unsafe { std::mem::zeroed() };
            attributes.c_lflag = line_discipline;
            attributes
        }

        #[test]
        fn a_command_line_with_parentheses_still_yields_the_start_time() {
            let stat = "  42 (we (ird) name) R 1 42 42 0 -1 4194560 100 0 0 0 \
                        1 2 3 4 20 0 3 0 987654 0 0";
            assert_eq!(start_ticks(stat), Some(987_654));
            // A line whose field is missing is not a start time.
            assert_eq!(start_ticks("42 (short) R 1 2"), None);
            assert_eq!(start_ticks("not a stat line"), None);
        }

        #[test]
        fn raw_mode_is_what_a_full_screen_program_leaves_behind() {
            assert_eq!(
                mode_from_attributes(attributes(0)),
                TerminalMode::FullScreen
            );
            assert_eq!(
                mode_from_attributes(attributes(libc::ICANON)),
                TerminalMode::Line
            );
            assert_eq!(
                mode_from_attributes(attributes(libc::ICANON | libc::ECHO)),
                TerminalMode::Line
            );
        }

        #[test]
        fn this_process_is_known_and_young() {
            let facts = facts(std::process::id() as i32);
            let running = facts.running_for.expect("our own process has a start");
            assert!(running < Duration::from_secs(3600), "{running:?}");
        }

        #[test]
        fn a_process_that_is_gone_yields_unknown_facts() {
            // A pid that cannot exist: nothing is older than the process table.
            let facts = facts(i32::MAX);
            assert_eq!(facts.running_for, None);
            assert_eq!(facts.terminal, TerminalMode::Unknown);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{LocalFacts, TerminalMode};

    /// Nothing is read on a platform Radar has no reader for, and the row says
    /// nothing it cannot support.
    pub fn facts(pid: i32) -> LocalFacts {
        let _ = pid;
        LocalFacts {
            running_for: None,
            terminal: TerminalMode::Unknown,
        }
    }
}

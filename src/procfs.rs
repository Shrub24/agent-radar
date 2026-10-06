//! What this machine can say about a process that the runtime cannot.
//!
//! Herdr reports which command holds a pane's foreground. The kernel reports
//! three further facts Radar presents: how long that process has been alive,
//! whether it has taken the terminal over, and whether it is running the
//! program that is installed now. None is in the Herdr API, so all are read
//! here and nowhere else — a process that is gone, or a platform whose state
//! cannot be read, yields unknown facts rather than a guess.
//!
//! Only the process's own executable is compared, and only with the program
//! `PATH` resolves for the name the runtime reports. A process's environment is
//! never read, so Radar's own `PATH` is the one that decides.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{BinaryFreshness, BinaryIdentity, LocalFacts, TerminalMode};

/// The kernel's marker on an executable link whose file has been unlinked or
/// replaced.
const DELETED: &str = " (deleted)";

/// One refresh's installed-program lookups.
///
/// The search directories come from Radar's own `PATH`, read once, and each
/// distinct program name is resolved through them once, so a refresh pays one
/// lookup per program rather than one per pane. Injecting the directories lets
/// tests exercise the comparison without this machine's store.
pub struct BinaryIndex {
    directories: Vec<PathBuf>,
    resolved: HashMap<String, Option<PathBuf>>,
}

impl Default for BinaryIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl BinaryIndex {
    /// A resolver over Radar's own `PATH`.
    pub fn new() -> Self {
        let directories = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        Self::searching(directories)
    }

    /// A resolver over explicit search directories, as tests and callers that
    /// must not read the environment supply.
    pub fn searching(directories: Vec<PathBuf>) -> Self {
        Self {
            directories,
            resolved: HashMap::new(),
        }
    }

    /// The installed executable for `program`, symlinks followed, or `None`
    /// when no search directory has an executable one. Each name is looked up
    /// once.
    ///
    /// The search follows `PATH` command resolution: only an executable regular
    /// file qualifies, so a directory or a same-named non-executable file earlier
    /// on the search path cannot mask the program. A name containing a path
    /// separator is a path, not a command name, and is not searched for.
    fn installed(&mut self, program: &str) -> Option<PathBuf> {
        if let Some(cached) = self.resolved.get(program) {
            return cached.clone();
        }
        let found = if program.contains(std::path::MAIN_SEPARATOR) {
            // A path, not a command name: nothing to search for on PATH.
            None
        } else {
            self.directories
                .iter()
                .find_map(|directory| executable_target(&directory.join(program)))
        };
        self.resolved.insert(program.to_string(), found.clone());
        found
    }
}

/// The resolved target of a search-path candidate, when it is a command.
///
/// `metadata` follows symlinks, so a launcher that points at the program is
/// judged by the file it resolves to. A directory, or a file with no execute
/// bit, is not a command and is passed over.
fn executable_target(candidate: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(candidate).ok()?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return None;
    }
    std::fs::canonicalize(candidate).ok()
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// How the running executable compares with the program `PATH` resolves for
/// `program`, through `binaries`.
///
/// `running` is the executable's link target and `deleted` whether the kernel
/// marked it so. This is the comparison behind [`facts`], exposed so it can be
/// driven with injected paths.
pub fn identity(
    running: &Path,
    deleted: bool,
    program: &str,
    binaries: &mut BinaryIndex,
) -> BinaryIdentity {
    match binaries.installed(program) {
        Some(installed) => classify(running, deleted, &installed),
        None => BinaryIdentity::default(),
    }
}

/// Reads what the operating system knows about `pid`, the process the runtime
/// reports as `program`.
pub fn facts(pid: i32, program: Option<&str>, binaries: &mut BinaryIndex) -> LocalFacts {
    platform::facts(pid, program, binaries)
}

/// Compares a running executable's link target with the installed program.
///
/// The comparison is between installations, not files: inside the Nix store a
/// wrapper and the binary it launches share a store root, so they agree. A
/// running file whose name is not the installed program's is not compared at
/// all — an interpreter or helper the runtime reports is never attributed to
/// the agent.
fn classify(running: &Path, deleted: bool, installed: &Path) -> BinaryIdentity {
    let (Some(running_name), Some(installed_name)) = (file_name(running), file_name(installed))
    else {
        return BinaryIdentity::default();
    };
    if running_name != installed_name {
        return BinaryIdentity::default();
    }
    let running_installation = installation(running);
    let installed_installation = installation(installed);
    let stale = deleted
        || (in_store(&running_installation)
            && in_store(&installed_installation)
            && running_installation != installed_installation);
    BinaryIdentity {
        freshness: if stale {
            BinaryFreshness::Stale
        } else {
            BinaryFreshness::Current
        },
        // The installations, not the files: a wrapper and its inner binary
        // inside one package name the same installation.
        running: Some(running_installation.display().to_string()),
        installed: Some(installed_installation.display().to_string()),
    }
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// The installation a path belongs to.
///
/// Under the Nix store that is the store root (`/nix/store/<hash>-<name>`), so
/// a package's wrapper and its inner binary agree; anywhere else it is the
/// resolved path itself.
fn installation(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("/nix/store")
        && let Some(root) = rest.components().next()
    {
        return Path::new("/nix/store").join(root);
    }
    path.to_path_buf()
}

/// Whether `path` is a store path's root, as [`installation`] produces.
fn in_store(path: &Path) -> bool {
    path.starts_with("/nix/store/")
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::CString;
    use std::os::fd::RawFd;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{BinaryIdentity, BinaryIndex, LocalFacts, TerminalMode, identity};

    pub fn facts(pid: i32, program: Option<&str>, binaries: &mut BinaryIndex) -> LocalFacts {
        LocalFacts {
            running_for: running_for(pid),
            terminal: terminal_mode(pid),
            binary: binary_identity(pid, program, binaries),
        }
    }

    /// How the process's executable compares with the installed program.
    fn binary_identity(
        pid: i32,
        program: Option<&str>,
        binaries: &mut BinaryIndex,
    ) -> BinaryIdentity {
        let Some(program) = program else {
            return BinaryIdentity::default();
        };
        // The kernel appends " (deleted)" when the file the process started
        // from has been unlinked or replaced — the ordinary result of an
        // upgrade. It is not stale on its own: with no installed match the
        // comparison is Unknown, never a mark.
        let Ok(link) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return BinaryIdentity::default();
        };
        let link = link.to_string_lossy();
        let (running, deleted) = match link.strip_suffix(super::DELETED) {
            Some(path) => (PathBuf::from(path), true),
            None => (PathBuf::from(link.as_ref()), false),
        };
        identity(&running, deleted, program, binaries)
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
            let mut binaries = BinaryIndex::searching(Vec::new());
            let facts = facts(std::process::id() as i32, None, &mut binaries);
            let running = facts.running_for.expect("our own process has a start");
            assert!(running < Duration::from_secs(3600), "{running:?}");
        }

        #[test]
        fn a_process_that_is_gone_yields_unknown_facts() {
            // A pid that cannot exist: nothing is older than the process table.
            let mut binaries = BinaryIndex::searching(Vec::new());
            let facts = facts(i32::MAX, Some("pi"), &mut binaries);
            assert_eq!(facts.running_for, None);
            assert_eq!(facts.terminal, TerminalMode::Unknown);
            assert_eq!(facts.binary, BinaryIdentity::default());
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{BinaryIndex, LocalFacts};

    /// Nothing is read on a platform Radar has no reader for, and the row says
    /// nothing it cannot support.
    pub fn facts(_pid: i32, _program: Option<&str>, _binaries: &mut BinaryIndex) -> LocalFacts {
        LocalFacts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_of(running: &str, deleted: bool, installed: &str) -> BinaryIdentity {
        classify(Path::new(running), deleted, Path::new(installed))
    }

    #[test]
    fn a_wrapper_and_its_inner_binary_are_one_installation() {
        // The shape a Nix package has: the PATH match is a different file from
        // the running one, inside one store root.
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            false,
            "/nix/store/aaa-pi-1.0.2/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Current);
        assert_eq!(identity.running.as_deref(), Some("/nix/store/aaa-pi-1.0.2"));
        assert_eq!(
            identity.installed.as_deref(),
            Some("/nix/store/aaa-pi-1.0.2")
        );
    }

    #[test]
    fn a_changed_store_root_is_stale() {
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            false,
            "/nix/store/bbb-pi-1.0.3/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Stale);
    }

    #[test]
    fn a_replaced_executable_is_stale_even_without_a_store_change() {
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            true,
            "/nix/store/aaa-pi-1.0.2/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Stale);
    }

    #[test]
    fn a_deliberate_other_build_is_not_stale() {
        // A checkout or dev build is not the installed program, but nothing
        // about it says the running one was replaced.
        let identity = identity_of(
            "/home/dev/agent-radar/target/debug/pi",
            false,
            "/nix/store/bbb-pi-1.0.3/libexec/pi/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Current);
        assert_eq!(
            identity.running.as_deref(),
            Some("/home/dev/agent-radar/target/debug/pi")
        );
        assert_eq!(
            identity.installed.as_deref(),
            Some("/nix/store/bbb-pi-1.0.3")
        );
    }

    #[test]
    fn a_non_store_program_on_path_is_current() {
        let identity = identity_of("/usr/local/bin/pi", false, "/usr/local/bin/pi");
        assert_eq!(identity.freshness, BinaryFreshness::Current);
    }

    #[test]
    fn an_interpreter_is_never_attributed_to_the_agent() {
        // The runtime reports the agent as `pi`, but the foreground process is
        // a python interpreter: nothing is claimed.
        let identity = identity_of("/usr/bin/python3", false, "/nix/store/bbb-pi-1.0.3/bin/pi");
        assert_eq!(identity.freshness, BinaryFreshness::Unknown);
        assert_eq!(identity.running, None);
        assert_eq!(identity.installed, None);
    }
}

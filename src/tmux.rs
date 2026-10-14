//! tmux as Radar's second physical backend.
//!
//! Every invocation names one server (`tmux -L <name>`) and strips `TMUX` from
//! the command's environment, so a command can reach the server it was
//! configured for and nothing else. `default` is not a server this adapter will
//! target: that name *is* the tmux server every other tmux user shares.
//!
//! Locations are tmux's own stable ids (`$session`, `@window`, `%pane`)
//! qualified by backend and server — `tmux:radar:%3`. A session is a workspace,
//! a window a tab and a pane a pane; labels, names and indexes are display text
//! and never identity. What tmux cannot express is simply not declared as a
//! capability, and no operation is emulated around it.
//!
//! Certainty follows the seam. Only input this adapter rejects before running
//! anything is `refused`; a command that ran and whose answer was not usable is
//! `unknown`, and is never retried. The adapter does not read tmux's
//! human-readable messages to tell those two apart — tmux exits non-zero for
//! both — so a failed command that was dispatched stays `unknown`.
//!
//! Dead panes expose typed exit status through a bounded format read. A missing
//! or unreadable pane remains unknown; it is not proof of process exit.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{FleetObservation, ForegroundEvidence, Location, Pane, Tab, Workspace};
use crate::runtime::{
    BackendIdentity, CLOSE_CAPABILITY, CREATE_CAPABILITY, CloseOutcome, CloseTarget, CreateOutcome,
    CreateRequest, CreatedLocation, EXIT_OBSERVATION_CAPABILITY, FOCUS_CAPABILITY, FocusOutcome,
    INPUT_CAPABILITY, InputOutcome, InputPayload, InputRequest, LAUNCH_CAPABILITY,
    LAUNCH_CONTEXT_CAPABILITY, LAUNCH_TOKEN_ENV, LIFECYCLE_TRACKING_CAPABILITY, LaunchOutcome,
    LaunchRequest, OBSERVE_CAPABILITY, OUTPUT_CAPABILITY, OutputOutcome, OutputRead, OutputRequest,
    OutputSource, PROCESS_INFO_CAPABILITY, PaneExitObservation, RuntimeProvider, SplitDirection,
    Target,
};

/// The backend word in every handle this adapter names.
const BACKEND: &str = "tmux";

/// The global hook a launch's exit wake is appended to.
const PANE_DIED_HOOK: &str = "pane-died";

/// What separates the fields of one record the adapter asks tmux for.
///
/// tmux refuses this byte in a session or window name, and the one field that
/// can contain it — a pane title, which whatever runs in the pane sets — is last
/// in its record, so a title can never be read as another field.
const FIELD: char = '\u{1f}';

/// The same separator as a tmux that cannot write the byte writes it.
///
/// Measured against Radar's own formats: tmux 3.5a (Debian trixie) writes the
/// byte above as this `\ooo` escape, and tmux 3.7c writes the byte. Both spell
/// the same field boundary and the same record. A boundary is never an
/// instruction — the text is not unescaped, and a display field that already
/// spells `\037` is read as the text tmux sent.
const FIELD_ESCAPED: &str = "\\037";

/// One `list-sessions` record: the session, then its name.
const SESSION_FORMAT: &str = "#{session_id}\u{1f}#{session_name}";

/// One `list-windows` record: the window, its session, its index, its name.
///
/// The index is display text — windows are reordered — so it is carried as a
/// number beside the id, never as the id.
const WINDOW_FORMAT: &str =
    "#{window_id}\u{1f}#{session_id}\u{1f}#{window_index}\u{1f}#{window_name}";

/// One `list-panes` record: the pane, its window, its session, its title.
const PANE_FORMAT: &str = "#{pane_id}\u{1f}#{window_id}\u{1f}#{session_id}\u{1f}#{pane_title}";

/// What one `display-message` answer reports about a pane's foreground.
const FOREGROUND_FORMAT: &str = "#{pane_pid}\u{1f}#{pane_current_command}";

/// One retained-pane exit record, with pane identity and typed status fields.
const EXIT_FORMAT: &str = "#{pane_id}\u{1f}#{pane_dead}\u{1f}#{pane_dead_status}\u{1f}#{pane_dead_signal}\u{1f}#{pane_dead_time}";

/// The identities each creation reports back, in the order they are read.
const CREATED_SESSION_FORMAT: &str = "#{session_id}";
const CREATED_WINDOW_FORMAT: &str = "#{window_id}\u{1f}#{session_id}";
const CREATED_PANE_FORMAT: &str = "#{pane_id}\u{1f}#{window_id}\u{1f}#{session_id}";

/// How often a running command is checked for exit, cancellation or its
/// deadline.
const WAIT_GRANULARITY: Duration = Duration::from_millis(10);

/// The most output one command may return, and the most of its error output a
/// diagnostic quotes.
const STDOUT_CAP: usize = 1024 * 1024;
const STDERR_CAP: usize = 8 * 1024;
const DIAGNOSTIC_CHARS: usize = 200;

/// How many rows of recent output are read when the caller names no count, and
/// the most tmux is ever asked for in one read.
const DEFAULT_RECENT_LINES: u32 = 200;
const MAX_RECENT_LINES: u32 = 1000;

/// Command names that mean the pane's own shell still holds its terminal.
///
/// The list matches the one the Herdr adapter uses, so the same program is
/// classified the same way whichever backend reports it.
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "dash",
    "ash",
    "zsh",
    "fish",
    "ksh",
    "ksh93",
    "mksh",
    "csh",
    "tcsh",
    "nu",
    "elvish",
    "yash",
    "osh",
    "oil",
    "xonsh",
    "pwsh",
    "powershell",
];

/// How this adapter reaches its server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxConfig {
    pub executable: PathBuf,
    /// The name of the tmux server every command targets.
    pub server: String,
    pub command_timeout: Duration,
}

impl TmuxConfig {
    /// The configuration for one named server, with the documented defaults for
    /// everything else.
    pub fn for_server(server: impl Into<String>) -> Self {
        Self {
            executable: PathBuf::from("tmux"),
            server: server.into(),
            command_timeout: Duration::from_secs(5),
        }
    }
}

/// tmux behind the daemon's backend seam.
pub struct TmuxRuntime {
    config: TmuxConfig,
}

impl TmuxRuntime {
    /// Builds the adapter for one server.
    ///
    /// Err is a configuration an operator has to change, reported where the
    /// daemon is configured rather than as a failed operation later.
    pub fn new(config: TmuxConfig) -> Result<Self, String> {
        if let Some(problem) = server_name_problem(&config.server) {
            return Err(problem);
        }
        Ok(Self { config })
    }
}

/// Why a server name cannot be targeted, or `None` when it can.
///
/// tmux puts the name in a path under its socket directory and reads it as
/// `<socket directory>/tmux-<uid>/<name>`, so a name with a separator escapes
/// that directory and the name `default` is the socket of the tmux server the
/// user and every other tmux user already share.
fn server_name_problem(server: &str) -> Option<String> {
    let problem = if server.is_empty() {
        "the tmux server name is empty".to_string()
    } else if server == "default" {
        "`default` is tmux's default server, which Radar never targets".to_string()
    } else if !server
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        "a tmux server name may only contain letters, digits, `-`, `_` and `.`".to_string()
    } else {
        return None;
    };
    Some(format!("the tmux server name is unusable: {problem}"))
}

/// One command's answer: what it printed, and whether the adapter's own bound
/// dropped part of it.
struct Answer {
    text: String,
    truncated: bool,
}

/// One failed operation, and what its failure establishes.
enum Failure {
    /// Nothing was dispatched: the command was never built.
    Refused(String),
    /// A command was dispatched and its answer could not be used, so the effect
    /// may have happened.
    Unknown(String),
}

impl Failure {
    fn refused(message: impl Into<String>) -> Self {
        Self::Refused(message.into())
    }

    fn unknown(message: impl Into<String>) -> Self {
        Self::Unknown(message.into())
    }

    fn into_focus(self) -> FocusOutcome {
        match self {
            Self::Refused(message) => FocusOutcome::Refused(message),
            Self::Unknown(message) => FocusOutcome::Unknown(message),
        }
    }

    fn into_close(self) -> CloseOutcome {
        match self {
            Self::Refused(message) => CloseOutcome::Refused(message),
            Self::Unknown(message) => CloseOutcome::Unknown(message),
        }
    }

    fn into_create(self) -> CreateOutcome {
        match self {
            Self::Refused(message) => CreateOutcome::Refused(message),
            Self::Unknown(message) => CreateOutcome::Unknown(message),
        }
    }

    fn into_input(self) -> InputOutcome {
        match self {
            Self::Refused(message) => InputOutcome::Refused(message),
            Self::Unknown(message) => InputOutcome::Unknown(message),
        }
    }

    fn into_output(self) -> OutputOutcome {
        match self {
            Self::Refused(message) => OutputOutcome::Refused(message),
            Self::Unknown(message) => OutputOutcome::Unknown(message),
        }
    }
}

/// Which tmux object a location id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Session,
    Window,
    Pane,
}

impl Kind {
    /// The word this kind is called in a location handle and a diagnostic.
    fn noun(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Window => "window",
            Self::Pane => "pane",
        }
    }

    /// The sigil tmux puts in front of one of its ids.
    fn sigil(self) -> char {
        match self {
            Self::Session => '$',
            Self::Window => '@',
            Self::Pane => '%',
        }
    }

    /// Whether `value` is an id of this kind as tmux writes them.
    fn names(self, value: &str) -> bool {
        match value.strip_prefix(self.sigil()) {
            Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
            None => false,
        }
    }
}

impl RuntimeProvider for TmuxRuntime {
    /// What this adapter implements, each one a tmux command this module runs.
    ///
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::tmux(self.config.server.clone())
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &[
            OBSERVE_CAPABILITY,
            PROCESS_INFO_CAPABILITY,
            FOCUS_CAPABILITY,
            CLOSE_CAPABILITY,
            CREATE_CAPABILITY,
            INPUT_CAPABILITY,
            OUTPUT_CAPABILITY,
            LAUNCH_CAPABILITY,
            LAUNCH_CONTEXT_CAPABILITY,
            EXIT_OBSERVATION_CAPABILITY,
            LIFECYCLE_TRACKING_CAPABILITY,
        ]
    }

    fn validate_launch(&self, request: &LaunchRequest) -> Result<(), String> {
        // The daemon's cross-backend preflight uses a synthetic pane id; backend
        // address validation belongs in `launch`, immediately before dispatch.
        request.validate()
    }

    fn launch(&self, request: &LaunchRequest, cancel: &AtomicBool) -> LaunchOutcome {
        if let Err(error) = self.validate_launch(request) {
            return LaunchOutcome::Refused(error);
        }
        let pane = match self.address(&request.pane_id, Kind::Pane) {
            Ok(pane) => pane,
            Err(error) => return LaunchOutcome::Refused(error),
        };

        // Set the pane option before respawning its process: an immediately
        // exiting child must still leave the pane and its status observable.
        if self
            .run(
                &["set-option", "-p", "-t", &pane, "remain-on-exit", "on"],
                cancel,
            )
            .is_err()
        {
            return LaunchOutcome::Refused(
                "tmux could not arm remain-on-exit before launch".to_string(),
            );
        }

        let mut args = vec![
            "respawn-pane".to_string(),
            "-k".to_string(),
            "-t".to_string(),
            pane,
        ];
        if let Some(cwd) = &request.cwd {
            args.push("-c".to_string());
            args.push(cwd.clone());
        }
        for (name, value) in &request.env {
            args.push("-e".to_string());
            args.push(format!("{name}={value}"));
        }
        // A launch outside a spawn has no token, and then the variable is not
        // in the child's environment at all rather than set to nothing.
        if let Some(token) = &request.spawn_token {
            args.push("-e".to_string());
            args.push(format!("{LAUNCH_TOKEN_ENV}={token}"));
        }
        args.push(request.executable.clone());
        args.extend(request.argv.iter().cloned());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.run(&borrowed, cancel) {
            Ok(_) => LaunchOutcome::Completed,
            // tmux's command line contains the private token and environment
            // values. Never forward its diagnostics across the daemon boundary.
            Err(_) => LaunchOutcome::Unknown(
                "tmux launch was dispatched but its outcome is unknown".to_string(),
            ),
        }
    }

    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        // Three reads of a live server folded into one observation. They are not
        // one atomic moment: a location created between them can appear as a
        // pane whose window or session record is missing, which the tree
        // tolerates rather than inventing a label for it.
        let sessions = self.run_whole(&["list-sessions", "-F", SESSION_FORMAT], cancel)?;
        let windows = self.run_whole(&["list-windows", "-a", "-F", WINDOW_FORMAT], cancel)?;
        let panes = self.run_whole(&["list-panes", "-a", "-F", PANE_FORMAT], cancel)?;
        Ok(FleetObservation {
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
            workspaces: self.decode_sessions(&sessions)?,
            tabs: self.decode_windows(&windows)?,
            panes: self.decode_panes(&panes)?,
            // tmux carries no agent records: what runs in a pane is the
            // machine's business, and this adapter reports no agent facts.
            agents: Vec::new(),
        })
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        // Unreadable evidence is inconclusive, never a claim about the pane.
        let Ok(pane) = self.address(pane_id, Kind::Pane) else {
            return ForegroundEvidence::Inconclusive;
        };
        let Ok(read) = self.run(
            &[
                "display-message",
                "-p",
                "-t",
                &pane,
                "-F",
                FOREGROUND_FORMAT,
            ],
            cancel,
        ) else {
            return ForegroundEvidence::Inconclusive;
        };
        decode_foreground(&read.text)
    }

    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        self.focus_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }

    fn focus_outcome(&self, target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        let focused = match target {
            Target::Pane(pane_id) => self.focus_pane(pane_id, cancel),
            Target::Workspace(workspace_id) => self.focus_workspace(workspace_id, cancel),
        };
        focused.map_or_else(Failure::into_focus, |()| FocusOutcome::Completed)
    }

    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        self.close_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }

    fn close_outcome(&self, target: &CloseTarget, cancel: &AtomicBool) -> CloseOutcome {
        self.close_target(target, cancel)
            .map_or_else(Failure::into_close, |()| CloseOutcome::Completed)
    }

    fn create(&self, request: &CreateRequest, cancel: &AtomicBool) -> CreateOutcome {
        self.create_location(request, cancel)
            .map_or_else(Failure::into_create, CreateOutcome::Completed)
    }

    fn input(&self, request: &InputRequest, cancel: &AtomicBool) -> InputOutcome {
        self.deliver(request, cancel)
            .map_or_else(Failure::into_input, |()| InputOutcome::Completed)
    }

    fn exit_observations(
        &self,
        pane_ids: &[String],
        cancel: &AtomicBool,
    ) -> BTreeMap<String, PaneExitObservation> {
        self.read_exit_observations(pane_ids, cancel)
    }

    fn output(&self, request: &OutputRequest, cancel: &AtomicBool) -> OutputOutcome {
        match self.read_output(request, cancel) {
            Ok(read) => OutputOutcome::Completed(read),
            Err(failure) => failure.into_output(),
        }
    }

    /// The tmux server's own PID, for the daemon to read the birth identity of.
    ///
    /// It is the one tmux fact that does not survive a restart under the same
    /// server name, so it is what tells one generation of this server's panes
    /// from the next. A server that cannot be asked answers none.
    fn server_process(&self, cancel: &AtomicBool) -> Option<i32> {
        let text = self
            .run_whole(&["display-message", "-p", "#{pid}"], cancel)
            .ok()?;
        text.trim().parse::<i32>().ok().filter(|pid| *pid > 0)
    }

    /// Appends the wake to the global `pane-died` slots, so a user's own hook in
    /// an earlier slot is left alone.
    fn install_exit_wake(
        &self,
        _subject_id: &str,
        command: &str,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        // tmux reads the hook as its own command line and the shell reads the
        // command inside it; any character that would end or re-quote either is
        // refused rather than escaped.
        if command
            .chars()
            .any(|c| matches!(c, '"' | '\\' | '$' | '`' | '#' | ';' | '\n'))
        {
            return Err("the wake command contains characters tmux would reinterpret".into());
        }
        let hook = format!("run-shell -b \"{command}\"");
        self.run(&["set-hook", "-ga", PANE_DIED_HOOK, &hook], cancel)
            .map(|_| ())
    }

    /// Unsets only the slots whose command names this launch. tmux indices are
    /// stable across an unset, so the slot read is the slot removed.
    fn remove_exit_wake(&self, subject_id: &str, cancel: &AtomicBool) -> Result<(), String> {
        let listing = self.run_whole(&["show-hooks", "-g", PANE_DIED_HOOK], cancel)?;
        let marker = format!("--subject {subject_id} ");
        for line in listing.lines() {
            let Some(rest) = line.strip_prefix(PANE_DIED_HOOK) else {
                continue;
            };
            let Some(index) = rest
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
                .map(|(index, _)| index)
            else {
                continue;
            };
            if line.contains(&marker) {
                let slot = format!("{PANE_DIED_HOOK}[{index}]");
                self.run(&["set-hook", "-gu", &slot], cancel)?;
            }
        }
        Ok(())
    }
}

impl TmuxRuntime {
    /// Reads retained exit status for the requested pane handles in one bounded
    /// server query. Unlisted, malformed, or unreadable panes remain unknown.
    fn read_exit_observations(
        &self,
        pane_ids: &[String],
        cancel: &AtomicBool,
    ) -> BTreeMap<String, PaneExitObservation> {
        let mut observations: BTreeMap<_, _> = pane_ids
            .iter()
            .cloned()
            .map(|pane_id| (pane_id, PaneExitObservation::Unknown))
            .collect();
        let mut requested = BTreeMap::new();
        for pane_id in pane_ids {
            let Ok(raw_id) = self.address(pane_id, Kind::Pane) else {
                continue;
            };
            requested.insert(raw_id, pane_id.clone());
        }
        if requested.is_empty() {
            return observations;
        }
        let Ok(text) = self.run_whole(&["list-panes", "-a", "-F", EXIT_FORMAT], cancel) else {
            return observations;
        };
        for line in records(&text) {
            let Some([raw_id, dead, code, signal, dead_time]) = split_record::<5>(line) else {
                continue;
            };
            if !Kind::Pane.names(raw_id) {
                continue;
            }
            let Some(handle) = requested.get(raw_id) else {
                continue;
            };
            let observation = match dead {
                "0" => PaneExitObservation::Running,
                "1" if !dead_time.is_empty() => {
                    let code = (!code.is_empty())
                        .then(|| code.parse::<i32>().ok())
                        .flatten();
                    let signal = (!signal.is_empty())
                        .then(|| signal.parse::<i32>().ok())
                        .flatten();
                    let value = PaneExitObservation::Exited { code, signal };
                    if value.validate().is_ok() {
                        value
                    } else {
                        PaneExitObservation::Unknown
                    }
                }
                _ => PaneExitObservation::Unknown,
            };
            observations.insert(handle.clone(), observation);
        }
        observations
    }

    /// Delivers one input request to a pane.
    fn deliver(&self, request: &InputRequest, cancel: &AtomicBool) -> Result<(), Failure> {
        let pane = self
            .address(&request.pane_id, Kind::Pane)
            .map_err(Failure::refused)?;
        let delivered = match &request.payload {
            // `-l` writes the text as bytes rather than reading it as key
            // names, and `--` keeps text that begins with a dash from being
            // read as an option.
            InputPayload::Text { text } => {
                self.run(&["send-keys", "-t", &pane, "-l", "--", text], cancel)
            }
            InputPayload::Keys { keys } => {
                let mut presses = Vec::with_capacity(keys.len());
                for key in keys {
                    match tmux_key(key) {
                        Some(name) => presses.push(name),
                        // tmux ignores a key name it does not know and still
                        // exits 0, so an unmapped name would be reported as
                        // input that was never delivered. It is refused here,
                        // with nothing sent.
                        None => {
                            return Err(Failure::refused(format!("tmux has no key named `{key}`")));
                        }
                    }
                }
                let mut args = vec!["send-keys".to_string(), "-t".to_string(), pane];
                args.extend(presses);
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                self.run(&borrowed, cancel)
            }
        };
        delivered.map(|_| ()).map_err(Failure::unknown)
    }

    /// Reads one bounded, non-consuming snapshot of a pane's output.
    ///
    /// `capture-pane -p` prints the pane as it is and leaves the pane, its
    /// history and any cursor alone, so the same read twice describes the same
    /// content.
    fn read_output(
        &self,
        request: &OutputRequest,
        cancel: &AtomicBool,
    ) -> Result<OutputRead, Failure> {
        let pane = self
            .address(&request.pane_id, Kind::Pane)
            .map_err(Failure::refused)?;
        let args = capture_args(&pane, request);
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let answer = self.run(&borrowed, cancel).map_err(Failure::unknown)?;
        let (text, dropped) = match request.lines {
            Some(lines) => last_lines(&answer.text, lines),
            None => (answer.text, false),
        };
        Ok(OutputRead {
            text,
            truncated: dropped || answer.truncated,
            // tmux reports no revision of a pane's content, and an invented one
            // would let a caller read two different snapshots as the same.
            revision: None,
        })
    }

    /// Closes one location: tmux closes a single object at a time, by id, and a
    /// window takes its panes with it.
    ///
    /// A close that ran and failed may still have closed the pane — the server
    /// applies the command before the client hears about it — so only a target
    /// this adapter rejects before dispatch is refused.
    fn close_target(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), Failure> {
        let (command, address) = match target {
            CloseTarget::Pane(pane_id) => ("kill-pane", self.address(pane_id, Kind::Pane)),
            CloseTarget::Tab(tab_id) => ("kill-window", self.address(tab_id, Kind::Window)),
            CloseTarget::Workspace(workspace_id) => {
                ("kill-session", self.address(workspace_id, Kind::Session))
            }
        };
        let address = address.map_err(Failure::refused)?;
        self.run(&[command, "-t", &address], cancel)
            .map(|_| ())
            .map_err(Failure::unknown)
    }

    /// Moves a pane's window into view and then makes the pane active in it.
    ///
    /// Two commands, because tmux splits them: `select-window` alone leaves the
    /// window's active pane where it was, and `select-pane` alone does not bring
    /// the window into view. A failure of the second leaves the first applied,
    /// so either failure is `unknown`.
    fn focus_pane(&self, handle: &str, cancel: &AtomicBool) -> Result<(), Failure> {
        let pane = self.address(handle, Kind::Pane).map_err(Failure::refused)?;
        self.run(&["select-window", "-t", &pane], cancel)
            .map_err(Failure::unknown)?;
        self.run(&["select-pane", "-t", &pane], cancel)
            .map_err(Failure::unknown)?;
        Ok(())
    }

    /// Switches the tmux client looking at the server to a session.
    ///
    /// tmux has no server-side notion of a current session: a session becomes
    /// current for a client. A clientless command still names the current
    /// client when one is attached, which is the focus this seam asks for; a
    /// server with nothing attached has none to switch, and tmux's failure to
    /// say so in a machine-readable way leaves the answer `unknown`.
    fn focus_workspace(&self, handle: &str, cancel: &AtomicBool) -> Result<(), Failure> {
        let session = self
            .address(handle, Kind::Session)
            .map_err(Failure::refused)?;
        self.run(&["switch-client", "-t", &session], cancel)
            .map_err(Failure::unknown)?;
        Ok(())
    }

    /// Creates one location, answering with the identities tmux reported.
    fn create_location(
        &self,
        request: &CreateRequest,
        cancel: &AtomicBool,
    ) -> Result<CreatedLocation, Failure> {
        match request {
            CreateRequest::Workspace { focus } => {
                if *focus {
                    // tmux focuses a session by switching a client to it, and
                    // this command has no client. Creating first and switching
                    // after would leave a workspace that exists and a caller
                    // told the creation was unknown, so nothing is created:
                    // refusing costs a retry, not a duplicate workspace.
                    return Err(Failure::refused(
                        "tmux focuses a session by switching a client, and the daemon's commands \
                         have no client to switch; create the workspace unfocused and focus a pane",
                    ));
                }
                // `-d` leaves the caller's client where it is, and the name is
                // tmux's own: this seam names no workspace.
                let answer = self
                    .run(
                        &["new-session", "-d", "-P", "-F", CREATED_SESSION_FORMAT],
                        cancel,
                    )
                    .map_err(Failure::unknown)?;
                let [session] = self.created(&answer.text, &[Kind::Session])?;
                Ok(CreatedLocation {
                    workspace_id: Some(self.handle(&session)),
                    ..CreatedLocation::default()
                })
            }
            CreateRequest::Tab {
                workspace_id,
                focus,
            } => {
                let session = self
                    .address(workspace_id, Kind::Session)
                    .map_err(Failure::refused)?;
                let mut args = vec!["new-window", "-t", session.as_str()];
                if !focus {
                    args.push("-d");
                }
                args.extend(["-P", "-F", CREATED_WINDOW_FORMAT]);
                let answer = self.run(&args, cancel).map_err(Failure::unknown)?;
                let [tab, created_workspace] =
                    self.created(&answer.text, &[Kind::Window, Kind::Session])?;
                // tmux reported the session it put the window in, so the answer
                // is its own and not this adapter's assumption.
                Ok(CreatedLocation {
                    workspace_id: Some(self.handle(&created_workspace)),
                    tab_id: Some(self.handle(&tab)),
                    ..CreatedLocation::default()
                })
            }
            CreateRequest::PaneSplit {
                pane_id,
                direction,
                focus,
            } => {
                let pane = self
                    .address(pane_id, Kind::Pane)
                    .map_err(Failure::refused)?;
                let mut args = vec![
                    "split-window",
                    match direction {
                        // A split that puts the new pane to the right is
                        // horizontal; the one below is vertical.
                        SplitDirection::Right => "-h",
                        SplitDirection::Down => "-v",
                    },
                    "-t",
                    pane.as_str(),
                ];
                if !focus {
                    args.push("-d");
                }
                args.extend(["-P", "-F", CREATED_PANE_FORMAT]);
                let answer = self.run(&args, cancel).map_err(Failure::unknown)?;
                let [created_pane, tab, created_workspace] =
                    self.created(&answer.text, &[Kind::Pane, Kind::Window, Kind::Session])?;
                Ok(CreatedLocation {
                    workspace_id: Some(self.handle(&created_workspace)),
                    tab_id: Some(self.handle(&tab)),
                    pane_id: Some(self.handle(&created_pane)),
                })
            }
        }
    }

    /// The ids one creation answered with, in the kinds they were asked for.
    ///
    /// An answer that does not carry exactly these ids cannot say what was
    /// created, so it is a failure of the creation rather than a location to
    /// guess at.
    fn created<const N: usize>(
        &self,
        answer: &str,
        kinds: &[Kind; N],
    ) -> Result<[String; N], Failure> {
        let line = answer.lines().find(|line| !line.is_empty()).unwrap_or("");
        let named = split_record::<N>(line).filter(|fields| {
            fields
                .iter()
                .zip(kinds)
                .all(|(field, kind)| kind.names(field))
        });
        let Some(fields) = named else {
            return Err(Failure::unknown(format!(
                "tmux answered a creation with `{line:?}`, which names no {}",
                kinds
                    .iter()
                    .map(|kind| kind.noun())
                    .collect::<Vec<&str>>()
                    .join(", ")
            )));
        };
        Ok(fields.map(str::to_string))
    }

    /// The handle for one tmux id: the backend and the server qualify it, and
    /// the id itself is tmux's stable one for the life of the server. The id's
    /// own sigil already says which kind of object it names.
    fn handle(&self, id: &str) -> String {
        format!("{BACKEND}:{}:{id}", self.config.server)
    }

    /// The tmux id inside a handle this adapter made for this server.
    ///
    /// Anything else — another backend's handle, another server's, an id of the
    /// wrong kind — is refused before a command is built.
    fn address(&self, handle: &str, kind: Kind) -> Result<String, String> {
        let mut parts = handle.split(':');
        let backend = parts.next();
        let server = parts.next();
        let id = parts.next();
        let extra = parts.next();
        let unusable = || {
            format!(
                "`{handle:?}` is not a tmux {} on server `{}`",
                kind.noun(),
                self.config.server
            )
        };
        if backend != Some(BACKEND) || server != Some(self.config.server.as_str()) {
            return Err(unusable());
        }
        let Some(id) = id.filter(|id| kind.names(id)) else {
            return Err(unusable());
        };
        if extra.is_some() {
            return Err(unusable());
        }
        Ok(id.to_string())
    }

    fn decode_sessions(&self, text: &str) -> Result<Vec<Workspace>, String> {
        let mut workspaces = Vec::new();
        for line in records(text) {
            let [id, name] =
                split_record::<2>(line).ok_or_else(|| malformed("session list", line))?;
            if !Kind::Session.names(id) {
                return Err(malformed("session list", line));
            }
            workspaces.push(Workspace {
                workspace_id: self.handle(id),
                label: text_field(name),
                // A tmux session has a name, not a number.
                number: None,
            });
        }
        Ok(workspaces)
    }

    fn decode_windows(&self, text: &str) -> Result<Vec<Tab>, String> {
        let mut tabs = Vec::new();
        for line in records(text) {
            let [id, session, index, name] =
                split_record::<4>(line).ok_or_else(|| malformed("window list", line))?;
            if !Kind::Window.names(id) || !Kind::Session.names(session) {
                return Err(malformed("window list", line));
            }
            tabs.push(Tab {
                tab_id: self.handle(id),
                workspace_id: self.handle(session),
                label: text_field(name),
                number: index.parse().ok(),
            });
        }
        Ok(tabs)
    }

    fn decode_panes(&self, text: &str) -> Result<Vec<Pane>, String> {
        let mut panes = Vec::new();
        for line in records(text) {
            let [id, window, session, title] =
                split_record::<4>(line).ok_or_else(|| malformed("pane list", line))?;
            if !Kind::Pane.names(id) || !Kind::Window.names(window) || !Kind::Session.names(session)
            {
                return Err(malformed("pane list", line));
            }
            panes.push(Pane {
                location: Location {
                    workspace_id: self.handle(session),
                    tab_id: self.handle(window),
                    pane_id: self.handle(id),
                },
                // tmux has no pane label of its own; a pane's declared label is
                // Herdr's. The title is the pane's terminal title, which the
                // program in it sets and may clear.
                label: None,
                title: text_field(title),
            });
        }
        Ok(panes)
    }

    /// Runs one tmux command against the named server.
    ///
    /// `Err` carries a one-line diagnostic; the command has been killed and
    /// reaped by then.
    fn run(&self, args: &[&str], cancel: &AtomicBool) -> Result<Answer, String> {
        let executable = &self.config.executable;
        let mut command = Command::new(executable);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .args(server_args(&self.config.server, args))
            // A client's environment names a server too, and tmux would read it
            // as the one to use. The configured server is the only one this
            // command may reach, and `TMUX_TMPDIR` is left alone: it says where
            // sockets live, not which one to use.
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run {}: {error}", executable.display()))?;

        // Drain both pipes on their own threads: waiting for a command that
        // filled a pipe would deadlock, and the same threads keep its output
        // bounded.
        let stdout = drain(child.stdout.take().expect("stdout is piped"), STDOUT_CAP);
        let stderr = drain(child.stderr.take().expect("stderr is piped"), STDERR_CAP);

        let status = wait_for_exit(&mut child, &self.config, cancel);
        // A tmux server a command started has its own session and is not in this
        // group, so ending the group ends the command and whatever it left
        // holding the pipes, never the server.
        #[cfg(unix)]
        unsafe {
            // SAFETY: process_group(0) assigned this child its own group; a
            // negative PID targets only that group, never Radar itself.
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
        let (stdout, truncated) = stdout.join().expect("stdout drain thread panicked");
        let (stderr, _) = stderr.join().expect("stderr drain thread panicked");
        let text = String::from_utf8_lossy(&stdout).into_owned();

        match status {
            Ok(status) if status.success() => Ok(Answer { text, truncated }),
            Ok(status) => Err(diagnostic(exit_diagnostic(status), &stderr)),
            Err(reason) => Err(diagnostic(reason, &stderr)),
        }
    }

    /// Runs one command whose whole answer is required: a short read would drop
    /// locations silently, so it is a failure instead.
    fn run_whole(&self, args: &[&str], cancel: &AtomicBool) -> Result<String, String> {
        let read = self.run(args, cancel)?;
        if read.truncated {
            return Err(format!(
                "tmux wrote more than the adapter's {STDOUT_CAP}-byte read bound"
            ));
        }
        Ok(read.text)
    }
}

/// The arguments every tmux invocation carries: the server first, so no command
/// can reach tmux's default server.
fn server_args(server: &str, args: &[&str]) -> Vec<String> {
    let mut argv = vec!["-L".to_string(), server.to_string()];
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    argv
}

/// The `capture-pane` arguments for one read.
fn capture_args(pane: &str, request: &OutputRequest) -> Vec<String> {
    let mut args = vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane.to_string(),
    ];
    if request.ansi {
        args.push("-e".to_string());
    }
    match request.source {
        // The visible pane is the bottom buffer the caller is looking at, which
        // is what a snapshot and a detection read both want.
        OutputSource::Visible | OutputSource::Detection => {}
        OutputSource::Recent | OutputSource::RecentUnwrapped => {
            if request.source == OutputSource::RecentUnwrapped {
                args.push("-J".to_string());
            }
            // `-S` counts up from the top of the visible pane, so a negative
            // start reads history and the screen together; without it, only the
            // screen would be read.
            let lines = request
                .lines
                .unwrap_or(DEFAULT_RECENT_LINES)
                .min(MAX_RECENT_LINES);
            args.push("-S".to_string());
            args.push(format!("-{lines}"));
        }
    }
    args
}

/// The last `lines` rows of a snapshot, and whether any earlier row was dropped.
fn last_lines(text: &str, lines: u32) -> (String, bool) {
    let rows: Vec<&str> = text.lines().collect();
    let kept = (lines as usize).min(rows.len());
    let dropped = rows.len() - kept;
    let mut snapshot = String::new();
    for row in &rows[rows.len() - kept..] {
        snapshot.push_str(row);
        snapshot.push('\n');
    }
    (snapshot, dropped > 0)
}

/// The tmux key name one normalized key name means, or `None` when tmux cannot
/// press it.
fn tmux_key(name: &str) -> Option<String> {
    let exact = match name {
        "esc" | "escape" => "Escape",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "shift+tab" | "btab" => "BTab",
        "space" => "Space",
        "backspace" | "bspace" => "BSpace",
        "delete" | "del" => "DC",
        "insert" | "ins" => "IC",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "shift+up" => "S-Up",
        "shift+down" => "S-Down",
        "shift+left" => "S-Left",
        "shift+right" => "S-Right",
        "ctrl+up" => "C-Up",
        "ctrl+down" => "C-Down",
        "ctrl+left" => "C-Left",
        "ctrl+right" => "C-Right",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        "f13" => "F13",
        "f14" => "F14",
        "f15" => "F15",
        "f16" => "F16",
        "f17" => "F17",
        "f18" => "F18",
        "f19" => "F19",
        "f20" => "F20",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_string());
    }
    // A modifier with one character: tmux writes control as `C-x` and meta as
    // `M-x`, in either case.
    for (prefix, tmux_prefix) in [("ctrl+", "C-"), ("alt+", "M-"), ("meta+", "M-")] {
        if let Some(key) = name.strip_prefix(prefix) {
            let mut characters = key.chars();
            if let (Some(character), None) = (characters.next(), characters.next())
                && character.is_ascii_alphanumeric()
            {
                return Some(format!("{tmux_prefix}{character}"));
            }
        }
    }
    None
}

/// What tmux's `#{pane_pid}` and `#{pane_current_command}` establish.
///
/// The pid is the pane's first process and the command name is the one holding
/// the pane's terminal: for a pane tmux started by running a program they are
/// the same process, and for a pane whose shell forked the foreground program
/// the name is that program while the pid still names the shell. This adapter
/// reports what tmux reports and claims no more — a caller that needs the
/// foreground process's identity has to corroborate the pid locally, which is
/// what the daemon's guarded close does. Anything tmux did not report is
/// inconclusive rather than a guess.
fn decode_foreground(record: &str) -> ForegroundEvidence {
    let Some([pid, command]) = split_record::<2>(record) else {
        return ForegroundEvidence::Inconclusive;
    };
    let Ok(pid) = pid.trim().parse::<i32>() else {
        return ForegroundEvidence::Inconclusive;
    };
    let Some(command) = text_field(command) else {
        return ForegroundEvidence::Inconclusive;
    };
    if SHELLS.contains(&command.trim_start_matches('-')) {
        return ForegroundEvidence::Shell;
    }
    ForegroundEvidence::command(pid, Some(command), None)
}

/// The records of one format read: one per line, with empty lines dropped.
fn records(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|line| !line.is_empty())
}

/// One record split into its fields.
///
/// A boundary is the raw separator byte or the escape a tmux that cannot write
/// the byte sends in its place; both spell the same record. The last field takes
/// the rest of the line, so the one field tmux cannot police — a title — is read
/// whole whatever it contains. An earlier field that carried a boundary would
/// shift the record, and the id checks that follow turn that into a failure
/// rather than a wrong location.
fn split_record<const N: usize>(line: &str) -> Option<[&str; N]> {
    let mut fields = [""; N];
    let mut rest = line;
    let mut remaining = fields.iter_mut();
    for field in remaining.by_ref().take(N.saturating_sub(1)) {
        let (value, after) = next_field(rest)?;
        *field = value;
        rest = after;
    }
    *remaining.next()? = rest;
    Some(fields)
}

/// The field before the next boundary in `rest` and what follows it, or `None`
/// when no boundary is left.
fn next_field(rest: &str) -> Option<(&str, &str)> {
    let (at, width) = match (rest.find(FIELD), rest.find(FIELD_ESCAPED)) {
        (Some(raw), Some(escaped)) if escaped < raw => (escaped, FIELD_ESCAPED.len()),
        (Some(raw), _) => (raw, FIELD.len_utf8()),
        (None, Some(escaped)) => (escaped, FIELD_ESCAPED.len()),
        (None, None) => return None,
    };
    Some((&rest[..at], &rest[at + width..]))
}

/// A field that carries text only when it carries something.
fn text_field(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The diagnostic for a record the adapter asked tmux not to send.
fn malformed(list: &str, line: &str) -> String {
    format!("malformed tmux {list}: {line:?} is not a record of the shape it was asked for")
}

/// Waits for a command to exit, killing and reaping it on timeout or
/// cancellation. `Err` is the diagnostic for having killed it.
fn wait_for_exit(
    child: &mut Child,
    config: &TmuxConfig,
    cancel: &AtomicBool,
) -> Result<ExitStatus, String> {
    let timeout = config.command_timeout;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return Err(format!("tmux could not be waited for: {error}"));
            }
        }
        if cancel.load(Ordering::SeqCst) {
            kill_and_reap(child);
            return Err("the request was cancelled".to_string());
        }
        if Instant::now() >= deadline {
            kill_and_reap(child);
            return Err(format!("tmux timed out after {timeout:?}"));
        }
        thread::sleep(WAIT_GRANULARITY);
    }
}

/// Kills a still-running command and reaps it, so a stalled command leaves no
/// process — not even a zombie — behind.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads one of a command's pipes to end of file on its own thread, keeping at
/// most `cap` bytes and reporting whether it dropped any.
///
/// Reading past the cap rather than stopping is deliberate: an unread pipe fills
/// at the OS buffer size and would block the command forever.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize) -> thread::JoinHandle<(Vec<u8>, bool)> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut dropped = false;
        let mut buffer = [0u8; 8 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => return (kept, dropped),
                Ok(read) => {
                    let room = cap - kept.len();
                    kept.extend_from_slice(&buffer[..read.min(room)]);
                    dropped |= read > room;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return (kept, dropped),
            }
        }
    })
}

/// How a command ended, in terms a user can act on.
fn exit_diagnostic(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("tmux exited with status {code}"),
        None => format!("tmux was killed ({status})"),
    }
}

/// One diagnostic: what happened, then the command's own error output where it
/// said anything.
fn diagnostic(reason: String, stderr: &[u8]) -> String {
    let excerpt = String::from_utf8_lossy(stderr);
    let excerpt = excerpt.trim();
    if excerpt.is_empty() {
        return reason;
    }
    let mut quoted: String = excerpt.chars().take(DIAGNOSTIC_CHARS).collect();
    if excerpt.chars().count() > DIAGNOSTIC_CHARS {
        quoted.push('…');
    }
    format!("{reason}: {quoted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(server: &str) -> TmuxRuntime {
        TmuxRuntime::new(TmuxConfig::for_server(server)).expect("a usable server name")
    }

    #[test]
    fn a_handle_names_the_backend_the_server_and_the_id() {
        let runtime = runtime("probe");
        assert_eq!(runtime.handle("%3"), "tmux:probe:%3");
        assert_eq!(runtime.handle("@1"), "tmux:probe:@1");
        assert_eq!(runtime.handle("$0"), "tmux:probe:$0");
    }

    /// The adapter names the server it was pointed at, so a recorded location
    /// and the handshake both say which tmux instance produced it.
    #[test]
    fn the_adapter_names_itself_and_the_server_it_targets() {
        assert_eq!(runtime("probe").identity(), BackendIdentity::tmux("probe"));
    }

    #[test]
    fn a_handle_of_another_backend_server_or_kind_is_refused() {
        let runtime = runtime("probe");
        for handle in [
            "wA:p1",           // Herdr's own handle
            "tmux:other:%3",   // another server
            "tmux:probe:@1",   // a window where a pane was asked for
            "tmux:probe:%",    // no id at all
            "tmux:probe:%3:x", // a field the format does not carry
        ] {
            assert!(
                runtime.address(handle, Kind::Pane).is_err(),
                "{handle} was accepted as a pane"
            );
        }
        assert_eq!(
            runtime
                .address("tmux:probe:%3", Kind::Pane)
                .expect("a pane"),
            "%3"
        );
    }

    #[test]
    fn a_server_name_that_is_the_default_or_a_path_is_refused() {
        assert!(server_name_problem("").is_some());
        assert!(server_name_problem("default").is_some());
        assert!(server_name_problem("a/b").is_some());
        assert!(server_name_problem("a b").is_some());
        assert!(server_name_problem("radar.probe-1_2").is_none());
        assert!(TmuxRuntime::new(TmuxConfig::for_server("default")).is_err());
    }

    #[test]
    fn every_command_names_the_server() {
        for args in [
            vec!["list-sessions", "-F", SESSION_FORMAT],
            vec!["list-windows", "-a", "-F", WINDOW_FORMAT],
            vec!["list-panes", "-a", "-F", PANE_FORMAT],
            vec!["kill-pane", "-t", "%3"],
            vec!["select-window", "-t", "%3"],
        ] {
            let argv = server_args("probe", &args);
            assert_eq!(argv[0], "-L");
            assert_eq!(argv[1], "probe");
            assert_eq!(argv[2..].len(), args.len());
        }
    }

    #[test]
    fn sessions_windows_and_panes_are_read_into_normalized_locations() {
        let runtime = runtime("probe");
        let sessions = runtime
            .decode_sessions("$0\u{1f}main\n$1\u{1f}second\n")
            .expect("a session list");
        assert_eq!(sessions[0].workspace_id, "tmux:probe:$0");
        assert_eq!(sessions[0].label.as_deref(), Some("main"));
        assert_eq!(sessions[0].number, None);

        let tabs = runtime
            .decode_windows("@1\u{1f}$0\u{1f}2\u{1f}editor\n")
            .expect("a window list");
        assert_eq!(tabs[0].tab_id, "tmux:probe:@1");
        assert_eq!(tabs[0].workspace_id, "tmux:probe:$0");
        assert_eq!(tabs[0].label.as_deref(), Some("editor"));
        assert_eq!(tabs[0].number, Some(2));

        let panes = runtime
            .decode_panes("%3\u{1f}@1\u{1f}$0\u{1f}a title\n")
            .expect("a pane list");
        assert_eq!(panes[0].location.pane_id, "tmux:probe:%3");
        assert_eq!(panes[0].location.tab_id, "tmux:probe:@1");
        assert_eq!(panes[0].location.workspace_id, "tmux:probe:$0");
        assert_eq!(panes[0].title.as_deref(), Some("a title"));
        assert_eq!(panes[0].label, None);
    }

    #[test]
    fn a_title_that_holds_the_separator_stays_one_field() {
        let runtime = runtime("probe");
        let panes = runtime
            .decode_panes("%3\u{1f}@1\u{1f}$0\u{1f}odd\u{1f}title\n")
            .expect("a pane list");
        assert_eq!(panes[0].title.as_deref(), Some("odd\u{1f}title"));
        assert_eq!(panes[0].location.pane_id, "tmux:probe:%3");
    }

    /// tmux 3.5a writes the separator as its `\ooo` escape, so it arrives as the
    /// literal text `\037` where tmux 3.7c writes the byte. The same record has
    /// to read the same either way, and text that spells the escape is still
    /// text.
    #[test]
    fn a_record_reads_the_same_whether_the_separator_arrived_escaped_or_raw() {
        let runtime = runtime("probe");

        assert_eq!(
            runtime
                .decode_panes("%3\\037@1\\037$0\\037a title\n")
                .expect("an escaped pane list"),
            runtime
                .decode_panes("%3\u{1f}@1\u{1f}$0\u{1f}a title\n")
                .expect("a raw pane list")
        );
        assert_eq!(
            runtime
                .decode_windows("@1\\037$0\\0372\\037editor\n")
                .expect("an escaped window list"),
            runtime
                .decode_windows("@1\u{1f}$0\u{1f}2\u{1f}editor\n")
                .expect("a raw window list")
        );
        let sessions = runtime
            .decode_sessions("$0\\037herdsman\n")
            .expect("an escaped session list");
        assert_eq!(sessions[0].workspace_id, "tmux:probe:$0");
        assert_eq!(sessions[0].label.as_deref(), Some("herdsman"));
        assert_eq!(
            decode_foreground("100\\037pi"),
            ForegroundEvidence::command(100, Some("pi".to_string()), None)
        );
        match runtime.created("@1\\037$0", &[Kind::Window, Kind::Session]) {
            Ok(ids) => assert_eq!(ids, ["@1".to_string(), "$0".to_string()]),
            Err(_) => panic!("a creation's escaped identities were rejected"),
        }

        // The escape is a boundary, never an instruction: text that spells it
        // stays the text tmux sent, and one in a field that should be an id is
        // a malformed record rather than a shifted location.
        let panes = runtime
            .decode_panes("%3\\037@1\\037$0\\037back\\037slash\n")
            .expect("an escaped pane list");
        assert_eq!(panes[0].title.as_deref(), Some("back\\037slash"));
        assert!(
            runtime
                .decode_panes("%3\\037@1\\037odd\\037$0\\037title\n")
                .is_err()
        );
    }

    #[test]
    fn a_record_whose_ids_are_not_tmux_ids_is_a_failure() {
        let runtime = runtime("probe");
        // A name that carried the separator would shift the fields, and the
        // shifted ids are not ids: the list fails rather than naming a location.
        let shifted = runtime.decode_panes("%3\u{1f}@1\u{1f}odd\u{1f}$0\u{1f}title\n");
        assert!(shifted.is_err(), "{shifted:?}");
        assert!(runtime.decode_panes("%3\u{1f}@1\n").is_err());
        assert!(
            runtime
                .decode_windows("$0\u{1f}@1\u{1f}2\u{1f}name\n")
                .is_err()
        );
        assert!(runtime.decode_sessions("main\n").is_err());
    }

    #[test]
    fn foreground_evidence_needs_a_pid_and_a_command() {
        assert_eq!(
            decode_foreground("100\u{1f}fish"),
            ForegroundEvidence::Shell
        );
        assert_eq!(
            decode_foreground("100\u{1f}-bash"),
            ForegroundEvidence::Shell,
            "a login shell is still the shell"
        );
        assert_eq!(
            decode_foreground("100\u{1f}pi"),
            ForegroundEvidence::command(100, Some("pi".to_string()), None)
        );
        // tmux answers an unknown pane with an empty line and status 0, and a
        // pane whose command it cannot name answers with no name.
        assert_eq!(decode_foreground("\n"), ForegroundEvidence::Inconclusive);
        assert_eq!(
            decode_foreground("\u{1f}fish"),
            ForegroundEvidence::Inconclusive
        );
        assert_eq!(
            decode_foreground("notapid\u{1f}pi"),
            ForegroundEvidence::Inconclusive
        );
    }

    #[test]
    fn a_recent_read_asks_for_history_and_a_visible_read_does_not() {
        let read = |source, lines, ansi| {
            let request = OutputRequest {
                pane_id: "tmux:probe:%1".to_string(),
                source,
                lines,
                ansi,
            };
            capture_args("%1", &request)
        };
        let visible = read(OutputSource::Visible, None, false);
        assert!(
            !visible.iter().any(|arg| arg == "-S"),
            "a visible read asks for no history: {visible:?}"
        );
        assert!(visible.contains(&"capture-pane".to_string()));

        let recent = read(OutputSource::Recent, None, false);
        assert!(
            recent.contains(&format!("-{DEFAULT_RECENT_LINES}")),
            "{recent:?}"
        );

        let capped = read(OutputSource::Recent, Some(u32::MAX), false);
        assert!(
            capped.contains(&format!("-{MAX_RECENT_LINES}")),
            "{capped:?}"
        );

        let unwrapped = read(OutputSource::RecentUnwrapped, Some(5), true);
        assert!(unwrapped.contains(&"-J".to_string()), "{unwrapped:?}");
        assert!(unwrapped.contains(&"-e".to_string()), "{unwrapped:?}");
        assert!(unwrapped.contains(&"-5".to_string()), "{unwrapped:?}");
    }

    #[test]
    fn a_read_that_names_a_count_keeps_the_last_rows() {
        let (text, dropped) = last_lines("one\ntwo\nthree\n", 2);
        assert_eq!(text, "two\nthree\n");
        assert!(dropped);
        let (text, dropped) = last_lines("one\ntwo\n", 5);
        assert_eq!(text, "one\ntwo\n");
        assert!(!dropped, "asking for more rows than exist drops none");
    }

    #[test]
    fn keys_tmux_can_press_are_named_and_the_rest_are_refused() {
        assert_eq!(tmux_key("enter").as_deref(), Some("Enter"));
        assert_eq!(tmux_key("esc").as_deref(), Some("Escape"));
        assert_eq!(tmux_key("shift+tab").as_deref(), Some("BTab"));
        assert_eq!(tmux_key("ctrl+c").as_deref(), Some("C-c"));
        assert_eq!(tmux_key("alt+x").as_deref(), Some("M-x"));
        assert_eq!(tmux_key("f12").as_deref(), Some("F12"));
        // tmux would ignore these and still exit 0, so the adapter refuses them.
        assert_eq!(tmux_key("nope"), None);
        assert_eq!(tmux_key("ctrl+"), None);
        assert_eq!(tmux_key("ctrl+shift+a"), None);
        assert_eq!(tmux_key("ctrl+é"), None);
    }
}

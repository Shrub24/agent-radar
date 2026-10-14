//! Confirmed container controls: what the TUI asks the daemon to do to a
//! container it records, and what came of it.
//!
//! One control runs at a time, off the UI thread. Each carries the caller's own
//! request id, so an answer that never arrives leaves an operation the operator
//! can read back rather than an effect nobody can name. Nothing here retries: a
//! control the daemon may have accepted is unknown, never unperformed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::control_plane::client::{
    ContainerAnswer, ContainerControls, ContainerCreate, ContainerDelete, ContainerReload,
};

/// Splits a typed command line into the words a launch runs, the way the recipe
/// contract takes them: an executable and its arguments.
///
/// Only quoting is understood. Whitespace separates words, single quotes keep
/// everything inside literal, double quotes keep it literal except `\\` and `\"`,
/// and a backslash outside quotes escapes the next character. Nothing is expanded:
/// `$VAR`, `~`, globs, pipes and redirections are ordinary text, because the
/// daemon runs an argv and refuses to invent a shell.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        match character {
            '\'' => {
                started = true;
                loop {
                    match characters.next() {
                        Some('\'') => break,
                        Some(inside) => word.push(inside),
                        None => return Err("an opening ' is never closed".to_string()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match characters.next() {
                        Some('"') => break,
                        Some('\\') => match characters.next() {
                            Some(escaped @ ('"' | '\\')) => word.push(escaped),
                            Some(other) => {
                                word.push('\\');
                                word.push(other);
                            }
                            None => return Err("an opening \" is never closed".to_string()),
                        },
                        Some(inside) => word.push(inside),
                        None => return Err("an opening \" is never closed".to_string()),
                    }
                }
            }
            '\\' => match characters.next() {
                Some(escaped) => {
                    started = true;
                    word.push(escaped);
                }
                None => return Err("the line ends after a backslash".to_string()),
            },
            space if space.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            other => {
                started = true;
                word.push(other);
            }
        }
    }
    if started {
        words.push(word);
    }
    if words.is_empty() {
        return Err("type a command to run".to_string());
    }
    Ok(words)
}

/// The absolute path a launch runs `name` as: the name itself when it is already
/// an absolute path, otherwise the first executable file named `name` on `path`.
///
/// The launch contract runs an absolute executable and never searches a `PATH`
/// of its own, so the dashboard, which holds the user's `PATH`, resolves it.
pub fn resolve_executable(name: &str, path: Option<&std::ffi::OsStr>) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |candidate: &std::path::Path| {
        std::fs::metadata(candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let candidate = std::path::Path::new(name);
        return if candidate.is_absolute() && executable(candidate) {
            Ok(name.to_string())
        } else {
            Err(format!("{name} is not an absolute path to an executable"))
        };
    }
    for directory in std::env::split_paths(path.unwrap_or_default()) {
        let candidate = directory.join(name);
        if directory.is_absolute() && executable(&candidate) {
            return candidate
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{name}: its path is not text"));
        }
    }
    Err(format!("{name} is not an executable on PATH"))
}

/// One confirmed control, as the operator asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// Create a pane slot, splitting it from a pane, optionally running a command.
    Create(ContainerCreate),
    /// Delete a container, with the target the operator confirmed.
    Delete(ContainerDelete),
    /// Run a pane container's recorded recipe again.
    Reload(ContainerReload),
}

impl Control {
    /// The request id this control is dispatched and read back under.
    pub fn request_id(&self) -> &str {
        match self {
            Self::Create(request) => &request.request_id,
            Self::Delete(request) => &request.request_id,
            Self::Reload(request) => &request.request_id,
        }
    }

    /// The verb, as the operator sees it in the outcome line.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Delete(_) => "delete",
            Self::Reload(_) => "reload",
        }
    }
}

/// What one finished control reported: what the daemon answered, or why nothing
/// was asked of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The verb the operator confirmed.
    pub verb: &'static str,
    /// The request id the control was dispatched under, so the operation can be
    /// read back whether or not an answer arrived.
    pub request_id: String,
    pub result: Result<ContainerAnswer, String>,
}

/// One control running on its own thread.
struct Request {
    cancel: Arc<AtomicBool>,
    outcome: Receiver<Result<ContainerAnswer, String>>,
    worker: JoinHandle<()>,
}

/// Runs one container control at a time off the calling thread.
pub struct ContainerControl {
    /// The daemon's controls, when this run talks to one. A direct adapter records
    /// no containers, so it has none and every control is refused.
    controls: Option<Arc<dyn ContainerControls>>,
    in_flight: Option<(Control, Request)>,
}

impl ContainerControl {
    /// A control worker that runs nothing until [`Self::start`].
    pub fn new(controls: Option<Arc<dyn ContainerControls>>) -> Self {
        Self {
            controls,
            in_flight: None,
        }
    }

    /// Whether a control is running. The TUI asks before it opens another
    /// confirmation: one control at a time is what keeps the outcome unambiguous.
    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Starts a confirmed control off the calling thread.
    ///
    /// One control runs at a time: a second is refused rather than started, because
    /// a control the daemon may already have accepted cannot be taken back, and its
    /// outcome would otherwise never reach the operator. A run without a daemon
    /// dispatches nothing and says so.
    pub fn start(&mut self, control: Control) -> Result<(), String> {
        if self.in_flight.is_some() {
            return Err("another container control is still running".to_string());
        }
        let Some(controls) = self.controls.clone() else {
            return Err("this run talks to a backend that records no containers".to_string());
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let cancel = Arc::clone(&cancel);
            let dispatched = control.clone();
            thread::Builder::new()
                .name(format!("radar-topology-{}", control.verb()))
                .spawn(move || {
                    let _ = sender.send(run(controls.as_ref(), &dispatched, &cancel));
                })
                .expect("container control thread")
        };
        self.in_flight = Some((
            control,
            Request {
                cancel,
                outcome,
                worker,
            },
        ));
        Ok(())
    }

    /// The finished control's outcome, if one is waiting. Never blocks.
    pub fn poll(&mut self) -> Option<Finished> {
        let (_, request) = self.in_flight.as_mut()?;
        let result = match request.outcome.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before returning,
            // so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                Err("the control request stopped unexpectedly".to_string())
            }
        };
        let (control, request) = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(Finished {
            verb: control.verb(),
            request_id: control.request_id().to_string(),
            result,
        })
    }

    /// Cancels an outstanding control and waits for its thread. Idempotent.
    ///
    /// Cancellation abandons a control still waiting on the daemon; one already
    /// dispatched is not retracted, and nothing here claims it was.
    pub fn shutdown(&mut self) {
        self.cancel_in_flight();
    }

    fn cancel_in_flight(&mut self) {
        if let Some((_, request)) = self.in_flight.take() {
            request.cancel.store(true, Ordering::SeqCst);
            let _ = request.worker.join();
        }
    }
}

/// One control, on the thread that runs it. The flag is the caller's own
/// cancellation: a control abandoned while it waits is cancelled, and one already
/// dispatched is not retracted.
fn run(
    controls: &dyn ContainerControls,
    control: &Control,
    cancel: &AtomicBool,
) -> Result<ContainerAnswer, String> {
    match control {
        Control::Create(request) => controls.create_container(request, cancel),
        Control::Delete(request) => controls.delete_container(request, cancel),
        Control::Reload(request) => controls.reload_container(request, cancel),
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_command_line(line).expect("a command line")
    }

    #[test]
    fn a_command_line_splits_on_whitespace_and_understands_only_quotes() {
        assert_eq!(words("pi --model fast"), ["pi", "--model", "fast"]);
        assert_eq!(words("  pi   -p  "), ["pi", "-p"]);
        assert_eq!(
            words("echo 'a b' \"c d\" e\\ f"),
            ["echo", "a b", "c d", "e f"]
        );
        // Quotes join to the text beside them, and an empty pair is an empty word.
        assert_eq!(words("a'b c'\"d\""), ["ab cd"]);
        assert_eq!(words("run '' x"), ["run", "", "x"]);
        // Nothing is expanded or interpreted.
        assert_eq!(
            words("echo $HOME ~ *.rs | tee > out"),
            ["echo", "$HOME", "~", "*.rs", "|", "tee", ">", "out"]
        );
        assert_eq!(words("say \"a\\\"b\" 'c\\d'"), ["say", "a\"b", "c\\d"]);
    }

    #[test]
    fn a_line_that_is_empty_or_unfinished_is_refused_with_a_reason() {
        for line in ["", "   ", "\t"] {
            assert!(
                split_command_line(line)
                    .unwrap_err()
                    .contains("type a command")
            );
        }
        for line in ["echo 'x", "echo \"x", "echo x\\"] {
            assert!(split_command_line(line).is_err(), "{line}");
        }
    }

    #[test]
    fn an_executable_resolves_to_the_first_match_on_path_as_an_absolute_path() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("radar-resolve-{}", std::process::id()));
        let (first, second) = (root.join("one"), root.join("two"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        for directory in [&first, &second] {
            let tool = directory.join("tool");
            std::fs::write(&tool, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // A file that is not executable is passed over.
        std::fs::write(first.join("plain"), "x").unwrap();
        std::fs::write(second.join("plain"), "x").unwrap();
        std::fs::set_permissions(second.join("plain"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();

        assert_eq!(
            resolve_executable("tool", Some(&path)).unwrap(),
            first.join("tool").to_str().unwrap()
        );
        assert_eq!(
            resolve_executable("plain", Some(&path)).unwrap(),
            second.join("plain").to_str().unwrap()
        );
        assert!(resolve_executable("missing", Some(&path)).is_err());
        assert!(resolve_executable("tool", None).is_err());
        // A path is used as given when absolute and executable, never searched for.
        let absolute = first.join("tool");
        assert_eq!(
            resolve_executable(absolute.to_str().unwrap(), None).unwrap(),
            absolute.to_str().unwrap()
        );
        assert!(resolve_executable("one/tool", Some(&path)).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}

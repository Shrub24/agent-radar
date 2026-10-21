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

/// Build a private launch recipe that starts in `cwd` through the user's login
/// shell, then lets direnv evaluate that directory before execing the configured
/// argv. The command remains positional data, never shell-interpolated text.
pub fn target_shell_recipe(
    command: &[String],
    shell: &str,
    path: Option<&std::ffi::OsStr>,
    cwd: &str,
) -> Result<crate::control_plane::containers::Recipe, String> {
    use std::path::Path;

    let Some(program) = command.first().filter(|program| !program.trim().is_empty()) else {
        return Err("the configured launch command is empty".into());
    };
    if !Path::new(cwd).is_absolute() {
        return Err("the selected launch directory is not absolute".into());
    }
    let program = program.clone();
    let shell = resolve_executable(shell, path)
        .map_err(|_| "SHELL is not an executable; set it to fish, bash, or zsh".to_string())?;
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let script = match shell_name {
        "fish" => "type -q direnv; and eval (direnv export fish); exec $argv",
        "bash" => {
            "if command -v direnv >/dev/null 2>&1; then eval \"$(direnv export bash)\"; fi; exec \"$@\""
        }
        "zsh" => {
            "if command -v direnv >/dev/null 2>&1; then eval \"$(direnv export zsh)\"; fi; exec \"$@\""
        }
        _ => {
            return Err(format!(
                "SHELL `{shell_name}` is unsupported for managed launches; use fish, bash, or zsh"
            ));
        }
    };
    let mut argv = vec![
        "-ilc".to_string(),
        script.to_string(),
        "--".to_string(),
        program,
    ];
    argv.extend(command.iter().skip(1).cloned());
    Ok(crate::control_plane::containers::Recipe {
        executable: shell,
        argv,
        cwd: Some(cwd.to_string()),
        env: Default::default(),
    })
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
    /// The container this control names, where it names one before it runs: a
    /// creation names the container it made only once the daemon answers.
    pub fn container_id(&self) -> Option<&str> {
        match self {
            Self::Create(_) => None,
            Self::Delete(request) => Some(&request.container_id),
            Self::Reload(request) => Some(&request.container_id),
        }
    }

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
    /// The container the control named, where it named one: the answer is about
    /// that record even when the daemon refused it.
    pub container_id: Option<String>,
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
            container_id: control.container_id().map(str::to_string),
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

    #[test]
    fn managed_launch_runs_the_configured_argv_in_a_target_login_shell() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("radar-launch-shell-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fish = root.join("fish");
        std::fs::write(&fish, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&fish, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&root]).unwrap();
        let recipe = target_shell_recipe(
            &[
                "pi".into(),
                "--model".into(),
                "a value; still one argument".into(),
            ],
            "fish",
            Some(&path),
            "/tmp/project",
        )
        .expect("a user fish shell");

        assert_eq!(recipe.executable, fish.to_string_lossy());
        assert_eq!(recipe.cwd.as_deref(), Some("/tmp/project"));
        assert!(recipe.env.is_empty(), "do not copy the TUI environment");
        assert_eq!(recipe.argv[0], "-ilc");
        assert!(recipe.argv[1].contains("direnv export fish"));
        assert!(recipe.argv[1].contains("exec $argv"));
        assert_eq!(recipe.argv[2], "--");
        assert_eq!(
            recipe.argv[3], "pi",
            "resolve the command after shell and direnv setup"
        );
        assert_eq!(recipe.argv[4], "--model");
        assert_eq!(recipe.argv[5], "a value; still one argument");
        let _ = std::fs::remove_dir_all(root);
    }
}

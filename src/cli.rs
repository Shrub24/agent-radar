//! The command line Radar answers.
//!
//! Every invocation is declared here once, so `--help`, the usage error a
//! mistyped flag produces and the generated shell completions all describe the
//! same surface the binary parses. What each invocation *does* stays in
//! `main.rs`: this module only turns arguments into a decision.
//!
//! `run --launch` and `lifecycle-wake` are not for operators. The daemon
//! generates them into a launched process's own argv, so their spelling and flags
//! are an internal contract between the daemon and a child it launched; they are
//! hidden from help, and the tests pin them against the constants the daemon
//! writes.

use clap::{Parser, Subcommand};

/// The configuration Radar reads, printed with `--print-config`.
const CONFIGURATION: &str = "\
Configuration is read from `$RADAR_CONFIG`, else `$XDG_CONFIG_HOME/radar/config.toml`,
else `~/.config/radar/config.toml`. Radar draws without one.";

/// The crate version and the control-plane wire revision this build speaks. The
/// revision is the compatibility contract pi-radar checks; the version is for
/// people. A test pins the literal to [`crate::control_plane::ops::PROTOCOL_REVISION`].
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (wire 11)");

/// Everything Radar can be asked to do.
#[derive(Debug, Parser)]
#[command(
    name = "radar",
    version = VERSION,
    about = "Read-only fleet overview for local coding agents",
    after_help = CONFIGURATION,
    disable_help_subcommand = true,
    disable_version_flag = true
)]
pub struct Cli {
    /// Print the version.
    #[arg(short = 'v', short_alias = 'V', long = "version", action = clap::ArgAction::Version)]
    pub version: (),
    /// Print the configuration in use, to copy and edit.
    #[arg(long)]
    pub print_config: bool,
    #[command(subcommand)]
    pub invocation: Option<Invocation>,
}

/// One top-level command. No invocation runs the dashboard.
#[derive(Debug, Subcommand)]
pub enum Invocation {
    /// Run the dashboard as a narrow tree for a split beside an agent's tmux client.
    Sidebar,
    /// Experimental tmux-hosted workspace: the sidebar beside an attachment to the agents' server.
    Workspace {
        /// Existing session on the daemon's tmux server; omit for tmux's default.
        #[arg(long)]
        session: Option<String>,
    },
    /// The popup behind a sidebar's picker or confirmation. Started by the
    /// sidebar, not by hand.
    #[command(hide = true)]
    Modal {
        /// The key whose picker or confirmation to open.
        #[arg(long)]
        key: char,
        /// The selected row's identity, as JSON.
        #[arg(long)]
        row: Option<String>,
    },
    /// Present one daemon-owned interaction in a detached terminal.
    #[command(hide = true)]
    Present {
        #[arg(long)]
        interaction: String,
    },
    /// Control the daemon's user units; with no action, serve in the foreground.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
    /// Control the tmux server a daemon serves, by name.
    #[command(subcommand)]
    Server(ServerAction),
    /// Reconcile unresolved operations, or abandon one.
    #[command(subcommand)]
    Lifecycle(LifecycleAction),
    /// Reset transient Radar state, managed scopes and its served tmux server.
    Reset {
        /// Also forget durable subject identities, relationships and communications.
        #[arg(long)]
        hard: bool,
        /// Apply the reset after displaying its destructive scope.
        #[arg(long)]
        yes: bool,
    },
    /// Print shell completions for this command line.
    Completions {
        /// The shell to generate a completion script for.
        shell: clap_complete::Shell,
    },
    /// Run a program in this pane inside a managed scope.
    #[command(name = "run")]
    Run {
        /// Run an agent: it will report in and be attached to a subject. Without
        /// this the program is a managed command, kept in a scope and nothing more.
        #[arg(long, conflicts_with = "launch")]
        agent: bool,
        /// Claim the launch the daemon recorded for this pane and run its program.
        /// The daemon starts panes this way; it is not for operators.
        #[arg(long, hide = true, requires = "socket")]
        launch: bool,
        /// The daemon's control socket, for `--launch`.
        #[arg(long, hide = true, requires = "launch")]
        socket: Option<String>,
        /// The program and its arguments, after `--`.
        #[arg(
            last = true,
            required_unless_present = "launch",
            conflicts_with = "launch"
        )]
        program: Vec<String>,
    },
    /// Report that a launched process is up, so the daemon can bind its pane.
    #[command(name = "lifecycle-wake", hide = true)]
    LifecycleWake {
        /// The daemon's control socket.
        #[arg(long)]
        socket: String,
        /// The subject this launch was created for.
        #[arg(long)]
        subject: String,
        /// The launching process, as `pid:start_ticks:boot_id`.
        #[arg(long)]
        server: String,
    },
}

/// A lifecycle action for the daemon's own user units.
#[derive(Debug, Subcommand)]
pub enum DaemonAction {
    /// Start the control socket, which lets the daemon run on the next connection.
    Start,
    /// Stop the socket, then the daemon; the tmux server keeps running.
    Stop,
    /// Restart the daemon.
    Restart,
    /// Report both units.
    Status,
}

impl DaemonAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Status => "status",
        }
    }
}

/// A tmux server action, each naming the server it applies to.
#[derive(Debug, Subcommand)]
pub enum ServerAction {
    /// Start the server.
    Start {
        /// The server to control; the configured one by default.
        name: Option<String>,
    },
    /// Stop the server.
    Stop {
        /// The server to control; the configured one by default.
        name: Option<String>,
    },
    /// Restart the server.
    Restart {
        /// The server to control; the configured one by default.
        name: Option<String>,
    },
    /// Report the server.
    Status {
        /// The server to control; the configured one by default.
        name: Option<String>,
    },
}

impl ServerAction {
    /// The action's verb and the server it names.
    pub fn parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Start { name } => ("start", name.as_deref()),
            Self::Stop { name } => ("stop", name.as_deref()),
            Self::Restart { name } => ("restart", name.as_deref()),
            Self::Status { name } => ("status", name.as_deref()),
        }
    }
}

/// The operator's end of unresolved-operation recovery.
#[derive(Debug, Subcommand)]
pub enum LifecycleAction {
    /// Re-read current evidence and report what it could establish.
    Reconcile,
    /// End one unresolved record's recovery obligation.
    Abandon {
        /// The unresolved request id, checked before anything is dialled.
        request_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon writes these names into a launched child's argv, so a rename
    /// here would break every launch instead of failing at compile time.
    #[test]
    fn the_wake_command_keeps_the_name_the_daemon_writes() {
        let command = <Cli as clap::CommandFactory>::command();
        let found = command
            .get_subcommands()
            .find(|sub| sub.get_name() == crate::control_plane::lifecycle::WAKE_COMMAND)
            .expect("the wake subcommand");
        assert!(found.is_hide_set(), "the wake is not hidden from help");
    }

    /// A launch the daemon creates starts `radar run --launch --socket <path>`
    /// and nothing else: the program comes from the daemon's record.
    #[test]
    fn a_generated_launch_invocation_parses_without_a_program() {
        let cli = Cli::try_parse_from([
            "radar",
            "run",
            "--launch",
            "--socket",
            "/run/user/1000/agent-radar/control.sock",
        ])
        .expect("a generated launch invocation parses");
        let Some(Invocation::Run {
            launch,
            socket,
            program,
            ..
        }) = cli.invocation
        else {
            panic!("not a run invocation");
        };
        assert!(launch && program.is_empty());
        assert_eq!(
            socket.as_deref(),
            Some("/run/user/1000/agent-radar/control.sock")
        );
        assert!(
            Cli::try_parse_from(["radar", "run", "--launch", "--socket", "s", "--", "pi"]).is_err()
        );
    }

    /// A wake is started by a shell, without `--`, so its three flags are the
    /// whole invocation.
    #[test]
    fn a_generated_wake_invocation_parses_its_three_flags() {
        let cli = Cli::try_parse_from([
            "radar",
            crate::control_plane::lifecycle::WAKE_COMMAND,
            "--socket",
            "/run/user/1000/agent-radar/control.sock",
            "--subject",
            "6b1f3d0e-0000-4000-8000-000000000000",
            "--server",
            "4242:991122:0f3d",
        ])
        .expect("a generated wake invocation parses");
        let Some(Invocation::LifecycleWake {
            socket,
            subject,
            server,
        }) = cli.invocation
        else {
            panic!("not a wake invocation");
        };
        assert_eq!(socket, "/run/user/1000/agent-radar/control.sock");
        assert_eq!(subject, "6b1f3d0e-0000-4000-8000-000000000000");
        assert_eq!(server, "4242:991122:0f3d");
    }

    #[test]
    fn reset_defaults_to_preserving_subjects_and_accepts_hard_confirmation_flags() {
        let cli = Cli::try_parse_from(["radar", "reset"]).expect("reset parses");
        assert!(matches!(
            cli.invocation,
            Some(Invocation::Reset {
                hard: false,
                yes: false
            })
        ));

        let cli = Cli::try_parse_from(["radar", "reset", "--hard", "--yes"])
            .expect("confirmed hard reset parses");
        assert!(matches!(
            cli.invocation,
            Some(Invocation::Reset {
                hard: true,
                yes: true
            })
        ));
    }

    /// `radar server start` names the configured server, so the name is optional.
    #[test]
    fn a_server_action_takes_an_optional_name() {
        let cli = Cli::try_parse_from(["radar", "server", "restart"]).expect("parses");
        let Some(Invocation::Server(action)) = cli.invocation else {
            panic!("not a server invocation");
        };
        assert_eq!(action.parts(), ("restart", None));

        let cli = Cli::try_parse_from(["radar", "server", "restart", "other"]).expect("parses");
        let Some(Invocation::Server(action)) = cli.invocation else {
            panic!("not a server invocation");
        };
        assert_eq!(action.parts(), ("restart", Some("other")));
    }
}

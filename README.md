[![Keep the Why](https://keepthewhy.com/assets/badge.svg)](https://keepthewhy.com)

# Agent Radar

A control plane for coding agents running in terminals. Radar launches agents,
watches a whole fleet of them, and ends or restarts them safely, from a
full-screen dashboard and a local daemon other tools can talk to.

- **See the fleet.** Every agent, background task and pane across your
  workspaces in one tree: what each is doing, what it is waiting on, and how
  fresh that knowledge is.
- **Run agents as managed executions.** `radar run --agent -- pi`, or `L` in the
  dashboard, starts a program in its own systemd scope under a verified identity,
  so Radar can later stop exactly that process and nothing else.
- **Close and restart with evidence.** `D` ends a row and `r` restarts an agent
  in its session. Radar reports what it verified, never what it assumed: an exit
  it could not confirm is shown as unverified, not as finished.
- **Build on it.** The daemon owns lifecycle, launch, observation and durable
  request records behind a versioned local protocol. The dashboard is one client
  of it, not the authority.

Radar drives a tmux server it owns as its terminal backend, and shows tmux
sessions and windows as workspaces and tabs. Agents are identified by their own
process, never by a title or a label, so a row means one running thing.

## Concepts

| Term | Meaning |
| --- | --- |
| **Subject** | An agent's durable identity. It outlives restarts. |
| **Execution** | One run of a subject: a process with a verified identity, in a scope of its own. |
| **Place** | A pane, tab or workspace an execution lives in. |
| **Managed** | Started through Radar (`radar run`, `L`, or a spawn). Radar can stop and restart a managed agent. An agent started some other way is *observed*: visible, but not controllable until it is restarted through Radar. |

## Install

Radar is a Nix flake, `github:Shrub24/agent-radar`, for `x86_64-linux` and
`aarch64-linux` (`aarch64` is evaluated, not built). It needs a systemd user
manager with cgroup v2 and tmux on `PATH`.

```sh
nix run github:Shrub24/agent-radar           # run it
nix build github:Shrub24/agent-radar         # ./result/bin/radar
```

As a flake input:

```nix
inputs.radar.url = "github:Shrub24/agent-radar";
inputs.radar.inputs.nixpkgs.follows = "nixpkgs";
# ...
environment.systemPackages = [ inputs.radar.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

Following your nixpkgs builds Radar with it, so it must provide Rust 1.88 or
newer.

### Home Manager

The flake exports a Home Manager module that installs Radar and declares its user
units.

```nix
{ inputs, ... }:
{
  imports = [ inputs.radar.homeManagerModules.default ];

  programs.radar = {
    enable = true;
    # Optional: without `settings`, only the package is installed.
    settings = {
      colors.done = "light-green";
      appearance.working = "pulse";
      processes.nvim = "N";
    };
    # Programs that always run as managed agents (see below).
    managedAgents = [ pkgs.pi ];
  };
}
```

`programs.radar.settings` is freeform TOML written to
`$XDG_CONFIG_HOME/radar/config.toml`. Its `null` default manages no file and
leaves Radar with your own configuration or its built-in defaults.
`programs.radar.package` replaces the package.

### The daemon and the tmux server

The module declares three user units:

- `radar-control.socket` holds the control socket at
  `$XDG_RUNTIME_DIR/agent-radar/control.sock` and starts the daemon on the first
  connection. A stopped socket is the single switch for explicit closure.
- `radar-daemon.service` runs `radar daemon`. Stopping it never stops the tmux
  server or its panes.
- `radar-tmux@.service` runs one tmux server per instance name. The daemon's
  default server is `radar`.

```sh
radar                  # start the socket, connect, open the dashboard
radar daemon stop      # stop the socket, then the daemon; the tmux server keeps running
radar server stop      # refused while the server holds a pane or a pending operation
radar server status
radar run --agent -- pi
radar reset            # clear transient state; durable subjects and handoffs are kept
radar --version        # the version and wire revision
radar completions fish # bash, zsh, fish or elvish
```

An explicit stop holds until the next `radar` run or the next login. A refused
stop or restart sends no signal and names what is in the way. `radar --help`
is generated from the same declaration that parses arguments.

## Running agents under Radar

`radar run [--agent] -- COMMAND [ARG...]` starts a command in the pane you ran it
from, inside a systemd user scope of its own. No shell interprets the command, so
a `$VAR`, a glob or a newline arrives intact. `L`, restart and spawned launches
all go through this same path.

```sh
radar run --agent -- pi    # a scoped Pi in this pane, reporting in as an agent
```

The daemon admits the launch before anything runs: it refuses unless the process
is the pane's own descendant with a verified identity, and it records that
identity first. If a scope cannot be created, nothing runs; there is no unscoped
fallback.

`--agent` makes the program an agent with a subject, a parent link and controls.
Without it, `radar run -- COMMAND` is a managed command: it gets a scope and
nothing else, never reports in as an agent, and its children inherit no identity.

To make a program always run as a managed agent, list it in
`programs.radar.managedAgents`. It is installed under its own name as a wrapper
that runs `radar run --agent -- <original>`; `lib.managedAgent pkgs program` from
the flake builds the same wrapper.

When the program exits, Radar records how it ended: its exit code, or the signal
that stopped it, reported by the process that supervised it. That record survives
the pane being closed. See [managed execution](docs/managed-execution.md) for the
full contract.

## The dashboard

Run `radar`. The tree shows workspaces, tabs, panes and agents; the details panel
beside it shows the selected row's facts.

| Key | Action |
| --- | --- |
| `j` `k` / arrows | Move the selection. This view's own; it moves nothing in tmux |
| `Enter` | Focus the selected row's pane or workspace |
| `/` | Filter |
| `n` / `N`, `w` / `W` | Next row needing attention / next working row |
| `L` | Launch the configured agent in a new tab or pane |
| `a`, `A` | New tab / new workspace |
| `D` | Close the selected row, after a confirmation that names what it loses |
| `r` | Restart the selected managed agent in its recorded session |
| `o` | Follow the terminal's focus |
| `p`, `s` | Cycle which panes are listed / the sort order |
| `d`, `!` | Toggle the details panel / open the last outcome in full |
| `q` | Quit |

All keys, the mouse, the detail pages, marks, colours and configuration are in the
[dashboard reference](docs/dashboard.md). Configuration is optional: Radar reads
`$RADAR_CONFIG`, else `$XDG_CONFIG_HOME/radar/config.toml`, and
`radar --print-config` prints what is in use.

## Closing and restarting

`D` ends a row and the daemon decides what that means for it: an agent's process
(and its pane when Radar made the pane), a pane, a tab, or a whole workspace and
everything recorded inside it. A confirmation lists what would be lost and starts
on Cancel.

Ending is graceful first: TERM, one shared wait, then the scope the agent ran in
is killed if it is still running. A place with several agents costs one wait, not
one per agent. The result says, for each agent, whether its exit was **verified**,
and separately whether the pane was removed. A process that could not be ended is
reported as unverified, never recorded as exited, and it does not hold the close
open.

## For tools and other agents

The daemon speaks a versioned local protocol: observation, launch, close,
restart, messages between agents, and durable request records readable by ID.
Start with the [daemon guide](docs/daemon.md), then the
[protocol reference](docs/control-plane.md), the
[agent registration contract](docs/agent-registration.md) and the
[`pi-radar`](pi-radar/README.md) extension, which lets a Pi session publish to and
read from Radar.

## More

| | |
| --- | --- |
| [Dashboard reference](docs/dashboard.md) | Keys, mouse, detail pages, marks, colours, configuration |
| [Managed execution](docs/managed-execution.md) | Scopes, admission, stopping, completion |
| [How Radar observes a fleet](docs/observation.md) | Where each fact comes from, stale and missing data, background tasks |
| [Process evidence](docs/process-evidence.md) | The Processes page: identity, installed-binary comparison, measurement |
| [Daemon guide](docs/daemon.md), [protocol](docs/control-plane.md), [mux contract](docs/mux.md) | Building against the daemon |
| [Development](docs/development.md) | Building, testing, source layout, logs |
| [ideas.md](ideas.md) | Deferred functionality and options |

Logs: the daemon logs to the journal (`journalctl --user -u radar-daemon`); the
dashboard logs to `~/.local/state/agent-radar/tui.log`.

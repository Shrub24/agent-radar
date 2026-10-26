# The Home Manager module: install Radar for one user, run its control daemon and
# tmux server as user units, and optionally write the configuration file Radar's
# own loader already reads.
#
# Home Manager is deliberately not a flake input of Radar. The module is
# evaluated with whatever Home Manager and nixpkgs the consumer already runs, so
# importing it from a flake input cannot drag a second nixpkgs along; only the
# default package comes from this flake.
#
# The units are three, and each has one job:
#
# - `radar-control.socket` owns the control socket path. Listening means a
#   connection may start the daemon; stopped means nothing can, which is how an
#   explicit `radar daemon stop` lasts.
# - `radar-daemon.service` runs `radar daemon`. `KillMode=process` keeps a daemon
#   stop from taking the tmux server and its agents down with it.
# - `radar-tmux@.service` runs one tmux server per instance name, in the
#   foreground. It is started on demand and never restarted by systemd.
#
# Managed executions add a dependency the module does not create: a transient
# `systemd-run --user --scope` per launch needs the user manager itself. The
# daemon unit already requires it (it is a user unit), and a launch refuses
# before running when the manager is unavailable — there is no unscoped
# fallback, so nothing silently runs outside a scope.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.radar;

  # No file at all unless something was asked for: the server then loads nothing
  # from the user's home, as before.
  tmuxConfig =
    if cfg.tmux.configFile == null && cfg.tmux.extraConfig == "" then
      "/dev/null"
    else
      pkgs.writeText "radar-tmux.conf" (
        lib.optionalString (cfg.tmux.configFile != null) ''
          source-file -q ${toString cfg.tmux.configFile}
        ''
        + cfg.tmux.extraConfig
      );

  # `systemctl start/restart` is the readiness contract for every caller. tmux
  # has no sd_notify support, so this post-start check waits for its socket and
  # fails the unit if the server never becomes usable.
  tmuxReady = pkgs.writeShellScript "radar-tmux-ready" ''
    server="$1"
    for attempt in {1..100}; do
      if ${cfg.tmuxPackage}/bin/tmux -L "$server" show-options -g >/dev/null 2>&1; then
        exit 0
      fi
      sleep 0.05
    done
    echo "tmux server '$server' did not answer within 5 seconds" >&2
    exit 1
  '';
in
{
  options.programs.radar = {
    enable = lib.mkEnableOption "Radar, a full-screen fleet overview for local coding agents";

    package = lib.mkOption {
      type = lib.types.package;
      description = ''
        The Radar package to install. The module exported by the Radar flake
        defaults this to the package that flake provides for the consumer's
        system; a consumer importing this file directly must set it.
      '';
    };

    tmuxPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.tmux;
      defaultText = lib.literalExpression "pkgs.tmux";
      description = ''
        The tmux that serves the Radar server unit and that the daemon drives.
        Radar's observation design needs tmux 3.8 or newer.
      '';
    };

    tmux = {
      configFile = lib.mkOption {
        type = lib.types.nullOr (lib.types.either lib.types.path lib.types.str);
        default = null;
        example = "~/.config/tmux/tmux.conf";
        description = ''
          A tmux configuration the Radar server sources, such as your own. The
          server is started with no configuration file of its own, so without
          this it ignores yours. A string is passed through as written, so `~`
          is expanded by tmux; a missing file is not an error.

          The server keeps the configuration it started with: a change applies to
          a new server, or to the running one with
          `tmux -L radar source-file <file>`. Restarting the unit would close
          every pane.
        '';
      };

      extraConfig = lib.mkOption {
        type = lib.types.lines;
        default = ''
          set -g extended-keys on
          # csi-u is what Pi reads modified keys from; tmux's default format
          # reports them the older xterm way.
          set -g extended-keys-format csi-u
        '';
        description = ''
          tmux commands run after {option}`configFile`. The default turns on
          extended keys, which Pi needs for modified keys such as Shift+Enter.
        '';
      };
    };

    managedAgents = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.pi ]";
      description = ''
        Agents to start as Radar managed launches. Each one is installed
        under its own name, ahead of the original, as a wrapper that runs
        `radar run --agent -- <the original program>`, so starting it from a
        pane enters a managed scope and reports in as an agent. The wrapper names the original by store path,
        so it cannot resolve back to itself. A program started any other way
        stays observed and loses nothing.
      '';
    };

    settings = lib.mkOption {
      type = lib.types.nullOr lib.types.attrs;
      default = null;
      example = {
        colors.done = "light-green";
        appearance.working = "pulse";
        processes.nvim = "N";
        daemon.tmux_server = "radar";
      };
      description = ''
        Freeform TOML written to {file}`radar/config.toml` under the XDG
        configuration directory. The default, `null`, manages no file at all.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [
        cfg.package
      ]
      ++ map (program: lib.hiPrio (pkgs.callPackage ./managed-program.nix { radar = cfg.package; inherit program; }))
        cfg.managedAgents;
    }

    # `null` is install-only: Radar reads the user's own file, or its built-in
    # defaults. An explicit attrset — an empty one included — is a configuration
    # the user asked Home Manager to own, and it is written as given: Radar's
    # loader already supplies a default for every key a document leaves out, so
    # mirroring them here would be a second copy to keep in step.
    #
    # `RADAR_CONFIG` is deliberately not set: the environment override is the
    # user's, and Radar's precedence is unchanged.
    (lib.mkIf (cfg.settings != null) {
      xdg.configFile."radar/config.toml".source =
        (pkgs.formats.toml { }).generate "radar-config.toml" cfg.settings;
    })

    {
      systemd.user.sockets.radar-control = {
        Unit.Description = "Radar control daemon socket";
        Socket = {
          ListenStream = "%t/agent-radar/control.sock";
          SocketMode = "0600";
          DirectoryMode = "0700";
          RemoveOnStop = true;
          Service = "radar-daemon.service";
        };
        Install.WantedBy = [ "sockets.target" ];
      };

      systemd.user.services.radar-daemon = {
        Unit = {
          Description = "Radar control daemon";
          Requires = [ "radar-control.socket" ];
          After = [ "radar-control.socket" ];
        };
        Service = {
          ExecStart = "${cfg.package}/bin/radar daemon";
          # The daemon's own stop must not reach the tmux server or the agents in
          # its panes: only the main process is signalled.
          KillMode = "process";
          Restart = "on-failure";
          # A state root this build cannot read is never repaired by restarting:
          # radar daemon exits with EX_CONFIG (78), so record one failure with the
          # explanation instead of looping. `radar reset --hard --yes` clears it.
          RestartPreventExitStatus = 78;
          # The daemon names `tmux` and `systemctl` by absolute path and carries no
          # `PATH` of its own. tmux gives a new session the creating client's
          # `PATH`, so a `PATH` set here would be the `PATH` of every pane. Left
          # unset, panes inherit the user manager's, which holds the user's
          # profiles.
          Environment = [
            "RADAR_TMUX=${cfg.tmuxPackage}/bin/tmux"
            "RADAR_SYSTEMCTL=${pkgs.systemd}/bin/systemctl"
          ];
        };
      };

      systemd.user.services."radar-tmux@" = {
        Unit = {
          Description = "Radar tmux server %i";
          # A switch that changes the tmux store path must not stop this unit:
          # the server it runs holds every pane and every agent in them, so a
          # restart here kills the fleet rather than updating anything. The new
          # tmux is used by the next server, not this one.
          X-SwitchMethod = "keep-old";
        };
        Service = {
          Type = "exec";
          ExecStart = "${cfg.tmuxPackage}/bin/tmux -D -L %i -f ${tmuxConfig}";
          ExecStartPost = "${tmuxReady} %i";
          TimeoutStartSec = "10s";
          KillMode = "control-group";
          Restart = "no";
        };
      };
    }
  ]);
}

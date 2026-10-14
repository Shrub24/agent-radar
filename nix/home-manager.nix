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
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.radar;
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
    { home.packages = [ cfg.package ]; }

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
          # The daemon runs `tmux` and `systemctl --user`; user units do not
          # inherit the login shell's PATH.
          Environment = [
            "PATH=${lib.makeBinPath [
              cfg.tmuxPackage
              pkgs.systemd
            ]}"
          ];
        };
      };

      systemd.user.services."radar-tmux@" = {
        Unit.Description = "Radar tmux server %i";
        Service = {
          Type = "simple";
          ExecStart = "${cfg.tmuxPackage}/bin/tmux -D -L %i -f /dev/null";
          KillMode = "control-group";
          Restart = "no";
        };
      };
    }
  ]);
}

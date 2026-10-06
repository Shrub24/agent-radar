# The Home Manager module: install Radar for one user, and optionally write the
# configuration file Radar's own loader already reads.
#
# Home Manager is deliberately not a flake input of Radar. The module is
# evaluated with whatever Home Manager and nixpkgs the consumer already runs, so
# importing it from a flake input cannot drag a second nixpkgs along; only the
# default package comes from this flake.
#
# Nothing here starts a service or creates a runtime directory: Radar is a
# program a user runs, and Herdr and the optional icon font stay separate
# installs.
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

    settings = lib.mkOption {
      type = lib.types.nullOr lib.types.attrs;
      default = null;
      example = {
        colors.done = "light-green";
        appearance.working = "pulse";
        processes.nvim = "N";
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
  ]);
}

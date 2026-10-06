{
  description = "Agent Radar: a full-screen fleet overview for local coding agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # The development shells keep every system the flake has always offered.
      devSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      # The package is supported and checked on Linux only; Darwin is deferred.
      packageSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forDevSystems = f: nixpkgs.lib.genAttrs devSystems (system: f nixpkgs.legacyPackages.${system});
      forPackageSystems =
        f: nixpkgs.lib.genAttrs packageSystems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forPackageSystems (pkgs: rec {
        radar = pkgs.callPackage ./nix/package.nix { };
        default = radar;
      });

      apps = forPackageSystems (pkgs: rec {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.radar}/bin/radar";
          meta.description = "Agent Radar: a full-screen fleet overview for local coding agents";
        };
        radar = default;
      });

      # The Home Manager module, for a consumer to import. It is a plain module
      # evaluated with the consumer's own Home Manager and nixpkgs, so this
      # flake adds no Home Manager input; the only thing supplied here is the
      # default package, which is the one this flake already exports.
      homeManagerModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/home-manager.nix ];
          programs.radar.package =
            lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.radar;
        };

      # The package derivation and its check are the same derivation, so a
      # consumer builds exactly what the package output offers.
      checks = forPackageSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.radar;
      });

      devShells = forDevSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            python3
          ];
        };
      });
    };
}

# Design

## Context

See `proposal.md` for motivation. `flake.nix` currently provides four development-shell systems only. The package is a single Rust 2024 crate, binary `radar`, MSRV 1.88, with a complete crates.io lockfile and no custom build script. The pinned nixpkgs Rust helper supports `cargoLock.lockFile`; its Rust toolchain satisfies the MSRV.

`src/config.rs` already owns native defaults and TOML parsing. Configuration resolves `RADAR_CONFIG`, XDG config and the user's default config directory in that order. Herdr is invoked from PATH; fonts are optional. Neither belongs to the build dependencies. No package or sandbox build was performed during scoping.

## Goals / Non-Goals

**Goals:** a checked locked package, stable flake-input exports, install-only or declaratively configured Home Manager use, and concrete consumer validation.

**Non-Goals:** service/daemon lifecycle, NixOS-specific configuration, overlays, a flake-parts migration, bundled runtime/font assets, Darwin distribution claims, application behavior changes and process analytics.

## Decisions

1. **Keep the plain flake and small Nix files.** Use a package expression under `nix/`, called with the flake's nixpkgs, and a separate HM module. Preserve all current dev-shell entries. Export package/app/check attributes for the two Linux systems; build the native host and evaluate the other Linux derivation without calling evaluation a successful build. Rejected: migrating to flake-parts for two small exports or advertising every dev-shell system as tested package support.

2. **Use the existing Rust packaging machinery.** `rustPlatform.buildRustPackage` reads `Cargo.lock`, with `radar` as `meta.mainProgram`; derive version from the existing manifest rather than maintaining a second version. Keep source filtering simple and retain `tests/fixtures/`, because library tests embed fixtures. Ordinary build/check phases remain offline; declared source fetches are separate. Do not assign an undeclared license to the repository. Rejected: custom vendoring, a new toolchain overlay, a second lockfile or developer-shell-only installation.

3. **Checks are part of distribution.** Keep the standard package test phase enabled and export `checks.<system>.package` as the same derivation, not a parallel test framework. Start with the existing Rust checks. Reproduce sandbox failures before adjusting targeted interpreter/utility setup; generated `#!/bin/sh` test scripts cannot be repaired merely by running `patchShebangs` on repository source. The standalone Python PTY smoke remains a development gate unless its sandbox requirements are proved. Rejected: disabling all checks because tests might fail, or putting the complete PTY harness into every package build without evidence.

4. **Home Manager owns per-user configuration.** Export `homeManagerModules.default` without adding Home Manager as a production flake input. The module uses `programs.radar.enable`, an overridable package defaulting to this flake's package, and `settings` of nullable TOML-format type. Null means install only; an explicit attrset, including an empty one, creates `xdg.configFile."radar/config.toml"`. Native defaults remain native; the module does not mirror the palette schema or force `RADAR_CONFIG`. Rejected: compulsory generated defaults, a typed duplicate of every Rust option, and a NixOS module for a program needing no privileged resources.

5. **Runtime dependencies remain operator choices.** Do not wrap a local Herdr checkout or add Herdr/font assets to the package/module. Document installing Herdr on PATH, optional icons and existing environment overrides. System-wide installation can use the package in `environment.systemPackages`. Rejected: tying distribution to the present mux before the portability audit or packaging a font with unclear provenance.

6. **Validate the consumer interface, not just Nix syntax.** Check native sandbox build, installed `--help`/`--print-config`, package/app/check equality and preserved dev shells. Evaluate an isolated HM consumer with pinned compatible HM/nixpkgs inputs, exercise disabled/install-only/settings/package-override cases and verify its TOML is loaded by the installed binary. The temporary consumer may reference this local flake; record its inputs/commands, and do not claim the GitHub revision already exports unpushed changes. Rejected: syntax-only evaluation as installation acceptance or changing the user's active Home Manager configuration.

## Risks / Trade-offs

- Generated fake-runtime scripts depend on `/bin/sh` and command utilities → diagnose in the actual sandbox; supply interpreter/PATH dependencies explicitly and preserve source-test behavior.
- A pre-existing UI test rejected the text-font rendering `π 01a0edc4`, although that is a correctly stripped title preceded by the vendor mark → reproduced in the sandbox and with `RADAR_ICONS=text`. Correct only its mode-dependent assertion, checking the exact normalized row segment so a genuinely duplicated prefix still fails. Do not force an absent font, skip the test or change production rendering.
- Kernel/process tests contain deadlines → run the package gate serially, distinguish an actual test failure from a successful build, and do not weaken timing checks speculatively.
- PTY availability varies between builders → keep the focused PTY smoke separate unless its sandbox behavior is demonstrated.
- Consumers following older nixpkgs may select Rust below MSRV → document compatible inputs; do not silently fetch a second toolchain.
- The second Linux architecture is not locally built, and Darwin is unverified → record evaluation versus native build evidence separately; Darwin package export is deferred.
- Nix-managed settings conflict with an existing file → configuration is opt-in and follows normal Home Manager file ownership; do not overwrite the user's configuration as part of verification.

## Migration Plan

No application migration is required. Existing source/dev-shell use remains valid. A consumer adds the Radar flake input, selects the package or imports the HM module, and optionally supplies settings. Rollback removes the module/package or pins the previous flake revision. This change neither creates owner-control transport nor restarts existing agents.

## Decisions recorded at apply

- Home Manager is the consumer's input, not this flake's: the module is a plain
  module evaluated with the consumer's own Home Manager and nixpkgs, so pinning one
  here would couple every consumer to this repository's revision.
- `settings` is freeform TOML. `src/config.rs` owns defaults, parsing and
  precedence, so a typed mirror in Nix would be a second copy.
- `settings = null` writes no file and so never overwrites a user's own config; an
  explicit attrset, an empty one included, is written as given.
- Rendering tests derive expected marks from `theme::logo`, never literals: the
  sandbox has no icon font, so a literal makes the result depend on the host.
- The package builds from an explicit fileset rather than the whole tree, so prose
  and plan edits do not rebuild it.

# Proposal

## Why

Radar currently requires a development shell and a local Cargo build. A consumer should be able to install the binary and configure it directly from this repository as a Nix flake input.

## What Changes

- Add a locked Rust package, default package/app outputs and a package check for Linux, preserving the existing development shells.
- Export a Home Manager module with `programs.radar.enable`, an overridable package and optional freeform TOML settings at the existing XDG configuration path.
- Keep runtime providers and fonts separately installed; the package must not depend on a developer checkout, running Herdr instance or owner-control directory.
- Document direct flake-input consumption and validate package builds and module configuration in an isolated consumer.

Out of scope: a service/daemon, a NixOS module, overlays, bundled Herdr/fonts, process analytics, new runtime behavior and unverified Darwin package support.

## Capabilities

### New Capabilities

- `nix-distribution`: reproducible binary installation through flake outputs and declarative per-user configuration through Home Manager.

### Modified Capabilities

None. Existing fleet observation, rendering and lifecycle requirements are unchanged.

## Impact

- `flake.nix` and small files under `nix/` for the package and module.
- `README.md` installation instructions and targeted distribution validation.
- Generated-script test interpreter/PATH handling only if required to make the existing automated tests work in the package sandbox; tests are not pre-emptively disabled.
- Uses the existing pinned nixpkgs Rust toolchain and `Cargo.lock`; no Rust dependency, production Home Manager flake input or plugin framework is added.

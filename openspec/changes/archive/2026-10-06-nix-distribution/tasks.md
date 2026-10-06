# Tasks

## 1. Checked package and app

- [x] 1.1 Add the locked Rust package under `nix/` and Linux package/default app/package-check outputs while preserving development shells. Retain fixture sources and enabled Rust checks; reproduce and fix only necessary sandbox test-environment assumptions (including interpreter/PATH setup and the proven font-dependent UI assertion). Document package use and prerequisites. Verify native sandbox `nix build`, installed `--help`/`--print-config` in an isolated environment, package/default/check equality and app path; evaluate the other Linux package and distinguish that from build evidence.

## 2. Home Manager consumer

- [x] 2.1 Export the small Home Manager module with enable, package override and nullable TOML settings, without a service or production HM input. Add direct-input installation/configuration documentation. Evaluate an isolated pinned HM consumer for disabled, install-only, configured, empty-settings and package-override cases; build its generated TOML and confirm the installed binary loads it at the native XDG path without forcing `RADAR_CONFIG`.

## 3. Independent integration gate

- [x] 3.1 Independently verify the checked native package/app and `nix flake check`, the pinned HM consumer and preserved dev-shell/source checks (fmt, all-target Clippy, locked Rust tests and focused PTY smoke). Run strict OpenSpec validation and record exact commands, revisions, native-build versus cross-system-evaluation evidence, and any limitations in `verification.md`. Do not activate the user's Home Manager configuration or touch a live runtime/control directory.

# Verification

Run on `x86_64-linux`, 2026-10-06, working copy over `fbcb0be0`. The lead ran every
command below independently of the worker's reports.

## Package (task 1.1)

- `nix build .#radar` ran the crate's own checks in the sandbox: 361 tests, 0 failed.
  Output `/nix/store/q17d8yi9y70cdcycv07cikh1fx4wfilg-agent-radar-0.1.0`, the same
  package the Home Manager consumer installs.
- `nix flake check` passes: `packages.radar`, `packages.default` and
  `checks.package` evaluate to one derivation, and `apps.*.program` is that
  output's `bin/radar`. Dev shells are unchanged.
- The installed binary, run under `env -i` with temporary `HOME` and
  `XDG_CONFIG_HOME`, answers `--help` and `--print-config`, and the output parses
  as TOML.
- The source fileset retains `docs/radar-bus.fixture.json` and `tests/`; `README.md`
  and `openspec/` are excluded, so documentation edits do not rebuild the package.

## Home Manager module (task 2.1)

Pinned consumer: nixpkgs `0954f7ee2f6bb3dc7d4e3d0d8bcb8fd4bde4cfc5`, home-manager
`f53f3267f5d009dd8f99443505e609389d7ff267`, Radar as a `path:` input
(a real consumer writes `github:Shrub24/agent-radar`). Evaluated with `nix eval`:

| Case | Package installed | `radar/config.toml` |
| --- | --- | --- |
| `enable = false` | none | none |
| `enable = true` | `agent-radar-0.1.0` | none |
| `settings = { colors.done = "red"; ... }` | `agent-radar-0.1.0` | written |
| `settings = { }` | `agent-radar-0.1.0` | written, 0 bytes |
| `package = pkgs.hello` | `hello-2.12.3` only | none |

`systemd.user.services` is empty in every case and the module never sets
`RADAR_CONFIG`. The generated document, placed at `$XDG_CONFIG_HOME/radar/config.toml`
and read by the installed binary, shows `done = "red"` and `working = "moon"`. An
empty document gives the built-in defaults (`done = "light-green"`, `working =
"pulse"`), and `RADAR_CONFIG` still takes precedence over the XDG file.

## Source gate

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked` (207 library tests plus integration suites, 0 failed) and both
`tests/terminal_smoke.py` scenarios pass. `openspec validate --all --strict` passes
(11 items).

## Limits

- `aarch64-linux` was evaluated, not built. Darwin packaging is out of scope;
  `nix flake check --all-systems` still fails on the pre-existing `x86_64-darwin`
  dev-shell entry.
- `nix flake check` reports "unknown flake output `homeManagerModules`": the
  conventional name is simply not recognised by the checker.
- The consumer used a local path input. A `github:Shrub24/agent-radar` consumer is
  unverified until the change is pushed.
- Following a consumer's nixpkgs needs a Rust of 1.88 or newer.
- The user's own Home Manager configuration was not activated and no live runtime or
  control directory was touched.
- Pre-existing and untouched: `a_finished_session_keeps_its_name_and_says_it_has_exited`
  still expects a mark under `RADAR_ICONS=none`.

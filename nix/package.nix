# The Radar package: one locked Rust binary built from this repository's own
# Cargo.lock.
#
# Herdr and the optional icon font are runtime choices, not build dependencies,
# so they are deliberately absent here. The version and name come from the
# manifest, so there is no second place to keep them in step.
{
  lib,
  rustPlatform,
}:

let
  manifest = lib.importTOML ../Cargo.toml;

  # Only what the Rust build reads. `tests/fixtures/` is embedded with
  # `include_str!`, and `docs/radar-bus.fixture.json` is read by the bus tests at
  # runtime through `CARGO_MANIFEST_DIR` — they panic without it. The README, the
  # plans and the VCS/dev directories are not build inputs.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../src
      ../tests
      ../Cargo.toml
      ../Cargo.lock
      ../docs/radar-bus.fixture.json
    ];
  };
in
rustPlatform.buildRustPackage {
  pname = manifest.package.name;
  version = manifest.package.version;

  inherit src;

  cargoLock.lockFile = ../Cargo.lock;

  # The crate is `agent-radar`; the program a user runs is `radar`.
  meta = {
    description = manifest.package.description;
    mainProgram = "radar";
  };
}

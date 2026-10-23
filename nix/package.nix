# The Radar package: one locked Rust binary built from this repository's own
# Cargo.lock.
#
# Herdr and the optional icon font are runtime choices, not build dependencies,
# so they are deliberately absent here. The version and name come from the
# manifest, so there is no second place to keep them in step.
#
# The build is two derivations, not one. `buildDepsOnly` compiles the dependency
# graph against a dummy crate, so its cache key is the manifest and Cargo.lock
# alone: editing `src/` reuses it, and the package derivation then compiles only
# this crate. That is the whole reason for crane here — one derivation cannot
# cache its dependencies across a source change.
{
  lib,
  pkgs,
  crane,
  installShellFiles,
}:

let
  manifest = lib.importTOML ../Cargo.toml;
  craneLib = crane.mkLib pkgs;
  inherit (craneLib) buildDepsOnly buildPackage;

  # Only what the Rust build reads. `tests/fixtures/` is embedded with
  # `include_str!`, and `docs/radar-bus.fixture.json` is read by the bus tests at
  # runtime through `CARGO_MANIFEST_DIR` — they panic without it. The SDK client is
  # embedded by the contract drift test. The README, the
  # plans and the VCS/dev directories are not build inputs.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../src
      ../tests
      ../Cargo.toml
      ../Cargo.lock
      ../docs/radar-bus.fixture.json
      ../pi-radar/extension/radar-client.ts
    ];
  };

  # What both derivations share. `cargoExtraArgs` defaults to `--locked`, so the
  # build cannot resolve anything the lock does not already name.
  common = {
    pname = manifest.package.name;
    version = manifest.package.version;

    inherit src;

    strictDeps = true;

    # The build and the test both use the release profile, so the test run reuses
    # the optimised dependency artifacts instead of compiling the graph twice. It
    # is set for both derivations so the caching they share cannot diverge.
    CARGO_PROFILE = "release";
  };

  # Dependencies only: the cache key is Cargo.toml and Cargo.lock, so a source
  # edit rebuilds this crate and nothing else. The release version is deliberately
  # not part of it — naming this layer with the crate's own version would rebuild
  # the whole graph on every release bump, which is churn the graph never caused.
  cargoArtifacts = buildDepsOnly (common // { version = "lock"; });
in
buildPackage (
  common
  // {
    inherit cargoArtifacts;

    doCheck = true;

    # Completions are generated from the built binary, so the tool that installs
    # them belongs to this derivation and not to the dependency layer.
    nativeBuildInputs = [ installShellFiles ];

    # Completions are installed where each shell already looks:
    # `share/bash-completion/completions`, `share/zsh/site-functions` and
    # `share/fish/vendor_completions.d`. Installed this way they arrive with the
    # package, so no consumer — Home Manager included — has to wire a shell up
    # for them.
    postInstall = ''
      installShellCompletion --cmd radar \
        --bash <($out/bin/radar completions bash) \
        --fish <($out/bin/radar completions fish) \
        --zsh <($out/bin/radar completions zsh)
    '';

    # The crate is `agent-radar`; the program a user runs is `radar`.
    meta = {
      description = manifest.package.description;
      mainProgram = "radar";
    };  }
)

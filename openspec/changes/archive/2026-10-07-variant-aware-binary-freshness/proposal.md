## Why

Every pi-bolt session shows no freshness at all. The process Radar reads holds
`…/<build>/lib/pi-bolt/pi`, whose file name is `pi`, while `PATH` resolves the
launcher the pane was started from (`pi-bolt`, `pi-bolt-child`); the comparison
requires both file names to match, so each lead and child pane falls to unknown
and the mark silently disappears. `pi` is the exec-replaced payload's own name —
it identifies neither the family nor the invocation, and today it is what the
comparison is keyed on.

## What Changes

- Replace the file-name comparison with an explicit **package family** one for
  the bolt variants: separate the store hash, the exact package name and the
  numeric version, and compare the running executable's package root with the
  payload package root of the installed counterpart for that exact name.
- Keep the existing comparison unchanged for every other program, including
  non-Nix and non-store paths. No heuristic package-name discovery.
- A same-version rebuild remains stale: differing store roots are stale by
  design, with no version-string equality exception.
- A deleted executable is stale on its own, without a resolved counterpart.
- Never use the runtime-reported program name (`pi`) to select an installed
  program; it is the observed process's name for display only.
- Show the executable identity on Processes for agents and ordinary panes that
  have current foreground evidence: the observed process's command name, the
  full sanitized executable path, the running and installed package identities,
  and the reason when the freshness is unknown. A current identity stays
  inspectable; the observed name and arguments are never presented as the
  original invocation, and no prompt arguments are shown.
- Resolve the installed counterpart through the `PATH` entrypoint for the exact
  family name, canonically and without pinning a profile path or a version. The
  primary supported shape is the derivations' own entrypoints — `pi`, `pi-bolt`
  and `pi-bolt-child` in a package's own `bin`, running that package's
  `lib/pi-bolt/pi` — where the entrypoint's package root already is the payload
  root and nothing is parsed.
- For the profile-level script shims in use today, whose entrypoint root and
  payload root differ, carry a bounded provisional resolver: only a script with
  exactly one strictly literal `exec <absolute store path>` target counts, and
  every other shape is unknown. It never depends on fixed script contents, never
  executes anything, never reads compiled content, and is deleted once packaging
  provides same-root entrypoints.
- Read no metadata file: a package identity/stamp document is separate future
  build-and-plugin introspection and there is no consumer for it in this change.

Out of scope: task metrics, lifecycle actions, the managed-worker build/fingerprint
records, a generic packaging parser or plugin/executable-set discovery, and any
new configuration key or environment read.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `stale-binary`: the comparison is by exact package family and package root
  rather than by matching file name; staleness from a deleted executable no
  longer needs a resolved counterpart; the details state package identities and
  the unknown reason on the Processes page for agents and ordinary panes.

## Impact

`src/procfs.rs` owns the comparison and the new bounded resolution;
`src/collector.rs` calls it; `src/model.rs` carries the identities and the
unknown reason; `src/ui.rs` draws the Processes fields and the row mark;
`src/tree.rs` supplies the mark. `tests/stale_binary.rs` and the `src/ui.rs`
render tests cover the new semantics, and README documents them. No new
dependency, no new configuration key, no environment access, no runtime-provider
change, and no execution of the programs being compared.

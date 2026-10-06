## Context

`LocalFacts` (`src/model.rs`) already carries what Radar reads from `/proc` for a pane's foreground process, composed in the collector above the runtime seam. Herdr's `pane process-info` supplies the pid and program name; the snapshot has no pid, so a process is known only after a per-pane query. Today those queries cover continuity candidates, or every pane while the process view is active.

On a Nix machine a live `pi` reports `/proc/<pid>/exe` as `/nix/store/<hash>-pi-1.0.2/libexec/pi/pi`, while `readlink -f` of the PATH entry gives `/nix/store/<hash>-pi-1.0.2/bin/pi`. The two files differ inside one package, so the comparison cannot be on the executable path.

## Goals / Non-Goals

**Goals:** an honest, conservative stale mark for live agents whose package has been replaced, with no false marks for deliberate other builds.

**Non-Goals:** extension or plugin versions; version-string parsing; restarting anything; a persisted setting; non-Linux support; marking processes that are not agents.

## Decisions

1. **Compare installations, not files.** A process's identity is its `/proc/<pid>/exe` target; the installed one is the PATH match for the process's own program name, resolved with symlinks followed. Under `/nix/store` the identity is the store root (`/nix/store/<hash>-<name>`), so a wrapper and its inner binary agree. Anywhere else it is the resolved path.
2. **Stale is conservative.** When an installed executable can be resolved for the same program, a row is stale when the executable link reads `(deleted)` (its file was replaced or collected: the usual result of any package manager's rename-over), or when both identities are under `/nix/store` and the roots differ. A process from elsewhere (a dev build, a checkout) is *not* the PATH program and is not stale: it has no mark, and the details say so. Rejected: marking every difference, which would flag a deliberate local build on every row.
3. **Three answers plus comparison identities.** The fact is `Current`, `Stale` or `Unknown`. `Current` means no replacement was detected; readable deliberate other builds retain differing identities so details can distinguish them without a fourth state. A process that is gone, an unreadable link, no executable PATH match or a platform that cannot be read is `Unknown`; nothing is shown and nothing is inferred. Basename mismatch is also unknown: a helper or interpreter is not the named program.
4. **One PATH lookup per program per refresh.** Resolve each distinct program name once per collection, with Radar's own `PATH`; do not read a process's environment. Radar launched with a different PATH than the sessions can therefore mislabel; the details line names both paths so the operator can see why.
5. **The sweep grows to live agent panes.** The collector's candidates become continuity candidates plus every pane that currently reports an agent. Cost is one `process-info` per agent pane per refresh (about 3 ms each, measured earlier). Rejected for now: caching by pid and start time; add it only if the sweep is measured to matter. An agent whose foreground is a shell or inconclusive has no process and so no mark.
6. **Composition stays above the seam.** The runtime reports pid and name as it does now; `procfs` reads the executable link and PATH. Nothing in `herdr.rs` or `runtime.rs` changes.
7. **A mark, not a state.** Stale agent rows get a Nerd Font warning glyph with a plain-text fallback in the stale colour. Details name the running and installed identities exactly; do not parse versions from package names. A readable deliberate other build gets `binary: not the installed program` with both identities. Unknown and retained rows show nothing. Colour is a new `[colors] stale` role so the operator owns it.
8. **Only current observations make a binary claim.** A stale/unavailable inventory keeps last-good facts for continuity but does not draw a binary warning or comparison. Rejected: presenting an old executable comparison as fresh OS evidence.
9. **PATH candidates must be programs.** Follow symlinks and accept executable regular files only, continuing past directories and non-executable files. Reject reported names containing path separators. Rejected: canonicalizing the first existing path, which can mask the actual installed executable and produce false marks.

## Risks / Trade-offs

- [PATH differs between Radar and the sessions] → the details line names the running and installed identities; only the Nix-store and deleted rules produce a mark, so a different PATH yields at most a wrong mark for a program installed in two Nix profiles. Revisit with a config key if it bites.
- [Wrapper scripts and interpreters] → `node`-hosted agents report the interpreter as program; the lookup finds `node`, which is not an agent identity. Limit v1 to agents whose reported program resolves to their own executable; otherwise Unknown.
- [Extra queries each poll] → bounded by the number of agent panes; measure and cache only on evidence.
- [A freshly installed program with the same store root as a running one] → equal roots read Current, never stale.

## Migration Plan

No data or config migration; the colour role defaults to a built-in. Reverting removes the mark and the extra sweep. Lifecycle's later "restart stale" reads this fact; it is not part of this change.

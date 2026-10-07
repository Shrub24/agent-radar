# Tasks

## 1. Family and package-root comparison

- [x] 1.1 Separate a store derivation name into hash, exact package name and numeric version beside the resolver in `src/procfs.rs`, handling only the exact names `pi-bolt` and `pi-bolt-child`; unit-test the separation for both families plus a name with no version, a hyphenated name and a malformed name.
- [x] 1.2 Compare the running executable's package root with the resolved counterpart's payload package root, keeping the existing matching-file-name comparison unchanged for every other program and for non-store paths; test main current, main stale, child current, child stale, same-version/different-root stale, and a non-bolt program keeping today's verdict.
- [x] 1.3 Make a `(deleted)` link stale with no counterpart resolved, and make every unresolved or ambiguous case unknown with a reason; test deleted without a counterpart, deleted with a counterpart, and unknown because no exact counterpart resolves.
- [x] 1.4 Carry the running executable's path, the running package identity, the installed counterpart identity and a small typed unknown reason through `BinaryIdentity` in `src/model.rs`, so a verdict that could not be made can state why while a readable executable stays nameable.
- [x] 1.5 Extend the `tests/stale_binary.rs` fixtures to package-shaped bolt trees with a launcher script and an injected `PATH`, and prove cross-family rejection: `pi-bolt` never accepts `pi-bolt-child`, a child payload reported as `pi` is never compared with the lead launcher, and a stock-pi payload is never compared through the `pi` alias.

## 2. Payload resolution

- [x] 2.1 An entrypoint inside a versioned package of the same family names that package root as the payload root, with nothing read from it; test both families in that shape, including a package whose own `bin` entrypoint sits beside its `lib/pi-bolt/pi`, and test that an unversioned launcher root is not taken as a payload root for being the entrypoint's own.
- [x] 2.2 Resolve a legacy cross-root launcher entrypoint — the profile's unversioned script package — only from exactly one unconditional literal `exec /nix/store/…/bin/<program>` line that the script ends with, documenting the accepted shape, its transitional status and its ceiling beside the code.
- [x] 2.3 Refuse every other entrypoint shape as unknown, with a test per shape: no `exec` line, several `exec` lines, a computed or quoted target, a conditional or looped target, a target outside the Nix store, a non-script file, a further hop and a cycle.
- [x] 2.4 Prove the mechanism's boundaries in tests: no subprocess is spawned, no environment is read, no compiled content is read, no metadata or stamp file is read, and each family is resolved once per refresh.

## 3. Processes identity display

- [x] 3.1 Draw the observed command name, the full sanitized executable path, the running package identity, the installed counterpart identity and the unknown reason on the Processes page, for agent rows and for ordinary pane rows whose foreground evidence is current.
- [x] 3.2 Keep a current identity inspectable, and never present the observed name, its arguments or the executable path as an invocation, an alias or a launcher; keep prompt and system-prompt arguments out entirely.
- [x] 3.3 Add render regressions for a current bolt row, a stale bolt row, an unknown verdict with its reason, an ordinary pane row, an exec-replaced `pi` name shown as an observed name, and withholding for retained rows, stale or unavailable sources, shells and inconclusive foregrounds.

## 4. Documentation and gates

- [x] 4.1 Document the family and package-root semantics, the same-root entrypoint rule, the transitional cross-root resolver and the follow-up that deletes it when packaging ships same-root entrypoints, the alias rule, and the corrected current profile path (`/etc/profiles/per-user/...`, not `~/.nix-profile`) in README.
- [x] 4.2 Run fmt, Clippy with warnings denied, locked tests and build, both PTY smokes, the checked Nix package and strict OpenSpec validation; record the commands, outcomes and anything unproven in `verification.md`.
- [x] 4.3 Observe the live matrix read-only — lead and child panes against the current profile — and record it, including that a same-version rebuild is stale by design.

# variant-aware-binary-freshness — implementation slice (groups 1–2)

Status: groups 1 (family/package-root comparison) and 2 (payload resolution)
implemented. Groups 3 (Processes-page identity display) and 4 (README docs +
live matrix) are **not** implemented — see Deferred.

No VCS mutations were made. OpenSpec task checkboxes left unticked for owner
verification.

## Delivered

- `src/procfs.rs` — resolver for bolt variants.
  - `BOLT_FAMILIES = ["pi-bolt", "pi-bolt-child"]`; `MAX_LAUNCHER_BYTES = 64 KiB`.
  - `store_package` splits a store root name into hash, exact package name and
    numeric version; `bolt_family` reads the family from the running file's own
    versioned store package only (exact name, never a prefix).
  - `counterpart_root` (exported, injectable paths) resolves a same-family
    counterpart; `script_target` + `literal_store_path` read only the accepted
    transitional shim shape and `script_target` names the launch target where
    the same-root metadata would live.
  - `bolt_identity` / `named_identity` / `compare_named`: runtime name never
    selects a bolt counterpart; every non-bolt program and every non-store path
    keeps the original file-name comparison.
  - `Procs::binary_identity` reads the exe first and reports `Unreadable`
    instead of a blank default; `identity(running, deleted, program, binaries)`.
- `src/model.rs` — `BinaryIdentity` carries `executable` (the running file's own
  path) and `unknown: Option<BinaryUnknown>`; new typed reasons `NoCounterpart`,
  `UnsupportedLauncher`, `Unreadable`, `NotCompared`. `BinaryFreshness` and the
  `running`/`installed` meaning are unchanged for non-bolt programs.
- `src/lib.rs` — re-exports `BinaryUnknown`.
- `src/ui.rs` — fixture literals only (`..BinaryIdentity::default()`); no
  presentation change.
- `tests/stale_binary.rs` — 7 new regressions alongside the updated `Option<&str>`
  signature: `a_lead_running_another_build_of_the_same_version_is_stale`,
  `a_child_is_resolved_by_its_own_family`,
  `a_child_is_never_compared_with_the_lead_or_the_alias`,
  `a_runtime_name_never_selects_a_bolt_counterpart`,
  `a_deleted_executable_is_stale_without_a_counterpart`,
  `a_versioned_packages_own_entrypoint_names_its_own_payload_root`,
  `only_a_launcher_shape_that_says_where_it_goes_is_read`,
  `an_unversioned_launcher_package_is_resolved_through_its_target`.
- OpenSpec artifacts under `openspec/changes/variant-aware-binary-freshness/`
  (proposal, design, `specs/stale-binary/spec.md`, tasks).

## Review-gap fix (exact-family transitional validation)

The review found `counterpart_root` accepted a launcher target whose root was any
unrelated package, so long as it was not the *other* recognized bolt family.
Closed narrowly:

- After `script_target`, the resolved target's package root must be a **versioned
  package of this exact family** (`name == family`). Another family's package, an
  unrelated program, an unversioned root — i.e. a further launcher hop — and a
  non-package root are all `UnsupportedLauncher`, never a stale claim.
- Target shape is now exactly `<root>/bin/<program>` (`literal_store_entrypoint`,
  replacing `literal_store_path`): a bare store file, a deeper path such as
  `lib/pi-bolt/pi`, and any `.`/`..` component are refused. No parser, no
  execution, no ELF access added.
- Real current transition wrapper (`…-pi-bolt/bin/pi-bolt` → `…-pi-bolt-0.7.1/bin/pi-bolt`)
  and same-versioned-root entrypoints still resolve; a different build of the
  same family is still accepted (stale by design) and now covered positively.
- `tests/stale_binary.rs` gained the refusal cases (unrelated versioned package,
  unversioned/further-hop root, non-`bin` target, `.`/`..` components, bare store
  file) and the same-family different-build positive case.
- `openspec/changes/variant-aware-binary-freshness/design.md` wording corrected to
  state the exact-family versioned-target rule and the `<root>/bin/<program>`
  shape.

## Deferred

- Group 3: Processes-page rendering of running package identity, installed
  counterpart identity and the unknown reason, plus its render regressions.
- Group 4: README documentation of family/package-root semantics, the alias
  rule and the corrected profile path; `verification.md`; live read-only matrix.

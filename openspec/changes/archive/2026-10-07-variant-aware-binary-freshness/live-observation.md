# Live read-only observation: resolver matrix

Task 4.3 of `variant-aware-binary-freshness`. Read-only; no production source
changed. This file is input to the lead's final verification.

## Provenance

- Date (UTC): 2026-10-07, observation run at 10:53:08Z.
- Host: `legion`, Linux 7.2.9.
- Resolver under test: the repository library at its current working tree,
  invoked through its **public seam only** —
  `agent_radar::procfs::identity(running, deleted, None, &mut BinaryIndex::new())`.
  Nothing was reimplemented; `BinaryIndex::new()` reads Radar's own `PATH` and
  `identity` performs the family/package-root comparison.
- Probe: a temporary Cargo project at `/tmp/radar-live-probe` (outside the
  repository) with a single `agent-radar = { path = ... }` dependency; deleted
  after the run. No application source or test in the repository was touched.
- Pane association: taken from runtime evidence, not guessed —
  `herdr pane process-info --pane <id>` for the pane whose foreground command is
  the process. Only the foreground PID and executable link were used; the runtime command returns argv alongside PID evidence, but this probe did not consume or record those arguments or prompts. No process environment was read.
- `PATH` resolution used by the probe (the profile entries that matter; the full
  `PATH` also carried dev-shell entries that contain no bolt command):
  - `pi-bolt` → `/etc/profiles/per-user/saurabhj/bin/pi-bolt`
    → `/nix/store/84rqbfd1rqhrvlm0spvmanma71gvad07-pi-bolt/bin/pi-bolt`
  - `pi-bolt-child` → `/etc/profiles/per-user/saurabhj/bin/pi-bolt-child`
    → `/nix/store/cm3s1620p5z2rsp17z5x5fxjnn76p2yn-pi-bolt-child/bin/pi-bolt-child`

## Launcher shape in effect (transitional cross-root branch)

The current profile's `PATH` entrypoints are the cross-root launcher scripts, so
the transitional branch is the one that runs today. Neither root carries a
version, so the same-root branch does not apply and the declared target does:

- lead launcher `/nix/store/84rqbfd1rqhrvlm0spvmanma71gvad07-pi-bolt/bin/pi-bolt`
  ends with
  `exec /nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1/bin/pi-bolt "${flags[@]}" "$@"`
- child launcher `/nix/store/cm3s1620p5z2rsp17z5x5fxjnn76p2yn-pi-bolt-child/bin/pi-bolt-child`
  ends with
  `exec /nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1/bin/pi-bolt "${args[@]}"`

Both are the accepted shape: one strictly literal
`/nix/store/<root>/bin/<program>` target, a versioned package of the exact same
family. The resolver reads the target and executes nothing.

## Matrix

| Row | pane | pid | running executable | running package root | installed (payload) root | freshness | unknown |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Lead (`pi-bolt`) | `w18:pQ` | 69343 | `/nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1/lib/pi-bolt/pi` | `/nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1` | `/nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1` | `Current` | `None` |
| Child (`pi-bolt-child`) | current pane | 395646 | `/nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1/lib/pi-bolt/pi` | `/nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1` | `/nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1` | `Current` | `None` |

Raw resolver output (2026-10-07T10:53:08Z):

```
pid 69343
  executable: /nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1/lib/pi-bolt/pi
  deleted: false
  freshness: Current
  running: Some("/nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1")
  installed: Some("/nix/store/ry6h18h4m3f66rb8cjhxq7i64i1a3i3x-pi-bolt-0.7.1")
  unknown: None
pid 395646
  executable: /nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1/lib/pi-bolt/pi
  deleted: false
  freshness: Current
  running: Some("/nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1")
  installed: Some("/nix/store/lly2cvdp1rlv2gsc3plz2jdz6b3v9702-pi-bolt-child-0.7.1")
  unknown: None
```

Each family's counterpart was selected by its own exact name (`pi-bolt` for the
lead row, `pi-bolt-child` for the child row), never by the runtime-reported `pi`
name, and never across families.

## Unproven live cases (not manufactured)

- **Stale / same-version rebuild.** Both sampled bolt processes ran the same package roots their launchers target. This two-row matrix did not observe a stale or same-version different-root process; it does not establish that none existed elsewhere on the host. The rule remains covered by the
  unit and integration tests (`a_lead_running_another_build_of_the_same_version_is_stale`,
  plus the accepted different-build case in `only_a_launcher_shape_that_says_where_it_goes_is_read`);
  no synthetic row was produced here.
- **Unknown verdict.** Neither sampled process lacked a counterpart, so this matrix observed no unknown verdict. The refusal reasons remain
  covered by `tests/stale_binary.rs`.
- **Same-root derivation entrypoints.** The profile in effect still installs
  cross-root launchers, so the primary same-root shape was not exercised live;
  it is covered by `a_versioned_packages_own_entrypoint_names_its_own_payload_root`.

## Cleanup

The temporary probe project `/tmp/radar-live-probe` and its built binary were
removed after the run. Only documentation was changed: `README.md` and this observation artifact (with design wording corrections permitted by the brief). No production source or test edits.

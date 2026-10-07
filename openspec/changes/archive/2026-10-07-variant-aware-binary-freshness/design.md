# Design

## Context

The shipped rule compares a running executable with the program `PATH` resolves
for the program name the runtime reports, and requires the two files to share a
file name before comparing installations (`src/procfs.rs:759 classify`). For
pi-bolt that name is never shared:

| Variant | Runtime-reported name | Running executable | `PATH` resolution |
| --- | --- | --- | --- |
| lead | `pi` | `…-pi-bolt-0.7.1/lib/pi-bolt/pi` | `/etc/profiles/per-user/saurabhj/bin/pi-bolt` → `zyhpx…-pi-bolt/bin/pi-bolt` (script) → `exec …-pi-bolt-0.7.1/bin/pi-bolt` → payload `…/lib/pi-bolt/pi` |
| child | `pi` | `…-pi-bolt-child-0.7.1/lib/pi-bolt/pi` | `/etc/profiles/per-user/saurabhj/bin/pi-bolt-child` → `nawk53…-pi-bolt-child/bin/pi-bolt-child` (script) → `exec …-pi-bolt-child-0.7.1/bin/pi-bolt` → payload `…/lib/pi-bolt/pi` |

Every bolt pane therefore falls to `BinaryFreshness::Unknown`, `binary_line`
returns `None`, and the Processes page says nothing — while the live sessions do
run different builds from the ones `PATH` now points at.

Evidence gathered read-only for this design:

- The store derivation name is the only family evidence. All four builds — both
  lead roots and both child roots — carry the byte-identical stamp
  `Pi-Bolt 0.7.1 (Pi 1.0.4), linux-x64-jit, JIT on, built 2026-10-07`
  (`lib/pi-bolt/pi-bolt.txt`), and their `lib/pi-bolt/package.json` files are one
  document (sha256 `248decde30021109…`) naming the upstream
  `@earendil-works/pi-coding-agent 1.0.4`. The version stamp and manifest are
  therefore unusable for telling family apart, and equally unusable for telling a
  rebuild from the build `PATH` points at.
- The two live lead roots and the two live child roots are genuinely different
  builds of the same version (different `sha256` for `lib/pi-bolt/pi` and for the
  wrapper), consistent with `stale-binary`'s roots-differ rule.
- `pi` in `PATH` is an alias: a symlink at
  `/etc/profiles/per-user/saurabhj/bin/pi` to the lead launcher's own file. Older
  notes that mention `~/.nix-profile` predate the current profile layout; nothing
  here may pin a profile path or a version.
- `herdr pane process-info` exposes only `argv`, `cmdline`, `name`, `pid` and
  `cwd` for the foreground process. There is no reported pane invocation command,
  so the invocation is unavailable to Radar by any means inside this change's
  constraints.
- The owner confirmed no declarative launch-target metadata exists today, and
  queued `nix-support/pi-bolt-identity.json` (family, version, piVersion,
  compiledPlugins, herdsmanRevision) for the dotfiles build. That file lives at
  the payload root, so on its own it does **not** bridge the outer launcher to
  the payload; this change consumes no metadata file at all.
- Upstream packaging is expected to drop the profile script layer and have each
  derivation provide `pi`, `pi-bolt` and `pi-bolt-child` in its own `bin`,
  running its own `lib/pi-bolt/pi`. The comparison is designed around that
  same-root entrypoint shape, which needs no parsing; today's cross-root profile
  script is the transition case (decision 6).

## Goals / Non-Goals

**Goals:** mark a pi-bolt lead or child session stale when it runs a package the
current `PATH` no longer selects, and make the executable identity inspectable —
including when it is current or unknown — without inventing an invocation.

**Non-Goals:** a general package-family mechanism or heuristic discovery for
other programs; reading the environment; executing any part of the launcher
chain; reading compiled content; task metrics, lifecycle actions, the
managed-worker launch fingerprint; any new configuration key; pinning a profile
path or a version.

## Decisions

### 1. Family comes from the store derivation name, exactly

`<hash>-<packageName>-<version>` is separated into hash, package name and
numeric version. Only the exact package names `pi-bolt` and `pi-bolt-child` are
handled; anything else keeps the existing rule untouched (decision 5). The
runtime-reported name is never a lookup key, and `pi` in particular selects
nothing.

Rejected: a version-stamp or manifest read (identical across families and across
the two builds); prefix or substring acceptance (would let `pi-bolt` claim
`pi-bolt-child`); deriving a family generically for every package (goes beyond
the observed need and risks claims about programs nobody asked about).

### 2. Compare package roots, and let a rebuild stay stale

The running executable's package root is compared with the payload package root
of the installed counterpart — the root the counterpart's launcher actually runs
— not with the launcher's own root. Differing roots are stale, including for the
same version.

Rejected: a version-string or manifest equality exception (two builds of
`0.7.1` with different compiled plugin sets are different binaries; the mark
answers "is this session running what `PATH` points at", not "is the version
number the same"), and comparing the launcher root instead of the payload root
(the launcher is a profile-level script whose root moves independently of the
payload).

### 3. A deleted executable is stale first

The kernel's `(deleted)` marker is a fact about the running file and needs no
counterpart. This also makes the spec's existing "replaced in place" scenario
reachable when no counterpart resolves.

### 4. Resolution goes through Radar's own `PATH`, canonically

The counterpart is found by searching Radar's own `PATH` for the exact family
name, following symlinks (as the existing `BinaryIndex` already does). No profile
path, store path or version is hard-coded, and the `pi` alias is display material
only.

### 5. Existing behaviour for everything else is preserved

Programs outside the handled families keep the current matching-file-name
comparison, including non-Nix, non-store and interpreter cases. The bolt path is
an addition beside it, not a replacement of the general comparison — a generic
"discover the family" rule is explicitly rejected.

### 6. Two counterpart shapes: same-root entrypoints first, a bounded legacy shim second

**Primary — same-root entrypoints, no parsing.** Upstream packaging is expected to
drop the profile-level bash layer and let each derivation provide `pi`, `pi-bolt`
and `pi-bolt-child` in its own `bin`, running its own `lib/pi-bolt/pi`. The
entrypoint's package root is then the payload root, so `PATH` resolution alone
identifies the payload and the existing installation rule (a wrapper and the file
it runs inside one store root agree) already expresses it. Nothing is read from
the entrypoint's contents, so no packaging detail is depended on. This is the
shape the comparison is designed around, and today's `…-pi-bolt-0.7.1/bin/pi-bolt`
(`makeBinaryWrapper`, target `<same root>/lib/pi-bolt/pi`) is already that shape.
The root must be a versioned package of the same family for this: an unversioned
launcher package — the profile's own `…-pi-bolt` root, which holds no payload — is
not a payload root merely because the entrypoint sits inside it, and it goes to
the legacy branch below.

**Legacy — the profile-level script shim.** Today's `PATH` entrypoints are
profile scripts whose root differs from the payload's (`zyhpx…-pi-bolt` →
`…-pi-bolt-0.7.1/bin/pi-bolt` → payload). Only for that transition shape is a
bounded resolver accepted: a script with exactly one strictly literal line of the
form `exec /nix/store/…/bin/<program> [args…]`, whose target's package root is a
versioned package of this exact family. Every other shape is unknown — no
target, several `exec` lines, a computed, quoted or conditional target, a loop or
variable indirection, a non-store target, a target that is not this family's
versioned package (another family's package, an unrelated program, an
unversioned root, or a file inside a package rather than its `bin` entrypoint), a
non-script file, a further hop, or a cycle. The shim never becomes a contract:
nothing checks a fixed script text, nothing is executed, and no compiled content
is read. Removing the shim in packaging retires this branch without a second
change here.

Rejected: taking the first `exec` line unconditionally or scanning for any
`/nix/store/` reference (both guess at what a script does); shelling out to the
entrypoint or to `nix-store -q --references` (execution, and a Nix CLI dependency
for a display label); reading the `makeBinaryWrapper` string table (compiled
content, and it only ever names the same root); reading
`PI_HERDSMAN_CHILD_COMMAND` (the pane's environment, excluded).

**Ceiling and retirement path.** The legacy branch is the whole compatibility
surface, and its unsupported shapes fail to unknown rather than to a guess; the
list of handled families is one line to extend. It retires when packaging ships
same-root entrypoints, which removes the cross-root case itself. Owner-declared
metadata would be a further shortcut, but this change has no metadata consumer:
the queued `nix-support/pi-bolt-identity.json` sits at the payload root and cannot
bridge an outer launcher to its payload on its own, and a payload stamp or
manifest is separate build-and-plugin introspection for a later change — and in
any case the two builds of `0.7.1` share both stamp and manifest, so neither
identifies a build.

### 7. Processes shows the identity, including when it is current or unknown

The page shows the observed command name, the running executable's full path, the
running package identity, the installed counterpart's identity when resolved, and
a typed reason when unknown — for agent rows and ordinary panes with current
foreground evidence. Sanitized like other external text. The observed name and
its arguments are the runtime's report of a running process, never an invocation,
an alias or a launcher, and prompt arguments are never drawn.

Rejected: showing the store path as part of the command line (it says where a
program lives, not what it is doing — the existing `command_line()` rule), and
suppressing the fields when the verdict is current (an identity is a fact worth
reading, not only a warning).

### 8. The page stays short and the verbose facts sit behind one block

A first attempt drew each fact as its own prose line, and the reader's verdict
was that the panel had become a wall: the boot id, the descendant caveat and the
meaning of every state were always on screen, and the executable repeated the
store prefix of the package named directly above it.

What the page draws now is short and deduplicated: the observed name, the
package as name-version (with a short hash only when two builds of one version
must be told apart), the executable relative to that package, and a verdict only
when there is one. Current draws nothing; stale is the mark with `(stale)` and
the installed target; a different build says so; an unknown comparison keeps a
short reason word.

Rejected: keeping the current verdict as a fact (the absence of the mark already
says it, and it cost the line `cpu` and `rss` share); the whole sentence as the
unknown reason on the line (the word finds it, the block explains it).

The kernel state is drawn as one short enumerable name — running, sleeping, disk
wait, stopped, traced, zombie, dead, idle, unknown — never a sentence, never
animated, and never in the working or failed ink: the scheduler's state is not a
verdict on the work underneath it, and a process flips between `running` and
`sleeping` constantly.

### 9. The block draws labelled rows, wrapped into their own column

The first block was a list of prose lines whose labels varied with the facts
present, so values did not line up; it repeated the store prefix on every root
and on the executable; it printed a nine-line glossary of kernel states the row
was not in; and a fact too long for the panel wrapped to the start of the
following row, under the label rather than the value.

The block now draws one labelled row per fact, in a column shared by the block's
rows, and wraps a long value into the value column so its continuation reads as
the same fact. The two roots are drawn as the part past the store they share,
which is stated once, and the executable as its place under the running root:
the whole path is three rows that name each piece once. The state row carries the
meaning of the state the row is actually showing, so the glossary is gone.

This needed the panel's content width before the page is built, which the page
previously did not have. `render` computes it from the body width with the same
rule the layout uses, and the block context carries it down; the drawing no
longer has to re-break text it was handed.

Rejected: a per-fact reason column (a reason is not a value, so it stays under
the value it qualifies); keeping a fixed 28-cell pairing threshold in the metric
section (it was chosen because the page was built before it was laid out, which
is no longer the constraint it was; the threshold still pairs at every width the
side-by-side panel is drawn at, so replacing it with the real column is a
cleanup, not a correction); a prose sentence per long fact (what the reader
called a wall); eliding a long path or eliding the middle of a root (an identity
that cannot be read whole is not an identity).

## Risks / Trade-offs

- **Launcher shape drifts** → the comparison goes unknown, so a session can lose
  its mark but can never gain a false one. The packaging change that removes the
  cross-root shim removes the risk with it.
- **An extra read per family per refresh** → the existing resolver caches each
  name for the refresh, so the cost is one `PATH` lookup and at most one script
  read per distinct family, not per pane. Same-root entrypoints cost only the
  lookup.
- **The legacy branch becomes dead code** once packaging ships same-root
  entrypoints → it is named as transitional here, so its deletion is a follow-up
  with no spec change, and its tests stay valid as fixtures of the old shape.
- **A transient wrapper-shell phase** is compared exactly as it is today (by
  matching file name); this change neither adds nor removes a claim there.
- **Most live bolt sessions will start showing stale** once the rule works. That
  is the intended outcome (the sessions do run other builds), and it is the
  visible behaviour the user asked to fix.
- **A third variant family** would go unmarked until added to the list. Its
  symptom is a missing mark, not a wrong one.

## Migration Plan

No persisted state and no data migration. The change alters what is displayed and
what is marked; reverting the commit restores today's behaviour. Tests run
against fixtures, so the rollout carries no live-machine dependency.

## Open Questions

- When upstream packaging ships the same-root entrypoints, this change's legacy
  resolver branch is deleted as a follow-up; nothing else here depends on it.
- Launch-target metadata remains the owner's separate queued item
  (`nix-support/pi-bolt-identity.json`), together with a payload stamp or manifest
  for build-and-plugin introspection. This change consumes neither, and the two
  live `0.7.1` builds share both stamp and manifest, so neither can identify a
  build.
- Whether the handled-family list should also name a third bolt variant now, or
  be extended when one appears.

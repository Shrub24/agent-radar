# Design

## Context

See proposal.md for motivation. Current state that shapes this design:

- `radar launch-bootstrap` already releases a launch after admission and `exec`s the resolved program, so a scope can wrap it without another long-lived process.
- Managed launches already run in a daemon-created tmux pane with direct argv and a recorded cwd/environment overlay.
- Presence/claims (`unified-pi-registration`) bind the Pi session subject; launch correlation binds that session to the admitted launch.
- Physical exit is already a typed tmux observation; scope state must not become a second one.
- The identity model still carries two names for one attempt (`subject_id` for the execution record and `logical_subject_id` for the durable subject) plus an ordinal. Section "Prerequisite" consolidates it, because every decision below names an attempt and would otherwise freeze the duplicate into scope and unit names.

## Goals / Non-Goals

**Goals:** one managed launch path; scope established before the application executes; argv, cwd, environment and the existing pane PTY unchanged; graceful-only stop preserved; recovery by recorded identity.

**Non-Goals:** resource limits, freezing, automatic suspension, task execution, headless/ordinary-terminal support, mux replacement, retroactive scoping, legacy migration.

## Feasibility evidence

Measured on this host (systemd 261.3, cgroup v2 user manager) with disposable units and a throwaway tmux server. Scripts: `/tmp/radar-feasibility.sh`, `/tmp/feas/stage2.sh`, `/tmp/feas/pidchain.sh`.

| Question | Scope mode | Transient service |
| --- | --- | --- |
| Program keeps the launcher's PID | Yes — `systemd-run` execs in place; parent is the caller | No — manager spawns it |
| Pane PTY, TERM, cwd, caller env | Preserved (`/dev/pts/12`, `tmux-256color`, caller cwd) | New env: `cwd=$HOME`, `TERM=dumb`, caller variables absent; `--same-dir` fixes cwd only |
| Records the command's exit status | **No**: `Result=success` for `exit 42` and for a SIGSEGV core dump; unit goes inactive immediately | **Yes**: `ExecMainCode=1`, `ExecMainStatus=7`, `Result=exit-code`, readable after exit without `--collect` |
| Group lifetime | Until every member exits; a lingering helper keeps it active after the launched command exits | Tied to `Type=`/`RemainAfterExit=`, configurable |
| Stop default | SIGTERM, then SIGKILL after `TimeoutStopSec`; zero survivors | Same |
| Graceful-only stop (`SendSIGKILL=no`) | Stop returns 0, workload survives, unit → `failed`/`timeout` | Same |
| Placement | `app.slice` by default; `--slice=X.slice` auto-creates the slice | Same |
| Created from a service (daemon/tmux-like caller) | Works, including with `DBUS_SESSION_BUS_ADDRESS` removed; lands in `app.slice`, **not** inside the caller service's cgroup | n/a || Accounting | `MemoryCurrent`, `CPUUsageNSec`, `TasksCurrent` present | Same |
| argv integrity | Environment expansion **on by default** — a literal `$HOME` argument became a path; `--expand-environment=no` preserves it | Same |
| tmux `pane_dead_status` | `42` for a plain scope-wrapped command | n/a |
| tmux pane PID | Equal to the workload PID for plain argv. A shell redirection or other metacharacter makes the shell fork, and `pane_pid` then stays the shell | n/a |

Two consequences that drive the decisions below:

Measurement caveats: the pane-PID result holds only when the pane command runs with plain argv; an earlier variant launched under `--pipe --wait` from inside a transient service failed and was not diagnosed, so scope creation from a service rests on the two cleaner runs that succeeded. The `pane_dead_status=42` result used a plain `/bin/sh -c 'exit 42'`; a shell command containing a variable reference failed under the pane's `fish` shell with status 127, which is a shell-quoting artifact of the harness, not a scope finding.
1. A scope is a grouping and control object with **no main-process result**. Exit evidence must come from the tmux pane observation that already exists, or later from a service for headless work.
2. Launching a scope from the tmux service or the daemon places the workload **outside** the caller's cgroup, so a `KillMode=control-group` stop of that caller does not reach it. PTY closure still can.

## Prerequisite: one identity per role

The scope work names an attempt everywhere — unit name, slice name, recovery, stop, exit — so it is built after the identity model is consolidated, not on top of the duplicate.

Three roles, three ids, three lifespans:

| Role | Id | Lifespan | Owner |
| --- | --- | --- | --- |
| The agent | `subject_id` | Durable identity of one Pi session, across processes and panes | Daemon |
| One attempt | `claim_id` | One launch or adoption, from pre-mint to verified end | Daemon mints, the process presents |
| One process incarnation | `registration_id` | One OS process | Process writes, daemon verifies |

Today's `subject_id` is the attempt and today's `logical_subject_id` is the agent; the durable identity carries the qualified name. The merge renames them so the agent owns the plain name and the attempt is a claim:

- **A claim is the attempt**, not only an attachment request. It already carries the launch facts that no client may write: the expected birth identity, the captured recipe, `signalled_at`, `exited_at`, pane evidence and the successor pointer. After the merge it also carries them for managed launches, because the daemon pre-mints it.
- **A managed launch pre-mints its claim before dispatch**, embeds the id in the launch environment, and the child presents it. Binding then requires the captured birth identity, so a leaked id cannot bind a different process. The per-edge spawn token retires: for a managed launch the edge and its claim are one to one.
- **Attachment moves off the attempt.** Presence release clears the subject's current claim; it does not end the attempt record and does not compete with exit on one state field.
- **`execution_generation` is dropped.** It existed to fence and to name the latest attempt. A claim id is an exact fence, and the subject's `current_claim` names the latest attempt without scanning for a maximum.
- **The subject becomes a record.** Identity, managed metadata, `parent_subject_id` and `current_claim` live on it once, instead of being copied forward onto each successor.

**Races resolve at pre-mint, which is what makes restart continuous.** Pre-minting is the exclusive act per subject: the first accepted pre-mint wins, and a competing launch or restart is refused or left pending. Because a restart pre-mints its successor before it dispatches the stop, the successor is already the subject's current claim at the moment the old process is signalled — the continuity gap closes there, not when the replacement eventually starts.

If the stop fails before replacement dispatch, Radar revokes that undispatched reservation, records its cancellation, and restores the exact incumbent claim under the transition lock. Leaving a processless pending claim current would strand the still-running agent. A successor that may have dispatched is never rolled back this way. Scoped reads and handoffs require a granted decision whose claim still matches the subject pointer.

pi-radar persists whether a report is the initial load: only that load spends the pre-minted launch claim. A later reload uses a fresh claim even if pending releases have already drained; deciding from the current release queue would accidentally reuse an old credential.

**Migration re-keys nothing.** An existing execution record's UUID becomes its claim id, its `logical_subject_id` is promoted to the subject record on read, and a stored generation is ignored. No new ids are minted, no store is added, and the private record filenames stay as they are.

## Decisions

### 0. Launch parentage is a durable key, read as the caller's own children

A launch's parent is the logical subject that asked for it, recorded on the edge when the launch is admitted, because that is the value `spawn.create` already validates and currently discards. A page of one subject's launches is then a key lookup, not a session correlation: a session moves on `/resume`, is copied by a fork and shared by a duplicate, so a session-keyed read disagrees with handoff addressing, which uses the parent subject.

The read is derived, not parameterised. The daemon resolves the caller's current subject from its presence claim and pages that subject's edges; an explicit parent filter belongs to the operator surface, where naming another subject is a legitimate act. A caller-facing parameter that names a parent would let any connected client enumerate another agent's children.

**Rejected:** keeping `spawn.list.v1`'s `parent_session_id`. It is correct only while a session and a subject coincide, and it carries caller-supplied identity into a read that should derive it.

### 1. Scopes for interactive managed execution; services deferred to headless work

Scopes win for interactive Pi: they preserve the pane PTY, environment and cwd, keep the PID chain, work from the daemon's own environment, and give group accounting. Services would force a manager-spawned environment and a new PTY for interactive use. Where a durable main-process result is genuinely needed and no terminal is involved, a transient service is the right primitive — later, for headless jobs, not here.

**Rejected:** a custom Radar supervisor that waits for the main process. It reproduces what systemd already does, adds a long-lived process per launch, and creates a second result-delivery boundary.

### 2. Run the existing bootstrap inside the scope

`radar run` resolves placement and the private recipe, records the launch, then execs `systemd-run --user --scope --expand-environment=no --unit=… --slice=… -- radar launch-bootstrap … -- pi …`. The scope is created before the bootstrap releases, so admission failure leaves an empty scope that disappears, and the application never executes unscoped. No new resident supervisor.

Required flags, each driven by a measured constraint:

- `--expand-environment=no` — the default expansion corrupts literal `$VAR` argv.
- `--unit` / `--slice` — deterministic identity for recovery; `--slice` creates the slice implicitly.
- `-p SendSIGKILL=no` and an explicit `TimeoutStopSec` — see decision 4.
- `--quiet` — never let systemd write launch chatter into the managed pane.

Do not wrap the pane command in a shell or a redirection: the shell then forks and the pane PID stops being the workload PID, which breaks pane-occupant verification.

### 3. Execution evidence stays with the mechanisms that actually observe it

| Fact | Source |
| --- | --- |
| Scope exists and is populated | systemd unit state / cgroup |
| Application exited with code/signal | existing tmux typed physical-exit observation (`pane_dead_status`, read from a normal format poll over `remain-on-exit` armed before launch — not control mode), which reported 42 through a scope wrapper |
| Application is ready / assignment settled | neither — unchanged domain facts |

Scope `Result` is never a command outcome: it reads `success` for nonzero exits and crashes. `ActiveState=inactive` means the group emptied, not that the command succeeded. Recovery must also not treat a missing transient unit as proof of a clean exit: without `--collect`, a completed unit is readable but is not durable history; `--collect` removes it immediately.

### 4. Graceful-only stop, explicitly

Default scope stop sends SIGTERM and then SIGKILL after `TimeoutStopSec`. The measured graceful-only configuration (`SendSIGKILL=no`) leaves the workload alive and puts the unit in `failed`/`timeout`. So:

- Managed scopes set `SendSIGKILL=no`; Radar never escalates on its own.
- A stop that leaves processes running is recorded as an unresolved stop with survivors named, not as success or failure of the workload.
- `failed`/`timeout` unit state is tolerated and cleared after the workload ends; it is not an execution outcome.
- Group stop targets the main scope only. Independently scoped tasks and subagents are untouched. A managed execution stops through its scope; the existing identity-fenced signal path stays as it is for observed sessions and receives no new work. The unit is stopped by name: names derive from a claim id that is never reused, and recovery still verifies a member's birth identity first, which covers a stale process rather than a hostile one.

### 5. Physical grouping without logical ownership

```text
radar.slice
  radar-<execution>.slice
    radar-<execution>-main.scope
```

The daemon and tmux server stay outside `radar.slice`. The execution slice is named from the launch's pre-minted claim id, not from the session subject, argv, prompt, cwd or environment, and the name is fixed at launch: it exists before exec, when no registration does. A later claim in the same process (a reload or `/resume` presents a fresh claim) does not rename or migrate the cgroup; it reaches the scope through the process identity the launch recorded. Unit names are visible to the user manager, journalled and `systemctl`-listable.

### 6. Recovery binds by recorded identity, never by name alone

A recovered execution requires the recorded unit name **and** a verified process birth identity inside the recorded cgroup. A reused unit name, a stale cgroup path or scope state by itself SHALL NOT authorise a signal, a termination or an outcome. Where the process cannot be verified, the association is unresolved.

### 7. Managed-first policy, no adoption parity

`radar run`, `L`, restart and Herdsman spawn converge on the same scoped launch after placement is chosen, and the managed path accepts no compromise to serve any other. An externally started Pi is observed: it keeps everything it has today (durable session identity, children, inbox, the controls that need no scope) and is marked as not first-class. It gets no new capability and no second implementation of a managed one, and nothing is removed from it merely because it is unmanaged. The expected route to managed is a `pi` shim that prefixes `radar run`, so observed sessions are a narrow case.

### 8. Reset instead of migration

No conversion of existing subjects or executions. Rollout uses an explicit reset whose scope is agreed separately: it must never be implemented as "stop the tmux server" and must state what durable history, handoffs and inboxes it discards.

### 9. The execution slice is established now, and the association lives on the claim

Create the per-execution slice from the start rather than adding it when the first independent task scope appears: moving later would rename every live group. Record the unit and slice names plus the verified member identity on the launch claim that already records that attempt; later claims of the same process resolve the scope by that process identity. A separate store would be a second place to keep one fact.

## Risks / Trade-offs

- **Scope state mistaken for an outcome** → only tmux's typed exit observation reports exit; unit `Result`/`ActiveState` are presentation facts.
- **`failed` after a graceful stop looks like breakage** → documented as an expected survivor state; repeat stop and clear are normal.
- **Unit name collision on retry** → derive the name from the launch's claim id so a lost reply cannot create a second attempt under a fresh name.
- **Environment expansion corrupting argv** → `--expand-environment=no` plus a test with a literal `$VAR` argument.
- **Pane PID no longer the workload** → direct argv only; refuse a recipe that would need shell metacharacters.
- **Radar becomes a startup dependency** → scope or admission failure refuses visibly before exec; no silent unscoped launch.
- **Secret leakage into unit metadata** → names and descriptions derive only from the claim id.
- **Scope survival after daemon stop** → intended; PTY closure can still end an interactive program, so neither fact implies the other.
- **Accounting misread as causal cost** → expose charged memory and CPU as measurements, not attribution.

## Migration Plan

Land the identity consolidation first, then ship the daemon-side scope launch and recovery, then `radar run`, the `L` cutover and the `pi` shim in the same release so the whole path is testable together, with pi-radar updated alongside. Reset rather than convert. Rollback removes scope creation: managed launches then refuse rather than silently falling back to an unscoped launch, so a half-rolled-back fleet is visible rather than misreported.

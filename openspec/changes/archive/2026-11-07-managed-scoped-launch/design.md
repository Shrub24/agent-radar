# Design

## Context

See proposal.md for motivation. Current state that shapes this design:

- `radar launch-bootstrap` already releases a launch after admission and `exec`s the resolved program, so a scope can wrap it without another long-lived process.
- Managed launches already run in a daemon-created tmux pane with direct argv and a recorded cwd/environment overlay.
- Presence/claims (`unified-pi-registration`) bind the Pi session subject; launch correlation binds that session to the admitted launch.
- Physical exit is already a typed tmux observation; scope state must not become a second one.
- Tasks 0.1–0.8 shipped subject/claim separation and removed generation fencing. Task 0.9 separates the process registration from its session claims before physical groups are added.

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
| Created from a service (daemon/tmux-like caller) | Works, including with `DBUS_SESSION_BUS_ADDRESS` removed; lands in `app.slice`, **not** inside the caller service's cgroup | n/a |
| Accounting | `MemoryCurrent`, `CPUUsageNSec`, `TasksCurrent` present | Same |
| argv integrity | Environment expansion **on by default** — a literal `$HOME` argument became a path; `--expand-environment=no` preserves it | Same |
| tmux `pane_dead_status` | `42` for a plain scope-wrapped command | n/a |
| tmux pane PID | Equal to the workload PID for plain argv. A shell redirection or other metacharacter makes the shell fork, and `pane_pid` then stays the shell | n/a |

Two consequences that drive the decisions below:

Measurement caveats: the pane-PID result holds only when the pane command runs with plain argv; an earlier variant launched under `--pipe --wait` from inside a transient service failed and was not diagnosed, so scope creation from a service rests on the two cleaner runs that succeeded. The `pane_dead_status=42` result used a plain `/bin/sh -c 'exit 42'`; a shell command containing a variable reference failed under the pane's `fish` shell with status 127, which is a shell-quoting artifact of the harness, not a scope finding.
1. A scope is a grouping and control object with **no main-process result**. Exit evidence must come from the tmux pane observation that already exists, or later from a service for headless work.
2. Launching a scope from the tmux service or the daemon places the workload **outside** the caller's cgroup, so a `KillMode=control-group` stop of that caller does not reach it. PTY closure still can.

## Prerequisite: one identity per role

The scope names a process, not a session attachment. Registration identity must be allocated before execution so scopes, recovery and stop do not depend on which session the process currently has open.

Three roles, three ids, three lifespans:

| Role | Id | Lifespan | Owner |
| --- | --- | --- | --- |
| The agent | `subject_id` | Durable identity of one Pi session, across processes and panes | Daemon |
| One attachment | `claim_id` | The subject's proof of attachment to a process and session, from reservation to release | Daemon reserves a launch's claim; a process presents its own otherwise |
| One process incarnation | `registration_id` | One OS process, launch to exit | Daemon pre-mints it at launch or allocates it on first recognition; the process reports it thereafter |

The first consolidation (tasks 0.1–0.8) gave the agent the plain `subject_id` and replaced execution generations with exact claims. Task 0.9 separates process lifetime from attachment lifetime:

- **The registration is the process identity, entirely.** A managed launch pre-mints the registration before dispatch and puts it in the launch environment. The launch facts no client may write (expected birth identity, captured recipe, `signalled_at`, `exited_at`, pane evidence, scope and unit) belong to the process, so they are recorded against the registration, and unit and slice names derive from it. Binding requires the captured birth identity, so a leaked id cannot bind a different process. The per-edge spawn token retires.
- **The claim is the subject's attachment**, identified by `(claim_id, subject_id, registration_id)`. A launch reserves one for its subject at the same time: standing `reserved`, meaning Radar has decided this registration may attach to this subject and the process has not received it yet. The claim id is never given to the process. At the process's first report Radar verifies the registration's birth identity, runs the duplicate check for the reported session, and replaces the reserved claim with the granted one under the `claim_id` the process presented. Reloads and `/resume` take the ordinary path with a fresh claim, so pi-radar needs no first-load rule and no claim in the environment.
- **`reserved` is a distinct standing.** `pending` already means a process's report is still being verified and the client retries it; `reserved` is waited on by Radar. It conveys no attachment authority: handoffs, derived reads and controls that need a granted claim ignore it. It is visible and controllable, so the TUI shows a starting row and `D` on it can cancel the launch.
- **Attachment moves off the attempt.** Presence release clears the subject's current claim; it does not end the process or its registration.
- **`execution_generation` is dropped.** It existed to fence and to name the latest attempt. A claim id is an exact attachment fence, and the subject's `current_claim` names its attachment without scanning for a maximum.
- **The subject becomes a record.** Identity, managed metadata, `parent_subject_id` and `current_claim` live on it once, instead of being copied forward onto each successor. `last_claim` retains the exact attachment provenance for a detached subject's history; it is never authority and never authorises a control.

Process evidence remains in the existing durable event stream. Its disposable projection stores physical facts once per registration and keeps attachment provenance separately; reads compose a claim view over that process. Private recipes are written once under the registration id. A fresh attachment cannot overwrite captured birth, signal, exit or successor evidence. A command without an agent subject records only its process, without a dummy claim.

A spawn-addressed control may follow a fresh claim in the same process registration, but must refuse if the subject has moved to a replacement process. The caller must still name the exact current claim.

**Races resolve at reservation, which is what makes restart continuous.** Reserving a launch's claim is the exclusive act per subject: the first accepted reservation wins, and a competing launch or restart is refused or left pending. Because a restart reserves its successor (a new registration and claim) before it dispatches the stop, the successor is already the subject's current claim at the moment the old process is signalled — the continuity gap closes there, not when the replacement eventually starts.

If the stop fails before replacement dispatch, Radar revokes that undispatched reservation, records its cancellation, and restores the exact incumbent claim under the transition lock. Leaving a processless reserved claim current would strand the still-running agent. A successor that may have dispatched is never rolled back this way. Scoped reads and handoffs require a granted decision whose claim still matches the subject pointer.

**Rollout.** Tasks 0.1–0.8 preserved old record keys. The remaining process-record and scope work uses the explicit reset in decision 8 rather than building another legacy migration. The destructive scope of that reset must be agreed before deployment.

## Decisions

### 0. Launch parentage is a durable key, read as the caller's own children

A launch's parent is the logical subject that asked for it, recorded on the edge when the launch is admitted, because that is the value `spawn.create` already validates and currently discards. A page of one subject's launches is then a key lookup, not a session correlation: a session moves on `/resume`, is copied by a fork and shared by a duplicate, so a session-keyed read disagrees with handoff addressing, which uses the parent subject.

The read is derived, not parameterised. The daemon resolves the caller's current subject from its presence claim and pages that subject's edges; an explicit parent filter belongs to the operator surface, where naming another subject is a legitimate act. A caller-facing parameter that names a parent would let any connected client enumerate another agent's children.

**Rejected:** keeping `spawn.list.v1`'s `parent_session_id`. It is correct only while a session and a subject coincide, and it carries caller-supplied identity into a read that should derive it.

### 1. Scopes for interactive managed execution; services deferred to headless work

Scopes win for interactive Pi: they preserve the pane PTY, environment and cwd, keep the PID chain, work from the daemon's own environment, and give group accounting. Services would force a manager-spawned environment and a new PTY for interactive use. Where a durable main-process result is genuinely needed and no terminal is involved, a transient service is the right primitive — later, for headless jobs, not here.

**Rejected:** a custom Radar supervisor that waits for the main process. It reproduces what systemd already does, adds a long-lived process per launch, and creates a second result-delivery boundary.

### 2. Enter the scope without a release gate

`radar run` resolves placement and the private recipe, records the launch, then execs `systemd-run --user --scope --expand-environment=no --unit=… --slice=… -- PROGRAM …` directly. The scope is created before the program runs, so admission failure leaves an empty scope that disappears and the application never executes unscoped. No new resident supervisor.

This path has no `launch-bootstrap` release gate, unlike `L`, restart and spawn, and one cannot be added without inventing a capability: `spawn.bootstrap` releases a subject-shaped launch by `subject_id` and `correlation`, a managed command has no subject by design, and an admitted agent has none until it binds. A gate would only guard a cancel that cannot be issued, because `cancel` and `stop` resolve their target through a subject or a spawn edge and an admitted process has neither. The fail-closed properties do not rest on it: the pane must be the caller's verified descendant, the birth identity must verify, and the scope exists before the program runs, so a cancel landing in between leaves a recorded, scoped process rather than a lost one.

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
- Group stop targets the main scope only. Independently scoped tasks and subagents are untouched. A managed execution stops through its scope; the existing identity-fenced signal path stays as it is for observed sessions and receives no new work. The unit is stopped by name: names derive from a registration id that is never reused, and recovery still verifies a member's birth identity first, which covers a stale process rather than a hostile one.

### 5. Physical grouping without logical ownership

```text
radar.slice
  radar-<execution>.slice
    radar-<execution>-main.scope
```

The daemon and tmux server stay outside `radar.slice`. The execution slice is named from the launch's registration id, not from the session subject, a claim, argv, prompt, cwd or environment. The registration lives as long as the process, so a reload or `/resume` (which only replaces the claim) neither renames nor migrates the cgroup. Unit names are visible to the user manager, journalled and `systemctl`-listable.

### 6. Recovery binds by recorded identity, never by name alone

A recovered execution requires the recorded unit name **and** a verified process birth identity inside the recorded cgroup. A reused unit name, a stale cgroup path or scope state by itself SHALL NOT authorise a signal, a termination or an outcome. Where the process cannot be verified, the association is unresolved.

### 7. Managed-first policy, no adoption parity

`radar run`, `L`, restart and Herdsman spawn converge on the same scoped launch after placement is chosen, and the managed path accepts no compromise to serve any other. An externally started Pi is observed: it keeps everything it has today (durable session identity, children, inbox, the controls that need no scope) and is marked as not first-class. It gets no new capability and no second implementation of a managed one, and nothing is removed from it merely because it is unmanaged. The expected route to managed is a `pi` shim that prefixes `radar run`, so observed sessions are a narrow case.

### 8. Reset instead of migration

No conversion of existing process records or executions. Rollout uses `radar reset`: by default it clears transient process/claim/history state while retaining durable subject IDs, parent relationships and handoffs/inboxes, then restarts the Radar-served tmux server and daemon. `radar reset --hard` also forgets durable subjects and communications. The reset is specified in `openspec/changes/radar-reset/`.

### 9. Establish the process slice now; its association belongs to the registration

Create the per-execution slice from the start rather than adding it when the first independent task scope appears: moving later would rename every live group. Record the unit and slice names plus the verified member identity on the registration that already records that process. A separate store would be a second place to keep one fact.

The slice is transient and its lifetime ends with the group. A slice is stopped, and its scope `reset-failed`, only when its cgroup is empty as read from `cgroup.procs`; emptiness is never inferred from a recorded exit. The two differ: a main process can be gone while a descendant keeps the group alive, and a recorded exit then makes the workload look finished while stopping the slice would kill that descendant. Cleanliness follows emptiness, so a stop of a scoped process whose main process is absent must still go through the scope stop and the survivor check rather than settling on the exit alone.

Empty slices are reconciled rather than cleaned only at exit. An exit is recorded only when something observes it: `radar run` installs no pane-death wake, and a command carries no claim, so the recovery sweep's exit observation never applies to it. Slices older than a state reset have no records at all. A sweep on its own cadence, first run one interval after the daemon starts so startup adds no manager calls, therefore stops every `radar-*.slice` whose cgroup is empty and `reset-failed`s dead `radar-*` scopes, paged like the other passes. An empty slice holds no authority, so the sweep is safe, and `radar reset` runs the same sweep rather than stopping slices one by one.

## Risks / Trade-offs

- **Scope state mistaken for an outcome** → only tmux's typed exit observation reports exit; unit `Result`/`ActiveState` are presentation facts.
- **`failed` after a graceful stop looks like breakage** → documented as an expected survivor state; repeat stop and clear are normal.
- **An empty slice outlives its process** → a slice is transient by design but systemd keeps it once the group empties, so leak growth is bounded by a reconcile sweep rather than by cleanup at exit alone.
- **Unit name collision on retry** → derive the name from the launch's registration id so a lost reply cannot create a second attempt under a fresh name.
- **Environment expansion corrupting argv** → `--expand-environment=no` plus a test with a literal `$VAR` argument.
- **Pane PID no longer the workload** → direct argv only; refuse a recipe that would need shell metacharacters.
- **Radar becomes a startup dependency** → scope or admission failure refuses visibly before exec; no silent unscoped launch.
- **Secret leakage into unit metadata** → names and descriptions derive only from the claim id.
- **Scope survival after daemon stop** → intended; PTY closure can still end an interactive program, so neither fact implies the other.
- **Accounting misread as causal cost** → expose charged memory and CPU as measurements, not attribution.

## Migration Plan

Land the identity consolidation first, then ship the daemon-side scope launch and recovery, then `radar run`, the `L` cutover and the `pi` shim in the same release so the whole path is testable together, with pi-radar updated alongside. Reset rather than convert. Rollback removes scope creation: managed launches then refuse rather than silently falling back to an unscoped launch, so a half-rolled-back fleet is visible rather than misreported.

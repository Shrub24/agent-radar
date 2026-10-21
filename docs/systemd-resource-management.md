# Systemd resource management exploration

## Status

Future direction, not an implementation specification. Preferred shape: optional
systemd user scopes for independently managed workloads, grouped into shallow
per-agent-execution slices. Background tasks may share their coordinator's slice;
subagents do not. No code, resource limits or automatic suspension are authorized
by this document.

The motivation is local workload accounting and control, not another identity or
lifecycle system. This is independent of unified Pi enrollment, daemon-managed
background tasks and tmux control-mode observation; none requires the others to
adopt this design first.

## Responsibility boundaries

| Component | Responsibility |
| --- | --- |
| Radar | Logical session subjects, process attachments, parent/child relationships, durable operations and control decisions |
| tmux | PTYs, terminal presentation and pane/window/workspace placement |
| systemd/cgroups | Physical process grouping, resource accounting and resource controls |
| Herdsman | Assignment/completion semantics and orchestration policy |

A scope is not a supervisor with an authoritative main-process result. It remains
active while any member remains, and ordinary program failure does not itself
make the scope fail. Radar still needs command exit/result accounting and its
existing verified execution bindings. Scope state does not establish readiness,
assignment completion or permission to dispose of an agent.

## Preferred grouping

Illustrative user-manager hierarchy:

```text
radar.slice
  radar-execA.slice
    radar-execA-main.scope
    radar-taskT1.scope
    radar-taskT2.scope
  radar-execB.slice           # B may be A's logical subagent
    radar-execB-main.scope
    radar-taskT3.scope
```

The daemon and tmux server stay outside this workload hierarchy. Unit names are
unique within the manager; slice names encode hierarchy using dashes, with
appropriate escaping for identifiers.

This layer earns its place by allowing:

- Slice-level accounting and a potential budget for one coordinator plus its
  background work.
- Independent task freeze/stop/priority controls.
- Explicit whole-group controls without including delegated subagents.

Ordinary subprocesses inherit their enclosing scope. A task needs a separate
scope only when it needs independent management; do not create a unit per shell
command or subprocess. Independently scoped tasks are siblings of the main
scope, not scopes nested inside it.

A coordinator exiting does not imply that its slice should be stopped. Tasks may
outlive it; retain their physical group while they remain. Distinguish actions on
"agent process" from actions on "agent and tasks". Subagents retain independent
resource groups even if Radar aggregates their costs for display.

### Physical group, not session identity

Bind the group to a physical execution, not permanently to a durable Pi session
subject. A process switching sessions does not migrate its allocations or
existing subprocesses. A replacement execution can have a new group while old
tasks remain in the old one. Logical ownership stays in Radar; do not move groups
whenever a relationship changes.

Current physical consumption and historical attribution to a session are different
questions. Do not label all accumulated counters as the new session's cost after
`/resume`. The exact reporting convention remains open; retrospective session
cost attribution is not a prerequisite for the first experiment.

## Scope creation and adoption

Prefer establishing membership before the workload starts allocating memory or
forking. The kernel explicitly documents that migrating a process does not move
its prior memory charges or already-existing child processes.

Initial candidates are Radar-managed agent launches and independently managed
background jobs. Manually adopted agents remain first-class managed subjects
without a dedicated scope. Late attachment may be useful later, but cannot claim
complete retroactive accounting or containment.

Compare these mechanisms in a feasibility experiment:

| Mechanism | Advantage | Question to settle |
| --- | --- | --- |
| `systemd-run --user --scope` | Existing CLI; scope mode inherits the caller's execution environment | Does its launcher/waiting process preserve Radar's pane/process verification, signals and exit semantics? |
| Transient-unit D-Bus API | Explicit unit properties, attachment and errors | How is execution gated until membership is established, including partial failure? |
| Executable bootstrap that joins a scope then execs | Can preserve PID and the existing PTY across exec | Is it necessary, and can it preserve launch guarantees without another long-lived controller? |

A standalone pre-exec bootstrap is not permission to do D-Bus/allocator work in a
Rust post-fork `pre_exec` callback. Preserve cwd, argv, PATH, shell/direnv setup,
privileges and the existing tmux PTY. Do not add another PTY by default.

Use systemd's management interfaces for its hierarchy. Do not also write its
cgroup tree directly. Managing a delegated subtree is a larger alternative, not
needed for this shape. Controller availability and effective limits remain
subject to user-manager delegation and ancestor constraints.

## Lifecycle and failure implications

Radar decides and records an operation; systemd applies the physical effect;
Radar observes the result. External unit stops and OOM kills are external
evidence, not successful application completion. Scope names or cgroup paths
alone must not authorize effects on a potentially replaced workload.

Daemon restart should leave workloads running and allow their groups to be
rediscovered. Transient units are not durable history and may disappear after
completion, so historical metrics require collection before that evidence goes
away.

The existing `radar-tmux@` service uses `KillMode=control-group`. Moving agents
into separate scopes changes that boundary: stopping the tmux service no longer
reaches them through its cgroup. PTY closure can still affect them. Do not assume
that stopping tmux terminates every managed workload, or that scope membership
makes an interactive program survive tmux termination.

Whether optional scope-creation failure permits an explicitly unscoped launch or
refuses the launch is unresolved. The experiment must make the outcome visible;
never report resource containment that was not established.

## Accounting and controls

Cgroup accounting is a stable grouping mechanism, not perfect causal accounting:

- Memory includes charged cache/kernel memory, not just application heap. Shared
  pages are charged according to kernel ownership rules, not proportionally to
  all users. Do not combine these values with RSS as if they were additive.
- Ancestor totals include descendants. Sum disjoint groups, not a slice plus its
  member scopes.
- Work delegated to a Nix daemon, remote builder or shared external service is
  outside the group unless separately accounted for.
- CPU/IO counters need execution-aware reset handling. Per-workload metric labels
  create cardinality; avoid prompts, environment values and private argv.
- IO weights depend on controller/storage support; configured values alone do
  not prove effective isolation. Same-user scopes are not a security sandbox.

Start with accounting. Explicit freeze/thaw and conservative resource controls
can follow a concrete need. Freezing does not release RAM; reclaim is best-effort;
`MemoryHigh` throttles/reclaims while `MemoryMax` can cause OOM kills.

A frozen coordinator stops heartbeats and acknowledgments. Intentional suspension
must not masquerade as unexpected loss or authorize replacement. Freezing the
main scope leaves independent task scopes running; freezing its slice includes
them. Either can strand dependencies and network operations. Hidden tmux panes
are not evidence of idleness, so focus-driven automatic freezing is deferred.

## Alternatives and limits

- **Existing process observations only:** cheapest, portable fallback; adequate
  for inspection and verified process targeting, weaker for stable descendant
  accounting and group resource controls.
- **Completely flat scopes:** less structure, but no kernel-enforced aggregate
  coordinator-plus-tasks budget. The shallow execution slice supplies that
  concrete benefit.
- **Nested subagent slices:** convenient hierarchical budgets, but couples
  ancestor limits/freezing to delegated workers and complicates independent
  lifetimes or transfers. Do not mirror Radar's ownership tree physically.
- **Transient services:** potentially useful for headless jobs, but introduce
  manager-launched environment and supervision semantics. Do not substitute them
  for interactive scopes without a demonstrated benefit.

Logical delegated totals can be computed from disjoint physical groups. An exact
kernel-enforced budget over an arbitrary changing logical graph is not supplied
by this shallow design. Revisit resource domains only when that capability is
actually required.

## Feasibility evidence still needed

On isolated resources, establish:

1. Interactive fidelity: original PTY, cwd/environment/direnv, argv, resize,
   Ctrl-C/job control and usable exit results.
2. Radar compatibility: registration, verified pane/process identity and restart
   recipe capture survive the launch mechanism.
3. Group boundaries: descendants inherit membership, a separate task can outlive
   its coordinator, and tmux/daemon/unrelated workloads remain unaffected.
4. Accounting: known allocations appear in the expected group; late attachment
   demonstrates its documented limitations rather than seeming retroactive.
5. Recovery and controls: daemon restart preserves workloads; failed creation is
   explicit; manual freeze/thaw affects only the chosen group and does not
   trigger erroneous lifecycle recovery.

Documentation was reviewed and the local host reported cgroup v2 and systemd
261.3. No scope-launch, freeze or resource-control experiment has been run for
this proposal. Controller availability, launch fidelity and metrics-export wiring
remain unverified.

Defer default limits, automatic suspension, reclaim policies, ownership-driven
migration, nested subagent groups, generalized cgroup delegation and a full
Vector/vmagent/VictoriaMetrics/Grafana rollout. First prove that the shallow layer
preserves existing behavior and provides useful accounting.

## Sources

- [systemd.scope](https://www.freedesktop.org/software/systemd/man/latest/systemd.scope.html): scope membership and lifetime, absence of a main-process result.
- [systemd.slice](https://www.freedesktop.org/software/systemd/man/latest/systemd.slice.html): grouping, hierarchy and naming.
- [systemd-run](https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html): scope versus service execution environments.
- [Kernel cgroup v2 documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html): process inheritance/migration, memory ownership, freezing and reclaim.
- [Systemd control-group interfaces](https://systemd.io/CONTROL_GROUP_INTERFACE/): systemd ownership, transient units and delegation; older examples are not a substitute for current API documentation.
- [Systemd D-Bus API](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html): transient units and resource operations.
- [Systemd resource controls](https://www.freedesktop.org/software/systemd/man/latest/systemd.resource-control.html): controller behavior and limits.

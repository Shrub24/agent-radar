# Changelog

Radar is alpha: breaking changes are expected and bump the minor version. The wire revision (`radar --version` prints it) is the compatibility contract with pi-radar; the version is for people.

## Radar 0.10.2

- Library APIs distinguish subject, claim and registration IDs with validated types; their JSON representation and schemas remain strings.
- Event pointers no longer identify an unattached process as a subject. Assignment publication events name the subject they describe, while process publication events name the registration; neither is published under the other identity kind.
- **Breaking:** an agent row is a registration and nothing else. `AgentObservation` no longer carries `name`, `label` or `lineage`, and `HerdsmanFacts` no longer carries `name`, `label` or `managed_metadata`; the observation no longer carries `metadata_agents` and its `containment` map is gone. A backend's own agent records and `pi_herdsman_*` pane tokens no longer exist as a second kind of agent: the registry is the only producer of an agent row.
- **Breaking:** `AgentObservation.registry` is required rather than optional, so a row either names its registration or is a pane row. Containment collapses to "an agent row is a registered agent", and nesting goes through the daemon's spawn edges alone.
- The row's name comes from the registration's label, else the title's session provider, else the source the registration was published under, which is also what selects its vendor mark. An unreported execution or assignment is now stated as `unknown — nothing reported` rather than omitted.
- The observation's shape changed, so the wire revision moves to 17: a daemon or dashboard of revision 16 is refused by name rather than served a document it cannot decode.

## Radar 0.10.1

- One wire revision, no tolerance: the dashboard refuses a daemon of any revision but its own, and pi-radar requires exactly wire 16, so a daemon that outlived an install and one that precedes it are both told to restart instead of served around.
- Removed record and protocol tolerance for earlier builds: `launch_identity` on context records and requests, and the note that a bound child's subject may be missing from an older edge.
- `RuntimeProvider` reports its dispatch certainty directly through `focus` and `close`; the `focus_outcome`/`close_outcome` pair and its "legacy adapters" default are gone.
- A message store holding a file this build does not write is refused by name, rather than described as a retired mailbox to drain.
- The daemon refuses to start over a process recipe or subject identity it cannot read, naming the file, as it already did for a spawn edge. A 0.9.0 recipe used to start a daemon whose `subject.list` and fleet listing then failed on every read. The 0.10.0 upgrade notes now give the cutover, checked against a copy of real 0.9.0 state.
- Closing a place that holds an agent Radar does not manage is refused as "holds an agent Radar does not manage" instead of naming an owner.
- `radar` no longer starts or consults the `radar-control.socket` unit when `RADAR_CONTROL_SOCKET` names another socket: that daemon is not the unit's.

## Radar 0.10.0

- **Breaking:** the shared command `delete` is now `close`, and the daemon decides what closing means for the row you selected: an execution is ended and its pane removed, a pane is closed, a container's whole branch is closed. No view derives a target any more, and no view re-resolves the row it selected to whatever is focused.
- **Breaking (library):** the unused `herdsman-control/v1` owner-file requester is removed: the `control` module (`ControlRequest`, `ControlResult`, `OwnerControl`, `RequestState`, `Outcome`, `NewRequest`, `ControlError`) and `ManagedActions`, `ManagedRequest`, `Update` and `UpdateKind`. Nothing in the product used them; managed close and restart are the daemon's.
- `D` is one gesture with one request. When the row's branch still holds agents it asks first and lists what the answer loses, exactly as before; the answer closes the branch once, signals every execution inside it, and reports what it could not verify instead of assuming it.
- For operations carrying a typed close report, the status line names unverified exits, kept panes, uncertain removals and attempted scope escalation. Program completion remains diagnostic detail on the record; a lost removal reply never claims the pane was kept.
- A reservation that never presented is now withdrawn even when no spawn edge recorded it: a configured `L` and a container reload leave a pending claim that nothing else resolved, so the tree drew a row for an agent that never existed. Withdrawal is keyed on the claim, and a refused create abandons its reservation in the same request.
- **Breaking:** `Closer`, `DirectClose`, `close_unmanaged`, `guard`, `registrations_in` and `registered_within` are gone from the public API. They were the guarded direct-close policy, unreachable from the product since managed close became the daemon's own.
- A close over more than one execution now reports each of them instead of one verdict for the whole place: `closed.executions` lists every execution the close addressed with its own end, and `completion` rides the execution that carries it. Wire revision 16 requires a matching daemon and consumer.
- A branch that is being closed now shows as `closing` on its rows until the close settles, instead of looking unchanged and then disappearing. The observation carries the held places (`closing`); wire revision 15 requires a matching daemon and consumer.
- **Breaking:** a close now reports what it established as typed parts on its record's `closed` field, and wire revision 14 requires a matching daemon and consumer. A read of `effects` prose to find out whether a pane was closed, or whether the process was verified gone, no longer works for `lifecycle.cancel`, `lifecycle.stop` and `topology.delete`; those operations publish `closed` instead and leave `effects` empty.
- Each entry of `closed.executions` has an `end` of `exited`, or an unverified or unsignalled result with the reason, and the `completion` it carries. `closed.resource` says what became of the pane — `removed`, `absent`, `kept` or `unverified` — and is absent for an operation that removes nothing, so a stop no longer has to imply it kept the pane. `closed.completion` carries the program's code and signal when the process supervising it reported, and is absent when it did not: an exit verified without a code is a normal outcome, not a failure.
- A verified exit and a pane removal are separate answers, and neither implies the other. A spontaneous exit keeps its pane on purpose; a removal may follow an exit whose code was never reported.
- Restarting an agent that `L` or a spawn launched starts the replacement in the same pane once its command has exited, instead of splitting a new pane and removing the old one. An agent that `radar run --agent` admitted from your own shell still gets a replacement beside that shell; nothing is typed into it.
- `D` on an agent no longer refuses with "the selected place changed": the daemon fences a named execution by its recorded claim and placement, because its own tmux inventory carries no registry joins to compare an occupant against. `r` on a plain pane now refuses instead of closing it.
- A supervised agent now joins its pane in the fleet listing. The pane's foreground process is the supervisor, so the join goes through the sampled descendant identities.
- The SIGKILL step of a shutdown is one function (`lifecycle::escalate`) called by the graceful deadline, so an operator request and a configuration that shorten or disable that deadline later have a single place to act on.
- `LocalProcfsVerifier` treats an exited-but-unreaped process as absent. It read `/proc/<pid>`, which survives until the parent reaps it, so a killed execution could be reported as still running.

### Upgrading to 0.10.0

Process recipes are now version 4 and spawn edges version 3. The daemon reads every
spawn edge, subject identity and process recipe at startup and refuses to start,
naming the file, when one is from another version; it does not migrate. (Before
this check, such a record left the daemon serving while every read that reached
it failed: the fleet listing, `subject.list` and the sweeper's launch pass.)

State written by 0.9.0 holds version-3 process recipes, so a 0.10.0 daemon refuses
it. The cutover, checked against a copy of a real 0.9.0 state directory:

```sh
# with the 0.9.0 binary still installed: end managed agents, keep a copy
cp -a ~/.local/state/agent-radar ~/agent-radar-0.9.0-state
# install 0.10.0, then
radar reset          # default reset: keeps subject IDs, sessions and inboxes
radar
```

The default reset purges claims, process records, spawn edges, requests and the
event journal, keeps `lifecycle/identity/` and `communications/`, and restarts
Radar's tmux server, so panes in it are lost. Afterwards the daemon starts over the
old subjects and serves them detached. Do not use `radar reset --hard` to upgrade:
it discards inboxes and history. To roll back, use the 0.9.0 binary with the copy,
not the state the new daemon wrote.

## Radar 0.9.0

- **Breaking:** the UI session contract changed shape, so wire revision 13 requires a matching daemon and frontend rather than a mixed pair; a pi-radar newer than the SDK floor refuses the daemon, and a frontend left on the previous UI contract finds no `ui.attach`. Upgrade both together, and keep the previous pair if you need to roll back.
- Focus and selection are now separate. The daemon reads which pane the one tmux operator client is looking at (and whether its window is zoomed) with every inventory, and every view receives it as shared **focus**. **Selection** is each view's own: moving, jumping and selecting send nothing to the daemon, so two views no longer drag each other's cursor. The sidebar follows focus by default, selecting the focused row when focus changes; the dashboard browses on its own. `o` turns following on or off. Following never asks tmux to move; Enter still does. The shared actions `SelectRow`, `Move`, `NextAttention` and `NextWorking` are removed from the UI session wire, fold actions name their row, and `SessionState.selected` is replaced by `focus`. A command carries the row its view selected; a row-dependent command from a view with nothing selected acts on nothing rather than on the focus, and a named row that has since gone is refused rather than replaced.
- Radar watches tmux through one control-mode client with no pane output and no size, attached once a session exists and reattached if its session goes. Its notifications only bring the next read forward; the read decides. Focus commands name the one operator client and refuse when several are attached, instead of picking the most recently active one. Radar needs tmux 3.8 or newer and refuses an older serving server with a message naming the version. A session with `destroy-unattached on` is kept alive by the observer.
- The workspace picker (`A`) lists git projects and `Ctrl+g` reveals every directory the project source prints. **Breaking:** `[projects] filter` is removed, since the toggle replaces it; a config that still sets it is refused as an unknown key. Delete the line.
- A row that stops qualifying for the listing because the fleet changed (an agent or session finishing, a pane leaving the running view) stays drawn for five seconds, so a finish is seen rather than a row vanishing. Changing the view, the filter or the history toggle still removes rows at once. The count in the footer includes lingering rows.
- A request that expired before it was claimed is settled as refused `not_executed` by the next reconciliation. It used to stay unfinished forever, block `radar server stop`, and be refused by `radar lifecycle abandon`. The stop refusal now names `radar lifecycle reconcile` and `abandon`.
- `radar workspace` on a server with no session no longer creates a `main` session. The agent area waits for the first workspace and the sidebar opens the workspace picker. Without `--session` it attaches to tmux's most recent session, not the first listed.

## pi-radar 0.14.0

- Requires a Radar daemon at wire revision 13 or later (`RADAR_MINIMUM_REVISION` was 12), so the client reads the UI session and observation shapes the current daemon serves. Upgrade the daemon first; a pi-radar newer than the daemon refuses it rather than guessing.
- The observation schema carries the daemon's `closing` places, so a branch being closed is visible to a consumer instead of looking unchanged and then vanishing. The field is optional, so a daemon that does not publish it is still read.

## pi-radar 0.13.3

- `createRadarAttention` takes `onDiagnostic(problem | undefined)`: a mailbox that cannot drain — an unreadable message, a consumer that declines or throws, a failed receipt, a message no consumer handles — is reported when the answer changes and cleared on recovery. Before, every failure was swallowed and a stuck mailbox was invisible to the host.
- A message whose kind no consumer handles is fetched once, not on every heartbeat. It stays pending and stays reported.

## Radar 0.8.2

- `a` adds an empty tab to the selected row's workspace and asks nothing. It used to split the pane when a pane row was selected, and it asked "this may lose …" before creating a tab from a workspace row, though an addition ends no process.
- A tab starts in its workspace's directory. tmux started it in the working directory of the daemon, so a confirmed `a` opened in the home directory.

## Radar 0.8.1

- Reading a recipient's attention no longer touches the disk or waits behind a write. `attention` runs on every heartbeat and every subscriber wake; it listed the mailbox directory and parsed every pending message in full under one global lock, which measured 2.4 ms at 128 pending 20 KB messages and 52 ms at 128 pending 380 KB messages. The store now keeps an index rebuilt from the files at start and changed only after the file it mirrors is durable, so a read is a scan of in-memory entries and the lock serialises writers only. Capacity is checked from the same index instead of statting every record. The wire and the stored files are unchanged.
- Expiry removes a message before its receipts. It used to remove the receipts first, so a crash between the two left a message with no receipts that was pending for every recipient again. A receipt whose message is gone is now swept when the daemon starts.

## pi-radar 0.13.2

- `extension/radar-attention.ts` is now in the package: `files`, `build`, the `typecheck` list and a `pi-radar/attention` subpath. 0.13.1 exported it from `index.ts` but did not ship the module, so a TypeScript consumer of the published package could not resolve it.
- `messages.watch` reports its own end. A socket that closed after its baseline resolved `ok: true` with a dead handle that `onEnd` never reported, so a consumer never reconnected; a socket that closed *before* its baseline resolved `ok: true` as well, which the loop then stored as a live subscription. A pre-baseline close is now a `transport` failure, an unexpected end after the baseline calls `onEnd`, and a close the consumer asked for reports nothing.
- The watch opener has a deadline: a daemon that accepts the connection and never answers fails with `timeout` instead of hanging.
- `createRadarAttention` no longer opens duplicate subscriptions: repeated `activity()` calls while the first start was still in flight each dialled the daemon separately, leaking every handle but the last.
- `publication.facts.onAttention(listener)` forwards the mailbox hint from an accepted publish and returns an unsubscribe. Without it the client's `attention` had no consumer, and no host could wire the heartbeat to the loop.

## pi-radar 0.13.1

Do not adopt 0.13.1; use 0.13.3. Two of its claims were false, and the corrections are inline below.

- Exports the mailbox consumer the 0.13.0 SDK described but did not export: `createRadarAttention` (with `RADAR_IDLE_HOLD_MS`, `RADAR_ATTENTION_BACKOFF_MS` and `RadarActivity`) is reachable from the package entry point at runtime. Its TypeScript source was not in the tarball, so the package's types did not resolve (fixed in 0.13.2).
- `publish()` returns the accepted reply's `attention`, so a heartbeat establishes the mailbox hint that an idle agent has no connection to discover otherwise.
- `watchAttention` accepts `onEnd`, but it was never called, so a socket that died after establishment was still held as if it were live and the loop never re-subscribed (fixed in 0.13.2).
- `RADAR_RUNTIME_VERSION` is 7: the runtime's communication surface changed incompatibly in 0.13, and a mixed installation must fail explicitly rather than at the first handoff.

## 0.8.0

- The handoff mailbox becomes one addressed message lane. A message has an immutable id, producer provenance, one tagged body and a frozen set of subject recipients, written once so a crash cannot expose half a fanout; each recipient's receipt is its own record and attests only to durable capture. `communication.publish|inbox|get|ack` keep their names and take `message`/`message_ids`/`next`/`message_id` on the wire. Receipt is per recipient and idempotent, and a body is retired only once every recipient has held it for the retention window. Capacity is checked before acceptance and refuses: pending work is never evicted to admit a publication.
- Handoff is now the first body kind (`body: {kind: "handoff", …}`) with its parent-only routing unchanged: a worker may address only its resolved recorded parent, so the envelope's recipient set is not a general send permission.
- An idle agent needs no connection. `agent.publish` replies carry `attention: {revision, pending}` for the caller's verified subject, with `attention_unavailable` when the mailbox could not be read and no field for an unmanaged caller. `messages.watch` opens the optional push tier: a JSON opener, then length-prefixed MessagePack frames of that same state, a gap-free baseline first, latest-wins after, at most 4 KiB each. Subscriptions get their own 16-connection budget, so agent watches cannot take a UI attachment slot or a control connection.
- The contract is `communication.messages.v1`; `communication.handoff.v1` is gone. Wire revision 12 — restart the daemon with pi-radar 0.13.0.
- Stored communication records from the retired mailbox are refused rather than migrated: the daemon will not start while `communications/` holds them, and an empty inbox does not clear it — acknowledged records are retained for 30 days. Upgrade in this order: capture any pending handoff with the previous build, stop the daemon, then either archive `communications/` elsewhere or discard it. `radar reset --hard` discards pending and acknowledged records alike and is destructive; a plain `radar reset` deliberately keeps `communications`, so it does not clear the refusal.

## 0.7.0

- `subject.child.close {registration_id,subject_id,intent}` closes a child's pane by subject. The recorded parent names only the subject; Radar resolves the child's current launch and verifies the pane's occupant by birth identity, exactly as `spawn.close` does, and answers `completed`, `already_exited`, `refused` or `unknown`. A retry under the same request id settles against the target its first admission recorded. The agent stays retained for continue.
- pi-radar: `children.close(subject, intent, requestId?)`; `client.childClose` takes a subject, not a spawn edge and a bound `source`/`incarnation`.
- Wire revision 10, pi-radar 0.12.0, runtime 6: restart the daemon with the pin.

## 0.6.0

- Agents are read and controlled by subject, not by process. `agent.get`/`agent.list` are gone. `subject.get {subject_id}`, `subject.list {selection,after,limit}` and, for a parent, `subject.children {registration_id}` / `subject.child.get {registration_id,subject_id}` / `subject.child.inspect {registration_id,subject_id}` replace them; no subject record carries a registration or claim id.
- `subject.list` distinguishes `live` (the default), `retained` (detached agents, kept for continue) and `all` (also `starting` and `unknown`). A verified end detaches an agent; it never deletes it. `subject.children` defaults to `all` so a parent can find a retained worker.
- A child assignment belongs to the subject: `subject.assignment.acquire|publish|retire` name the child subject and take the caller's own registration as authority. The process-addressed `agent.acquire|publish|retire` refuse `channel: "assignment"`, and Radar carries a still-valid assignment to the replacement's granted attachment without a parent read.
- `process.get {registration_id}` serves one registration's evidence and channels for diagnostics; it is not an agent record.
- Wire revision 9 — restart the daemon with pi-radar 0.11.0.

## Unreleased

- Removes every compatibility path for state written by earlier builds. A spawn edge must be version 2 with a session parent, an exit observation and a bound `registration_id`; the `agent_id` and `bound_agent_id` read aliases, the pre-migration `IdentityMapped` / `IdentityUnresolved` / `Imported` lifecycle transitions, `execution_generation`, `identity_unresolved`, fingerprint-only operation records, per-execution alias admission and the registration-less `bind_spawn` are gone. Run `radar reset --hard` once after installing; older state is refused rather than adapted.
- Serves tmux only. The Herdr backend, its adapter, its config value and `RADAR_CONTROL_BACKEND=herdr` are gone, so a config naming `herdr` now refuses startup; `[daemon] backend` accepts `tmux` alone. The daemon's inventory now comes from the tmux server it owns. `tests/terminal_smoke.py` was the fake-backend PTY smoke and is gone with it; `tests/tmux_acceptance.py` is the dashboard acceptance.
- `agent.get` serves the same record `agent.list` entries are: it gained `claim_standing` (the claim decision behind the registration, `null` when it never claimed a session) and `scoped` (whether its process runs in a managed scope). Additive; the wire revision stays 8.
- Renames the stored claim keys: a claim is stored under `claim_id` and its durable agent under `subject_id`, where they were `subject_id` and `logical_subject_id`. `lifecycle::read_logical` and `list_logical_after` are now `current_attachment` and `list_subjects_after`.
- An agent record reports the location its attachment verified with presence, falling back to the one it registered with. A process that only ever reported through presence used to name no place, so the observation could not join it to the pane it occupies and the dashboard showed it as a detached subject instead of an agent row.
- A state root this build cannot replay is refused with a message naming the file and the mismatch, and the daemon exits `78` (`EX_CONFIG`), which the unit lists in `RestartPreventExitStatus`: journald records one explained failure instead of a restart loop. `radar reset --hard --yes` is the recovery path and now clears such a root too, managing any active scope by unit name when the process records in it cannot be read.

## pi-radar 0.10.1

- Fixes `children.list`, `observe`, `inspect` and the assignment follow, which refused as unsupported: `spawnList` pinned a `spawn.list.v1` contract no daemon advertises. It now requires `registry.v1`, which serves `spawn.list`.

## 0.5.0

- The daemon withdraws a launch that has not presented within 30 seconds: it revokes the claim, stops a captured process through its scope, closes the launch's pane and records `withdrawn` (`at`, `reason`) on the spawn edge. The subject is then free to continue. Applies to `L`, spawn, continue and restart.
- `lifecycle.restart` answers with `launch_request_id`, the edge of the replacement.
- A subject's authority pointer is the one check behind reserving and granting a claim. The separate `successor` field is gone from stored records, events and the observation read.
- Wire revision 8 — restart the daemon with pi-radar 0.10.0, which drops `timeoutMs` from `launchRadarWorker`, reports `failed` instead of `refused`, and adds `children.restart`.

## 0.4.0

- Every tracked launch (`L`, restart, spawn) starts `radar run --launch` in its pane; the daemon answers the claim with the program from its own record and the process enters its scope. `launch-bootstrap` and `spawn.bootstrap` are gone.
- `spawn.create` accepts `continue_subject_id`: a parent starts its own released child again in the session the daemon recorded, with any session flag in the caller's argv replaced. `lifecycle.restart` also accepts a parent naming itself with the program to restart as, unfenced by claim; the operator's restart keeps its claim fence. Wire revision 7 — restart the daemon with pi-radar 0.9.0.

## 0.3.1

- A spawn edge's `bound` record names the child's `subject_id` beside its registration, so a consumer can address a child by the agent that survives its restart. Additive: the wire revision stays 6. pi-radar 0.8.0 needs this daemon, since it refuses a launch whose edge binds without a subject.

## 0.3.0

- A managed launch pre-mints its process identity: the launch passes `PI_RADAR_REGISTRATION_ID`, and its claim is `reserved` (no authority) until the first verified report replaces it with the session's own claim. Process evidence belongs to the registration. Wire revision 6 — restart the daemon with pi-radar 0.7.0.
- Every tracked launch (`L`, restart, Herdsman) runs in a systemd user scope, and `radar run -- COMMAND` starts one in the current pane. Managed stops go through the scope, are graceful only (`SendSIGKILL=no`, 2s) and name survivors. Observed agents keep their controls and are marked.
- `radar run --agent -- PROGRAM` launches an agent; `radar run -- COMMAND` is a managed command (a scope, no subject, no identity in its environment). Home Manager `programs.radar.managedAgents` and `lib.managedAgent` wrap an agent so it starts under `radar run --agent`.

## 0.2.0

- Sessions attach through one verified presence claim; manually started Pi sessions are adopted without an operator step.
- The agent is published as `subject_id` and an attachment as `claim_id`; `execution_id` and `execution_generation` are gone. Wire revision 5 — restart the daemon with pi-radar 0.6.0.
- `D` is the single delete key and ends nested agents gracefully before closing a place.
- `radar --version`, a clap command line, and shell completions.

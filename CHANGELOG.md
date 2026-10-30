# Changelog

Radar is alpha: breaking changes are expected and bump the minor version. The wire revision (`radar --version` prints it) is the compatibility contract with pi-radar; the version is for people.

## Radar 0.9.0

- The workspace picker (`A`) lists git projects and `Ctrl+g` reveals every directory the project source prints. **Breaking:** `[projects] filter` is removed, since the toggle replaces it; a config that still sets it is refused as an unknown key. Delete the line.
- A row that stops qualifying for the listing because the fleet changed (an agent or session finishing, a pane leaving the running view) stays drawn for five seconds, so a finish is seen rather than a row vanishing. Changing the view, the filter or the history toggle still removes rows at once. The count in the footer includes lingering rows.
- A request that expired before it was claimed is settled as refused `not_executed` by the next reconciliation. It used to stay unfinished forever, block `radar server stop`, and be refused by `radar lifecycle abandon`. The stop refusal now names `radar lifecycle reconcile` and `abandon`.
- `radar workspace` on a server with no session no longer creates a `main` session. The agent area waits for the first workspace and the sidebar opens the workspace picker. Without `--session` it attaches to tmux's most recent session, not the first listed.

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

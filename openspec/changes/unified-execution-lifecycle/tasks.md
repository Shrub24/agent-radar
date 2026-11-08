# Tasks

## 1. One execution constructor

- [x] 1.1 Build the single execution constructor over the registration, with optional placement and optional launch intent; adoption's process creation is the base case. Delete `create_with_intent`'s minted subject and the reserved presence decision for launches with no subject.
- [x] 1.2 Key `launch.claim`, capture, the pane-death wake, withdrawal and the sweeper cursor on the registration. Each has a test that fails if it reads the old claim key.
- [x] 1.3 Configured `L` and container recipes reserve an execution only. First presence attaches a subject through enrollment; a launched program reports presence and gets a subject.

## 2. Restart

- [x] 2.1 Restart uses the same constructor and attaches its successor claim to the existing subject through the shared attach step. Withdrawal revokes that claim and frees the subject. Existing restart tests pass unchanged.

## 3. Spawn

- [x] 3.1 `spawn.create` creates its child through the constructor and writes the edge keyed on the child's registration. The edge binds to the child's subject at grant; a resumed session continues its existing subject.
- [x] 3.2 Re-key the edge's exit observation, topology reads and `spawn.close` on the registration.

## 3a. The request that addresses an execution

- [x] 3a.1 `lifecycle.cancel` and `lifecycle.stop` address the execution. `SubjectParams.claim_id` is optional and only fences the attachment the caller saw; `cancel_target` resolves the execution from either address and refuses a launch that never presented only when the caller names a claim it cannot have. `subjects::stop(registration)`, `subjects::cancel(registration)` and `revoke_execution` replace the claim-keyed wrappers, and `revoke(claim)` is gone with them.
- [x] 3a.3 A settled withdrawal is withdrawn once: `settled` (revoked, no process, no live attachment) is the one rule the scan in `unclaimed` and `withdraw_locked` both ask, so a reservation with no spawn edge is not offered or processed twice.
- [ ] 3a.2 `fence_registration` re-took the registry's transition lock, which `bind_locked` already holds, so every registration carrying a spawn token deadlocked the process. Fixed by keeping it a step whose caller serializes; a `*_locked` sibling split proved redundant and was dropped.

## 4. Observation and display

- [ ] 4.1 The observation lists only attached subjects; the tree draws no subject row for a launch in flight. Decide from the tree's needs whether the pane row carries a published launch state. The launch row is gone, but a subject whose execution ended now renders as `unresolved` and a restarted agent's row as `unplaced`, so this decides what those rows say. The rule is applied: `lifecycle.get` and `lifecycle.list` read **subjects**, and a launch in flight is read from its execution record and `spawn.get`. The observation layer is execution-keyed (`observe_exit`, `observe_pane`, the sweep, the pane-death wake) and the client's `LifecycleControlRequest` names a subject *or* the spawn that made one. 167 of 176 control-plane tests pass; the nine left are listed under 4.
- [x] 4.2 Move the spawn binding to the registration and settle the wire revision.

- [x] 4.4 Every control-plane test reads a launch as what it is. A launch that has not presented is read from its execution record and `spawn.get`; a launch that has presented is read by the subject its grant created. The supervisor's code is reported through `launch.complete` and read back from the execution, which is the only place it lives. 176 pass.

## 5. Close out

- [x] 5.1 Update the docs, changelog and upgrade note (earlier launch records are refused; `radar reset`).
- [ ] 5.2 Gate: fmt, strict Clippy, `cargo test --locked`, pi-radar checks, OpenSpec strict, checked Nix build.
- [ ] 5.3 Disposable live acceptance: `L`, restart, `spawn.create`, an unpresented launch withdrawn, and branch close. `L` and `spawn.create` pass; restart settles but leaves the predecessor subject `unresolved` and the successor `unplaced`, which is 4.1.

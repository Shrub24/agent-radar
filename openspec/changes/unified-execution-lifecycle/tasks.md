# Tasks

## 1. One execution constructor

- [ ] 1.1 Build the single execution constructor over the registration, with optional placement and optional launch intent; adoption's process creation is the base case. Delete `create_with_intent`'s minted subject and the reserved presence decision for launches with no subject.
- [ ] 1.2 Key `launch.claim`, capture, the pane-death wake, withdrawal and the sweeper cursor on the registration. Each has a test that fails if it reads the old claim key.
- [ ] 1.3 Configured `L` and container recipes reserve an execution only. First presence attaches a subject through enrollment; a launched program reports presence and gets a subject.

## 2. Restart

- [ ] 2.1 Restart uses the same constructor and attaches its successor claim to the existing subject through the shared attach step. Withdrawal revokes that claim and frees the subject. Existing restart tests pass unchanged.

## 3. Spawn

- [ ] 3.1 `spawn.create` creates its child through the constructor and writes the edge keyed on the child's registration. The edge binds to the child's subject at grant; a resumed session continues its existing subject.
- [ ] 3.2 Re-key the edge's exit observation, topology reads and `spawn.close` on the registration.

## 4. Observation and display

- [ ] 4.1 The observation lists only attached subjects; the tree draws no subject row for a launch in flight. Decide from the tree's needs whether the pane row carries a published launch state.
- [ ] 4.2 Move the spawn binding to the registration and settle the wire revision.

## 5. Close out

- [ ] 5.1 Update the docs, changelog and upgrade note (earlier launch records are refused; `radar reset`).
- [ ] 5.2 Gate: fmt, strict Clippy, `cargo test --locked`, pi-radar checks, OpenSpec strict, checked Nix build.
- [ ] 5.3 Disposable live acceptance: `L`, restart, `spawn.create`, an unpresented launch withdrawn, and branch close.

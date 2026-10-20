# Tasks

> Historical task record: completed session-aware spawn tasks remain accepted.
> Unchecked restart-execution tasks are superseded by
> `../daemon-managed-lifecycle/tasks.md`, not completed or independently runnable.

## 1. Logical subject and topology contract
- [x] 1.1 Define the stable parent pair as exact `parent agent_id` plus explicit `parent_session_id`; keep process birth identity and mux location separate, and do not resolve a session UUID to a process by searching registrations.
- [x] 1.2 Extend spawn request and edge formats to carry both exact process parent and session parent. Require fresh published session context for that exact agent to equal `parent_session_id` before effects; keep legacy v1 edges explicitly process-parented and readable without inferred session identity.
- [x] 1.3 Test valid exact process+session admission, missing/stale/mismatched session context refusal before create, duplicate attaches remaining distinct with no auto-selection, and legacy v1 reads. Existing edges remain bound to the explicitly named process/session pair when a second same-session registration is added; authorized process-replacement stability remains pending until restart binding exists.

## 2. TUI restart admission and operation record
- [ ] 2.1 Restart remains unavailable: Herdr's documented pane creation/input operations provide no pane-ready or shell-ready acknowledgement. Do not add a TUI restart action or infer readiness using timing, output heuristics or marker injection. Once upstream readiness exists, admit only subjects with captured launch provenance and revalidate exact session/process identity.
- [ ] 2.2 Pending upstream readiness contract: after nonfocused pane creation, Herdr must expose a bounded positive acknowledgement that the pane's interactive shell is ready to accept one atomic text+Enter request; absence/timeout must be distinguishable from ready. No restart operation is implemented until this evidence exists.
- [ ] 2.3 Durable restart outcomes, private subject binding, replacement correlation and TUI dispatch remain blocked on 2.2; no public protocol/CLI surface is added.

## 3. Verified physical restart without child restart
- [ ] 3.1 Blocked: do not signal until pane-ready launch capability is available; when resumed, use exact pidfd + SIGTERM and bounded positive exit confirmation only.
- [ ] 3.2 Blocked on 2.2: launch exact captured executable/argv/cwd/environment/session once in a fresh nonfocused pane, then correlate and verify replacement registration.
- [ ] 3.3 Blocked: reconcile subject binding and preserve assignment/execution facts with durable partial/unknown outcomes.
- [ ] 3.4 Blocked until successful restart path exists; preserve children and mark missing evidence unresolved.

## 4. Operator integration and independent verification
- [ ] 4.1 No restart action is exposed while readiness is absent. If a relevant managed target is selected, explain the missing Herdr readiness capability; child and automatic restart remain unavailable.
- [ ] 4.2-4.3 Blocked until 2.2 and restart implementation exist.
- [x] 4.4 Verify the completed session-aware spawn slice with formatting, strict Clippy, full locked tests, publisher fixture, terminal smoke, strict OpenSpec, and checked Nix gates. Restart-specific live smoke remains blocked.
- [ ] 4.5 Blocked: run restart-operation, replacement, child-preservation, and gated live Herdr restart tests only after the positive pane-readiness capability is implemented and documented.

# Tasks

## 1. Registration records and persistence

- [x] 1.1 Implement strict versioned registration, backend-qualified location and private launch-spec types; document shapes and add codec tests proving duplicate session attaches stay distinct, unknown state vocabulary survives, and launch arguments never appear in public responses.
- [x] 1.2 Implement atomic private registry storage and idempotent registration; verify restart restoration, malformed/symlink/oversized record refusal, identical retry and conflicting identity rejection with temporary state roots.

## 2. Direct publication and freshness

- [x] 2.1 Implement generation handles, complete execution/assignment snapshots, sequences, leases and retirement/replacement; document reconnect semantics and test stale-writer rejection, equal-content replay, conflicting/older sequences, lease expiry and freshness reset after daemon restart using an injected clock.
- [x] 2.2 Add injectable process verification using boot/PID/start identity outside registry locks; test PID reuse, disappearance, unavailable procfs and independently labelled publication freshness without physical controls.

## 3. Daemon endpoints and publisher contract

- [x] 3.1 Serve agent.register/publish/retire/get/list over the trusted control socket with bounded pagination and an independent agent_registry capability; test real-socket operations with no mux backend, corrupt-store errors, slow verification responsiveness and unchanged existing mux-report semantics.
- [x] 3.2 Publish protocol documentation, canonical fixtures and a reconnecting extension-publisher example suitable for Herdsman's existing owner projection; execute that example against a disposable daemon and verify private launch data, expired writer fencing and no Herdr calls. Record the pi-extensions port boundary; do not edit that repository in this task.

## 4. Independent integration gate

- [x] 4.1 Independently verify fmt, strict Clippy, locked tests/build, existing PTY smokes, checked Nix package and strict OpenSpec validation; record source/build provenance and daemon restart/reconnect evidence in verification.md, distinguishing temporary-socket tests from live publisher integration.

## Workflow follow-up

- Port Herdsman lead/worker registration and assignment advertising in pi-extensions against the published contract, leaving optional Herdr metadata mirroring as fallback.
- Specify physical launch/stop/resume execution using these records, then owner-coordinated recovery and stale-lead restart plans; this change does not enable them.
- Sync and archive after acceptance; do not claim agent lifecycle execution from registration tests.

# Superseded — archived without syncing specs

Archived 2026-10-22 with 13/21 tasks done. Do not sync its delta into `openspec/specs/`.

**Why.** Its lifecycle model — operator enrollment (3.1) of a registered Pi, and executions identified by logical subject plus execution generation — no longer exists. Presence (`ce4fac4d`) adopts a session without an operator step, and `managed-scoped-launch` task 0 replaced the execution/generation pair with `subject_id` and `claim_id`. `lifecycle.enroll.v1` is still advertised but nothing reaches it.

**Landed from it.** The durable event foundation, TUI extraction, operator stop/cancel through exact process fences, restart via common stop and bootstrap, and the lifecycle and child-spawn methods. These live in the code and in the current specs, not here.

**Still open elsewhere.** Real disposable tmux/Pi acceptance (5.2) is operator-run and now belongs to `managed-topology` and `daemon-service`. Restart's continuity across a replacement (3.3) is what the pre-minted claim provides. The verification notes and Herdsman brief in this directory are historical.

**Revisit if** operator enrollment of a process Radar did not see launch is wanted again.

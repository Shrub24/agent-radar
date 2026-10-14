# Tasks

## 1. Audit

- [ ] 1.1 List every place that detects an exit, a missing pane or a drift for a Radar-owned subject (recovery sweep, pidfd watchers, spawn-close checks, topology reconcile) and record whether it commits through one path. Output: a short table in `design.md`; no code.

## 2. One transition path

- [ ] 2.1 Route observer-originated and command-originated transitions for lifecycle subjects through one reducer under the existing record fence, with an evidence summary on the committed event. Test the exit-races-stop case and an unchanged repeated observation appending nothing.
- [ ] 2.2 Add evidence strength and the confirmation window for weak evidence, with the thresholds as daemon settings. Test a single missed listing, a persistent absence and a contradicting observation clearing the count.
- [ ] 2.3 Add the ended-externally outcome for a Radar-launched pane or process that disappears outside Radar, reusing the lifecycle vocabulary. Test it through a real tmux pane killed outside Radar.

## 3. Reconciliation

- [ ] 3.1 Name the reconcile triggers (start, changed serving identity, unknown record, inconsistency, operator request) behind one entry point that commits through the reducer and never dispatches an effect. Test each trigger and that steady state runs none.
- [ ] 3.2 Add an operator reconcile request to the CLI and control plane and document it.

## 4. Unresolved records

- [ ] 4.1 Surface the reason on an unresolved record in reads and events. Test an unobservable effect staying unresolved and not replayed.
- [ ] 4.2 Add an abandon operation that commits a terminal abandoned outcome and an event, and make the tmux stop gate ignore abandoned records. Test the gate before and after.

## 5. Documentation and gate

- [ ] 5.1 Document ownership, transitions, confirmation, reconciliation triggers and abandonment in `docs/control-plane.md` and `docs/daemon.md`; add the assignment/execution ownership direction to `ideas.md`.
- [ ] 5.2 Run fmt, strict Clippy, the locked test suite, tmux acceptance, strict spec validation and the checked Nix build; record real exit codes.

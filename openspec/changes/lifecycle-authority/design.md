# Design

## Context

The daemon already records effectful requests before dispatch, keeps unknown outcomes unknown, tracks launches as lifecycle subjects with fenced bindings, and runs a recovery sweep on start. What is missing is a stated owner for the resulting state and a single rule for how observations change it. Today exits are detected in several places and the display re-derives what it needs.

## Decisions

### 1. Ownership follows execution

If Radar executed it (launch, stop, close, container create/delete/reload), Radar owns the lifecycle and the durable result, and every transition goes through Radar. State Radar did not execute and cannot verify (an agent's model, its context usage, the text of its assignment) belongs to the publisher and is held only as source-labelled, freshness-bounded evidence. Radar persisting a copy of such evidence does not make it the owner.

Long term the assignment relation (who is assigned what, since when, with what outcome) is expected to become Radar state, with assignment details remaining external, and execution likewise; this change does not move them. They are recorded in `ideas.md` with the trigger that Herdsman's assign/release flow is expressed as daemon commands.

### 2. Observation is input, transition is output

An observer produces a condition about a Radar-owned subject. A reducer compares it with the subject's durable state and commits a transition only when the state changes, appending one semantic event with an evidence summary (source, strength, observed time). Snapshots and samples never enter the log; only the transition they cause does. One writer per subject: observers and command paths (cancel in flight while an observer sees the exit) go through the same fenced reducer, so the record cannot take two conflicting transitions.

### 3. Evidence strength decides smoothing

Definitive evidence transitions immediately: a verified process exit (pidfd or matching birth identity), a tmux notification naming the exact pane id and server identity. Weak evidence needs confirmation: a pane absent from one listing, a failed read, a stale source. The rule is a per-subject, per-condition counter or first-seen time with a bounded confirmation window, cleared on any contradicting observation. This is the soft/hard state idea from service monitoring (a check fails several times before the state is hard), the `for:` duration of alerting rules, and probe failure thresholds in orchestrators; it needs a few lines of state and no library. The thresholds are settings, not contract.

### 4. Reconciliation is recovery

Reconciliation recomputes unresolved Radar-owned lifecycle facts from evidence. Triggers: daemon start, a changed serving identity, an operation left unknown, an inconsistency the reducer detects (for example a bound subject whose process identity no longer matches), and an operator request. It reads, decides and commits transitions through the same reducer, and it never retries an effectful operation. There is no periodic reconcile: subscribed observation already keeps state current.

### 5. Unknown stays rare and visible

Unknown describes uncertainty about an effect Radar dispatched, not a disagreement between lifecycle and observation. It auto-triggers the targeted verification described above. When evidence is unavailable or the effect is unobservable (sent input, a vanished pane) the record stays unresolved and visible with its reason. A record that can never resolve blocks things (the tmux server stop gate); the operator abandons it, which commits a terminal `abandoned` outcome and an event.

### 6. Events are the contract

Consumers read semantic events with opaque cursors. The journal's internal record shape, retention and snapshots may change without a contract change.

## Risks and open items

- Drift of a Radar-launched pane closed outside Radar becomes an `ended externally` transition with evidence; the exact state names are chosen with the lifecycle owner's existing vocabulary during implementation.
- Confirmation windows trade latency for false transitions; defaults are chosen conservatively and measured against `tmux-control-mode`.
- Observers that stop when no display is subscribed would leave durable state stale. Tracked subjects therefore keep event-driven watchers (pidfd, later tmux notifications) independent of display demand; only display sampling is demand-driven.

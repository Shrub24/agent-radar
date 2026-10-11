# Design

## Decisions

- **Long-poll cursor, not streaming.** The protocol is one request and one response per line over at most 16 bounded workers. A subscription stream would hold a worker and need multiplexing. `events.read` with `wait_ms` (capped at one second, below the 120 s idle and 5 s client budget rules) keeps the framing and gives resumable delivery. Alternative: a streaming connection; rejected for worker exhaustion and framing change.
- **Journal in the existing store, fact first.** The store has independent atomic files and no multi-file transaction, and one is not worth building. The fact is made durable, then its event is appended, so an event never exists without its fact. A crash between the two can lose an event; a dirty marker written at start and removed on clean stop makes any unclean restart begin a new epoch, so consumers get `cursor_expired` and resynchronise by full read. Alternative: a manifest/transaction scheme; rejected as a larger foundation than the guarantee needs. Retention is bounded by count and bytes; the oldest are dropped.
- **Pointer events.** `{seq, kind, agent_id?, spawn?, request_id?, at}`. Consumers re-read via `agent.get`, `spawn.get` and request readbacks, so privacy rules stay in one place.
- **Resynchronisation.** A cursor older than retention, or from a different journal epoch (daemon store reset), returns `cursor_expired` plus the current cursor. Ordering within one daemon is total by sequence.
- **Freshness events.** Staleness is time-derived, not committed. The journal records a `stale` transition only when the daemon observes it on a read or sweep; consumers must not treat absence of an event as freshness.
- **No assignment semantics.** Events never claim completion, readiness or exit.

## Risks

- Wake-up of waiting readers must be bounded and must not hold locks across the wait.
- Wait budgets interact with the 5 s client budget and shutdown joining; the wait must be cancellable.

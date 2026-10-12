# Design

## Fact ownership

| Fact | Channel | Writer |
|---|---|---|
| activity, waiting reason, last outcome, actions | execution (existing) | the agent |
| model, provider, thinking, context usage, session display name, background tasks | **facts (new)** | the agent |
| role/definition, run, request, assignment text and start, awaited items, ask | assignment (extended) | the registered owner |

The split is by who knows the fact. The agent knows its model and its own background tasks whether or not anyone owns it, so those must not depend on Herdsman. Only an owner knows what it assigned and what it awaits. The dashboard derives `waiting` from awaited items plus background tasks; that derivation stays in the dashboard, never in a channel.

## Decisions

1. **A third channel, not widening execution.** Execution changes with every turn and carries the agent's own lease; facts change slowly and are bounded separately. Separate fencing keeps one writer's failure from freezing the other. Cost: one more acquire for a publisher.
2. **Assignment is extended, not duplicated.** Its publisher reports owner provenance; it is not an authenticated owner credential. New fields are optional and additive. Keep the existing same-UID socket trust boundary: neither acquisition nor publication compares `reporting_owner` with registration `owner`. Owner authorship is the publisher contract, not daemon-enforced authority. The user explicitly chose this over introducing either a new-fields-only name gate or a whole-channel authorization change.
3. **Bounds and shapes.** Facts are an optional-field complete snapshot: `model`, `provider`, `thinking`, `context_usage` (bounded producer display text, no state authority), `session_name`, `background_tasks` (`{id, phase}` entries), and `background_started_unix_ms`. Text uses the existing 1 KiB bound; task lists are capped at 64 with unique nonempty ids. Timestamps are nonnegative Unix milliseconds, never lease/freshness clocks. Assignment retains its existing activity/outcome/actions and adds optional `role`, `definition`, `run`, `request`, `assignment`, `assigned_started_unix_ms`, `awaited` (at most 64 bounded nonempty items), and `ask`. Nested objects reject unknown fields. Existing whole-record byte limits still apply; no truncation. Omitted fields in a new snapshot withdraw old values.
4. **No derived state in the registry.** Waiting, lost and settling are projections the dashboard computes from facts and freshness.
5. **Absent is unavailable.** A fact nobody published is shown as unavailable, never defaulted or taken from Herdr.
6. **Protocol v2.** Keep the shipped method names and channel fencing. Advertise the additive facts contract as `registry.facts.v1`, derived alongside `registry.v1`; publishers must negotiate it before acquiring `facts` or publishing the extended assignment fields. Old durable execution/assignment records still decode; no new physical-backend dependency.
7. **Publisher side is not decided here.** Whether Herdsman's reporter or a standalone Pi reporter writes the facts channel is pi-extensions' decision; the daemon contract is the same either way.

## Risks

- The channel multiplies writers per agent. Mitigated by identical fencing and replay rules and one fixture.
- Free-form words can drift. Mitigated by preserving unknown vocabulary and keeping the dashboard's rendering tolerant.

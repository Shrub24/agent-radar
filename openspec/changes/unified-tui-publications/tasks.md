# Tasks

Planning only. Build on the shared-UI implementation; do not migrate pi-radar or other daemon lanes. Each group lands its own necessary tests and docs; no parallel legacy TUI delivery path remains at completion.

## 1. Portable content and framing foundation

- [x] 1.1 Define typed head/reference, lane kind, opening handshake, binary request/result and failure DTOs; document named fields, enum tags, exact integers and hash representation in `docs/control-plane.md`. Verify hand-written portable fixtures cover each of the three lane types and a counter above JavaScript's safe-number range without changing existing SDK wire fixtures.
- [x] 1.2 Add `rmp-serde` and XXH3-128 support and one domain-agnostic bounded encoder/hash object. Verify a known byte/hash fixture, map-insertion-order invariance and metadata exclusion; audit observation fields so actual sample/freshness facts survive while publication counters/times do not cause payload churn. Do not test dependency hash algorithms separately.
- [x] 1.3 Add common immutable-content references and a byte/entry-bounded server store with pinned current payloads. Verify fetch-by-hash, A→B→A reuse, expired lookup recovery and bounded retained bytes at the store boundary; unchanged objects must be reused without subscriber-specific serialization.
- [x] 1.4 Add one bounded length-prefixed MessagePack frame reader/writer using the existing transport/runtime protections. Verify oversized advertised lengths, partial frames and codec failure at the actual framing boundary; document initial 1 MiB payload / 4 MiB frame limits and confirm encoding cannot grow beyond its bound before refusal.

## 2. Three-payload actor head and observation adapter

- [x] 2.1 Adapt the existing observation feeder to supply immutable facts plus one encoded observation reference; derive tree/session from that same accepted observation input. Verify through the session boundary that a new-observation head never names rows from one sample and facts from another; no sampling authority moves into the actor.
- [x] 2.2 Replace the two-payload JSON/BLAKE3 publication and row-only retained cache with the common three-reference head/store. Verify navigation changes only session identity, observation-only changes can retain tree identity, no-op retries preserve receipts without artificial heads, and all content is stored before references publish. Keep one publication counter and no per-lane counter.
- [x] 2.3 Carry typed local process-demand updates through the UI attachment path and aggregate them with existing observation readers. Verify active demand keeps collection enabled, disconnect releases only that attachment's demand, and non-TUI demand remains effective; update demand documentation without changing sampler policy.

## 3. TUI transport and coherent client cutover

- [x] 3.1 Wire the TUI-only opening handshake and binary action/content and subscription roles to the existing serving transport. Verify baseline readiness, cancellation during handshake, coalesced missing-content recovery and subscription limits through real sockets; reuse existing assertions rather than duplicate per-lane transport tests.
- [x] 3.2 Implement current-head plus one-target client adoption using the shared codec/content structures. Verify delayed content cannot produce mixed-head rendering, older fetch completion cannot roll back a newer head, and replaced/superseded payloads are released. Include reconnect under a fresh attachment and explicit content-expiry recovery.
- [x] 3.3 Preserve prompt gate invalidation independently of content/render adoption and ordered action receipts. With a controlled delayed reply/content boundary, verify a remote gate change blocks stale ordinary input while pending actions and optimistic navigation do not cause rollback or hide the gate; retain presenter/result retry behavior.
- [x] 3.4 Switch dashboard/sidebar observation and detail rendering to the coherent head; remove `SnapshotFetcher`, its thread/connection and superseded UI hash/cache paths together. Verify existing row identity joins, empty canonical projection and safe unexplained-row behavior, and exercise a complete project/confirmation presenter workflow through the new path. Keep local geometry and source-current judgement local.
- [x] 3.5 Implement optional inline payload delivery using three explicit lane selections, defaulting to the full TUI set and session-only for modal presenters. Verify a coalesced subscriber can recover skipped content and a presenter need not fetch fleet observations merely to answer its interaction. Do not add filtered observation datasets or a generic subscription plugin registry.

## 4. Failure handling and boundedness

- [x] 4.1 Return typed oversized-candidate diagnostics while retaining the previous coherent head; verify an oversized baseline shows an explicit unavailable state, no partial content is adopted and the client does not reconnect endlessly to the same rejected candidate. Document retry on a newer candidate or explicit reconnect.
- [ ] 4.2 Exercise a stalled writer and burst of heads through the serving path; verify bounded pending state, timely teardown, unaffected second subscriber/actor progress and unchanged admitted-operation semantics. Reuse or strengthen existing slow-reader/capacity tests rather than creating a new fake-daemon framework.

## 5. Integration, evidence and contract reconciliation

- [x] 5.1 Measure representative and larger observation payload encoding/hash/decode cost, bytes, populated join and action-to-adoption/render tails over sockets under ordinary daemon work. Record workload, build, timing boundaries and limits in this change; no performance thresholds in routine tests and no summing unrelated medians as claimed end-to-end latency.
- [x] 5.2 Update protocol/acceptance docs and reconcile superseded two-payload JSON/BLAKE3 requirements from `shared-ui-session` before sync. Verify the new specs do not coexist with contradictory host, content or framing contracts; preserve inherited `sidebar-mode` sync prerequisites and leave live acceptance explicitly unperformed until the owner runs it.
- [x] 5.3 Run fmt, strict Clippy, Rust suites, existing SDK tests, applicable generated-contract drift checks, strict OpenSpec validation for affected changes and rationale lint. Verify non-TUI JSON/event/ingest/presence fixtures are unchanged and no out-of-scope consumer has been wired to the new primitives. Nix is required only for release/build verification or when needed to run tests.

## Workflow follow-up

- Owner performs live Kitty/tmux acceptance using the updated procedure; record terminal-visible behavior separately from in-memory render timing.
- Review and sync the preceding shared-UI baseline and this delta in dependency order before archival.
- Propose separate pi-radar and other daemon-lane migrations from `plan.md`; preserve event cursors/gaps and command/admission semantics instead of applying snapshot coalescing universally.

## Evidence and gaps

- 5.1 (release, 744 rows, 120x40): observation 62,470 B, tree 82,507 B, session 174 B, full frame 145,479 B; observation encode+hash median 154 µs, decode 281 µs; key handler to in-memory frame median 1.19 ms, p95 1.53 ms, over 50 inputs through real sockets. Excludes physical terminal I/O and daemon load.
- 4.2 stays open: stalled-writer isolation is tested; a burst-of-heads test is not.
- Not verified: live Kitty/tmux acceptance (owner); Nix build; Rust/TypeScript cross-decode (fixtures are independently authored bytes decoded in Rust only).
- Archive: daemon-transport and ui-session deltas modify requirements that exist only after `shared-ui-session` is synced.

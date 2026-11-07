# Design

## Context

See proposal.md for motivation. `Session::project()` currently serializes rows and session state as JSON to hash them with BLAKE3, retains a small queue of typed historical row vectors and publishes through a latest-state watch. `ui.content.get` retrieves only rows. The frontend's `SnapshotFetcher` separately waits on observation revisions on its own thread. The daemon observation hub already owns collection, immutable snapshots, source freshness and optional process demand. Its existing read response has a 512 KiB size bound; a socket write deadline is not a replacement for that bound.

The graph index predates these branch changes; these seams were checked directly in the separate shared-UI workspace. `shared-ui-session` is implemented but not synced. This change succeeds its publication contract and must not silently compete with it at spec sync.

## Goals / Non-Goals

**Goals:** one coherent publication/adoption path; shared immutable encoded content; portable wire conventions; modular primitives independent of TUI semantics; bounded memory and socket work; delete the old frontend observation path.

**Non-Goals:** migrate pi-radar or other daemon lanes, redesign the sampler or durable store, smooth observations, add a lane registry/plugin system, per-consumer filtered observations, compression, deltas, chunking, a second runtime, or frontend payload history. Existing single-tmux-client focus assumptions and domain admission remain unchanged.

## Decisions

### 1. One head, three typed references

A head contains one monotonic `update_id`, three explicit content references (observation, tree, session), and a gate-generation safety stamp derived from the referenced session payload. Attachment identity scopes delivery across reconnect/restart; hashes identify lane bytes, not order. Preserve action counters/receipts and interaction/domain-request identities. No per-lane sequence is needed for adoption. An observation hub revision may remain internal provenance; it is not another client ordering mechanism.

Publish a changed head after a transition and permit bounded draining/coalescing of queued latest state. Duplicate retries, content reads and unchanged transitions do not allocate heads merely because a mailbox message arrived. Receipts and retained presentation requests remain separate from render coalescing. A gate stamp invalidates input promptly even before the complete head can be displayed; it must equal the referenced session's gate, not become an independent authority.

**Why:** independent streams force the frontend to reconstruct consistency. One head fixes the combination while separate hashes avoid rebuilding or resending unchanged parts. Publishing every read/duplicate message would create meaningless ordering churn.

### 2. MessagePack and XXH3-128, with explicit stable bytes

Use `rmp-serde` named-map encoding, existing explicit string enum tags, UTF-8 field names, sorted string-key maps, and domain-defined list order. Do not round-trip through `serde_json::Value` just to sort every payload. Wire floats must be finite; preserve real readings rather than round them as an accidental smoothing policy. Use shortest consistent integer encoding from the chosen codec; document it in fixed golden fixtures. Unsigned 64-bit counters remain exact integers; future TypeScript decoders must preserve values beyond the safe-number range as BigInt. Hashes use 32 lowercase hex digits representing the XXH3-128 value, independent of host endianness. Explicit lane kind accompanies content to select its typed decoder.

A common encoder produces immutable `EncodedContent { hash, bytes: Arc<[u8]> }`; hash the exact payload bytes served, not a separately reconstructed value. Framing and publication envelopes are not hashed. A read verifies hash and lane type before adoption. XXH3 is non-adversarial identity, not an authentication mechanism; preserve existing local socket trust protections.

Audit payload fields rather than hashing the current `Snapshot` wrapper wholesale: hub epoch/revision, actor update IDs, publication time and connection bookkeeping belong in provenance/envelopes. Preserve actual collection times, source success times, source errors, process birth times and domain IDs as facts. A new collection time legitimately changes observation identity even when fleet contents match; never hide freshness evidence to manufacture cache hits. Compute display age locally, not as a ticking payload field.

**Why:** MessagePack is portable to pi-radar without a Rust layout contract; XXH3-128 fits noncryptographic identity. JSON is retained for unrelated RPCs, not as a parallel TUI payload implementation. Bincode/Postcard were not chosen because cross-language adoption matters; rkyv adds representation constraints without a demonstrated decode bottleneck.

### 3. Reusable mechanics, typed domain adapters

Extract the smallest common encoding/hash, content-reference/store and bounded frame reader/writer functions into existing appropriate transport/content ownership (a focused shared module is acceptable). Their APIs operate on bytes, hashes and serializable types and must not import navigation, row, observation or interaction types. Use the same functions for all three payloads; no separate lane caches or ad hoc hash helpers. Keep three explicit fields and typed payload producers in the UI-session layer, not an arbitrary lane registry, dynamic dispatch hierarchy or speculative public SDK.

Reuse the same immutable object when inputs did not change. Changed inputs create one encoded candidate; matching retained content reuses its encoded object. Equal values do not automatically share an allocation: dependency reuse precedes encoding, content deduplication follows encoding. Map ordering and field audits make byte identity meaningful.

Prepare the observation payload once per accepted hub snapshot outside the synchronous actor hot loop using the existing feeder boundary. The actor accepts its facts and encoded reference together, derives navigation from those same facts, inserts all content before exposing the new head, and reuses it for unrelated navigation. Do not introduce asynchronous tree building or worker generation protocols without measured need.

**Why:** remove duplicate mechanics while preserving the existing domain ownership. Shared utilities make later adoption possible; speculative adapters for other consumers do not.

### 4. TUI-only binary connections on the existing transport

Keep one ordered TUI action/content connection and one subscription connection. Select a documented TUI mode through a bounded JSON opening request on the existing socket, then switch those connections to four-byte big-endian length-prefixed MessagePack frames. No byte sniffing or second socket server. The opener declares protocol/codec and connection role; success has one documented boundary after which all frames are binary. Requests/results retain UI method/action identities; domain RPCs, compatibility bus and existing observation reads stay JSON. A missing baseline remains bounded and cancellable.

Use one frame I/O implementation for both roles, existing Tokio tasks/runtime and the transport's trust/shutdown/capacity protections. Binary action/content requests take the existing control permit per request, not for their connection lifetime, and retain idle/shutdown checks. They do not consume a subscription permit. Do not put blocking sampling or domain work on its writer. A subscription writer holds at most its in-flight head and the latest replacement; once a frame starts it cannot be overtaken. Encode each payload once and write its shared bytes with bounded envelope/prefix storage rather than rebuilding a combined payload per subscriber.

Baseline delivery includes all requested payloads. Default TUI demand includes all three lanes; a typed lane mask permits a modal presenter to request session payloads only while still receiving the full head references. Updates may inline changed content, using the writer's last delivered references rather than assuming the consumer received every actor publication. Missing content remains fetchable by hash through the ordered content request path. Receipt processing must not synchronously wait for a recovery fetch on the terminal task.

**Why:** two ordinary connections preserve action/subscription separation without migrating unrelated daemon protocols. Inline content avoids a compulsory fetch round trip; fetch remains necessary because coalescing and reconnect can skip earlier payload delivery.

### 5. Current-only consumer with bounded handover

The client owns a displayed head and its required payloads, one latest pending target, and at most one executing materialisation. Reuse unchanged references. The receiver never waits for content: a bounded, scoped materialiser can finish an already-started request while the latest target replaces its predecessor. Fence the old result before adoption, release superseded bytes and adopt only complete heads. There is no queued history. An already-started content RPC shares the ordered link, so cancelling that socket would also destroy unrelated action receipt recovery; bounded completion plus fencing is preferred. Do not add an LRU or per-head frontend history. A presenter that requests only session content needs only that payload to adopt its subscribed view.

Process receipts and gate invalidation independently from complete render adoption. Gate changes clear pending interpreted input and key-sequence state promptly; old stamped actions remain rejected centrally. Publication adoption must integrate with existing optimistic navigation/floor handling rather than discard it or hide gate changes while actions are pending. During content failure preserve the previous coherent display with a local stale/error indication. Identity joins still handle genuine unexplained rows safely; unified delivery removes transport skew, not gaps in daemon knowledge.

**Why:** historical caching has no user benefit here. Atomic adoption is the value of the head; mixing facts from one head with rows from another would recreate the problem under a new codec.

### 6. Explicit bounds, not a throughput inference

Initial limits: 1 MiB per encoded payload, 4 MiB per binary frame including envelope, and 16 MiB / 32 entries of historical server content. Current head payloads remain pinned separately under their individual limits. Bound each client to displayed plus incoming content and bounded socket buffers. Limit encoding while writing into the candidate buffer, not after arbitrary allocation. Check incoming lengths before allocating; validate decoder collection/depth limits or implement equivalent bounded decoding checks. Retain existing write deadlines and subscription capacity.

A payload-size refusal produces a small typed `content_too_large` diagnostic tied to the candidate, leaves the last coherent head available and wakes the consumer without a reconnect loop. A client with no prior head shows an explicit unavailable state. Retry after a newer candidate or explicit reconnect, not continuously against the same rejected one. An expired content fetch returns the latest head for recovery. Slow writes close the affected connection; admitted domain work continues.

**Why:** a 116 KiB tree sample and a two-second deadline do not bound allocation or validate all fleet sizes. The current observation-read limit demonstrates that bounded content is already an owned behavior. Exact limits are documented policy, not hidden allocation assumptions; measure observation-sized frames before tuning them.

### 7. Preserve observation authority and demand

Adapt the existing observation-hub feeder, not collection logic. It supplies both immutable facts and encoded content for the actor's accepted input. Active attachments keep base observation demand; a small typed demand update carries local process-panel requirements. Aggregate with existing non-TUI readers and release on attachment loss. Keep source-current judgement and elapsed-age rendering local. Sampling errors remain freshness changes, not disappearance. No smoothing now; future publication coalescing/smoothing belongs centrally and must preserve failure semantics.

**Why:** deleting `SnapshotFetcher` must not accidentally turn off sampling or permanently enable whole-fleet process collection.

## Risks / Trade-offs

- Large observation frames can delay navigation on the subscription → bound frames, coalesce pending heads, share encoded bytes and measure slow-reader behavior. Chunking, compression and extra connections are not initial solutions.
- Genuine sample-time changes reduce observation hash reuse → retain truthful freshness and reuse across unrelated actor actions; do not promise unchanged hashes for genuinely new observations.
- Noncryptographic collision risk → use 128 bits and trusted local input; never describe the hash as an authentication guarantee.
- Modal input could remain live while content fetch is delayed → immediate gate stamp processing, plus actor gate fencing, independently of render adoption.
- JSON-schema tooling does not define binary byte layout → retain structural DTO tooling where applicable and add byte-level MessagePack fixtures; no migration of existing SDK schemas.

## Migration Plan

1. Land shared primitives and portable fixtures, then wire all three UI payload producers to the same store.
2. Replace TUI framing/client adoption and observation delivery together as one functional cutover; remove superseded row-only content/cache/hash paths and the frontend observation-fetch path. No dual-codec compatibility layer for old TUI builds.
3. Preserve non-TUI contracts with existing fixtures. Update UI docs and reconcile the superseded `shared-ui-session` publication requirements before sync/archive; `sidebar-mode` remains a separate prerequisite inherited by that change.
4. Validate boundedness, gate/backlog safety, adoption, demand release and complete UI workflows. Measure populated observations through sockets and render; do not sum disjoint microbenchmarks and call them end-to-end latency.
5. Rollback is reverting the coherent code/contract change, not retaining a parallel live path. The project has no deployed-state migration obligation for this UI contract.

## Follow-on boundary

`plan.md` records later pi-radar/other-lane use of these primitives. Do not change their codecs, client implementations, event cursors, ingest/presence acknowledgement or lifecycle admission in this change. Reusable code is established by all three TUI payloads and both TUI connection roles, not by wiring an out-of-scope consumer as a demonstration.

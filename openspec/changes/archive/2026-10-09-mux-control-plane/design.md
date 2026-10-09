## Context

`RuntimeProvider` already normalizes inventory, foreground evidence, focus and close. Its Herdr adapter owns CLI/socket transport; managed close/restart separately uses `herdsman-control/v1`. Extensions still call Herdr directly for mux primitives and metadata reporting.

The user's clarified direction is wrapping first: provide a daemon that abstracts the mux, then grow agent/session lifecycle awareness and topology over it. The initial draft coupled this seam to a registry, inferred resume commands and lead recovery. Those promises are removed, not prerequisites for the wrapper.

The ownership vocabulary is explicit: **Pi supplies execution evidence; Herdsman
supplies assignment authority; the coordinator supplies backend-independent
physical observations and controls.** The daemon implements that coordinator seam
in this change, not the other two authorities. Pi's upstream OSC 7501 Program status
is the future execution evidence source; its support and capture are planned in
`plan.md`, not implemented by this mux-wrapper slice.

## Goals / Non-goals

Provide a usable local mux control plane for Radar and extension clients. Keep Herdr wire details behind one backend, prove the normalized interface with a fake backend, and preserve existing lifecycle behavior.

No new assignment authority, session registry, recovery protocol, guessed launcher, automatic restart, parent/child topology or production tmux backend in this change. A future tmux backend implements the same primitives and declares unsupported capabilities.

## Decisions

### 1. One binary, a separate daemon command

`radar daemon` serves without starting the TUI. Clients do not implicitly start it. This avoids a new package output and surprising long-lived processes. A separate executable could be added later if packaging warrants it.

### 2. Local versioned transport

Use newline-delimited JSON over a Unix socket. Resolve `RADAR_CONTROL_SOCKET`, then `$XDG_RUNTIME_DIR/agent-radar/control.sock`, then `/tmp/agent-radar-<uid>/control.sock`. Socket and state directories must be user-owned, private and non-symlinked; reads and connection counts are bounded. Requests and responses identify protocol version and request ID. Unknown versions/methods refuse explicitly.

State lives under `RADAR_CONTROL_STATE`, then `$XDG_STATE_HOME/agent-radar/control`, then `~/.local/state/agent-radar/control`. Atomic private records survive daemon exits. HTTP and file-only request transport add no benefit here.

### 3. Reuse the runtime model; wrap concrete consumers

Start with existing normalized inventory/foreground evidence/focus/close types. Add creation/split, literal text/key input, bounded output reads and metadata reporting only with their real protocol consumers. Publish explicit capabilities rather than a generic Herdr RPC passthrough. Herdr-specific requests, response decoding and socket discovery stay inside its adapter. Existing transport helpers should be reused, not copied.

A fake backend exercises the operation layer without Herdr. A live disposable Herdr surface proves adapter behavior. tmux comes later; the interface does not claim a tmux implementation exists.

### 4. Durable effectful requests, ordinary reads

Inventory, foreground evidence, output reads, ping and record lookup are reads, with no operation record. Focus, close, creation, input and metadata writes are recorded once accepted and before dispatch. Over-capacity refusal occurs before admission, creates no record and explicitly says not accepted; it cannot dispatch. A repeated ID returns its record, never re-executes; a reused ID with different contents refuses. Compare full JSON parameter values, method, target and requester, not a hash whose equality can collide. Private operation records may therefore contain literal terminal input/metadata; this is documented, not presented as redacted.

Conflicting mutations on an overlapping target serialize or refuse across methods, not merely per target-and-method. A daemon-wide mutation lane is acceptable initially and avoids inventing an elaborate scheduler. Execution and reply work must be bounded and joined at shutdown; bounded connection workers may execute requests synchronously rather than spawning a detached thread per line.

Persist execution-started before handing an effect to the backend. Completed/refused records identify actual effects; a transport failure after dispatch or crash after started is unknown. Unknown remains suppressed and is never replayed. Expired unstarted work is not executed. A client disconnect does not cancel accepted execution.

These semantics avoid an automatic retry closing a replacement pane or creating a second tab. They are not a claim of atomicity between backend observation and mutation.

### 5. Existing close safety stays at both boundaries

Radar retains its default-Cancel confirmation and frozen identities. The daemon re-observes immediately before close, compares the expected identity and applies the existing positive-unmanaged and whole-container containment checks. Merely naming a managed run is not authorization to close it directly. Managed close/restart remains on the existing owner transport; no fallback from an owner refusal into daemon close or launch.

Input/create are explicit mux primitives for trusted local clients, not advertised as agent restart or recovery. Radar does not expose a new restart action in this change.

### 6. Reporting is a bridge, not a registry

Provide backend-neutral wrappers for state/session/display-metadata reporting needed by extension clients. Preserve caller source, sequence, TTL and reported fields through the Herdr adapter, declaring reporting unsupported on backends without it. Do not invent authority, merge reports into a new store or derive parent/child topology. A daemon-owned registry can later replace this bridge.

### 7. Fallback is chosen before operations, never after ambiguous dispatch

Radar keeps the direct Herdr runtime as an explicitly selected mode and startup fallback. If a configured daemon cannot pass its handshake, report that choice and select the direct adapter before any actions. Once using the daemon, failure remains a diagnostic; do not retry focus/close/input through direct Herdr after an uncertain response. Runtime backend selection must not silently switch the fleet underneath the operator.

### 8. Process launch is not recovery

No command is derived from executable plus session path in this wrapper. Launchers may encode variants, arguments and environment that those observations omit. A later explicit relaunch primitive needs a named launch surface and command, verified old-process exit and honest partial effects. Even then a new PID or repeated session UUID does not establish recovery of children, assignments or pending results.

## Risks / Trade-offs

Private local clients can send terminal input; document that capability plainly. Never exercise destructive primitives against the live fleet in verification: use fake backends and disposable panes only. Socket loss after accepted execution is unknown, not cancellation. Backend snapshot and mutation are not atomic; fresh checks reduce but do not eliminate races.

Durable records need bounded listing and an operator-visible way to inspect unknown requests. Avoid automatic pruning of unresolved records. Reporting remains Herdr-backed during migration; that limitation is explicit, not hidden by the interface.

## Migration Plan

1. Complete the existing daemon/protocol/store slice and verify it independently.
2. Add Herdr backend observation and focus through the real socket.
3. Add guarded close, creation/input/output and the reporting bridge, with request records and disposable tests.
4. Add Radar client selection without changing managed lifecycle eligibility.
5. Publish the versioned protocol/reference and hand pi-extensions a usable mux port.
6. Grow tmux support, session awareness, recovery and topology in subsequent changes as actual consumers require them.

## Decisions to record

The initial scope was reduced after the user's clarification: wrapping first, lifecycle authority later. A transport seam does not imply recovery or assignment authority. The chosen alternative ships useful primitives now rather than blocking them on a speculative registry/recovery model.

## Context

See proposal.md. collector::collect currently runs Herdr snapshot/process-info commands and decodes them. focus::focus uses workspace CLI focus or discovers Herdr's socket and sends pane.focus. main constructs these workers independently. Radar already has normalized FleetObservation, ForegroundEvidence and workspace/pane focus targets; its App produces those targets without wire operations.

## Goals / Non-Goals

**Goals:** move mux knowledge to one adapter and make the collector/focuser consume the same small interface. Preserve observable behavior, including failure and shutdown paths.

**Non-Goals:** no close/restart operations, owner-control transport, plugin discovery, alternate production mux, remote process namespace, general event bus or new dependency. Do not change the public configuration file or keys.

## Decisions

1. **One interface, three current operations.** Define a Radar-owned RuntimeProvider for inventory, per-pane foreground evidence and focus. Return existing normalized observation/evidence/target values and diagnostics, never raw JSON or CLI stdout. Use it in both current worker paths. A raw-command executor interface was rejected: it would leave callers dependent on Herdr names and schemas.
2. **Herdr owns its transport.** Keep snapshot/process decoders, executable configuration, CLI execution, socket discovery and pane.focus request/answer inside the Herdr adapter. Reuse the current bounded runner, socket budget, cancellation and child cleanup rather than layering another implementation around them. Existing polling and background-thread coordination remain in Collector/Focuser. Local /proc facts remain a separate OS reader, composed into normalized foreground evidence.
3. **Inject at executable assembly.** main supplies the Herdr adapter to the collector and focuser. The same seam accepts a deterministic fake in core tests. Do not add a source-selection option or registry when Herdr is the only production adapter; adding a second mux later is an adapter/assembly change, not a rewrite of App/tree/UI.
4. **Adapter-specific configuration stays with the adapter.** Preserve the default executable and current command/socket timing semantics. Poll scheduling belongs to the collector; Herdr executable/socket knowledge does not. Adapt internal constructor/test configuration only where needed, with no redundant compatibility wrapper lacking a current caller.
5. **Extend only when an action consumes it.** Pane/tab close will extend this runtime seam during lifecycle implementation. Managed close/restart belongs to a separate owner-control interface; do not add unused method stubs, capability tables or empty owner modules here.

## Risks / Trade-offs

- [Moving the runner changes subprocess cleanup or focus timing] → preserve its existing budgets and cancellation checks; retain fake-executable and stub-socket timeout/shutdown tests at the adapter seam.
- [A trait only wraps existing Herdr-specific callers] → route actual collection and focus through normalized provider operations, and use a fake provider without Herdr CLI or socket fixtures in core tests.
- [Tests duplicate transport behavior above and below the seam] → core tests cover normalized refresh/focus behavior; adapter tests cover wire parsing and real cancellation/transport failures. Move coverage rather than blindly adding another full matrix.
- [Accidental wider refactor] → no changes to source reconciliation, task joining, activity derivation, tree identity or presentation. The existing spec suite remains the behavior contract.

## Landed slice: collection through the seam

`RuntimeProvider` (`src/runtime.rs`) has two operations, both in Radar's
normalized values: `inventory` and `foreground_evidence`. `HerdrRuntime`
(`src/herdr.rs`) is the only production implementation, and owns what only
Herdr needs: the executable and command deadline (`HerdrConfig`), the CLI
arguments, the decoders and the bounded runner that kills and reaps a stalled
command. `Collector` owns only the poll schedule — one refresh in flight,
cancellation that abandons rather than waits, the assignment-age stamp, and the
local `/proc` facts composed onto the evidence the provider returned. It names
no executable, no argument and no wire shape; `tests/collector.rs` drives it
with an in-memory fake, and the fake-executable transport checks moved to the
adapter in `tests/runtime.rs`. Focus reaches the adapter's runner directly only
until the focus slice below.

## Landed slice: focus through the same seam

`RuntimeProvider` gains a third operation, `focus`, taking the normalized
workspace or pane `Target` the view already produces and returning the same
one-line refusal or failure message a user reads today. `HerdrRuntime` owns both
focus transports: `herdr workspace focus` for a workspace, and `herdr status
--json` for the socket plus the `pane.focus` request and its answer for a pane,
along with the socket budget and child cleanup. `src/focus.rs` keeps only the
off-thread worker, its cancellation and the message plumbing, and names no
command, socket or wire shape. `Focuser` takes the same provider the collector
does, and assembly shares one adapter handle with both. Core focus tests drive
an in-memory provider in `tests/focus.rs`; the stub-socket and hung-CLI
deadline, cancellation and no-delayed-shutdown checks moved to the adapter in
`tests/runtime.rs`. Managed-agent lifecycle — close and restart — is a separate
owner-control interface, not an operation stubbed on this seam.

## Migration Plan

Land collection through the seam first, then focus through the same adapter and update main assembly. Every intermediate slice must compile and keep a production caller; no speculative framework lands ahead of it. Use thin sequential assignments on the retained worker, with the owner checking each landing. The final gate runs serially to avoid the recorded timing-test contention. Rollback restores the previous internal wiring; source formats, files and operator controls do not migrate.

## Why

Radar's collector and focuser currently know Herdr commands and socket messages. Extract that knowledge before adding lifecycle controls, so core observation and interaction depend on a small Radar-owned runtime interface rather than one mux's transport.

## What Changes

- Put inventory, foreground evidence and focus behind a runtime-provider interface using Radar's existing normalized facts and focus targets.
- Make Herdr the first production adapter; keep its CLI arguments, JSON decoding, socket discovery and messages inside it.
- Inject the provider into the existing off-thread collection/focus paths and test those paths with a small in-memory fake.
- Preserve polling, timeouts, cancellation/reaping, freshness, pane continuity and every current key/action. Keep managed-agent lifecycle separate from mux operations.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is a behavior-preserving refactor, marked `skip_specs: true`; existing specs remain the acceptance contract.

## Impact

The runtime interface and Herdr adapter, `src/collector.rs`, `src/focus.rs`, executable wiring and transport/core tests. No new dependency, plugin loader, second production mux, source-selection setting, destructive action, owner mailbox or speculative close/restart method. Pane/tab actions will extend the same seam when their lifecycle change has a real consumer.

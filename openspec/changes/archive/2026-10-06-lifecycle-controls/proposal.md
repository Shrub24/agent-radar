## Why

Radar can locate work but cannot retire an unused pane or restart an idle managed worker. Add confirmed lifecycle actions without bypassing the runtime provider or Herdsman's ownership checks.

## What Changes

- `x` requests close of the selected agent/pane; `X` requests close of its tab; `r` requests restart of an idle managed worker. Task and workspace rows do not acquire destructive shortcuts.
- A separate confirmation names the exact target and what can be lost. Opening it sends nothing; Cancel is initially selected.
- Positively unmanaged pane/tab close crosses the runtime-provider seam. Managed close/restart uses `herdsman-control/v1`, never direct process control.
- Refuse stale inventory, uncertain containment, unsupported restart targets and changed confirmation targets. A managed or uncertain tab is refused whole.
- Show file-derived request outcomes without blocking collection, input or quitting; never retry a claimed request or treat a client timeout as cancellation.
- Keep managed actions unavailable until the extension owner confirms implementation acceptance, then require trusted owner-created directories at runtime.

## Capabilities

### New Capabilities
- `lifecycle-actions`: confirmed operator close/restart, routing, containment and outcome handling.

### Modified Capabilities
- `focus-action`: remove the old focus-only action restriction; ordinary Enter still focuses and sends no lifecycle request.

## Impact

Extend `src/runtime.rs` and `HerdrRuntime` for pane/tab close only. Add a separate owner-control client and off-thread action worker, with UI confirmation and result presentation. Normalize managed-presence evidence if the current decoder discards it. Use the upstream contract and fixture; no bus control operations or extension changes. Tests use fake providers, trusted/untrusted temp directories and a stub owner. No real pane is closed during automated checks.

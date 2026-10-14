# Design

## Decisions

### 1. Adopt on the daemon's own timer

The sweeper already runs every two seconds without a client. Adoption reads the inventory in that pass, so containers exist before any TUI asks and several clients agree. It is edge-triggered: a signature of the verified incarnation and the place handles is compared with the last pass, and an unchanged server costs one read and no write. A failed read keeps the old signature, so the next pass retries. `tmux-control-mode` replaces the timer later without changing adoption.

### 2. Adopt only under a verified incarnation

A handle alone cannot be told from a later server reusing it, so with no verified serving identity nothing is adopted and the signature changes when one appears. Parents resolve within the same incarnation, so a tab's container names its workspace's.

### 3. Provenance stays truthful

Adopted containers are `observed`. They carry no recipe, so reload keeps refusing, and they are never reported as created by Radar. Delete keeps its default-Cancel confirmation and refuses to end a live registered worker.

### 4. Why a focus can no longer block

Unknown means an effect may have happened and must not be replayed. A focus changes no durable state, so its unknown outcome protects nothing and held a workspace against every later creation. The store's unresolved search skips `mux.focus`. The tmux adapter refuses before any command when `list-clients` is empty, and the TUI will use that exact refusal to attach (`tui-launch-and-attach`).

### 5. Scope by pane, not by lease

An earlier draft kept an unplaced registration while its publisher held a live lease. That kept sessions from other servers visible, so visibility is now only: the reported pane is in this inventory, or the daemon has a spawn edge for the registration.

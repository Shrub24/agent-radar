# Design

## Decisions

### 1. Why not keep capabilities for the TUI

The TUI and the daemon live in this repository and ship together, so the TUI can assume the set it was built against. The only gap is a daemon that outlives an install. A single revision handles that with one comparison, a clear diagnostic and no per-name state.

### 2. Why contracts stay for external consumers

Herdsman and library users release separately. They need to ask "does this daemon serve X" without a version table. The contract list is that question, and it remains small because it names only what consumers pin.

### 3. Typed contracts and primitives

`enum Contract` carries `#[serde(rename = "spawn.v1")]` for each name and an `Unknown` variant for names this build does not know, which are tolerated and never credited. `Contract::needs(self)` is an exhaustive `match` returning a `Primitives` flag set, so adding a variant fails to compile until its needs are stated. The backend returns `Primitives` by value. The advertised set is the contracts whose needs the backend meets, plus flags such as the task lane's availability.

### 4. Order of work

Revision and contract list first (wire and clients), then the typed internals, then the test backends. The last step is the largest and mostly mechanical: the fakes in `tests/control_plane.rs` declare primitive slices today.

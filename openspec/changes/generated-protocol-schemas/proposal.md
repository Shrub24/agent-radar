# Proposal

## Why

The extracted `pi-radar` SDK and Rust daemon describe the same wire payloads independently, so a compatible Rust change can still make the TypeScript client stale. Generate a portable schema from the Rust-Serde wire types and validate the SDK's generated declarations against it, beginning with a small public subset and an explicit integer policy.

## What Changes

- Add Schemars-derived, directional JSON Schemas for the public daemon handshake, facts snapshot, and spawn-exit observation DTOs consumed by `pi-radar`.
- Generate TypeScript declarations from those schemas for the corresponding SDK types, checking in both artifacts and a deterministic drift check.
- Audit exposed integer fields: keep small/bounded values as JSON numbers, constrain the context counts and timestamp to JavaScript's exact integer range where the existing SDK requires exact numbers, and record high-lifetime counters that need a future decimal-string wire change without changing them in this slice.
- Keep Rust's domain validators authoritative; generated JSON Schema describes structure and wire representation, not lifecycle or cross-field execution rules.
- **BREAKING only if required by the audit:** do not silently convert existing protocol integers to strings; any such wire change needs its own revision and consumer migration. This slice preserves existing encodings.

## Capabilities

### New Capabilities

- `public-protocol-schemas`: machine-readable directional schemas and generated SDK types for the selected public DTOs, with a reproducible drift check and explicit integer mapping.

### Modified Capabilities

None. This adds build-time contract artifacts; it does not change RPC semantics or lifecycle behavior.

## Impact

- Rust wire DTOs and validation in `src/control_plane/registry/publication.rs`, `src/control_plane/registry/spawn.rs`, `src/control_plane/ops.rs`, and the ping response path in `src/control_plane/server.rs`.
- Schema generation tooling and checked artifacts under `schema/` and `pi-radar/`.
- `pi-radar` package scripts, generated TypeScript declarations, and tests. No runtime dependency is added; Schemars and schema-to-TypeScript tooling are development/build dependencies.
- Canonical fixtures and documentation may be updated to describe the exact integer mapping.

Non-goals: exporting every RPC method or private Rust type; replacing Serde or the JSON-RPC transport; changing unrelated `u64` fields; migrating the active `pi-extensions`/Herdsman checkout; or expanding lifecycle architecture work.

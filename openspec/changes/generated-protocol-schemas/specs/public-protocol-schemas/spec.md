# public-protocol-schemas Specification

## Purpose

Keep selected public daemon wire shapes and the extracted TypeScript SDK synchronized through portable, generated contract artifacts.

## ADDED Requirements

### Requirement: Public DTOs have generated directional schemas

Radar SHALL generate JSON Schemas for the selected public DTOs from their Rust Serde types. For each DTO, the generated deserialize schema SHALL describe accepted incoming JSON and the generated serialize schema SHALL describe JSON emitted by Radar; optional fields, defaults, omission, nullability, tagged unions and integer representation SHALL match the actual wire behavior. The initial selected DTOs are the daemon ping result, the `registry.facts.v1` snapshot and the spawn-exit observation.

#### Scenario: Optional facts are omitted on output
- **WHEN** a facts snapshot omits an optional value
- **THEN** its serialize schema permits the field to be absent and does not invent a default value

#### Scenario: Tagged spawn exit is represented
- **WHEN** an SDK consumer reads an exit observation
- **THEN** generated contracts distinguish the `unknown` and `exited` `state` variants and describe the fields of each variant

### Requirement: Generated SDK types follow the selected schemas

The `pi-radar` package SHALL generate TypeScript declarations from the checked public JSON Schemas for the selected DTOs. Its client-facing handshake, facts and spawn-exit type declarations SHALL consume those generated types rather than maintain structurally separate copies. Generated artifacts SHALL be reproducible and a repository check SHALL fail when regeneration changes checked-in output.

#### Scenario: Rust DTO changes
- **WHEN** a selected Rust DTO changes its serialized shape
- **THEN** schema regeneration updates the corresponding contract artifact and the generated TypeScript declarations, and the drift check detects stale checked-in artifacts

#### Scenario: Generated TypeScript is exercised
- **WHEN** the package test suite runs
- **THEN** representative fixtures for optional facts, handshake data and both spawn-exit variants satisfy the generated declarations and match the daemon's canonical JSON fixtures

### Requirement: Selected TypeScript integers are exact

Selected context counts and the facts timestamp SHALL remain JSON numbers bounded by JavaScript's safe-integer maximum. Rust SHALL continue enforcing context-window domain invariants. The initial schema set SHALL NOT convert unrelated high-lifetime counters to strings.

#### Scenario: Unsafe context integer is refused
- **WHEN** a facts snapshot contains a selected context count or timestamp greater than JavaScript's safe-integer maximum
- **THEN** the daemon refuses the snapshot and preserves the previous accepted snapshot

#### Scenario: Domain-invalid but safely represented context is refused
- **WHEN** a context count is within JavaScript's safe-integer range but its window is zero or tokens exceed the window
- **THEN** existing Rust domain validation refuses it independently of the generated structural schema

### Requirement: Schemas do not replace domain validation

Generated schemas SHALL describe structural wire shapes and numeric representation constraints only. Rust SHALL retain authoritative cross-field, lifecycle, authorization, fencing and state-transition validation; clients MAY use generated schemas for structural validation but SHALL NOT treat schema conformance as authorization or proof of domain validity.

#### Scenario: Structurally valid request violates a domain rule
- **WHEN** JSON matches a generated structural schema but violates an existing Rust domain invariant
- **THEN** the daemon continues to refuse it using the existing domain validation

### Requirement: Public schema generation excludes private protocol state

Only explicitly selected public DTOs SHALL be exported. Private store records, secrets, launch arguments, internal journal types and unselected request/response shapes SHALL NOT enter generated schemas merely because they implement Serde.

#### Scenario: Private fields are added
- **WHEN** an internal record gains a field not present on the public DTO
- **THEN** no public schema changes unless the selected wire representation changes

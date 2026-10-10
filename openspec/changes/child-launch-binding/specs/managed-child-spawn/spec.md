## ADDED Requirements

### Requirement: Spawn preserves explicit launch context
A spawn SHALL accept an optional absolute working directory and an environment overlay of UTF-8 name/value pairs. Supplied values SHALL reach the child unchanged; omitted fields SHALL retain existing behavior. Overlay values SHALL override inherited values. The daemon SHALL supply its own spawn token and refuse caller attempts to set that reserved variable.

#### Scenario: Herdsman child launch context
- **WHEN** a spawn supplies a cwd and mailbox, owner, run and role environment values
- **THEN** the child observes that cwd and those exact values alongside the daemon-supplied private token

#### Scenario: Old request omits launch context
- **WHEN** a compatible spawn request omits cwd and environment
- **THEN** launch retains the existing inherited environment and placement-derived cwd behavior

#### Scenario: Reserved token override
- **WHEN** a caller supplies PI_RADAR_SPAWN_TOKEN in its environment overlay
- **THEN** admission refuses without accepting the request or creating a pane

### Requirement: Launch context is representable before creation
The daemon SHALL validate launch-context shape, bounds and selected-backend representability before accepting a spawn or creating a pane. Unsupported cwd or environment transport SHALL refuse explicitly, never silently drop supplied fields. Herdr SHALL refuse newline or terminal-control values that its single-line launch transport cannot represent.

#### Scenario: Unrepresentable environment value
- **WHEN** a Herdr-backed spawn supplies an environment value containing a newline
- **THEN** the spawn refuses before creation and does not leave a pane or accepted operation

#### Scenario: Shell-sensitive values
- **WHEN** supported launch values contain spaces, quotes, dollar signs or command-substitution syntax
- **THEN** the child receives those values literally and no embedded shell commands execute

### Requirement: Topology names the exact child registration
A new token-bound spawn SHALL report the exact bound child agent_id alongside existing source/incarnation provenance. Identical registration replay SHALL preserve that binding. Legacy records without agent_id SHALL remain readable without inferred identity. A rename, shared session or similar label SHALL NOT change the binding.

#### Scenario: Binding readback
- **WHEN** a child successfully registers with its spawn token and the caller reads spawn.get
- **THEN** the response names the same agent_id returned to that registration

#### Scenario: Legacy binding
- **WHEN** a legacy bound edge without agent_id is read
- **THEN** its recorded source/incarnation remains available and no agent_id is guessed

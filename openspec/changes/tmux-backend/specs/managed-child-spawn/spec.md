## ADDED Requirements

### Requirement: Spawn topology reports exit observation
Spawn topology reads SHALL include a physical exit observation field for each edge: unknown, or `exited` with code or signal and observed time. Backends that cannot observe exit SHALL report unknown, and legacy edges SHALL omit the field without inference.

#### Scenario: Herdr child
- **WHEN** a Herdr-launched child's edge is read
- **THEN** its exit observation is unknown

#### Scenario: tmux child exited
- **WHEN** a tmux-launched child has exited
- **THEN** the edge reads exited with its code or signal, once and stably across restarts

## ADDED Requirements

### Requirement: Spawn topology reports exit observation
Spawn topology reads SHALL include `exit_observation` for each current edge as `{state:"unknown"}` or `{state:"exited", code|signal, observed_at}`. Backends that cannot observe exit SHALL report unknown, and legacy edges SHALL omit the field without inference.

#### Scenario: Herdr child
- **WHEN** a Herdr-launched child's edge is read
- **THEN** its exit observation is unknown

#### Scenario: tmux child exited
- **WHEN** a tmux-launched child has exited
- **THEN** the edge reads exited with its code or signal, once and stably across restarts

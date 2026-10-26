# Spec Delta

## ADDED Requirements

### Requirement: Panel sizing and focus remain local and legible
A dashboard SHALL provide a discoverable local control for its tree/details split, clamp it after terminal resize and visibly identify the keyboard-focused panel. Hiding details SHALL return focus safely to the tree. Split changes SHALL NOT change the canonical fleet selection, another frontend's layout or the private state of a detached presenter.

#### Scenario: Resize one dashboard
- **WHEN** one dashboard adjusts its details split while another sidebar is attached
- **THEN** its columns resize without changing the sidebar, shared selection or detail focus in another frontend

#### Scenario: Terminal becomes narrow
- **WHEN** the terminal cannot display the requested split
- **THEN** geometry is clamped or the existing narrow presentation is used, without invisible keyboard focus or lost access to details

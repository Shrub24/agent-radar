## MODIFIED Requirements

### Requirement: Workspace-grouped fleet tree

Radar SHALL provide a standalone full-screen tree grouped by the daemon's logical workspace containers rather than by a backend's own workspace ids. A container row SHALL carry its durable identity and its backend binding as separate facts, so the binding's backend, serving instance and handle are shown as detail rather than as the row's identity. Radar SHALL nest agents using unambiguous explicit ownership within a container, without requiring a shared tab. Unresolvable or cyclic ownership SHALL leave agents visible without unsafe nesting. A container Radar did not create SHALL appear as unmanaged.

#### Scenario: Owner and worker in different tabs

- **WHEN** an owner and worker share a container and have an explicit ownership link but occupy different tabs
- **THEN** the worker appears beneath the owner and its tab remains available as location detail

#### Scenario: Ownership cannot be resolved

- **WHEN** ownership is missing, ambiguous, cyclic or references an owner outside the container
- **THEN** affected agents remain visible in their container without fabricated nesting

#### Scenario: Provider prefix in a reported title

- **WHEN** a reported title begins with a provider mark or name followed by a separator
- **THEN** the row shows the title with that prefix removed, because the row already carries the provider's mark
- **AND** a title that would be emptied by the removal is shown unchanged

#### Scenario: A container's binding is unresolved

- **WHEN** a container's backend binding cannot be verified, or its handle now exists under a different serving instance
- **THEN** the row stays visible, marks its location as unresolved and offers no destructive action for it

#### Scenario: An unmanaged container

- **WHEN** a workspace, tab or pane exists that Radar did not create
- **THEN** it is shown as unmanaged with its own identity and location, and no reload is offered for it

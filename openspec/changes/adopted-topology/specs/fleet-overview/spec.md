## MODIFIED Requirements

### Requirement: Workspace-grouped fleet tree

Radar SHALL provide a standalone full-screen tree grouped by workspace. It SHALL nest agents using unambiguous explicit ownership within a workspace, without requiring a shared tab. Unresolvable or cyclic ownership SHALL leave agents visible without unsafe nesting. Radar SHALL show only what is inside the server it serves: a registration of another backend or server, and a registration whose reported pane is not in the inventory and for which the daemon holds no spawn edge, SHALL NOT be shown.

#### Scenario: Owner and worker in different tabs
- **WHEN** an owner and worker share a workspace and have an explicit ownership link but occupy different tabs
- **THEN** the worker appears beneath the owner and its tab remains available as location detail

#### Scenario: Ownership cannot be resolved
- **WHEN** ownership is missing, ambiguous, cyclic or references an owner outside the workspace
- **THEN** affected agents remain visible in their physical workspace without fabricated nesting

#### Scenario: Provider prefix in a reported title
- **WHEN** a reported title begins with a provider mark or name followed by a separator
- **THEN** the row shows the title with that prefix removed, because the row already carries the provider's mark
- **AND** a title that would be emptied by the removal is shown unchanged

#### Scenario: Window with one root agent
- **WHEN** a tab holds a root agent and other agents or panes
- **THEN** the root agent's row is the tab and the others appear beneath it, with the tab name shown as its tab label

#### Scenario: Window with several root agents
- **WHEN** a tab holds more than one agent with no owner in that tab
- **THEN** the tab has no single identity, and its agents and panes remain flat beneath its workspace

#### Scenario: A session of another server
- **WHEN** a session registered a pane of a different tmux server
- **THEN** it is not shown

#### Scenario: A registration naming no pane here
- **WHEN** a registration names no pane, or one the inventory lacks, and no spawn edge exists for it
- **THEN** it is not shown and no unplaced group appears

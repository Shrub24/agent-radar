## MODIFIED Requirements

### Requirement: Workspace-grouped fleet tree

Radar SHALL provide a standalone full-screen tree grouped by workspace. Each window of a workspace SHALL be drawn as a tab whose identity is its root agent: the one agent in that window that has no owner in the same window. The other agents and ordinary panes of that window SHALL nest beneath that root agent. An owner link across windows SHALL NOT be used for nesting. A window with no root agent, or with more than one, SHALL keep the flat placement beneath its workspace. Unresolvable or cyclic ownership SHALL leave agents visible without unsafe nesting.

#### Scenario: Owner and worker in different tabs
- **WHEN** an owner and worker share a workspace but occupy different windows
- **THEN** the worker is not nested under the owner and is grouped by its own window, so the tree never fabricates a cross-window parent

#### Scenario: Window with one root agent
- **WHEN** a window holds a root agent and other agents or panes
- **THEN** the root agent's row is the tab and the others appear beneath it, with the window name shown as its tab label

#### Scenario: Window with several root agents
- **WHEN** a window holds more than one agent with no owner in that window
- **THEN** the window has no single identity, and its agents and panes remain flat beneath the workspace

#### Scenario: Ownership cannot be resolved
- **WHEN** ownership is missing, ambiguous, cyclic or references an owner outside the workspace
- **THEN** affected agents remain visible in their window without fabricated nesting

#### Scenario: Provider prefix in a reported title
- **WHEN** a reported title begins with a provider mark or name followed by a separator
- **THEN** the row shows the title with that prefix removed, because the row already carries the provider's mark
- **AND** a title that would be emptied by the removal is shown unchanged

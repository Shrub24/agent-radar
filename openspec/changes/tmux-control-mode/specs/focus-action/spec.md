## ADDED Requirements

### Requirement: TUI selection and terminal focus are separate
A TUI focus invocation SHALL explicitly identify its locally selected row. Browsing and inspecting details SHALL NOT execute focus. Available observed terminal focus SHALL be indicated independently of selection. A focus request SHALL NOT publish its intended destination as observed success before confirming state through a read.

#### Scenario: Inspect elsewhere
- **WHEN** the user selects B while tmux displays A
- **THEN** details describe B, the focus marker identifies A, and tmux remains at A

#### Scenario: Enter requests focus
- **WHEN** Enter invokes focus on selected B outside filter editing or confirmation
- **THEN** the command names B and the existing focus execution and no-client recovery rules apply

#### Scenario: Focus fails
- **WHEN** the backend refuses focus on B
- **THEN** observed focus is not changed to B and the failure is reported

### Requirement: Missing row means observed focus for row-dependent commands
A row-dependent command with an explicit row SHALL validate and freeze that identity. With no row supplied it SHALL resolve and freeze the current observed focused place. Invalid explicit rows SHALL NOT fall back to focus. No selected row in a TUI SHALL be reported locally rather than accidentally invoking omitted-row semantics. Commands without row targets SHALL remain targetless.

#### Scenario: Selected row differs from focus
- **WHEN** a TUI invokes a row-dependent command with selected B while focus is A
- **THEN** the actor validates B, not A

#### Scenario: Intentional implicit invocation
- **WHEN** a caller omits a row for a row-dependent command and current focus is available
- **THEN** the actor resolves that focused place through command eligibility and freezes its target

#### Scenario: No eligible focus
- **WHEN** a row-dependent command omits a row but focus is unavailable, stale, ambiguous or has no eligible target
- **THEN** the actor refuses without guessing a row

#### Scenario: No local selection
- **WHEN** a TUI has no selected row and the user invokes a row-dependent action
- **THEN** it reports no selection instead of silently acting on focus

# Spec Delta

## MODIFIED Requirements

### Requirement: Captured intent cannot be retargeted by shared selection
Every row-dependent invocation, including Delete, Restart, Focus and configured Launch, SHALL identify the row displayed by its originating frontend. The actor SHALL resolve that identity against its current canonical tree, derive eligibility and freeze the command's target, identities and relevant preconditions. Existing domain guarantees SHALL revalidate execution preconditions. A stale, absent or invalid target SHALL refuse without substituting the current shared selection or a different row at the same position. Configured Launch SHALL retain frontend-supplied command/environment preparation without deriving its target from execution-time shared selection.

#### Scenario: Another frontend changes selection
- **WHEN** a frontend invokes a row-dependent command after another frontend selected a different row
- **THEN** the command is validated against the explicitly supplied row or refused, never redirected to the new global selection

#### Scenario: Rapid navigation before command
- **WHEN** an optimistic move to B is followed immediately by Delete, Restart, Focus or configured Launch while publications are delayed
- **THEN** each command identifies B and is ordered after the move, rather than selecting its target by a different command-specific rule

#### Scenario: Target disappears
- **WHEN** the explicitly named row is absent from the actor's current canonical tree
- **THEN** the command refuses without acting on a neighbour that now occupies its former screen position

#### Scenario: Replacement before confirmation
- **WHEN** execution identity changes while confirmation is open
- **THEN** domain revalidation refuses the incompatible frozen target rather than treating the replacement as the original subject

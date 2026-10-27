# Spec Delta

## MODIFIED Requirements

### Requirement: Jump to the next row of a kind

Radar SHALL move shared selection to the next or previous visible row in a kind, wrapping at the ends: default bindings n/N for attention (blocked, lost, waiting, unknown) and w/W for working. Frontends SHALL submit semantic jumps; the session SHALL evaluate them against its canonical filtered/folded ordering and publish the resulting identity.

#### Scenario: Attention
- **WHEN** n is pressed and rows exist after the selection that are blocked, lost, waiting or unknown
- **THEN** shared selection moves to the nearest of them, skipping working, idle and done rows

#### Scenario: Working
- **WHEN** w is pressed
- **THEN** shared selection moves to the nearest working row after it, skipping rows of every other state

#### Scenario: Wrapping
- **WHEN** no row of the wanted kind lies after the selection
- **THEN** the search continues from the top of the list and selects the first such row

#### Scenario: Repeating
- **WHEN** the key is pressed again without moving otherwise
- **THEN** shared selection advances to the next row of that kind

#### Scenario: Different terminal dimensions
- **WHEN** a sidebar invokes a jump while a dashboard shows the same session
- **THEN** both receive the same selected identity even though their viewport sizes differ

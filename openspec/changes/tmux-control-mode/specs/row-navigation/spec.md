## MODIFIED Requirements

### Requirement: Jump to the next row of a kind
Radar SHALL move only the invoking frontend's selection to the next or previous visible row in a kind, wrapping at the ends: n/N for attention (blocked, lost, waiting, unknown) and w/W for working. Jumps SHALL use the canonical filtered/folded ordering and SHALL NOT request terminal focus or move another frontend's selection.

#### Scenario: Attention
- **WHEN** n is pressed and attention rows exist after this view's selection
- **THEN** only this view selects the nearest qualifying visible row

#### Scenario: Working
- **WHEN** w is pressed with a working row after this view's selection
- **THEN** this view selects the nearest working row, skipping every other state without switching terminals

#### Scenario: Wrapping
- **WHEN** no row of the wanted kind lies after this view's selection
- **THEN** this view selects the first qualifying visible row from the top without switching terminals

#### Scenario: Repeating
- **WHEN** the jump key is pressed again
- **THEN** this view advances to the next qualifying row

#### Scenario: Independent views
- **WHEN** one frontend jumps while another inspects a different row
- **THEN** the other frontend's selection and observed terminal focus remain unchanged

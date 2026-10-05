# Spec Delta

## Purpose

Reach the row that matters without scrolling the whole fleet.

## ADDED Requirements

### Requirement: Jump to the next row of a kind

Radar SHALL move the selection to the next or previous row in a kind, wrapping at the ends: `n` and `N` for rows needing attention (blocked, lost, waiting, unknown) and `w` and `W` for working rows.

#### Scenario: Attention
- **WHEN** `n` is pressed and rows exist after the selection that are blocked, lost, waiting or unknown
- **THEN** the selection moves to the nearest of them, skipping working, idle and done rows

#### Scenario: Working
- **WHEN** `w` is pressed
- **THEN** the selection moves to the nearest working row after it, skipping rows of every other state

#### Scenario: Wrapping
- **WHEN** no row of the wanted kind lies after the selection
- **THEN** the search continues from the top of the list and selects the first such row

#### Scenario: Repeating
- **WHEN** the key is pressed again without moving otherwise
- **THEN** the selection advances to the next row of that kind

### Requirement: A jump that finds nothing moves nothing

Radar SHALL leave the selection where it is when no row of the wanted kind is visible.

#### Scenario: Nothing to find
- **WHEN** `w` is pressed and the view holds no working row
- **THEN** the selection is unchanged and nothing else about the view changes

#### Scenario: Folded and filtered views
- **WHEN** a row of the wanted kind is hidden by a fold or excluded by the filter
- **THEN** a jump does not select it

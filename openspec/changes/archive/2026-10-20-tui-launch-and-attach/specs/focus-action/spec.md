## MODIFIED Requirements

### Requirement: Focus the selected row

Radar SHALL, on ordinary Enter outside confirmation, ask the runtime provider to focus the selected row's location: the pane of an agent or pane row, the workspace of a workspace row. Radar SHALL keep running and keep its selection. Enter on the tree SHALL not submit lifecycle actions. A retained row is focusable while its pane is observed. When the runtime refuses because no client is attached to its server, Radar SHALL open a terminal attached to that server, using the configured `[tui] terminal` or else `$TERMINAL -e`, and ask again until a client exists or a bounded time passes; when neither names a terminal Radar SHALL say which settings to use.

#### Scenario: An agent row
- **WHEN** the user presses Enter on an agent row with a live pane outside confirmation
- **THEN** the runtime provider is asked to focus that pane and Radar remains open with the same row selected

#### Scenario: A workspace row
- **WHEN** the user presses Enter on a workspace row outside confirmation
- **THEN** the runtime provider is asked to focus that workspace

#### Scenario: No client is attached
- **WHEN** the runtime refuses a focus because no client is attached
- **THEN** a terminal is opened attached to the target's server and the focus is asked again, and no second terminal is opened by those retries

#### Scenario: No terminal is configured
- **WHEN** a focus needs a terminal and neither `[tui] terminal` nor `$TERMINAL` is set
- **THEN** a one-line message names both settings and nothing is opened

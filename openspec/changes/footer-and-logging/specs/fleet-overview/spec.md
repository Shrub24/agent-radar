## ADDED Requirements

### Requirement: Footer messages are one cut line and the whole text is logged
The dashboard SHALL show each footer message and outcome as one line cut with an ellipsis at the terminal width, SHALL remove a completed outcome after a short time while keeping a failure until the operator dismisses it, and SHALL write every message and outcome in full to its log, which SHALL carry identifiers and outcomes and never a command, an environment or a recipe.

#### Scenario: A long refusal
- **WHEN** a refusal is longer than the terminal is wide
- **THEN** the footer shows its first line ending in an ellipsis and the log holds the whole text

#### Scenario: A completed outcome
- **WHEN** an operation completes
- **THEN** its footer line leaves after a short time without any key

#### Scenario: A failure
- **WHEN** an operation fails
- **THEN** its footer line stays until `c` dismisses it

# Spec Delta

## MODIFIED Requirements

### Requirement: Project candidates come from a configured source
Radar SHALL offer the directories printed by `[tui] project_source` (default `zoxide query -l`) that are absolute, existing and accepted by `[tui] project_filter` (default: containing `.git`), each once, in the order the source printed them. The candidate list SHALL NOT depend on the working directory of the process that requested it. When the source yields no accepted directory, including when it is missing, fails or exceeds its time limit, Radar SHALL offer one configured base directory, defaulting to the user's home directory, and SHALL say why in one line. Radar SHALL NOT run anything the source prints.

#### Scenario: No source is installed
- **WHEN** the project source program is missing
- **THEN** the picker offers only the base directory and a one-line message names the failure

#### Scenario: Requested from different directories
- **WHEN** the picker is opened from two frontends whose working directories differ
- **THEN** both are offered the same candidates in the same order

#### Scenario: Source accepted nothing
- **WHEN** the source succeeds but every directory it prints is rejected by the filter
- **THEN** the picker offers the base directory and says that no listed project was accepted

#### Scenario: Base directory unusable
- **WHEN** the configured base directory is absent or not an existing directory
- **THEN** the picker offers no candidate and says so rather than substituting another directory

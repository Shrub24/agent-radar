## ADDED Requirements

### Requirement: A new workspace starts in a chosen project directory
Radar SHALL, on `A`, offer a fuzzy picker of project directories and create a workspace in the chosen one, named for the folder of the git root containing it, or for the directory's own folder when it is in no repository. Cancelling the picker SHALL create nothing. Radar SHALL NOT follow a later change of directory in the workspace.

#### Scenario: A project is chosen
- **WHEN** the user presses `A` and chooses a directory inside a git repository
- **THEN** a workspace is created in that directory and named for the repository's folder

#### Scenario: The picker is cancelled
- **WHEN** the user escapes from the picker
- **THEN** no workspace is created and the dashboard is drawn again

### Requirement: Project candidates come from a configured source
Radar SHALL offer the directory it was started in first, then the directories printed by `[tui] project_source` (default `zoxide query -l`) that are absolute, existing and accepted by `[tui] project_filter` (default: containing `.git`), each once. Radar SHALL NOT run anything the source prints. A source that is missing, fails or exceeds its time limit SHALL leave only the start directory and say so in one line.

#### Scenario: No source is installed
- **WHEN** the project source program is missing
- **THEN** the picker offers only the start directory and a one-line message names the failure

### Requirement: A workspace directory is checked by the runtime
The runtime SHALL refuse a workspace creation whose directory is relative or is not an existing directory before creating anything, and SHALL start a created tmux session in the directory it was given. A runtime that cannot start a workspace in a directory SHALL refuse it.

#### Scenario: A missing directory
- **WHEN** a workspace creation names a directory that does not exist
- **THEN** it is refused and no session is created

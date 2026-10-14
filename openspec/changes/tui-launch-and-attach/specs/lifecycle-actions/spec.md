## ADDED Requirements

### Requirement: A command can be run in a new place from the tree
Radar SHALL, on `L` over a recorded container, take a one-line command and run it in a new tab (from a workspace) or a split pane (from a pane) as the created container's recipe, after the same default-Cancel confirmation the other container controls use. The confirmation SHALL show the resolved command and its place.

#### Scenario: A command is typed and reviewed
- **WHEN** the user presses `L` on a workspace, types a command and presses Enter
- **THEN** a confirmation shows the resolved command, Cancel is selected, and nothing has been sent

### Requirement: A typed command is split by quoting alone
Radar SHALL split the line into an executable and arguments by quoting and backslashes only, SHALL NOT expand or interpret anything else, and SHALL resolve the executable to an absolute path from its own `PATH`. A line that is empty, unfinished or names a program not on `PATH` SHALL be refused with a reason before any request is sent. The command SHALL NOT appear in any record, read or event.

#### Scenario: A program that is not installed
- **WHEN** the typed program is not on `PATH`
- **THEN** no dialog opens and a one-line message says so

#### Scenario: No shell is invented
- **WHEN** the line contains `$HOME` or a pipe
- **THEN** each is passed as ordinary text in the argument list

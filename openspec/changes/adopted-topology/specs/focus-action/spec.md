## MODIFIED Requirements

### Requirement: A failed focus is stated and harmless

Radar SHALL NOT block input or quitting while a focus request is outstanding, and SHALL state a failure in one line that does not replace the source diagnostic. A focus SHALL NOT hold its target against a later mutation however it ended. A tmux focus with no client attached to the server SHALL be refused before anything changes, with a refusal that names that case.

#### Scenario: The backend does not answer
- **WHEN** a focus request exceeds the command timeout
- **THEN** the interface remains responsive, the request is abandoned and a failure line is shown

#### Scenario: Filter entry
- **WHEN** the filter is being edited and the user presses `Enter`
- **THEN** the filter is kept as before and no focus request is sent

#### Scenario: No client is attached
- **WHEN** a focus is requested on a tmux server with no client attached
- **THEN** it is refused with the no-client refusal and the server is unchanged

#### Scenario: A focus that ended unknown
- **WHEN** a focus of a workspace ended unknown
- **THEN** creating a tab in that workspace is not refused because of it

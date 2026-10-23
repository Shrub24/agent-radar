## REMOVED Requirements

### Requirement: A launched or registered lead can be enrolled
**Reason**: Enrollment no longer exists as an operator or launch action. A Pi session binds to its subject when the process reports presence, so there is nothing for `L` to send and no confirmation to offer.
**Migration**: None. `E` stays bound but reports the selected session's standing instead of enrolling it; `lifecycle.enroll` is no longer an operator path.

## ADDED Requirements

### Requirement: The standing of a Pi session is readable in place
Radar SHALL answer `E` on a selected Pi session with a one-line message naming its claim standing (granted, duplicate, reserved, pending or released), or stating that it is waiting for the next presence report when none is recorded. It SHALL send no request, open no dialog and change nothing.

#### Scenario: A granted session
- **WHEN** `E` is pressed on a Pi session whose claim is granted
- **THEN** a message says it already has a Radar subject and nothing is sent

#### Scenario: A rejected duplicate
- **WHEN** `E` is pressed on a session whose claim was rejected as a duplicate
- **THEN** the message says to reload or resume from its last live state

#### Scenario: Not a Pi session
- **WHEN** `E` is pressed on a pane that is not a Pi session
- **THEN** a one-line message says only a Pi session can be managed and nothing else happens

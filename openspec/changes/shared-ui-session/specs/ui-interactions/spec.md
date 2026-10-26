# Spec Delta

## Purpose

Continue shared user commands across inline and detached presentations while preserving captured intent and the existing domain operation's authority and durability.

## ADDED Requirements

### Requirement: Blocking interactions have one presenter and private mechanics
The session SHALL allow at most one shared blocking interaction and one valid presenter for it. All dashboards SHALL show the gate and refuse ordinary interaction while blocked. Picker queries, highlights, scrolling and editing SHALL remain private to its presenter. Local help, error inspection and command browsing SHALL NOT create a shared gate by themselves.

#### Scenario: Project picker in a popup
- **WHEN** one sidebar invokes workspace creation
- **THEN** one picker is presented, other dashboards show that the session is blocked, and none mirrors the picker's private highlight

### Requirement: The actor waits logically without suspending processing
An unresolved interaction SHALL suspend its command continuation, not observation, cancellation, result processing or attachment handling. Once its result is accepted, subsequent domain execution SHALL NOT keep the shared gate closed unless the command explicitly requires another blocking interaction.

#### Scenario: Slow operation after a choice
- **WHEN** a valid project choice is accepted and workspace creation is pending
- **THEN** presentations can resume ordinary interaction while the admitted operation's outcome is tracked

### Requirement: A gate generation rejects delayed ordinary input
Ordinary shared actions SHALL carry the gate generation under which they were interpreted. Opening and closing a shared blocking interaction SHALL advance it. Old-generation actions SHALL be rejected even after the session returns to idle. Frontends SHALL discard queued ordinary actions and pending key sequences at gate transitions rather than replaying them afterward.

#### Scenario: Delayed Enter
- **WHEN** an ordinary Enter interpreted before a modal arrives after that modal has closed
- **THEN** it is rejected rather than focusing or invoking the newly selected row

### Requirement: Captured intent cannot be retargeted by shared selection
Target-dependent invocation SHALL identify the target displayed by the originating frontend. The session SHALL capture the command's identities and relevant preconditions and revalidate them through existing domain guarantees before dispatch. A stale, absent or changed target SHALL refuse without substitution from current global selection.

#### Scenario: Another frontend changes selection
- **WHEN** one frontend invokes a destructive command for its displayed row after another frontend has selected a different row
- **THEN** the invocation is validated against its explicit target or refused, never redirected to the new selection

#### Scenario: Replacement before confirmation
- **WHEN** execution identity changes while confirmation is open
- **THEN** confirming refuses with a reason and sends no operation for the replacement

### Requirement: Presentation delivery is addressed and acknowledged
Popup presentation requests SHALL have stable effect identity and a designated host attachment. They SHALL remain available until acknowledged or superseded. Host acceptance and actual presenter claim SHALL be distinguished and accepted in either arrival order. Duplicate delivery SHALL NOT grant a second presenter authority. Reconnection SHALL NOT blindly reopen historical effects.

#### Scenario: Popup claims before host acknowledgement
- **WHEN** a fast presenter attaches before the host's acknowledgement arrives
- **THEN** the valid claim is accepted without waiting for the tmux command to exit

#### Scenario: Host accepted but no presenter
- **WHEN** a host accepts dispatch but no presenter claims within the bounded presentation wait
- **THEN** the session reports abandoned or failed presentation, releases its gate and does not claim a domain operation ran

### Requirement: Presenters submit typed results directly
Inline owners and detached presenters SHALL submit interaction-ID-addressed typed results to the shared session. A detached presenter SHALL obtain its fixed specification, claim the interaction and await result acknowledgement. Tmux exit status and parent-process callbacks SHALL NOT carry choices or continue the operation. Incompatible or conflicting results SHALL be refused.

#### Scenario: Parent sidebar exits
- **WHEN** the initiating sidebar disappears after its popup has claimed the interaction
- **THEN** the popup can submit its choice directly and the session can continue without the sidebar

#### Scenario: Duplicate choice
- **WHEN** the same valid result is retried after its acknowledgement was lost
- **THEN** it returns the retained disposition or an explicit terminal/expired answer without dispatching another operation

### Requirement: Cancellation is session-wide and cannot undo admitted work
Any attached dashboard SHALL be able to cancel the identified active interaction. Resolution and cancellation SHALL have one ordered winner. The presenter SHALL observe cancellation and close. A missing presenter SHALL be bounded by abandonment handling. Closing a presentation SHALL NOT cancel an already admitted domain operation.

#### Scenario: Escape in another dashboard
- **WHEN** another dashboard cancels the active project picker
- **THEN** the picker closes, the shared gate opens and no operation is admitted from a later stale choice

#### Scenario: Popup disappears after admission
- **WHEN** a presenter exits after its result has caused durable domain admission
- **THEN** the operation retains its established lifecycle and outcome records

### Requirement: Project preparation preserves the invoking environment
Project discovery SHALL run the configured source in the invoking frontend's execution environment and SHALL read the configuration that frontend loaded. It SHALL NOT silently substitute the daemon's environment or depend on any process's working directory. The resulting fixed picker choices SHALL preserve candidate validation, fallback and diagnostics; selecting a directory SHALL still pass normal domain validation.

#### Scenario: Different service environment
- **WHEN** the user's project source is available in the invoking frontend but not in the service's PATH
- **THEN** discovery runs in the invoking context and does not fail solely because the actor moved into the daemon

#### Scenario: Working directory differs
- **WHEN** two frontends request the picker from different working directories with the same configuration and source output
- **THEN** their choices and highlighted default are identical

### Requirement: UI acceptance and domain admission are distinct
The session SHALL distinguish volatile intent acceptance, pending/uncertain submission, durable admission and authoritative completion/refusal/unresolved outcome. It SHALL freeze one domain request ID and its complete submission before dispatch and preserve existing admission/replay checks. Uncertain submission SHALL be queried by that ID, not retried under a new one.

#### Scenario: Lost completion after admission
- **WHEN** workspace creation is recorded but its UI-facing completion is lost
- **THEN** recovery reads the original request and does not create another workspace

#### Scenario: Crash before admission
- **WHEN** the daemon loses a volatile interaction before any domain admission
- **THEN** restart does not fabricate an outcome or automatically submit a replacement operation

### Requirement: Dispatch refusal cannot leave a continuation pending
Effects SHALL distinguish preparation, dispatcher acceptance, possible submission, durable admission and outcome. A full/stopped dispatcher SHALL produce a typed refusal that resolves pending UI state without claiming execution. Correlated refusals/completions SHALL NOT be silently dropped. When admission may have occurred, recovery SHALL use the frozen request ID rather than classify it as not submitted.

#### Scenario: Effect queue is full
- **WHEN** a valid picker result prepares workspace creation but the dispatcher cannot accept it
- **THEN** the session exposes scheduling refusal, leaves no indefinitely pending continuation and claims no domain effect

#### Scenario: Worker fails after accepting work
- **WHEN** an accepted effect loses its completion after submission might have occurred
- **THEN** the session preserves uncertainty and consults the original domain request rather than declaring a safe refusal or retrying under a new ID

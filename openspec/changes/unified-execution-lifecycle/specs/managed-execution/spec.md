# Spec Delta

## ADDED Requirements

### Requirement: An execution exists by its registration
A managed execution SHALL exist independently of a subject or claim, with optional verified placement and launch intent. Process identity, scope, signalling, completion and cleanup SHALL belong to that execution. A claim SHALL represent a proposed or established attachment, including reserved, pending, granted, denied or released standing; reserved and granted claims SHALL name a subject. It SHALL NOT own copies of process lifecycle facts.

#### Scenario: A fresh launch has no preassigned subject
- **WHEN** a fresh configured launch reserves its execution before any session reports presence
- **THEN** its registration and launch intent exist without an invented subject or claim

#### Scenario: A session attaches to a launched execution
- **WHEN** a verified launched process reports its first presence without a conflicting reservation
- **THEN** its session's subject is resolved or created and a claim is granted to the same registration

#### Scenario: Execution evidence outlives an attachment
- **WHEN** a granted attachment releases and its execution has recorded completion
- **THEN** that completion remains readable by registration without requiring a current claim

### Requirement: Every way of managing a process uses one execution lifecycle
Adoption, configured launches, recipe launches, restart and spawn SHALL use the same execution lifecycle. Identification MAY occur at creation or after launch. A subject attachment MAY be reserved before launch when the subject is known; reservation SHALL NOT grant authority. Spawn provenance SHALL compose independently of whether an attachment exists.

#### Scenario: Restart reserves rather than grants
- **WHEN** a managed agent's successor execution is reserved
- **THEN** its registration exists with a reserved claim for the existing subject, while the incumbent's grant remains current until released

#### Scenario: Adoption identifies before attachment
- **WHEN** an already running process is admitted without a subject
- **THEN** its execution records verified identity and can later acquire a subject through presence

### Requirement: Default execution rows join by registration
The default view SHALL show each included registration once, enriched by optional claim, subject, public facts and verified placement, without a duplicate plain-pane row. It SHALL retain unregistered topology panes and unplaced execution rows, but SHALL NOT create execution rows from subjects alone. Existing lifecycle and history filters SHALL apply.

#### Scenario: A revoked launch never presented
- **WHEN** a revoked execution never reported presence
- **THEN** it is hidden from the default view while its execution evidence remains readable

#### Scenario: A launch gains an attachment
- **WHEN** a placed launch transitions from reservation through identification to granted presence
- **THEN** one execution row retains its registration identity and gains attachment and subject data without a separate pending subject row

#### Scenario: A reserved successor shares a subject
- **WHEN** a predecessor and its reserved successor are both included by the view's filters
- **THEN** they appear as distinct registration rows enriched with the same subject and their distinct attachment standings

#### Scenario: An unregistered pane remains topology
- **WHEN** a pane is not associated with any represented registration
- **THEN** it remains an ordinary topology row rather than being omitted

#### Scenario: An execution has no verified pane
- **WHEN** an included execution has no verified placement
- **THEN** its registration row remains visible as unplaced rather than becoming a standalone subject row

#### Scenario: A subject remains after execution history is filtered
- **WHEN** a retained subject has no registration included by the default view
- **THEN** it does not create a default execution row

#### Scenario: The pane names its occupant
- **WHEN** a live execution's recorded placement names a pane
- **THEN** the published pane names that registration as its occupant, resolved once by the collector, and rows read it rather than re-deriving occupancy

#### Scenario: An ended execution leaves an ordinary pane
- **WHEN** the execution placed in a pane has ended or was withdrawn before presenting
- **THEN** the pane names no occupant and remains an ordinary pane row

#### Scenario: A successor and its predecessor name one pane
- **WHEN** two live executions are placed in one pane
- **THEN** the pane's occupant is the one holding an attachment, deterministically, not the one that sorts first by key

#### Scenario: A launch that has not presented is closed through its pane
- **WHEN** the operator closes the row of a placed execution that has no agent registration yet
- **THEN** the ordinary fenced pane close is requested for its pane

#### Scenario: A failed intended attachment is published
- **WHEN** a registration's own reservation was settled because another session presented
- **THEN** its execution observation carries the failure reason, and the TUI names it on the row and in details

### Requirement: Execution row composition does not create another state owner
The execution row record SHALL be a derived join of registration, optional claim, optional subject and placement. It SHALL NOT be a persisted row store or independently maintained placement authority. A topology link SHALL NOT bypass backend-generation verification.

#### Scenario: A row is rebuilt after a pane move
- **WHEN** a fresh observation verifies a different placement for a registration
- **THEN** the joined row follows that placement without synchronising a separate durable row record

### Requirement: Public observation supplies execution joins before presence
The public observation SHALL provide execution identity, lifecycle state, verified placement and optional attachment and subject references before presence. It SHALL distinguish reserved from granted attachments and cover executions without spawn edges. It SHALL NOT expose private recipes, environment values, launch correlation or completion keys.

#### Scenario: A recipe launches without a spawn edge
- **WHEN** a recipe execution has a recorded pane but has not reported presence
- **THEN** the observation supplies its registration and placement for one joined row without requiring a subject or spawn edge

#### Scenario: A consumer reads a reserved attachment
- **WHEN** an observation includes a reserved claim
- **THEN** it identifies the intended subject and registration without representing that reservation as granted authority or exposing private launch credentials

## MODIFIED Requirements

### Requirement: Managed executions have claim-scoped placement
Every managed execution SHALL identify its registration and verified hosting pane independently of claim attachment and spawn parentage. Current placement SHALL reflect verified pane moves without changing subject identity or historical spawn provenance. A pane handle without its backend generation SHALL NOT authorize removal.

#### Scenario: Admitted agent has no spawn edge
- **WHEN** `radar run --agent` admits an agent in an existing pane
- **THEN** container discovery can find its exact execution through verified placement without inventing a spawn edge

#### Scenario: Pane moves between tabs
- **WHEN** fresh topology verifies a managed pane moved
- **THEN** current containment follows its new location while subject identity and recorded producer parentage remain unchanged

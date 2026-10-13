## MODIFIED Requirements

### Requirement: An edge binds through a single-use correlation token
The daemon SHALL mint a private token per spawn and pass it to the child in its launch environment. The pending edge SHALL bind only to the registration presenting that token; spent or revoked tokens SHALL NOT bind another child. On a lifecycle-tracked launch, registration SHALL match the daemon-captured process birth identity. Cancellation and binding SHALL have one durable winner.

#### Scenario: The child registers with its token
- **WHEN** a child registers with its valid spawn token and matching captured process identity when tracking is available
- **THEN** its exact registration binds and the token is spent

#### Scenario: The token never arrives
- **WHEN** launch is confirmed but registration never arrives
- **THEN** the edge remains unbound while independently captured process evidence remains available

#### Scenario: A second registration presents the token
- **WHEN** a registration presents a token already consumed
- **THEN** binding refuses and existing history remains unchanged

#### Scenario: Revoked token
- **WHEN** a registration presents a token revoked by cancellation
- **THEN** binding refuses and cancelled bootstrap authority is not restored

### Requirement: Spawn authority is bounded
A spawn SHALL create only its requested child and SHALL NOT stop, resume or restart another process or write assignment facts. The daemon SHALL record spawn edges only for launches it created. A separately admitted lifecycle operation MAY manage the corresponding subject; explicit operator enrollment of an existing registered process SHALL NOT fabricate a spawn edge. Pane-close records SHALL remain separate from immutable spawn history and SHALL NOT change execution or assignment publications.

#### Scenario: A child spawned outside the daemon
- **WHEN** an agent launches outside the daemon
- **THEN** it has no fabricated spawn edge, even if explicitly enrolled later

#### Scenario: It does not touch anything else
- **WHEN** a spawn completes
- **THEN** it has not stopped or restarted another process or changed assignment facts

#### Scenario: A bound child pane is closed later
- **WHEN** a caller uses `spawn.close` for a daemon-created edge
- **THEN** the edge and original child binding remain unchanged and cleanup is recorded separately

## ADDED Requirements

### Requirement: Managed launch references survive replacement
A new lifecycle-tracked spawn SHALL expose an additive managed `subject_id` referencing its daemon-owned lifecycle record. That subject SHALL retain the originating edge while later process generations receive separate history and correlations. Legacy edges SHALL remain readable without inferred subject or process identity.

#### Scenario: Replacement registration
- **WHEN** a managed subject later binds a replacement process
- **THEN** the original spawn still names its original child and subject, rather than rewriting its registration binding

### Requirement: Lifecycle tracking is established before execution
A lifecycle-capable spawn SHALL use the daemon's bounded launch bootstrap to capture the process birth binding before the requested program executes. The bootstrap SHALL preserve literal executable/argv, cwd and inherited-plus-overlay environment semantics and SHALL NOT count as Pi application readiness. An uncertain dispatch SHALL NOT be retried or reconstructed from a later pane occupant.

#### Scenario: Program has no reporter
- **WHEN** the requested program never registers
- **THEN** the daemon still tracks the captured launch process without claiming agent readiness

#### Scenario: Existing launch context
- **WHEN** a tracked spawn supplies cwd and environment overlay
- **THEN** the requested program receives the same literal context and private registration token through an exec-preserved process identity

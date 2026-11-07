# managed-child-spawn Specification

## Purpose
Let the daemon create a managed child — pane, launch and identity binding — as
one durable operation, so the parent/child runtime relationship is authored
where the child is created rather than reconstructed from pane titles.

## Requirements

### Requirement: A spawn names its parent and is recorded before any effect

A caller SHALL name the parent runtime subject for a spawn. The daemon SHALL
record the request and its parent before creating anything, and SHALL NOT
create a pane wherever the backend happens to have focus.

#### Scenario: A named parent
- **WHEN** a caller requests a spawn under a parent it names
- **THEN** the recorded request names that parent and the created pane is created under it

#### Scenario: An unknown parent
- **WHEN** a caller names a parent the daemon has no runtime subject for
- **THEN** the spawn is refused before dispatch and nothing is created

#### Scenario: The same request arrives twice
- **WHEN** a spawn request id is replayed with identical content
- **THEN** the daemon answers with the recorded outcome and performs no second effect

### Requirement: An edge binds through a single-use correlation token

The daemon SHALL mint a private token per spawn and pass it to the child in its
launch environment. The pending edge SHALL bind only to the registration that
presents that token, and a spent token SHALL NOT bind a second child.

#### Scenario: The child registers with its token
- **WHEN** a child registers carrying the token minted for its spawn
- **THEN** that edge binds to the child's exact identity and the token is spent

#### Scenario: The token never arrives
- **WHEN** a launch is confirmed but no registration ever presents the token
- **THEN** the edge reads unbound with its created location, and no child is claimed

#### Scenario: A second registration presents the token
- **WHEN** a registration presents a token that already bound a child
- **THEN** it is refused as a spent token and the existing edge is unchanged

### Requirement: No edge is bound by appearance

An edge SHALL NOT be bound by pane title, alias, label, position or session
UUID. Nothing observable about a pane SHALL be treated as the identity of the
process running in it.

#### Scenario: A renamed or moved pane
- **WHEN** a bound child's pane is renamed, moved or left alone
- **THEN** the edge still names the same child

#### Scenario: Two similar children
- **WHEN** two spawned children carry the same label and title
- **THEN** each binds only through its own token and neither is attributed to the other

### Requirement: Spawn reports partial effects separately

Created, launched and bound SHALL be reported separately, each completed,
refused or unknown. A launch that may have been dispatched but is unconfirmed
SHALL be unknown and SHALL NOT be reported as launched.

#### Scenario: An unconfirmed launch
- **WHEN** the backend is sent the child command and no confirmation arrives
- **THEN** the record reads launched unknown and still names the created pane

#### Scenario: A refused launch
- **WHEN** the backend refuses the child command after the pane was created
- **THEN** the refusal names the created pane and the daemon does not remove it

### Requirement: Launch is a declared backend capability

A backend that does not declare the launch capability SHALL refuse a spawn
before dispatch, leaving no pane and no recorded effect. A launch SHALL execute
only the command resolved for that spawn.

#### Scenario: A backend without launch
- **WHEN** a spawn is requested against a backend that does not declare launch
- **THEN** it is refused before dispatch and nothing is created

#### Scenario: The declared capability is what runs
- **WHEN** a spawn is served by a backend that declares launch
- **THEN** the command resolved for that spawn is the one launched, unchanged

### Requirement: Topology reads report edges without conclusions

A spawn read SHALL report the parent, the bound child when there is one, the
edge state, the created location and freshness. Edges SHALL survive a daemon
restart and SHALL be re-verified rather than revised by inference.

#### Scenario: Read after a daemon restart
- **WHEN** edges recorded before a restart are read afterwards
- **THEN** each reports its state and its location's freshness, with stale facts marked

#### Scenario: A location that no longer exists
- **WHEN** a recorded location cannot be found in the backend
- **THEN** the edge reads unresolved, which is not a report that the child stopped

### Requirement: Spawn authority is bounded
Spawn edges SHALL describe daemon-requested child creation and binding, not provide the universal execution-placement model. Spawning SHALL NOT settle assignments. Later authorized child close SHALL use common managed termination and pane removal, MAY record verified execution completion and SHALL preserve the original edge and binding. Direct admission SHALL NOT fabricate a spawn relationship.

#### Scenario: A child spawned outside the daemon
- **WHEN** a process is admitted without a daemon-requested child creation
- **THEN** it may have a managed execution and verified placement without an edge, and that absence is not an error

#### Scenario: It does not touch anything else
- **WHEN** a spawn completes
- **THEN** no assignment fact changes and unrelated executions are not stopped, resumed or restarted

#### Scenario: A bound child pane is closed later
- **WHEN** an authorized caller closes a spawned child
- **THEN** common close records its execution/removal outcomes while preserving its original edge and binding and leaving assignment settlement to its owner

### Requirement: Spawn preserves explicit launch context
A spawn SHALL accept an optional absolute working directory and an environment overlay of UTF-8 name/value pairs. Supplied values SHALL reach the child unchanged; omitted fields SHALL retain existing behavior. Overlay values SHALL override inherited values. The daemon SHALL supply its own spawn token and refuse caller attempts to set that reserved variable.

#### Scenario: Herdsman child launch context
- **WHEN** a spawn supplies a cwd and mailbox, owner, run and role environment values
- **THEN** the child observes that cwd and those exact values alongside the daemon-supplied private token

#### Scenario: Old request omits launch context
- **WHEN** a compatible spawn request omits cwd and environment
- **THEN** launch retains the existing inherited environment and placement-derived cwd behavior

#### Scenario: Reserved registration id override
- **WHEN** a caller supplies PI_RADAR_REGISTRATION_ID in its environment overlay
- **THEN** admission refuses without accepting the request or creating a pane

### Requirement: Launch context is representable before creation
The daemon SHALL validate launch-context shape, bounds and selected-backend representability before accepting a spawn or creating a pane. Unsupported cwd or environment transport SHALL refuse explicitly, never silently drop supplied fields. The adapter SHALL refuse newline or terminal-control values that its single-line launch transport cannot represent.

#### Scenario: Unrepresentable environment value
- **WHEN** a spawn supplies an environment value containing a newline
- **THEN** the spawn refuses before creation and does not leave a pane or accepted operation

#### Scenario: Shell-sensitive values
- **WHEN** supported launch values contain spaces, quotes, dollar signs or command-substitution syntax
- **THEN** the child receives those values literally and no embedded shell commands execute

### Requirement: Topology names the exact child registration
A token-bound spawn SHALL report the exact bound child registration_id alongside source/incarnation provenance. Identical registration replay SHALL preserve that binding. A rename, shared session or similar label SHALL NOT change the binding.

#### Scenario: Binding readback
- **WHEN** a child successfully registers with its spawn token and the caller reads spawn.get
- **THEN** the response names the same registration_id returned to that registration

### Requirement: Spawn topology reports exit observation
Spawn topology reads SHALL project the linked claim's recorded execution completion, distinguishing unavailable status from captured code/signal and preserving observation provenance. Reads SHALL NOT fetch status from the backend. A missing code SHALL NOT prevent verified execution exit or pane cleanup, and pane-command status SHALL NOT be mislabelled as managed-program status.

#### Scenario: A child on a backend that observes no exit
- **WHEN** neither supervised program completion nor valid other evidence records the child's exit
- **THEN** reads report completion as unknown without a backend status fetch

#### Scenario: tmux child exited
- **WHEN** a managed tmux child exits and its supervisor records its program status
- **THEN** the linked edge projects that status stably after pane removal and daemon reopen

#### Scenario: Execution exited without a code
- **WHEN** the execution's exit is verified but status capture was unavailable
- **THEN** reads distinguish verified exit from missing code without zero-filling or blocking cleanup

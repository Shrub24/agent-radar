## MODIFIED Requirements

### Requirement: A live agent is marked stale when its binary was replaced

Radar SHALL select the installed counterpart of a running executable from the running executable's own package family, read from its Nix store derivation name by separating the store hash, the exact package name and the version. It SHALL compare the running executable's package root with the payload package root of that exact counterpart, and SHALL mark the agent stale only when the running executable's link reports it deleted, or when both package roots are Nix store paths that differ. An entrypoint named by a versioned package of the same family SHALL mean that package root is the payload root, with nothing read from the entrypoint; an entrypoint anywhere else — an unversioned launcher package of that name included — SHALL be resolved only when it is a script whose single unconditional target is one strictly literal absolute store path, and every other entrypoint SHALL be unknown. Differing store roots SHALL be stale even when the two packages carry the same version, and no version string, build stamp or manifest SHALL be consulted to excuse a difference. A runtime-reported program name SHALL NOT select a bolt family's counterpart, which the running executable's package family alone selects; for every other program the runtime-reported name remains the name resolved on the search path, and a package name SHALL match exactly rather than as a prefix. For every program outside the handled families the existing comparison by matching file name SHALL be unchanged. Radar SHALL read nothing from a process's environment, SHALL NOT execute the counterpart, and SHALL NOT read the counterpart's compiled content. When the running process or its executable cannot be read, the exact package name or version cannot be separated, the counterpart cannot be resolved, the resolution is ambiguous, or the platform has no reader, the freshness SHALL be unknown and no claim SHALL be shown.

#### Scenario: Package updated after the session started

- **WHEN** a live agent's executable is under one Nix store package root and the resolved counterpart's payload is under a different package root
- **THEN** the agent is stale, and this holds when both packages carry the same version

#### Scenario: Bolt lead variant

- **WHEN** a live agent runs `<root>-pi-bolt-0.7.1/lib/pi-bolt/pi` and `PATH` resolves `pi-bolt` to a launcher whose payload package root is a different `pi-bolt` package root
- **THEN** the agent is stale, and it is current when the payload package root is the running one

#### Scenario: Bolt child variant

- **WHEN** a live agent runs `<root>-pi-bolt-child-0.7.1/lib/pi-bolt/pi`
- **THEN** its counterpart is the installed `pi-bolt-child` payload, never `pi-bolt` and never the runtime-reported name

#### Scenario: Exact family names only

- **WHEN** the running package name is `pi-bolt-child` and only `pi-bolt` resolves on `PATH`
- **THEN** the freshness is unknown and no mark appears, because a prefix is not a family

#### Scenario: Runtime name does not select a program

- **WHEN** the runtime reports the foreground program as `pi` — the name of the exec-replaced payload — and `PATH` resolves `pi` to the lead variant's launcher
- **THEN** the comparison uses the running executable's own package family, and a child payload reported as `pi` is unknown rather than compared with the lead variant

#### Scenario: Executable replaced in place

- **WHEN** the running executable's link reports it deleted
- **THEN** the agent is stale, and it is stale whether or not an installed counterpart could be resolved

#### Scenario: Derivation-provided entrypoint

- **WHEN** `PATH` resolves the family name to an entrypoint inside a versioned package of the same family, which provides the running `lib/pi-bolt/pi`
- **THEN** that package root is the counterpart's payload root and the agent is current, without anything being read from the entrypoint

#### Scenario: Unversioned launcher package

- **WHEN** `PATH` resolves the family name to an entrypoint in a package whose root carries no version, such as a profile launcher
- **THEN** that root is not taken as the payload root for being the entrypoint's own; the package root of the entrypoint's one declared target is used instead, and an entrypoint without exactly one such target is unknown

#### Scenario: Launcher and payload inside one package

- **WHEN** the running executable is `<root>/lib/pi-bolt/pi` and the resolved counterpart's payload is the same package root
- **THEN** the agent is current

#### Scenario: Unsupported launcher shape

- **WHEN** the installed counterpart's launcher offers no target, several possible targets, a computed or conditional target, a target outside the Nix store, or a cycle
- **THEN** the freshness is unknown and no mark appears, rather than one of the possibilities being chosen

#### Scenario: A deliberate other build

- **WHEN** the running executable is outside the Nix store and is not the counterpart
- **THEN** the agent is not stale and has no mark

#### Scenario: Nothing to compare

- **WHEN** the process has gone, its link is unreadable, no exact counterpart resolves, or the platform cannot read it
- **THEN** freshness is unknown and no mark or claim appears

### Requirement: Staleness is a label, not a state

A stale agent row SHALL carry a mark in the configured stale colour, and its details SHALL name the running package identity and, when it was resolved, the installed counterpart's, with both full package roots readable from the page. Staleness SHALL NOT change the row's state word, ordering, attention or working jumps, filtering, folds or retained/exited treatment. A retained row has no live process and SHALL show no staleness.

#### Scenario: Stale row

- **WHEN** an agent is stale
- **THEN** its row shows the stale mark and its details name the running and installed package identities
- **AND** its state, order and jump behaviour are as if it were current

#### Scenario: Not the installed program

- **WHEN** the running executable is a different program from the counterpart without being outdated
- **THEN** the details say it is a different build from the installed one, name that target, and the row has no mark

#### Scenario: Retained row

- **WHEN** an agent row is retained after its agent returned
- **THEN** it shows no staleness

## ADDED Requirements

### Requirement: Executable identity is inspectable on the Processes page

For an agent row or an ordinary pane row whose foreground evidence is current, the Processes page SHALL show the foreground process's command name as the runtime reports it, the running package named by its version and, where two builds of one version must be told apart, a short hash of its store root; the executable named relative to that package root when it lies inside it; the installed counterpart when it differs from the running one; and a short reason when the freshness is unknown. A current comparison SHALL show no verdict of its own, the absence of the stale mark being the statement. Radar SHALL keep the verbose facts readable from the page in one block the reader opens: both package roots whole, the executable's whole path, the process birth identity, what each kernel state it names means, the qualification a descendant sum carries, and the whole reason a comparison or a value could not be made. Exhibited text SHALL be sanitized as other external text is. A current identity SHALL remain inspectable rather than being omitted because there is nothing to mark. Radar SHALL NOT present the observed name, its arguments or the executable path as the original invocation, as an alias or as a launcher, and SHALL NOT show prompt or system-prompt arguments. A retained row, a stale source observation and a row without current non-shell foreground evidence SHALL show none of these fields and SHALL offer no block to open. If current foreground evidence exists but its executable link is unreadable, the page SHALL show the observed name and a comparison-unavailable reason, but no executable path or package identity. Process arguments SHALL NOT be displayed on this page.

#### Scenario: Current identity is inspectable

- **WHEN** an agent runs the payload its installed counterpart points at
- **THEN** the Processes page names the observed command, the package and the executable within it, and the row carries no mark and no verdict

#### Scenario: Stale identity names both

- **WHEN** an agent is stale
- **THEN** the Processes page carries the stale mark and names the installed target beside the running package, and both roots whole are readable in the page's block

#### Scenario: Unknown identity states why

- **WHEN** the freshness is unknown
- **THEN** the page states in short that the comparison could not be made, the whole reason is readable in the page's block, and the row carries no mark

#### Scenario: Ordinary pane

- **WHEN** an ordinary pane has current foreground evidence for a non-shell process
- **THEN** its Processes page shows the same command, package, executable and unknown-reason fields as an agent row

#### Scenario: The observed name is not an invocation

- **WHEN** the runtime reports the foreground program as `pi` for a bolt payload
- **THEN** the page shows `pi` as the observed process's name and never as what the pane was started with

#### Scenario: The executable is not printed twice

- **WHEN** the running executable lies inside its own package root
- **THEN** the page names that root once as the package and prints only the part of the path beneath it

#### Scenario: Withheld

- **WHEN** the row is retained, the source observation is stale or unavailable, or the foreground is a shell or inconclusive
- **THEN** no executable path, package identity or unknown reason is drawn for it, and no block is offered for the keyboard to open

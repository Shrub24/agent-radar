# Why

Subjects are agents; registrations are process incarnations. The child SDK currently reconstructs agent identity from historical spawn edges and moves assignments between process-addressed writers. That makes a client responsible for lifecycle state the daemon already owns.

# What Changes

- **BREAKING** Replace public registration-addressed agent discovery and child inspection with subject-addressed reads and controls. Keep the caller's own registration and claim for presence and publication authority.
- Radar resolves current attachment, process, location and channels internally. Consumers neither receive nor supply child registration or claim ids.
- Separate live subjects from retained subjects without deleting identity on process exit. Retained children remain inspectable and continuable.
- Move child assignment ownership and replacement delivery into the daemon; reads have no publication side effects.
- Preserve duplicate/refused process visibility in the fleet diagnostic projection without pretending those processes are additional subjects.
- Update pi-radar, generated contracts, examples and the Herdsman adoption brief together; bump wire revision and matching releases. No compatibility layer.

# Capabilities

## New Capabilities

- `subject-control`: subject-addressed discovery, child authority, inspection and assignment lifecycle.

## Modified Capabilities

- `pi-radar-sdk`: child operations call daemon subject operations rather than resolving registrations or replaying assignments.
- `agent-registration`: public channel reads use subjects; registration remains process identity and self-publication machinery.

# Impact

`src/control_plane/registry_read.rs`, lifecycle and registration/subject-control handlers, assignment storage/delivery, `pi-radar/extension/radar-children.ts`, launch/inspection/client modules, public schemas, examples and consumer documentation. The TUI keeps internal process evidence for pane joins and refused-claim warnings. No new mux backend, polling framework, session-store ownership or Herdsman policy is introduced.

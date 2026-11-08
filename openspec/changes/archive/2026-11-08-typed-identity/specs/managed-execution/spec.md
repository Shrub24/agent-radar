## ADDED Requirements

### Requirement: Lifecycle operations accept only the identity kind they act on
Radar SHALL keep subject, claim and registration identities as distinct kinds. A lifecycle operation, record lookup or request field SHALL accept only the kind it acts on and SHALL NOT resolve one kind through another. Typing SHALL NOT change the stored or wire representation of any identity.

#### Scenario: A subject id where a claim id is required
- **WHEN** a request names a subject id in a field that names a claim
- **THEN** it is refused as not found, and no execution of that subject is resolved or signalled

#### Scenario: Representation is unchanged
- **WHEN** a record or reply carrying an identity is written by this build
- **THEN** the identity is the same canonical UUID string an earlier build wrote

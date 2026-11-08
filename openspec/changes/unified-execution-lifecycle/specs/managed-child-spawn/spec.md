# Spec Delta

## ADDED Requirements

### Requirement: A spawn edge is composed onto the execution it launches
A spawn SHALL create its child through the same execution lifecycle as every other launch, and its edge SHALL be a record of the parent's intent keyed on the child's registration. The edge SHALL NOT require the child to have a subject or claim before its program runs; it SHALL bind to the child's subject when the child's session attaches.

#### Scenario: A child launches without a subject
- **WHEN** a spawn has created its child and the child has not yet reported a session
- **THEN** the edge names the child's registration and no subject exists for the child

#### Scenario: A child's session attaches
- **WHEN** the child reports its first presence
- **THEN** the edge binds to the subject that session holds, which is an existing subject when the session was already owned

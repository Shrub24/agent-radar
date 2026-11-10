# Spec Delta

## ADDED Requirements

### Requirement: A spawn edge is composed onto the execution it launches
A spawn SHALL create its child through the shared execution lifecycle and record parent provenance keyed on the child's registration. The edge SHALL NOT require a subject or claim before the child runs. A fresh child SHALL attach its session's subject at grant; continuation of a known released child SHALL reserve that subject's successor claim before launch. Both SHALL use the same execution control and observation semantics.

#### Scenario: A child launches without a subject
- **WHEN** a fresh spawn has created its child and the child has not yet reported a session
- **THEN** the edge names the child's registration and accepted parent intent without inventing a subject or claim

#### Scenario: A child's session attaches
- **WHEN** a fresh child reports a verified session without a conflicting attachment reservation
- **THEN** the edge binds to the subject resolved for that session, continuing its existing subject when applicable

#### Scenario: A parent continues its released child
- **WHEN** a parent is authorised to continue its own recorded detached child
- **THEN** a new execution reserves a claim for that existing child subject and grants it only after verified presentation

#### Scenario: Child metadata becomes subject metadata
- **WHEN** a child's attachment is granted with accepted nonempty parent, owner, run or label metadata
- **THEN** the subject receives the accepted launch metadata rather than losing it during enrollment

#### Scenario: Continuation supplies no replacement metadata
- **WHEN** a continuing session attaches without new managed metadata
- **THEN** its subject retains its prior managed metadata

#### Scenario: A child fails before attachment
- **WHEN** a fresh child ends before acquiring a subject
- **THEN** its execution and spawn outcome remain readable by registration and request without creating a detached subject

## MODIFIED Requirements

### Requirement: Gate safety survives delayed content
Input gates SHALL be invalidated promptly when a newer head announces a different generation, even while replies or required content are pending. Gate observation SHALL NOT adopt partial payload state. The previous coherent head MAY remain displayed during materialisation, but stale ordinary input SHALL be fenced immediately. Coherent adoption SHALL NOT overwrite local browsing selection with a global cursor. Local selection changes SHALL follow projection validity or the explicit follow policy. Receipts, retained presentation requests and admitted operations SHALL survive publication coalescing.

#### Scenario: Modal opens while content is delayed
- **WHEN** a gate-changing head arrives during content recovery or an outstanding acknowledgement
- **THEN** old-gate ordinary input is stopped or refused before rendering catches up

#### Scenario: Independent selection differs from focus
- **WHEN** a non-following view selects B while a gate-changing coherent head reports focus A
- **THEN** B remains selected if still valid, while receipts and frozen command targets retain their actor ordering

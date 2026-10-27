# Spec Delta

## Purpose

Allow copying relevant identifiers and readable text from a local Radar panel without selecting adjacent terminal columns or changing shared navigation and execution state.

## ADDED Requirements

### Requirement: Copy actions use structured local content
Radar SHALL offer context-appropriate copy actions for displayed identifiers, paths or text without parsing terminal screenshots or changing the shared selection. Copied text SHALL retain established sanitization/privacy rules. An absent or unsupported value SHALL produce an explanation rather than copying another row's value.

#### Scenario: Copy a request identifier
- **WHEN** the user copies the displayed operation's request ID
- **THEN** that ID is submitted to the configured clipboard mechanism without reissuing the operation

### Requirement: Panel selection is confined and content-relative
Application-managed selection SHALL remain inside the chosen panel and use logical content positions across wrapping, scrolling and resize. It SHALL NOT include neighboring borders/panels or reuse a selection against a replacement content identity. Selection mechanics SHALL remain local and SHALL NOT issue lifecycle or mux actions.

#### Scenario: Resize selected content
- **WHEN** the panel is resized and the same content rewraps
- **THEN** selection still refers to the same logical text instead of unrelated screen cells

#### Scenario: Selected subject changes
- **WHEN** a panel is rebound to a different subject/content identity
- **THEN** an old text selection is cleared or explicitly revalidated rather than copied from the replacement

### Requirement: Clipboard capability and failure are explicit
Clipboard integration SHALL respect the user's mechanism/security configuration and report unsupported or failed delivery. A terminal escape request without acknowledgement SHALL NOT be described as verified clipboard ownership. Copy handling SHALL NOT silently execute copied text or emit arbitrary source-controlled escape sequences.

#### Scenario: No supported clipboard mechanism
- **WHEN** a copy action cannot use a configured local or terminal mechanism
- **THEN** the UI explains the limitation without claiming the clipboard was updated

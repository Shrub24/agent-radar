# Spec Delta

## Purpose

Give Radar commands stable semantic identity independent of terminal keys, while allowing local keymaps, contextual help and multi-step sequences to share one catalogue.

## ADDED Requirements

### Requirement: Commands have semantic identity independent of keys
Keyboard, mouse and palette invocation SHALL converge on typed semantic commands. A command SHALL NOT be implemented by replaying artificial terminal keypresses. Command identity, label, description and target requirements SHALL have one catalogue, distinct from context-dependent availability and local binding choice.

#### Scenario: Palette and shortcut agree
- **WHEN** the same operation is invoked by palette or shortcut
- **THEN** it reaches the same semantic command and identity checks without synthetic input

### Requirement: Input modes and sequences remain local
Frontends SHALL resolve terminal events locally, including normal/editor/modal contexts and multi-step or leader sequences. Only completed shared commands SHALL cross the session boundary. Prefix timeout, failed sequence, ambiguity and Escape behavior SHALL be deterministic, and focus or gate changes SHALL clear pending sequences.

#### Scenario: Sequence completion
- **WHEN** the configured multi-step navigation binding is completed in tree context
- **THEN** one semantic navigation action is submitted rather than its constituent keys

#### Scenario: Typing in an editor
- **WHEN** a command key is typed in a text editor
- **THEN** it is handled as editor input unless that editor explicitly binds it

#### Scenario: Modal interrupts a leader
- **WHEN** a shared blocking interaction opens while a leader prefix is pending
- **THEN** the prefix is cleared and cannot complete after the modal closes

### Requirement: Local controls do not mutate the shared session
Details navigation, split sizing, viewport movement and informational overlays SHALL be handled locally. They SHALL NOT require a shared action or alter canonical tree selection, folds or filtering. Existing modal and editor input precedence SHALL be preserved.

#### Scenario: Reading details
- **WHEN** one dashboard changes its detail page and scrolls
- **THEN** neither another frontend's details nor the shared selection changes

### Requirement: Binding presentation reflects effective configuration
Binding help SHALL be derived from effective local bindings and the command catalogue, not a separate list of operation definitions. An unavailable command SHALL remain subject to authoritative session/domain checks regardless of its displayed binding or local menu state.

#### Scenario: Rebound command
- **WHEN** configuration changes the key for a command
- **THEN** help and invocation agree on its effective binding without changing that command's semantic identity

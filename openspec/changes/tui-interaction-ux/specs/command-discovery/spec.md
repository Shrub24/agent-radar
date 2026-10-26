# Spec Delta

## Purpose

Make available Radar commands discoverable through contextual help and a fuzzy palette without duplicating command definitions or changing shared interaction authority.

## ADDED Requirements

### Requirement: Contextual help uses the effective command catalogue
Help and footer hints SHALL use the established typed catalogue, effective local bindings and current command availability. They SHALL identify the active input context and make relevant unavailable-command reasons reachable. Rebinding SHALL update help without redefining the operation or bypassing session/domain checks.

#### Scenario: Command is unavailable
- **WHEN** restart is not available for the displayed target
- **THEN** help or the palette explains its unavailability rather than suggesting an executable alternative target

#### Scenario: Binding changes
- **WHEN** a command is rebound
- **THEN** displayed help and actual resolution agree on the effective binding

### Requirement: Fuzzy command browsing is local and invokes typed actions
The command palette SHALL provide local fuzzy matching over command labels/descriptions with readable selection, effective binding hints and empty results. Opening, searching and cancelling it SHALL NOT block other frontends. Invoking a choice SHALL submit its typed command with the displayed target evidence where required, not replay keys or use a newly substituted selection.

#### Scenario: Cancel browsing
- **WHEN** the user filters the palette and closes it without choosing
- **THEN** no shared command is invoked and another frontend's navigation was not blocked

#### Scenario: Choice needs confirmation
- **WHEN** a chosen command requires a shared confirmation
- **THEN** the session establishes that interaction and the local palette no longer owns input

### Requirement: Pending key sequences are visible and bounded
A pending multi-step or leader sequence SHALL have a visible local indication and reachable completions/help. Timeout, failure and Escape SHALL clear it predictably. Input-context or shared-gate changes SHALL cancel the pending sequence so it cannot invoke a command in a later context.

#### Scenario: Interrupted leader
- **WHEN** a shared blocking interaction interrupts a leader prefix
- **THEN** its pending indication clears and later keys do not complete the old sequence

### Requirement: Picker states are explicit across presentation roots
Pickers SHALL expose loading/preparation, empty results, diagnostics, selection and cancellation consistently whether mounted inline or in a detached presenter. Query/highlight state SHALL remain private. A discovery failure SHALL preserve valid fallback choices rather than appear as an unexplained empty list.

#### Scenario: Source unavailable in detached picker
- **WHEN** project discovery reports a source failure but provides the invoking directory
- **THEN** the detached picker shows that fallback and the diagnostic, with the same result semantics as the inline picker

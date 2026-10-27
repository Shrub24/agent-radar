# Spec Delta

## ADDED Requirements

### Requirement: Presentation roots share semantic visual vocabulary
Dashboard, sidebar and detached presenters SHALL use the same configured meanings for focus, selection, muted content, pending, refused, unresolved and shared blocking states. Narrow layouts SHALL reduce detail without conflating those meanings. New styling SHALL preserve configured colors, fallback diagnostics, observation freshness and existing motion settings.

#### Scenario: Blocked sidebar and dashboard
- **WHEN** a shared picker is active in another presenter
- **THEN** attached sidebar/dashboard views visibly indicate blocking and cancellation without implying their own local informational overlay is the shared presenter

#### Scenario: Unresolved is not successful
- **WHEN** an operation has an unresolved outcome
- **THEN** its mark, text and inspector distinguish it from both pending admission and verified completion, regardless of theme

### Requirement: Visual transitions do not invent progress
Animation or highlighting SHALL correspond to an established UI/domain change and respect motion settings. Retained/stale facts SHALL NOT animate as live work. A transition SHALL NOT fabricate progress percentages, hide a refusal or require continuous redraw when nothing visible animates.

#### Scenario: Motion disabled
- **WHEN** the user disables the relevant motion
- **THEN** the same state remains legible without effects or unnecessary animation redraws

# Design

## Context

See `proposal.md` for scope. `shared-ui-session` owns typed commands, canonical projections, modal gates, addressed host requests and domain handoff. This change starts after its milestone C is demonstrated; it does not redefine that protocol.

The current frontend already has one generic `Picker<T>` in `src/picker.rs`, one renderer in `src/ui/picker.rs`, per-page details, an identity-stable process table, configurable colors/motion, and extensive `TestBackend` helpers in `src/ui.rs`. Improve these seams rather than treating the UI as unimplemented. Existing operation reads expose records by request ID and bounded recent/unfinished lists, while events.read exposes a retained cursor-based journal with subject/request links and timestamps. Neither restores a complete persisted UI-session history.

## Goals / Non-Goals

**Goals:** visibly useful discovery and feedback; reusable interaction components in both roots; local panel affordances; capability-honest copy; small independently reviewable UX increments.

**Non-Goals:** another state authority, remote rendering protocol, shared details/editor drafts, generic desktop-style docking system, terminal/mux migration, new operation persistence or full Vim grammar.

## Decisions

The confirmed frontend and ownership decisions are recorded through Keep the Why in [UI session architecture](../../../context/ui-session.md). Candidate libraries below remain evaluations; record a consequential selection/rejection there only when its rationale is established and confirmed.

### 1. Depend on milestones, not completion of all command migration

Begin after core A–C: the catalogue/resolver seam, read-only shared projection, local state boundary, host delivery and both interaction roots must exist. Work may overlap core D only with explicit file ownership. Core owns public contracts and command admission; this change owns presentation. Archive core first. Fleet deltas here add new requirements and do not duplicate core's modified requirements.

One change per UX widget would obscure the common presentation vocabulary; bundling UX into core would make ownership validation depend on unrelated polish. Keep one UX change with E1–E4 task groups and independently reviewable commits.

### 2. Catalogue-driven discovery, not another action registry

Use the core typed catalogue, current availability and effective local bindings for palette/help/footer. Use the same matching primitive as appropriate for local command search, but do not turn the shared fleet into a local navigation copy. Keep palette query/selection local; invocation sends the captured target evidence and typed command through the established session path.

Surface active focus/editor/mode and pending leader completions. The key resolver's actual timeout/failure/cancellation policy comes from core, not another keybinding library. Commands shown as disabled still need actor/domain enforcement.

### 3. Reuse components in inline and detached roots

Keep Picker<T> independent of socket and process lifetime. Its owner handles confirmation/cancellation and directly submits the typed session action. Inline root removes the component; detached root waits for acknowledgement and exits. The same component should show source diagnostics, empty results and fallback choices consistently.

`tui-popup` is only an inline frame-overlay candidate, not tmux orchestration. Do not rebuild C's host protocol here. Local error/help/outcome overlays may capture local input but never claim a global modal gate. When a shared gate exists its indication and Escape cancellation remain dominant.

### 4. Inspect operations and recorded events without manufacturing history

Build a local inspector from shared pending/result references and existing request read/list APIs. Separate domain admission from UI receipt, and pending from refusal/unresolved/completion. Footer expiry does not destroy retained outcome access. Scrolling/closing is local; any shared acknowledgement is an explicit action, not an accidental effect of closing a panel.

Add a bounded subject-relevant event view using existing events.read cursor/expiry behavior and public request links. Show request IDs, recorded timestamps and available failure details. Separate authoritative operation records, recorded lifecycle events, observation-derived status and diagnostic information; do not present them as one equivalently authoritative history. Correlate by established subject/execution/request links, never wall-clock proximity or a reused pane. Expose retention, cursor gaps and unavailable sources. No new log backend, exhaustive subject-history index or operation↔subject navigation is required initially.

Keep source provenance, freshness and missing-record diagnostics. A restart may lose UI associations; list only what available domain records establish and label its boundedness. Keep command/environment/recipe values out of public logs and inspector summaries under current privacy rules. No retry button may bypass existing explicit command checks.

### 5. Local layout and text selection

Provide a discoverable split-sizing control with visible keyboard focus and resize clamping. Preserve current detail-page/process identity behavior. Evaluate rat-widget's splitter/focus or rat-text only against a concrete gap; no second application runtime and no actor involvement in local geometry.

Structured copy is the first increment: identifiers, paths and displayed text via a local capability-aware adapter. Then evaluate tui-panel-select for logical content-relative selection, wrapping, resize and scroll. Preserve its useful selection model without adopting its terminal guard/event loop: Radar keeps one terminal owner and existing cleanup. Reset selection on content/subject identity replacement.

Clipboard policy is local/configured. Local tools and OSC52 have different acknowledgement limits; report request submission versus verified local completion honestly. Never execute copied strings. A host that cannot support clipboard delivery reports that limitation. Do not claim tmux transparency without user acceptance.

### 6. Semantic theme vocabulary before animation

Use Radar-owned styles for focused/unfocused selection, muted/retained content, pending, refusal, unresolved and blocking, with explicit text/marks as well as color. Reuse current color configuration and document any new slots; do not rename working slots as a theme-framework migration. Keep width-dependent priorities and diagnostics consistent across roots.

Only add transitions tied to established changes. Existing spinner/motion settings remain authoritative. TachyonFX is an optional implementation choice if a specific transition improves comprehension; it is not required merely to complete a dependency checklist. Do not invent percentage progress or animate retained observations as live activity.

### 7. Choose libraries per capability, not as a bundle

| Candidate | Evaluation seam | Avoid |
| --- | --- | --- |
| tui-input / tui-widget-list | picker query/list ergonomics | replacing Picker<T> solely for uniformity |
| tui-popup / tui-widgets | inline overlay geometry/chrome | assuming detached-popup delivery is supplied |
| tui-scrollview | outcome/long-content rendering | duplicating operation records |
| tui-tree-widget | rendering/navigation ergonomics | local authority over shared selection/folds |
| tui-panel-select | local logical text selection/copy | second terminal owner or unverified clipboard success |
| rat-widget / rat-text | splitters/focus/editors | importing an overlapping app framework |
| theme examples / TachyonFX | semantic styling / selected transitions | unrelated dependency and motion expansion |

Verify exact Ratatui/Crossterm types, Rust minimum version, license and Nix closure impact before adding a crate. A failed candidate is not a reason to implement a general framework. Record a specific gap and choose the smallest acceptable component. The specified copy/selection behavior is a deliverable, not satisfied merely by evaluating a crate.

## Risks / Trade-offs

- Concurrent D/E edits collide → stabilize A–C and assign frontend ownership before overlapping work; no public-contract changes hidden in UX.
- A widget reintroduces local canonical state → controlled rendering adapter over the shared projection; canonical commands remain actor-owned.
- Clipboard methods differ across hosts → explicit mechanism/limits and user-run tmux acceptance; test the adapter's owned decisions, not external clipboard internals.
- Screen snapshots become brittle → extend current focused TestBackend assertions first; use a few named-width snapshots only for deliberate layout contracts.
- Motion obscures errors or stale facts → semantic text/marks first, motion off remains fully legible.

## Migration Plan

Implement E1 discovery/pickers, E2 operation inspector, E3 panel sizing/copy/selection and E4 vocabulary/polish as incremental task groups. Existing UI remains usable after each. Preserve core's public/session contracts and current config meanings. Do not ship feature flags or compatibility branches solely to carry the old widget implementation alongside its replacement.

Reuse normalized fixtures and TestBackend helpers. Probe new primitives in existing tests/examples where possible rather than a live smoke harness. User performs real popup, nested-terminal copy, resize and motion acceptance. Run relevant frontend tests per increment and the established final gate when completing the change. No unrelated worktree cleanup belongs here.

## Open Questions

Widget versions, exact split-resize bindings and optional transition timing remain local implementation choices. Select them against the effective keymap and documented compatibility constraints. A choice that changes shared command semantics must go back to the core contract rather than being treated as UX configuration.

# Tasks

Start implementation only after `shared-ui-session` milestones A–C are demonstrated and its frontend contracts are stable. Coordinate file ownership if core D continues in parallel. Candidate library adoption is subordinate to the specified behavior, compatibility and existing component seams.

## 1. E1 — Command discovery and reusable picker feedback

- [ ] 1.1 Build contextual help/footer from the established typed catalogue, availability and effective bindings; use focused frontend tests to verify rebound keys, unavailable reasons and local input-context precedence without a second command registry.
- [ ] 1.2 Add local fuzzy palette matching and useful binding/description rendering using the established matcher; extend existing picker/palette tests for empty results, cancellation and captured-target invocation through the real semantic command path.
- [ ] 1.3 Surface pending leader/sequence state and reachable completions; verify timeout, failed prefix, Escape and shared-gate changes clear it without submitting a delayed command.
- [ ] 1.4 Improve preparation/loading/empty/diagnostic states in the shared picker component for both roots; evaluate tui-input/list or tui-popup only for a demonstrated gap, verify pinned dependency compatibility if adopted, and test equivalent inline/detached result semantics.
- [ ] 1.5 Document help/palette bindings and private versus shared interaction behavior; verify examples match the effective keymap and include the source-failure fallback state.

## 2. E2 — Operation and event inspection

- [ ] 2.1 Add a local inspector backed by retained session references and existing request reads/listing; test pending/refused/unresolved/completed distinctions and a refusal that causes no fleet change.
- [ ] 2.2 Preserve inspector access after footer expiry, expose retention/stale/read-failure limits and separate local closure from shared acknowledgement; verify inspection issues no operation retry or canonical selection mutation.
- [ ] 2.3 Add a bounded subject-relevant lifecycle-event view using existing events.read and request links; show recorded timestamps/IDs/failure details, distinguish record/event/observation/diagnostic provenance, and test cursor expiry, missing associations and reused-pane identity without inventing history or a log backend.
- [ ] 2.4 Improve long-content rendering, evaluating tui-scrollview only if existing scrolling is insufficient; extend TestBackend checks for one narrow and one normal layout, sanitize text and document bounded history rather than claiming restart restoration.

## 3. E3 — Local sizing, focus and copy/selection

- [ ] 3.1 Add discoverable local split sizing and clear focus cues with resize clamping; evaluate a focused rat-widget primitive if useful and verify another frontend's geometry/shared selection and existing details-page behavior are unchanged.
- [ ] 3.2 Add structured local copy actions for identifiers, paths and applicable displayed text behind one clipboard adapter; test selected-value identity, absent values, sanitization and unsupported/failed mechanism reporting without external clipboard smoke tests.
- [ ] 3.3 Evaluate and implement panel-confined logical text selection, preferably through tui-panel-select if compatible; verify wrapping/resize/scroll, replacement-content reset and no neighbor-panel capture while retaining Radar's single terminal/event-loop ownership.
- [ ] 3.4 Document copy/selection controls and clipboard/OSC52 limitations; include a concise user-run acceptance procedure for the supported tmux path without claiming unverified remote clipboard success.

## 4. E4 — Consistent status vocabulary and targeted polish

- [ ] 4.1 Apply one semantic style vocabulary across dashboard, sidebar and presenter while preserving existing color slots/default diagnostics; test focused/unfocused selection, shared blocking, stale facts and unresolved-versus-completed outcomes at representative widths.
- [ ] 4.2 Improve tree mark/spacing priorities without changing canonical folds/order; compare any candidate tree widget only through a controlled renderer and verify existing task ancestry/identity rendering tests still pass.
- [ ] 4.3 Preserve motion-off and idle redraw behavior; add a targeted transition only if it improves a named state change, otherwise record the no-new-effects decision. Verify no fabricated progress and no animation of retained facts; evaluate TachyonFX only for that targeted need.
- [ ] 4.4 Update theme/status and local-panel documentation with the implemented behaviors; use Keep the Why in `context/ui-session.md` (or the existing relevant topic) for confirmed dependency/architecture forks and rejected alternatives. Add only a few intentional visual snapshots if existing TestBackend assertions cannot express the changed layout contract.

## 5. Integrated verification

- [ ] 5.1 Run relevant frontend tests per increment and the established completion gate (format, strict Clippy, Rust/SDK suites as affected, schema guards where applicable, strict OpenSpec validation and Nix); make new source Git-visible before Nix and record exact results.
- [ ] 5.2 Have the user accept inline/detached picker parity, contextual help, outcome inspection, resizing, panel selection/copy and motion settings on the actual host; leave unchecked until real observations are supplied.

## Workflow follow-up

- Archive `shared-ui-session` before this UX change; its published contracts are prerequisites, not deltas to reapply here.
- Keep optional crate/version decisions bounded to the accepted capability; do not use a widget limitation to introduce a terminal/mux rewrite or another application framework.
- Commit/push/tag only under the project's explicit user-approved workflow.

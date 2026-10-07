## 1. Verified per-member samples

- [x] 1.1 Extend the process model and shared confirmed scan with observed name, state, parent birth identity and per-member CPU/RSS; project descendants from the same members used by qualified totals, with no extra per-child scan. Add sampler regressions for names, two-sample CPU, PID reuse, disappearance, reparenting/subtree exclusion and incomplete scans; verify root exclusion and unchanged bounded read counts with the focused sampler tests and Clippy.

## 2. Processes table presentation

- [x] 2.1 Add an initially collapsed process table to the existing details line model with a root anchor, deterministic parent/child ordering and aligned name/PID/CPU/RSS columns. Keep qualified descendant totals separate and hide stale/retained samples. Update the Processes documentation and add wide/narrow/short-panel render tests proving that rows do not overlap or wrap into adjacent processes, unavailable values remain distinct from zero, and child selection never borrows root binary identity.

## 3. Selection and branch folding

- [x] 3.1 Add birth-identity-keyed process selection and branch folds, explicit `t` process-row navigation, mouse row/fold targets and selected-process compact details. Preserve page switching, panel hand-off, scrolling and lifecycle refusal from details. Add app/layout regressions for disappearance, PID reuse, root changes, folded selected descendants, resize, cursor visibility and no control/focus effects; document the keys and verify a PTY navigation/folding smoke.

## 4. Independent integration verification

- [x] 4.1 Independently run formatting, Clippy with warnings denied, the full locked test suite, both PTY smoke scenarios, the checked Nix package build and strict OpenSpec validation. Record the exact gates and any live-observation limits in verification.md; confirm the combined descendant table remains usable with the existing compact identity disclosures and Source lineage details.

## Workflow follow-up

- Apply in thin sequential slices on a retained worker after planning review.
- Sync and archive after implementation acceptance; commit/push only when authorized.

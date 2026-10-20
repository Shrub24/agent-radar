# Tasks

## 1. Contract

- [x] 1.1 Replace `context_usage` with `context_window { tokens, window }` in `FactsSnapshot`, with its validation and the existing unknown-field refusal. Test valid values, zero window, tokens above window, a negative or non-integer value, and omission withdrawing the fact.
- [x] 1.2 Carry the typed value into `HerdsmanFacts`, `registry_observation::project` and the observation model; keep the Herdr token as a percentage-only legacy input. Test that registry counts are not shown as a Herdr percentage and the reverse.

## 2. Display

- [x] 2.1 Derive and render the percentage from the typed value in `ui.rs`, and drop text parsing. Test the rendered percentage for a count and window and the legacy percent-only row.

## 3. Fixtures and documentation

- [x] 3.1 Update the canonical fixtures, `tests/agent_publisher.py` and `docs/control-plane.md` to the new field.
- [x] 3.2 Record in `ideas.md` that the Pi extension and Herdsman publishers must publish `context_window`, and run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; record real exit codes.

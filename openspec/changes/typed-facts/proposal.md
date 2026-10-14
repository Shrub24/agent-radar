# Proposal

## Why

The facts channel carries `context_usage` as display text such as `43%`. A consumer cannot compute with it, and the Radar display can only echo it. The Pi extension knows the token count and the window. Publishing them as integers lets every reader derive the percentage, a remaining budget, or a threshold, and removes a free-text field from a contract whose other fields are typed.

## What Changes

- **BREAKING**: replace `context_usage` (text) in the facts channel with `context_window`: `{ "tokens": <int>, "window": <int> }`. `tokens` is the tokens currently in context and `window` the model's context size. Validation: both are non-negative integers, `window` is greater than zero, and `tokens` does not exceed `window`. Omission withdraws the fact, like every other optional field.
- The normalized model carries the same typed value. The display derives the percentage from it and no longer parses or echoes publisher text.
- The Herdr token `context_usage` keeps working only as a percentage-only legacy input, shown as a percentage and never converted to counts it does not have, until the Herdr path is removed.
- Update the canonical fixtures, `docs/control-plane.md` and the publisher fixture.

The name is `context_window` rather than `context` because `agent.context` already names the session identity.

Non-goals: typing the other facts (`model`, `provider`, `thinking`, `session_name` stay display text); changing task entries; generated contract types (a separate effort tied to extracting the Pi extension); compatibility with the old field.

## Capabilities

### Modified Capabilities

- `agent-registration`: the facts channel's context usage becomes a typed token count and window.

## Impact

- `src/control_plane/registry/publication.rs` (`FactsSnapshot` and validation), `src/model.rs`, `src/registry_observation.rs`, `src/herdr.rs`, `src/ui.rs`, `tests/agent_publisher.py`, canonical fixtures and docs.
- Outside this repo: the Pi extension and Herdsman publishers must publish the new field; until they do, context usage is not shown for them.

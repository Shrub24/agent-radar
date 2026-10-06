# Verification

## Unmanaged close (task 1.2)

The owner independently ran the sequential gate after delegated implementation and the identity-guard correction:

- Format and all-target locked Clippy with warnings denied: pass.
- Locked tests: 321 passed, 0 failed (193 library, 1 animation, 20 bus, 12 bus detail, 10 collector, 1 descendants, 4 focus, 12 lifecycle, 37 nested tree, 20 runtime, 11 stale binary).
- Locked build: pass.
- Both PTY smoke scenarios: pass.
- Strict lifecycle-controls validation: pass.

Review found confirmation/worker initially checked target id and containment without comparing occupants. The worker added a normalized identity guard, with a reported red-before-fix regression and tests for session replacement, tab membership drift, unchanged close and selection movement. The owner reviewed the guard and reran the whole gate before acceptance.

Direct close remains non-atomic across mux inventory and command. It requires positive unmanaged containment, default-Cancel confirmation, latest-observation identity checking and another off-thread inventory/identity check. No real pane was closed during automated verification.

## Managed file client (task 2.1)

Owner review rejected the first port because it reduced the upstream fixture and misstated provenance. The correction preserves all 15 request, 10 result and 4 derivation cases, with only a source key added. Owner independently compared source-free JSON to the canonical upstream document and verified SHA-256 `d5e882d418775d3aeea5d94a7360a0c190c0de821084b455e0f30336820409e1`.

Review also corrected expected-operation correlation for all outcomes, bounded regular-file reads, per-call exact 0700 directory trust and target UUID validation. No red-before-fix run was captured for these corrections; this is not claimed as test-first evidence.

The owner's sequential gate passed format, all-target Clippy, 342 locked tests (196 library and 18 control boundary tests), locked build, both PTY smokes and strict lifecycle validation. No dependencies were added. ## Managed UI and final integration (tasks 2.2 and 3.1)

Owner review found and corrected Unknown-result handling releasing duplicate suppression, and owner-idle/derived-Unknown permitting restart. The worker captured failing regressions before the correction; the owner reviewed the retained Unknown guard and ran the final sequential gate independently.

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked --quiet`: 362 passed, 0 failed (207 library, 1 animation, 20 bus, 12 bus detail, 10 collector, 18 control, 1 descendants, 4 focus, 12 lifecycle, 9 managed, 37 nested tree, 20 runtime, 11 stale binary).
- `cargo build --locked`: pass.
- `python3 tests/terminal_smoke.py`: both scenarios pass.
- `openspec validate --all --strict`: 10 items pass, 0 fail. Existing long-requirement informational notices remain.

All five tasks are accepted. Managed operations use the exact owner/label/run identity and file client, never mux lifecycle; direct close stays positively-unmanaged only. Pending, Started and Unknown outcomes suppress duplicates; notice dismissal does not release the target. Restart requires both owner-advertised and derived idle. Published request files survive shutdown; no real control request, worker close or restart was performed during checks.

Live destructive Radar-to-owner smoke remains operator-run and is not claimed. Existing owner processes must reload to create/watch trusted control directories. Upstream owner acceptance is recorded in design.md; its checks are separate from Radar verification. No live Radar-to-owner destructive smoke is claimed.

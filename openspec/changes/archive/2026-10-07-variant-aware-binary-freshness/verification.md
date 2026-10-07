# Verification

## Independent gate — 2026-10-07

Primary-session command:

```sh
nix develop --command bash -c 'cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --locked && python3 tests/terminal_smoke.py'
openspec validate --all --strict
nix build .#checks.x86_64-linux.package --no-link
```

The combined managed command completed with exit 0 (bg-829, certified result retrieved). Formatting, warnings-denied Clippy, locked tests/build, both PTY smoke scenarios and strict OpenSpec validation passed. Checked Linux package built successfully at `/nix/store/s97zxfwgxpm71s0cpy0k9v8n7m84ksbv-agent-radar-0.1.0`, via the configured remote builder.

The resolver review correction requires a versioned target of the exact same family and an absolute store `bin/<program>` entrypoint without dot components. Regression cases include unrelated versioned packages, unversioned/further-hop launchers, non-bin targets and dot components. Main/child remain separate even though their payloads are named `pi`. Same-version rebuilt store roots remain stale.

Processes rendering was independently checked with current, stale and all typed unknown reasons, shared agent/pane presentation, sanitization and retained/stale withholding. Process arguments are deliberately not rendered on this page because positional prompts cannot safely be distinguished from flags.

## Compact presentation follow-up — 2026-10-07

After the first presentation landed the reader rejected it as a wall of prose: a
full sentence per fact, the boot id and descendant caveat always on screen, the
executable repeating the package's store prefix, and a `current` verdict that
said nothing new. Two slices replaced it, each verified in the primary session
with the full gate (fmt, Clippy `-D warnings`, locked tests and build, and both
PTY smoke scenarios):

1. **Metric sections.** `process` and `descendants` headings, short labelled
   values sharing a line where both fit the narrowest panel, short enumerable
   kernel-state names, and every honesty property kept: a measured zero differs
   from a first sample or an unavailable interval, a partial total shows its
   lower-bound mark and its reason, and each reason is drawn under the value it
   qualifies.
2. **Identity and one block.** The visible identity is the observed name, the
   package, the executable relative to it and a verdict only when there is one
   (stale, a different build, or a short unknown reason). Both package roots
   whole, the whole executable path, the birth identity, each state's meaning,
   the descendant qualification and the whole reasons moved into one `details`
   block, which the page offers only where it draws a live process.

One defect found in review and fixed in the primary session: the block was offered
on a stale-source row, where the page draws the process withheld, so `Enter` would
toggle a block with no marker. The registry now consults the same cached
source-current flag the focus targets use, and a regression covers it.

## Limits

- Accepted read-only resolver matrix is in `live-observation.md`: sampled lead PID 69343 and child PID 395646 both resolve Current against their exact respective family. The current cross-root literal launchers were exercised. Same-root, stale and unknown cases were tested in fixtures but not observed live; this matrix is not an exhaustive host survey.
- No destructive lifecycle smoke, process-environment reads or wrapper execution.
- No loaded extension/plugin build metadata claim; packaging metadata consumption is deferred.
- Linux package only; other platforms were not exercised.

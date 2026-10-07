# Verification

## Independent gate — 2026-10-07

Primary-session command:

```sh
nix develop --command bash -c 'cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --locked && python3 tests/terminal_smoke.py'
openspec validate --all --strict
nix build .#checks.x86_64-linux.package --no-link
```

The combined managed command completed with exit 0 (bg-829, certified result retrieved). Formatting, warnings-denied Clippy, locked tests/build, both PTY smoke scenarios and strict OpenSpec validation passed. Checked Linux package built successfully at `/nix/store/s97zxfwgxpm71s0cpy0k9v8n7m84ksbv-agent-radar-0.1.0`, via the configured remote builder. That build covered the resolver and the first presentation; the compact presentation follow-up below was gated the same way and its checked package built at `/nix/store/rjf2blgxd3v6msv1gyh1dmzjikfyql51-agent-radar-0.1.0` (bg-865, certified result retrieved).

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

## Follow-up: the block's rows reflowed

Read by the user and rebuilt in the primary session. The first block drew prose
lines whose labels changed with the facts present, repeated the store prefix on
every root and on the executable, carried a nine-line glossary of kernel states
the row was not in, and wrapped a fact too long for the panel to the start of the
following row — under its label rather than its value.

The block now draws one labelled row per fact in a column the rows share, wraps a
long value into that value column, states the shared store once with each root's
part past it, draws the executable as its place under the running root, and
carries the meaning of the state the row is actually showing. `render` computes
the panel's content width with the layout's own rule and the block context
carries it, so the drawing no longer re-breaks text it was handed.

- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked` (15 suites, 273 library tests), `cargo build --locked`,
  both `tests/terminal_smoke.py` scenarios and `openspec validate --all
  --strict` (13/13) pass.
- New regression `the_block_draws_facts_beside_their_labels_and_hangs_what_wraps`:
  one `/nix/store/` in the block, the labelled rows in one column, no state
  glossary, and a wrapped fact continued under the value column in the 90-column
  drawing. Nine identity tests were re-pointed to the row vocabulary.

Limit: at the narrowest side-by-side panel (28 content cells) a root longer than
the value column still breaks inside its own token, continuing under the value
column rather than under the label. Eliding the path was rejected, so the break
is the honest one for a path that cannot fit.

# Design

## Context

`App` holds the visible rows as a flat, ordered `Vec<VisibleRow>` built from the observation (`src/tree.rs`), and `ui.rs` draws them into a `List`. Selection is an index into that vector, reconciled by `RowId` after each refresh. Nothing today can reorder the rows or tell where a drawn row sits on screen.

## Decisions

### 1. Three orders, cycled by one key

`s` cycles `source` (the default: Herdr's own order, which is how the fleet is actually laid out) → `state` → `name` → back to `source`. The hint line names the current one (`s state`), in the same form as the pane view's `p running`, because a hidden mode that changes what you see is the worst kind. That form was chosen over an explicit `s sort: state` at implementation time: the hint line already pairs a key with the value it cycles, and one extra word per toggle is what pushes the hints past a line at 100 columns. There is no configuration key: order is a view you flip while looking, not a preference.

*Rejected:* a settings entry per order. It cannot be flipped while reading the tree, and the tree is where the choice is made.

*Rejected:* separate keys per order. Three keys to remember where one cycle plus a named hint does the same job.

### 2. Sorting is inside a level, and never across one

Rows are ordered per level: workspace headings at the top, then the rows under each heading, then their children. A child always stays under its parent and a descendant never overtakes an ancestor. With `state`, a heading sorts by the highest-priority state anywhere under it — so a workspace holding a blocked agent rises above an idle one — and within a heading rows sort by the same priority. With `name`, each level sorts alphabetically by the label it draws.

The state order is attention first: `blocked`, `lost`, `waiting`, `working`, `settling`, `unknown`, an unrecognised native word, `idle`, `done`, and a retained row last. The order is a list of ranks in one place, so adding a state is a one-line change.

Two ranks were added at implementation time. A native status Radar cannot name sits with `unknown`, just after it, because "unreadable" is the same kind of fact as "unknown". A retained row ranks after every observed one whatever state it holds: it keeps the last status anyone saw, so it cannot be what needs a human now. A pane row, having no state of its own, ranks last by the same rule.

### 3. A sort moves rows, not the cursor's subject

Selection is kept by `RowId`: the row the user was on stays selected wherever it moves, and if it disappears the existing reconciliation picks the nearest row. A sort never re-folds, never clears a filter, never changes the pane view, and never changes what the details panel shows.

### 4. The jump keys stop on a kind, and wrap

`n`/`N` stop on rows a human is needed for: `blocked`, `lost`, `waiting`, `unknown`, and a native word Radar cannot name (the same row the state order puts after `unknown`). `w`/`W` stop on `working` rows. Working rows are deliberately not in the attention set: an agent in flight is not asking for anything, and mixing the two makes the attention key land on almost every row. Both pairs wrap and are repeatable — press `n` again to keep going, which is the vim habit the keys come from.

*Rejected:* one key that jumps to "the next interesting row". Two kinds of interesting do not belong on one key.

*Deferred:* bracket keys for jumping between levels (`[` `]` for workspace, `{` `}` for tab), and making `n` repeat the last filter match as vim does. The first needs a level concept the flattened row list does not have yet; the second collides with `n`'s attention meaning here, where the filter is entered with `/` and applied with `Enter`.

### 5. Mouse capture, and clicking twice means focus

Mouse capture is enabled with raw mode at startup and disabled on exit, including on the panic path the terminal restoration already covers. Hit-testing uses the area the tree was drawn into and the scroll offset, so a click maps to a `RowId` rather than to a screen line.

- **Wheel over the tree** scrolls the tree. Over the details panel it scrolls the details.
- **Click on a workspace heading** folds or unfolds it, exactly as `Space` does on that row (and selects it).
- **Click on a row** selects it.
- **Click on the row that is already selected** focuses its pane, sending nothing when the row has no observed pane — the same rule as `Enter`.

Clicking twice rather than once to act on a row is deliberate: a single click has to be safe to do while browsing, and a focus request moves the user's terminal. A double-click timer is not needed, because the second click on a selected row is unambiguous.

*Rejected:* focus on any single click. Browsing the fleet would move Herdr's focus under the user.

*Rejected:* mouse disabled by default with a toggle. The wheel is the reason to have mouse support at all, and a toggle makes it a mode to notice rather than a gesture to use.

### 6. Capture costs text selection, and that is stated

A terminal that has released mouse events to an application does not select text on drag until the user holds Shift (or whatever the terminal uses). Herdr passes the choice through. This is written in the README beside the keys, because a user who cannot copy a pane title will otherwise think Radar is broken.

# Tasks

## 1. Tree grouping

- [x] 1.1 Group each workspace's agents by window and nest the window's others under its single root agent, falling back to flat placement for a window with no root or several roots.
- [x] 1.2 Nest the window's ordinary panes beneath its tab row, and keep plain-pane windows flat.
- [x] 1.3 Replace the owner-across-tabs test with window-grouping tests: one root, cross-window owner, several roots, and a no-agent window.

## 2. Verification

- [x] 2.1 Fmt, strict Clippy, the full locked suite and both PTY smokes pass.
- [x] 2.2 Strict OpenSpec validation passes.

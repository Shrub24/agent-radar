# Design

## Context

Radar changes nothing in Herdr today; every Herdr call is a read (`src/collector.rs`). Herdr exposes `workspace focus <id>` and `tab focus <id>` on its CLI, but `pane focus` there moves by direction only. The socket API schema (`herdr api schema --json`) lists `pane.focus`, `tab.focus` and `workspace.focus` requests.

## Decisions

### 1. Focus is the one action, and it is not a command channel

Radar asks Herdr to focus a location it observed. It sends nothing that changes an agent, a pane's contents or a layout. This is the narrow answer to the open question in `plan.md` about what Radar may do to Herdr; anything beyond focus is a separate decision.

### 2. Use the CLI where it suffices and the socket request where it does not

Workspace and tab focus use `herdr workspace focus` and `herdr tab focus`. Pane focus has no CLI form by id, so the implementation issues the schema's `pane.focus` request (the worker reads the schema for its exact shape and the socket path Herdr documents). If a single request focuses all three levels, it is used alone. Whatever is chosen runs off the UI thread with the collector's timeout and reaping rules, so a slow Herdr never freezes input or delays quitting.

### 3. The target is the row's observed location

An agent or pane row focuses its pane. A workspace row focuses the workspace. A row without a live pane (a retained row whose pane Radar can no longer prove, or a stale source) does nothing and says why, rather than focusing a location Radar is not sure of.

### 4. Radar stays open, and says when it failed

Focusing moves the user's attention, not Radar's: Radar keeps running and keeps its selection. A failure (Herdr unreachable, pane gone, timeout) appears as a one-line message that clears on the next key press or after a few seconds; it never replaces the fleet diagnostic.

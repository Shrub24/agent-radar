# Proposal

## Why

Radar shows the whole fleet but cannot take the user to anything in it. Finding an agent and then locating its pane by hand in Herdr is the step that makes the overview a dead end.

## What Changes

- `Enter` on a row asks Herdr to focus that row's workspace, tab and pane, and Radar stays open.
- Focus is the only thing Radar asks Herdr to do. It is a deliberate, narrow exception to Radar being a read-only observer, and it changes nothing about an agent's state.
- A refusal or failure is stated in the interface and does not disturb the view.

Out of scope: any other action on Herdr or on an agent (stop, close, send input); mouse and click-to-focus (a later change); following the selection with focus; leaving Radar after focusing.

## Capabilities

### New Capabilities

- `focus-action`: Focusing the selected row's pane in Herdr, and what happens when it cannot be done.

### Modified Capabilities

None.

## Impact

Adds a small module that issues the focus request off the UI thread, an `Enter` binding and a hint, and a transient failure line. No new dependency. Herdr's CLI offers workspace and tab focus by id but pane focus only by direction; its socket API has a `pane.focus` request, which is the one place Radar will speak the socket protocol directly.

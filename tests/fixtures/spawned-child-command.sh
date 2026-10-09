#!/bin/sh
set -eu
state="$RADAR_SPAWN_FAKE_STATE"
if [ "$1" = status ] && [ "${2:-}" = --json ]; then
  printf '{"server":{"socket":"%s/herdr.sock"}}\n' "$state"
elif [ "$1" = api ] && [ "$2" = snapshot ]; then
  if [ -f "$state/created" ]; then
    printf '{"result":{"snapshot":{"workspaces":[],"tabs":[],"panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"},{"pane_id":"wA:p2","tab_id":"wA:t1","workspace_id":"wA"}],"agents":[]}}}\n'
  else
    printf '{"result":{"snapshot":{"workspaces":[],"tabs":[],"panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"}],"agents":[]}}}\n'
  fi
else
  printf 'unexpected fake Herdr invocation: %s\n' "$*" >&2
  exit 2
fi

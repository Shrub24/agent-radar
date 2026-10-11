#!/bin/sh
# Succeeds at the preflight pane-option command, then emits its argv on a
# simulated dispatched-launch failure. The adapter must not surface it.
case "$*" in
  *"set-option"*) exit 0 ;;
  *"respawn-pane"*) printf '%s\n' "$@" >&2; exit 1 ;;
  *) exit 1 ;;
esac

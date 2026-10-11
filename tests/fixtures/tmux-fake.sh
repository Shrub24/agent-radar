#!/bin/sh
# A stand-in for `tmux` that records what it was asked to do and then fails, so
# an adapter test can read back the real command line without a server.
#
# State is keyed to the process that ran it and the server it was pointed at:
# every invocation carries `-L <server>` first, and the log belongs to the one
# test that chose that server name, so tests never share a file.
{ printf '%s|' "${TMUX-<unset>}"; printf '%s ' "$@"; printf '\n'; } >>"${TMPDIR:-/tmp}/tmux-fake-$PPID-$2"
exit 1

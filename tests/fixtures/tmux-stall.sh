#!/bin/sh
# A stand-in for `tmux` that never answers, so a test can watch the adapter's own
# deadline or cancellation end it. It records its pid under state keyed to the
# process that ran it and the server it was pointed at, so the test that killed
# it can prove the process is gone.
echo $$ >"${TMPDIR:-/tmp}/tmux-stall-$PPID-$2"
sleep 300

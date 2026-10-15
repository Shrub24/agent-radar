#!/bin/sh
# A stand-in for `tmux` that answers record reads the way tmux 3.5a does: the
# adapter's field-separator byte arrives as tmux's `\ooo` escape, the literal
# text `\037`. tmux 3.7c writes the byte in the same records; the adapter must
# read both as the same record. The shapes here are the ones real tmux 3.5a
# answered Radar's formats with.
#
# `printf '%s'` prints its argument's bytes, so the four characters stay literal
# here — a backslash in printf's own format string would become the byte.
case "$*" in
  *pane_dead*) printf '%s\n' '%1\0371\0370\037\0371700000001' ;;
  *list-sessions*) printf '%s\n' '$0\037/tmp/herdr\037herdsman' ;;
  *list-windows*) printf '%s\n' '@1\037$0\0372\037editor' ;;
  *list-panes*) printf '%s\n' '%1\037@1\037$0\037/tmp/herdr\037a title' ;;
  *display-message*) printf '%s\n' '4242\037pi' ;;
  *new-window*) printf '%s\n' '@2\037$0' ;;
  *split-window*) printf '%s\n' '%3\037@2\037$0' ;;
  *) exit 1 ;;
esac

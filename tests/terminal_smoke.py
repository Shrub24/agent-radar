#!/usr/bin/env python3
"""Check the production loop against a controlled fake CLI and a stub publisher.

Two things are exercised end to end: source failure/recovery with its terminal
cleanup, and the bus — a real client over the socket Radar binds in this process,
whose tasks must reach the selected agent's details, and which must not delay a
quit while it is connected. The details are read through the keys a reader uses:
Tab into the panel, ←/→ to the page a fact lives on, Space and Enter to open the
block a page holds behind its marker, Escape back to the tree.
"""

import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

# The session UUID the fake pane publishes as `pi_herdsman_session`, and the
# session the stub publisher greets with.
SESSION = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"


def check(binary, bus_off=False):
    """One dashboard run. `bus_off` squats the socket first, so binding the bus
    fails and the run must stay usable and quit promptly anyway."""
    with tempfile.TemporaryDirectory(prefix="radar-terminal-") as directory:
        directory = Path(directory)
        ready = directory / "ready"
        mode = directory / "mode"
        mode.write_text("error\n")
        # One agent whose pane carries the Herdsman session token, so a bus
        # publisher can join to it, and the background tasks it reports: two
        # outstanding, both running, with the count that speaks for them.
        snapshot = directory / "snapshot.json"
        snapshot.write_text(json.dumps({"result": {"snapshot": {
            "workspaces": [{"workspace_id": "wA", "label": "main", "number": 1}],
            "tabs": [{"tab_id": "wA:t1", "workspace_id": "wA", "label": "agent tab", "number": 1}],
            "panes": [{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                       "label": "worker task", "terminal_title_stripped": "worker task"}],
            "agents": [{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                        "agent": "pi", "agent_status": "working",
                        "terminal_title_stripped": "worker task",
                        "tokens": {"pi_herdsman_session": SESSION, "pi_bg_running": "2",
                                   "pi_bg_tasks": "bg-7:running,bg-8:running"}}],
        }}}) + "\n")
        socket_path = directory / "radar.sock"
        # The Processes page's own process table needs a real process this
        # machine's sampler can read, with a process beneath it: a python
        # leader and the sleep it started. The fake CLI reports it as the
        # pane's foreground command, so the table is a tree of this run.
        helper = subprocess.Popen(
            [sys.executable, "-c",
             "import subprocess, time\n"
             "child = subprocess.Popen(['sleep', '120'])\n"
             "print(child.pid, flush=True)\n"
             "time.sleep(120)\n"],
            stdout=subprocess.PIPE, text=True,
        )
        child_pid = int(helper.stdout.readline().strip())
        process_info = directory / "process-info.json"
        process_info.write_text(json.dumps({"result": {"process_info": {
            # The group leader is the python process; the shell pid is some
            # other pid, which is what makes the foreground a command rather
            # than a shell.
            "shell_pid": 1,
            "foreground_process_group_id": helper.pid,
            "foreground_processes": [{"pid": helper.pid, "name": "python3",
                                      "cmdline": "python3 -c pass"}],
        }}}) + "\n")
        fake = directory / "herdr"
        fake.write_text(f'''#!/bin/sh
read -r mode < '{mode}'
case "$mode" in
  error) printf 'controlled source failure\\n' >&2; exit 7 ;;
  stall) printf '%s' $$ > '{ready}'; exec sleep 30 ;;
esac
if [ "$1" = "pane" ] && [ "$2" = "process-info" ]; then cat '{process_info}'; exit 0; fi
cat '{snapshot}'
''')
        fake.chmod(0o755)
        squatter = None
        if bus_off:
            # A listener that answers a connect: another Radar owns the path.
            squatter = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            squatter.bind(str(socket_path))
            squatter.listen(1)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        process = subprocess.Popen(
            [str(binary)], stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, "TERM": "xterm-256color", "PATH": f"{directory}:{os.environ['PATH']}",
                 "RADAR_SOCKET": str(socket_path)},
            start_new_session=True,
        )

        # Everything the dashboard has written, kept whole: a sequence written
        # once at startup must still be assertable after later frames.
        seen = bytearray()

        def drain():
            while select.select([master], [], [], 0)[0]:
                chunk = os.read(master, 65536)
                seen.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")

        def settle(seconds=0.35):
            # A key that is clamped to the layout of the frame before it — a
            # scroll is. Keys written in one batch are handled before that
            # draw, so a step waits for the redraw the spinner forces anyway.
            time.sleep(seconds)
            drain()

        def expect(text):
            deadline = time.monotonic() + 3
            while text not in seen and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    chunk = os.read(master, 65536)
                    seen.extend(chunk)
                    if b"\x1b[6n" in chunk:
                        os.write(master, b"\x1b[1;1R")
                assert process.poll() is None, f"dashboard exited: {bytes(seen)!r}"
            assert text in seen, f"missing {text!r}"

        def hello(client, session):
            # One hello per connection: a second one on the same connection is a
            # protocol error and closes it.
            client.sendall(json.dumps({"type": "hello", "v": 1, "session": session,
                                       "pane": "wA:p1", "ops": []}).encode() + b"\n")

        def tasks(client, items):
            client.sendall(json.dumps({"type": "tasks", "tasks": items}).encode() + b"\n")

        def connect():
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.connect(str(socket_path))
            return client

        def expect_panel(has, lacks=None):
            # The details panel on one freshly drawn frame. Hiding and showing
            # it makes the redraw write the panel whole: a redraw diffs against
            # the frame before it, so a phrase only survives in a fresh panel.
            # The page the panel is on is not part of hiding it.
            #
            # A phrase spanning a style change — a label and its value, two
            # words of help text — is written in runs with cursor moves between
            # them, so only a run of one style is assertable: whole words, not
            # sentences.
            deadline = time.monotonic() + 3
            last = b""
            while time.monotonic() < deadline:
                os.write(master, b"dd")
                output = b""
                until = time.monotonic() + 0.2
                while time.monotonic() < until:
                    if select.select([master], [], [], 0.02)[0]:
                        output += os.read(master, 65536)
                assert process.poll() is None, f"dashboard exited: {output!r}"
                if has in output and (lacks is None or lacks not in output):
                    return
                last = output
            raise AssertionError(
                f"the details panel never showed {has!r} without {lacks!r}: {last!r}"
            )

        def after(keys, text, timeout=3):
            # A phrase only the next frame carries: the bytes read from now on,
            # not the run's whole history, so a phrase the previous frame wrote
            # cannot stand in for one that is no longer drawn. `seen` keeps the
            # run either way.
            os.write(master, keys)
            fresh = bytearray()
            deadline = time.monotonic() + timeout
            while text not in fresh and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    chunk = os.read(master, 65536)
                    seen.extend(chunk)
                    fresh.extend(chunk)
                    if b"\x1b[6n" in chunk:
                        os.write(master, b"\x1b[1;1R")
                assert process.poll() is None, f"dashboard exited: {bytes(fresh)!r}"
            assert text in fresh, f"missing {text!r} after {keys!r}: {bytes(fresh)!r}"

        def focus_details():
            # Tab hands the details the keyboard, and their own key hints are
            # what says so: the tree's hints never name a scroll.
            after(b"\t", b"scroll")

        def escape_details():
            # Escape hands it back, and the tree's hints are back with it.
            after(b"\x1b", b"fold")

        client = None
        try:
            if bus_off:
                # The heading says the bus is off from the first frame, when
                # the source is still pending; that frame is the only one that
                # writes those cells.
                expect(b"bus off")
            expect(b"UNAVAILABLE")
            # Mouse capture is on from the first frame: a wheel turn and a click
            # only arrive while it is, and SGR mode is the sequence that says so.
            expect(b"\x1b[?1006h")
            if bus_off:
                # The fleet is observed as usual, and quitting waits for
                # nothing.
                mode.write_text("ok\n")
                expect(b"current")
                start = time.monotonic()
                os.write(master, b"q")
                process.wait(timeout=1)
                assert process.returncode == 0
                assert time.monotonic() - start < 1, "quit waited on the bus"
                drain()
                assert b"\x1b[?1006l" in seen, "mouse capture was not released"
                print("PASS: a socket another Radar owns leaves the fleet observed, says "
                      "`bus off`, and quits promptly")
                return
            mode.write_text("ok\n")
            expect(b"current")
            mode.write_text("error\n")
            expect(b"STALE")
            mode.write_text("ok\n")
            expect(b"current")

            # A real publisher over the socket this Radar bound. The pane token
            # says two are running; the list says one, which the details state
            # rather than resolve.
            deadline = time.monotonic() + 3
            while not socket_path.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert socket_path.exists(), "radar did not bind the bus socket"
            client = connect()
            hello(client, SESSION)
            tasks(client, [{"id": "bg-1", "state": "running",
                            "command": "nix build .#radar",
                            "cwd": "/home/dev/proj"}])
            os.write(master, b"j")  # the first row is the workspace; this selects the agent

            # The panel opens on Overview, where what the row is and what it
            # awaits are written. The published task is the Tasks page's, so it
            # is reached the way a reader reaches it: Tab into the details, →
            # past Processes, and Escape back to the tree.
            focus_details()
            expect_panel(b"awaiting:")
            focus_details()
            os.write(master, b"\x1b[C")  # Overview → Processes
            escape_details()
            focus_details()
            os.write(master, b"\x1b[C")  # Processes → Tasks
            expect_panel(b"bg-1")

            # The page's long text starts behind its marker: Space puts the
            # keyboard on the block and Enter opens it, and Enter closes it
            # again. The fresh bytes are what says so — the frame before the key
            # does not carry the directory, and the hidden-and-shown panel after
            # it does not either.
            focus_details()
            os.write(master, b" ")
            after(b"\r", b"/home/dev/proj")
            os.write(master, b"\r")
            expect_panel(b"bg-1", lacks=b"/home/dev/proj")

            # The page's own keys read the row instead of moving the fleet: `k`
            # scrolls the page, where unfocused it would select the row above and
            # take the joined task with the selection.
            focus_details()
            os.write(master, b"k")
            expect_panel(b"bg-1")

            # The publisher goes away: the task's detail goes with it and the
            # pane tokens are the row's facts again, on the page that carries
            # them.
            client.close()
            expect_panel(b"bg-7", lacks=b"bg-1")

            # A new connection takes the session over, and stays connected.
            client = connect()
            hello(client, SESSION)
            tasks(client, [{"id": "bg-3", "state": "flushing"}])
            expect_panel(b"bg-3")

            # The Processes page's process table, navigated the way a reader
            # does: the table is the real process tree this run started, the
            # python leader with a sleep beneath it. A taller panel first, so
            # the page's blocks are read whole rather than scrolled — the run's
            # own size comes back for the quit that is timed at the end; Space
            # and Enter open both blocks, t names the table's own mode, j moves
            # over its rows, Enter folds the root's branch away, and the same
            # key opens it again.
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 70, 100, 0, 0))
            settle()
            focus_details()
            os.write(master, b"\x1b[D")
            settle()  # Tasks → Processes
            os.write(master, b" \r \r")
            settle()  # both of the page's blocks open
            expect_panel(b"process table")
            focus_details()
            after(b"t", b"navigating")
            os.write(master, b"j")
            settle()  # the cursor moves to the process beneath the root
            os.write(master, b"k")
            settle()  # and back to the root
            os.write(master, b"\r")
            settle()  # Enter folds the root's branch
            expect_panel(str(helper.pid).encode(), lacks=str(child_pid).encode())
            focus_details()
            os.write(master, b"t")
            settle()
            os.write(master, b"\r")
            settle()  # Enter opens the branch again
            expect_panel(str(child_pid).encode())
            # Back to the table's ordinary page and then to the fleet, whose
            # own hints are what say the panel let the keyboard go.
            focus_details()
            escape_details()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
            settle()

            mode.write_text("stall\n")
            deadline = time.monotonic() + 3
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert ready.exists(), "collector did not start the stalled command"
            # Quit belongs to the main loop: q in filter entry must be text.
            os.write(master, b"/q")
            expect(b"filter:")
            start = time.monotonic()
            os.write(master, b"\x1b")
            assert select.select([master], [], [], 1)[0], "Escape did not redraw the view"
            os.read(master, 65536)
            os.write(master, b"q")
            process.wait(timeout=1)
            assert process.returncode == 0
            assert time.monotonic() - start < 1, "quit waited for the command timeout"
            # The publisher is still connected: quitting must not wait on it.
            assert client.fileno() != -1, "the client was closed before the quit"
            drain()
            assert b"\x1b[?1049l" in seen, "alternate screen was not restored"
            assert b"\x1b[?1006l" in seen, "mouse capture was not released"
            assert termios.tcgetattr(slave) == original, "terminal input modes changed"
        finally:
            if helper.poll() is None:
                helper.kill()
                helper.wait()
            helper.stdout.close()
            try:
                os.kill(child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            if client is not None:
                client.close()
            if squatter is not None:
                squatter.close()
            if process.poll() is None:
                process.kill()
                process.wait()
            if ready.exists():
                try:
                    os.killpg(int(ready.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            os.close(master)
            os.close(slave)
    print("PASS: source failure/empty/recovery; stub publisher connect/replace/disconnect; "
          "details focus, page cycling, a scroll key, a disclosure opened and closed "
          "with Space/Enter, process-row navigation with t, a branch folded and opened "
          "on the process table, and Escape back to the tree; "
          "q in filter entry; stalled quit with a client connected; mouse capture taken and "
          "released; terminal restoration")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, nargs="?")
    args = parser.parse_args()
    target = subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], text=True)
    binary = args.binary or Path(json.loads(target)["target_directory"]) / "debug" / "radar"
    binary = binary.resolve()
    check(binary)
    check(binary, bus_off=True)

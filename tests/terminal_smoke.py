#!/usr/bin/env python3
"""Check the production loop against a controlled fake CLI and a stub publisher.

Two things are exercised end to end: source failure/recovery with its terminal
cleanup, and the bus — a real client over the socket Radar binds in this process,
whose tasks must reach the selected agent's details, and which must not delay a
quit while it is connected.
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
        fake = directory / "herdr"
        fake.write_text(f'''#!/bin/sh
read -r mode < '{mode}'
case "$mode" in
  error) printf 'controlled source failure\\n' >&2; exit 7 ;;
  stall) printf '%s' $$ > '{ready}'; exec sleep 30 ;;
esac
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
                            "command": "nix build .#radar"}])
            os.write(master, b"j")  # the first row is the workspace; this selects the agent
            expect_panel(b"bg-1")

            # The publisher goes away: the task's detail goes with it and the
            # pane tokens are the row's facts again (the frame writes cells in
            # runs, so only whole words are assertable here).
            client.close()
            expect_panel(b"awaiting:", lacks=b"bg-1")

            # A new connection takes the session over, and stays connected.
            client = connect()
            hello(client, SESSION)
            tasks(client, [{"id": "bg-3", "state": "flushing"}])
            expect_panel(b"bg-3")

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

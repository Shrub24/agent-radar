#!/usr/bin/env python3
"""Acceptance: the dashboard reads every agent fact from a disposable daemon's
registry over a real disposable tmux server, with Herdr unavailable.

What this run establishes, end to end and on a real backend:

- One tmux server of our own whose two panes run a real foreground process, not
  a shell. The dashboard reaches it only through a disposable daemon whose
  physical backend is tmux.
- `herdr` is a fail-fast sentinel on PATH that records every invocation. The run
  asserts the dashboard made no Herdr call and drew no fallback diagnostic, so
  nothing read Herdr agent state.
- Registered agents publish execution, owner assignment and self facts
  independently. Two registrations join their own panes by the exact procfs
  birth identity of the pane's own process; one reports a pane the inventory
  does not hold and publishes nothing; one lets its facts lease expire.
- The three authorities keep their own values and freshness on the screen, and a
  session's finished execution never completes the owner's assignment.
- Close is refused for a registration, joined or not, with no confirmation.
- Every resource is removed on success and on failure: the dashboard, the
  daemon, the tmux server with its panes, and the sockets under one temporary
  directory. The run fails if anything survives.

usage: python3 tests/tmux_acceptance.py [binary] [--dump]
"""

import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "publisher", ROOT / "examples/agent-publisher" / "publisher.py")
publisher = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = publisher
spec.loader.exec_module(publisher)

SESSION = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"
OWNER = "accept-owner"
# The lease a channel that must read stale asks for, and the longest one.
STALE_LEASE_MS = 1_000
MAX_LEASE_MS = 300_000
# A real command, never a shell: `exec` makes the command the pane's own process,
# so the daemon names it and this harness can claim its birth identity.
PANE_PROGRAM = "sleep"
PANE_COMMAND = f"exec {PANE_PROGRAM} 900"
WINDOW = (40, 120)
# How long any one bounded wait here may take.
WAIT = 5
# One token of a drawn frame: a control sequence, or the text a run writes.
TOKEN = re.compile(r"\x1b\[([0-9;?]*)([A-Za-z])|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]|\x1b.|[^\x1b]+", re.S)


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def call(path, method, params):
    return publisher.call(path, method, params)


def birth_identity(pid):
    """This process's Linux boot id and procfs start-tick identity."""
    boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rfind(")") + 2:].split()
    return {"pid": pid, "boot_id": boot_id, "start_ticks": int(fields[19])}


def alive(pid):
    """Whether the pid is a live process. A zombie is finished work, not a
    process still holding a pane, and only a live one is a leftover."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except (FileNotFoundError, ProcessLookupError):
        return False
    return stat.rsplit(")", 1)[1].split()[0] != "Z"


def frame_text(raw):
    """The frame a byte stream draws, as lines of text.

    A redraw writes only the cells that changed, so a cell left blank is simply
    absent: text runs are joined without the spaces between them. The cursor
    position each run carries is the only thing that puts them back, so this
    reads the stream as a screen instead of deleting escapes from it. Cells never
    written stay blank, which is what they are after the panel was hidden.
    """
    cells = {}
    row, column = 1, 1
    for token in TOKEN.finditer(raw.decode("utf-8", "replace")):
        text = token.group(0)
        if text.startswith("\x1b["):
            if token.group(2) in ("H", "f"):
                params = [int(value) if value else 0
                          for value in (token.group(1) or "").split(";")]
                row = params[0] if params and params[0] else 1
                column = params[1] if len(params) > 1 and params[1] else 1
            continue
        if text.startswith("\x1b"):
            continue
        for character in text:
            if character == "\n":
                row, column = row + 1, 1
                continue
            if character == "\r":
                column = 1
                continue
            cells[(row, column)] = character
            column += 1
    if not cells:
        return ""
    lines = []
    for line in range(1, max(row for row, _ in cells) + 1):
        width = max((column for at, column in cells if at == line), default=0)
        lines.append("".join(cells.get((line, column), " ")
                             for column in range(1, width + 1)).rstrip())
    return "\n".join(lines)


class Fleet:
    """One tmux server, one daemon, one dashboard run, and their cleanup."""

    def __init__(self, binary):
        self.binary = binary
        self.root = Path(tempfile.mkdtemp(prefix="radar-tmux-acceptance-"))
        os.chmod(self.root, 0o700)
        self.server = f"radar-accept-{os.getpid()}"
        self.socket = self.root / "control.sock"
        self.state = self.root / "state"
        self.bus = self.root / "bus.sock"
        self.config = self.root / "config.toml"
        self.sentinel_log = self.root / "herdr.log"
        self.daemon_log = self.root / "daemon.log"
        self.daemon = None
        self.dashboard = None
        self.panes = {}
        self.seen = bytearray()

    # -- setup -------------------------------------------------------------

    def sentinel(self):
        """A `herdr` that fails fast and records that it was asked anything."""
        path = self.root / "herdr"
        path.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> {self.sentinel_log}\nexit 1\n')
        path.chmod(0o755)

    def tmux(self, *args):
        """One tmux command against this run's server, read as its output."""
        result = subprocess.run(["tmux", "-L", self.server, *args],
                                capture_output=True, text=True)
        check(result.returncode == 0,
              f"tmux {' '.join(args)} failed: {result.stderr.strip()}")
        return result

    def start_tmux(self):
        self.sentinel()
        # The server runs under the unit the daemon requires for its own servers, so
        # the daemon's ownership check sees the server it is meant to serve.
        # An installed template refuses a transient unit of the same name, so it
        # starts the server itself when present.
        if subprocess.run(["systemctl", "--user", "cat", "radar-tmux@.service"],
                          capture_output=True).returncode == 0:
            subprocess.run(["systemctl", "--user", "start",
                            f"radar-tmux@{self.server}.service"], check=True)
        else:
            subprocess.run(["systemd-run", "--user", "--collect", "--quiet",
                            f"--unit=radar-tmux@{self.server}",
                            "--property=Type=simple", "--property=KillMode=control-group",
                            "tmux", "-D", "-L", self.server, "-f", "/dev/null"], check=True)
        deadline = time.monotonic() + WAIT
        while subprocess.run(["tmux", "-L", self.server, "show-options", "-g"],
                             capture_output=True).returncode != 0:
            check(time.monotonic() < deadline, f"tmux server {self.server} never answered")
            time.sleep(0.05)
        subprocess.run(["tmux", "-L", self.server, "new-session",
                        "-d", "-x", str(WINDOW[1]), "-y", str(WINDOW[0]), PANE_COMMAND],
                       check=True)
        subprocess.run(["tmux", "-L", self.server, "new-window", "-d", PANE_COMMAND],
                       check=True)
        # The command replaces the shell it was started from, so the pane's own
        # process is the command. That takes a moment: wait, bounded, rather than
        # reading the shell that is still starting it.
        deadline = time.monotonic() + WAIT
        commands = []
        while time.monotonic() < deadline:
            listed = self.tmux("list-panes", "-a", "-F",
                               "#{pane_id}\u001f#{pane_pid}\u001f#{pane_current_command}").stdout
            panes = [line.split("\u001f") for line in listed.strip().splitlines()]
            commands = [pane[2] for pane in panes]
            if len(panes) == 2 and all(command == PANE_PROGRAM for command in commands):
                self.panes = {pane[0]: int(pane[1]) for pane in panes}
                break
            time.sleep(0.05)
        else:
            raise AssertionError(
                f"the panes never ran their own process: {commands!r}")
        for pane_id, pid in self.panes.items():
            check(alive(pid), f"the pane {pane_id} process {pid} is already gone")

    def start_daemon(self):
        env = {**os.environ, "RADAR_CONTROL_BACKEND": "tmux",
               "RADAR_TMUX_SERVER": self.server,
               "RADAR_CONTROL_SOCKET": str(self.socket),
               "RADAR_CONTROL_STATE": str(self.state),
               "PATH": f"{self.root}:{os.environ['PATH']}"}
        with open(self.daemon_log, "w") as log:
            self.daemon = subprocess.Popen([str(self.binary), "daemon"], env=env,
                                           stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + WAIT
        while time.monotonic() < deadline:
            check(self.daemon.poll() is None,
                  f"daemon exited {self.daemon.returncode}: {self.daemon_log.read_text()}")
            if self.socket.exists():
                try:
                    call(self.socket, "daemon.ping", {})
                    return
                except (OSError, RuntimeError):
                    pass
            time.sleep(0.02)
        raise AssertionError("daemon socket did not appear")

    def inventory(self):
        """The daemon's own view: the only location facts a registration may join
        on, which is why the registrations below are built from it."""
        return call(self.socket, "mux.observe", {})["inventory"]

    # -- registration ------------------------------------------------------

    def register(self, label, location, claim):
        """One registration. Each agent is its own source incarnation: an
        incarnation names exactly one registration, and reusing one for different
        content is refused rather than silently replaced. A label is optional, and
        one registration here is left without it."""
        registration = {"source": "acceptance", "incarnation": str(uuid.uuid4()),
                        "session": SESSION, "owner": OWNER, "run": "run-1",
                        "location": location}
        if label is not None:
            registration["label"] = label
        if claim is not None:
            registration["process"] = claim
        return call(self.socket, "agent.register", registration)["registration"]["agent_id"]

    def publish_execution(self, agent_id, activity):
        writer = call(self.socket, "agent.acquire", {
            "agent_id": agent_id, "channel": "execution",
            "publisher": {"source": "pi", "incarnation": str(uuid.uuid4())}})["writer"]
        channel = call(self.socket, "agent.publish", {
            "agent_id": agent_id, "channel": "execution", "writer_handle": writer["handle"],
            "sequence": 1, "snapshot": {"activity": activity},
            "lease_ms": MAX_LEASE_MS})["channel"]
        check(channel["snapshot"]["freshness"] == "fresh", str(channel))

    def publish_assignment(self, agent_id):
        writer = call(self.socket, "agent.acquire", {
            "agent_id": agent_id, "channel": "assignment",
            "publisher": {"source": "herdsman", "incarnation": str(uuid.uuid4()),
                          "reporting_owner": OWNER}})["writer"]
        channel = call(self.socket, "agent.publish", {
            "agent_id": agent_id, "channel": "assignment", "writer_handle": writer["handle"],
            "sequence": 1, "lease_ms": MAX_LEASE_MS,
            "snapshot": {"activity": "working", "role": "worker",
                         "definition": "acceptance-worker", "run": "run-1",
                         "assignment": "keep the acceptance honest",
                         "awaited": ["bg-1"], "actions": ["nix build .#radar"],
                         "waiting_reason": "reading the panel",
                         "last_outcome": {"result": "partial", "detail": "pending"}}})["channel"]
        check(channel["snapshot"]["freshness"] == "fresh", str(channel))

    def publish_facts(self, agent_id, lease_ms, snapshot):
        return publisher.publish_facts(
            self.socket, agent_id, {"source": "pi", "incarnation": str(uuid.uuid4())},
            snapshot, 1, lease_ms=lease_ms)

    # -- the dashboard -----------------------------------------------------

    def start_dashboard(self):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", *WINDOW, 0, 0))
        self.original_termios = termios.tcgetattr(slave)
        self.config.write_text("")
        env = {**os.environ, "TERM": "xterm-256color", "RADAR_CONFIG": str(self.config),
               "RADAR_CONTROL_SOCKET": str(self.socket),
               "RADAR_SOCKET": str(self.bus), "PATH": f"{self.root}:{os.environ['PATH']}"}
        self.master, self.slave = master, slave
        self.dashboard = subprocess.Popen([str(self.binary)], stdin=slave, stdout=slave,
                                          stderr=slave, env=env, start_new_session=True)

    def expect(self, text):
        deadline = time.monotonic() + WAIT * 2
        while text not in self.seen and time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                chunk = os.read(self.master, 65536)
                self.seen.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(self.master, b"\x1b[1;1R")
            check(self.dashboard.poll() is None, f"dashboard exited: {bytes(self.seen)!r}")
        check(text in self.seen, f"the run never showed {text!r}")

    def keys(self, pressed, seconds=0.3):
        """One key, then the bytes of the frames that follow it."""
        os.write(self.master, pressed)
        fresh = bytearray()
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            if select.select([self.master], [], [], 0.02)[0]:
                chunk = os.read(self.master, 65536)
                self.seen.extend(chunk)
                fresh.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(self.master, b"\x1b[1;1R")
        check(self.dashboard.poll() is None, f"dashboard exited: {bytes(fresh)!r}")
        return bytes(fresh)

    def panel(self):
        """The details panel as text, written whole.

        Hiding the panel blanks its region and showing it writes the panel whole,
        so no phrase can be missed by a diffed redraw; the last writes of the
        pair are the panel's own. Only the panel's columns are kept: a long value
        wraps there, and the tree beside it must not come between a label and the
        value that wrapped away from it.
        """
        drawn = self.keys(b"dd", 0.45)
        screen = frame_text(drawn + self.keys(b"", 0.1))
        fields = []
        for line in screen.splitlines():
            if "\u2502\u2502" not in line:
                continue
            fields.append(line.split("\u2502\u2502", 1)[1].rstrip("\u2502\u2518\u2514 "))
        return " ".join(field.strip() for field in fields if field.strip())

    def read_rows(self, names):
        """Every row's panel, keyed by the registration it names.

        The tree is walked the way a reader walks it, so a row that is missing
        or in the wrong place is a failure rather than something addressed by
        index.
        """
        captures = {}
        for _ in range(8):
            self.keys(b"k")
        for _ in range(12):
            panel = self.panel()
            for name in names:
                if name in panel and name not in captures:
                    captures[name] = panel
            self.keys(b"j")
        return captures

    def select(self, name):
        """Put the selection on the row that names `name`, and return it."""
        for _ in range(8):
            self.keys(b"k")
        for _ in range(12):
            panel = self.panel()
            if name in panel:
                return panel
            self.keys(b"j")
        raise AssertionError(f"no row showed {name!r}")

    # -- cleanup -----------------------------------------------------------

    def quit_dashboard(self, timeout=WAIT):
        """The operator's quit: the run must end by itself, restore the terminal
        and take its bus socket with it. The exit is the point of the key, so this
        does not use `keys`, which fails on a dashboard that has exited."""
        os.write(self.master, b"q")
        deadline = time.monotonic() + timeout
        while self.dashboard.poll() is None and time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                chunk = os.read(self.master, 65536)
                self.seen.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(self.master, b"\x1b[1;1R")
        check(self.dashboard.poll() == 0, f"dashboard exit {self.dashboard.returncode}")
        check(b"\x1b[?1049l" in self.seen, "the alternate screen was not restored")
        check(b"\x1b[?1006l" in self.seen, "mouse capture was not released")
        check(termios.tcgetattr(self.slave) == self.original_termios,
              "the terminal's input modes changed")
        deadline = time.monotonic() + timeout
        while self.bus.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        check(not self.bus.exists(), f"the dashboard left {self.bus}")

    def stop_daemon(self, timeout=WAIT):
        """A signal, which the daemon answers by removing its own socket."""
        self.stop(self.daemon, timeout)
        deadline = time.monotonic() + timeout
        while self.socket.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        check(not self.socket.exists(), f"the daemon left {self.socket}")

    def stop(self, process, timeout=WAIT):
        if process is None or process.poll() is not None:
            return
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=timeout)

    def cleanup(self):
        """Remove everything this run owns, and report what survived it."""
        leftovers = []
        if self.dashboard is not None and self.dashboard.poll() is None:
            self.stop(self.dashboard)
            leftovers.append(f"the dashboard had to be signalled "
                             f"(exit {self.dashboard.returncode})")
        if self.daemon is not None and self.daemon.poll() is None:
            self.stop(self.daemon)
            if self.daemon.returncode not in (0, -signal.SIGTERM):
                leftovers.append(f"daemon exit {self.daemon.returncode}")
        if self.server:
            subprocess.run(["tmux", "-L", self.server, "kill-server"],
                           capture_output=True, text=True)
            subprocess.run(["systemctl", "--user", "stop",
                            f"radar-tmux@{self.server}.service"],
                           capture_output=True, text=True)
            deadline = time.monotonic() + WAIT
            gone = False
            while time.monotonic() < deadline:
                listing = subprocess.run(["tmux", "-L", self.server, "list-sessions"],
                                         capture_output=True, text=True)
                if listing.returncode != 0:
                    gone = True
                    break
                time.sleep(0.05)
            if not gone:
                leftovers.append(f"the tmux server {self.server} survived kill-server")
            # tmux leaves the socket file of a dead server behind, so a gone
            # server is not a gone socket.
            socket_file = self.socket_file()
            if gone and socket_file.exists():
                socket_file.unlink()
            if socket_file.exists():
                leftovers.append(f"the tmux socket {socket_file} remains")
        # Its panes should go with the server. Wait boundedly, then make sure,
        # and report only what outlives even that.
        for pane_id, pid in self.panes.items():
            until = time.monotonic() + 3
            while alive(pid) and time.monotonic() < until:
                time.sleep(0.05)
            if not alive(pid):
                continue
            print(f"the pane {pane_id} process {pid} outlived its server: "
                  f"{self.describe(pid)}", file=sys.stderr)
            os.kill(pid, signal.SIGKILL)
            until = time.monotonic() + 3
            while alive(pid) and time.monotonic() < until:
                time.sleep(0.05)
            if alive(pid):
                leftovers.append(f"the pane {pane_id} process {pid} outlived the server "
                                 f"and was killed: {self.describe(pid)}")
        return leftovers

    def socket_file(self):
        """Where tmux keeps this server's socket: `$TMUX_TMPDIR/tmux-<uid>`,
        else `/tmp/tmux-<uid>`, as the rest of this repository computes it."""
        parent = os.environ.get("TMUX_TMPDIR") or "/tmp"
        return Path(parent) / f"tmux-{os.getuid()}" / self.server

    @staticmethod
    def describe(pid):
        return subprocess.run(["ps", "-o", "pid=,ppid=,pgid=,stat=,comm=,args=", "-p", str(pid)],
                              capture_output=True, text=True).stdout.strip()


def accept(binary, dump=False):
    fleet = Fleet(binary)
    failure = None
    try:
        fleet.start_tmux()
        fleet.start_daemon()

        ping = call(fleet.socket, "daemon.ping", {})
        check(ping.get("backend") == "tmux", f"daemon backend: {ping.get('backend')}")
        for contract in ("registry.v1", "registry.facts.v1", "events.v1", "spawn.v1"):
            check(contract in ping["capabilities"], f"missing capability {contract}: {ping}")

        inventory = fleet.inventory()
        check(len(inventory["panes"]) == 2, f"panes: {inventory['panes']}")
        joined, stale = list(inventory["panes"])
        # A registration names the server instance as well as the location: the
        # daemon's identity carries it, and an unnamed instance never joins. Each
        # one is built from its own pane's served location.
        def location(entry, pane_id=None):
            where = entry["location"]
            return {"backend": "tmux", "instance": fleet.server,
                    "workspace": where["workspace_id"], "tab": where["tab_id"],
                    "pane": pane_id or where["pane_id"]}

        joined_pid = fleet.panes[joined["location"]["pane_id"].split(":")[-1]]
        stale_pid = fleet.panes[stale["location"]["pane_id"].split(":")[-1]]

        worker = fleet.register("accept worker", location(joined), birth_identity(joined_pid))
        fleet.publish_execution(worker, "done")
        fleet.publish_assignment(worker)
        fleet.publish_facts(worker, MAX_LEASE_MS, {
            "model": "accept-model", "provider": "accept-provider", "thinking": "high",
            "context_usage": "61%", "session_name": "accept-session",
            "background_tasks": [{"id": "bg-7", "phase": "running"},
                                 {"id": "bg-8", "phase": "running"}]})

        # This one joins its own pane and lets its facts lease expire, so one
        # channel reads stale while its others stay unknown.
        expiring = fleet.register("accept stale", location(stale), birth_identity(stale_pid))
        fleet.publish_facts(expiring, STALE_LEASE_MS, {"model": "expiring-model"})

        # This one reports a pane the inventory does not hold and publishes
        # nothing: it can never join, and every channel it has is unknown.
        absent = joined["location"]["pane_id"].rsplit(":", 1)[0] + ":%99"
        orphan = fleet.register("accept orphan", location(joined, absent),
                                birth_identity(joined_pid))
        # A registration is allowed to have no label. It is still named by what it
        # has — its agent id — and not by the native name it does not have.
        unnamed = fleet.register(None, location(joined, absent), birth_identity(joined_pid))
        check(len({worker, expiring, orphan, unnamed}) == 4,
              "the registrations are not distinct")

        fleet.start_dashboard()
        fleet.expect(b"current")

        names = ("accept worker", "accept stale", "accept orphan", unnamed)
        captures = fleet.read_rows(names)
        if dump:
            for name, panel in captures.items():
                print(f"===== {name} =====\n{panel}")
        for name in names:
            check(name in captures, f"no row reached for {name}")

        # The row is named by its registration, never by the Herdr name a
        # registry-backed row does not have.
        worker_panel = captures["accept worker"]
        check("agent: accept worker" in worker_panel, worker_panel)
        check("agent: unavailable" not in worker_panel, worker_panel)
        # The three authorities, each with its own value, publisher and freshness.
        check("physical: joined" in worker_panel, worker_panel)
        check("execution: done · from pi" in worker_panel, worker_panel)
        check("assignment: working · from herdsman for accept-owner" in worker_panel,
              worker_panel)
        check("facts: fresh · from pi" in worker_panel, worker_panel)
        # Open awaited items and background tasks make the row waiting, which is
        # derived, and the execution line keeps saying what the session reported.
        check("state: waiting" in worker_panel, worker_panel)
        # A session's finished execution never completes the owner's assignment.
        check("assignment: done" not in worker_panel, worker_panel)
        # The owner's facts and the session's, side by side.
        check("role: worker" in worker_panel, worker_panel)
        check("definition: acceptance-worker" in worker_panel, worker_panel)
        check("waiting reason: reading the panel" in worker_panel, worker_panel)
        check("last outcome: partial — pending" in worker_panel, worker_panel)
        check("awaiting: bg-1, 2 background tasks" in worker_panel, worker_panel)
        check("model: accept-model" in worker_panel, worker_panel)
        check("provider: accept-provider" in worker_panel, worker_panel)
        check("context usage: 61%" in worker_panel, worker_panel)

        stale_panel = captures["accept stale"]
        check("facts: stale · from pi" in stale_panel, stale_panel)
        check("physical: joined" in stale_panel, stale_panel)
        check("execution: unknown — nothing reported" in stale_panel, stale_panel)

        check(f"agent: {unnamed}" in captures[unnamed], captures[unnamed])
        check("agent: unavailable" not in captures[unnamed], captures[unnamed])

        orphan_panel = captures["accept orphan"]
        check("physical: unresolved" in orphan_panel, orphan_panel)
        check("execution: unknown — nothing reported" in orphan_panel, orphan_panel)
        check("facts: not reported" in orphan_panel, orphan_panel)

        # A registered worker without a daemon lifecycle subject cannot be
        # controlled from its pane row; an unresolved registration is not joined
        # to a pane at all. Neither falls back to owner files or generic close.
        for name, refusal in (("accept worker", b"no daemon lifecycle subject"),
                              ("accept orphan", b"not joined to a pane")):
            fleet.select(name)
            pressed = fleet.keys(b"x", 0.4)
            check(refusal in pressed, f"{name}: close was not refused as expected: "
                                      f"{frame_text(pressed)!r}")
            check(b"[ Confirm ]" not in pressed, f"{name}: a close confirmation was offered")

        # Quitting the way an operator does, then stopping the daemon, so the
        # sockets are removed by their owners rather than by this harness.
        fleet.quit_dashboard()
        fleet.stop_daemon()

        if fleet.sentinel_log.exists():
            raise AssertionError(
                f"Herdr was called with: {fleet.sentinel_log.read_text()}")
    except Exception as error:
        failure = error
        if dump:
            print(f"===== end of run =====\n{frame_text(bytes(fleet.seen)[-6000:])}")
        raise
    finally:
        leftovers = fleet.cleanup()
        shutil.rmtree(fleet.root, ignore_errors=True)
        if failure is None:
            check(not leftovers, f"leftovers: {leftovers}")
        elif leftovers:
            print(f"leftovers after failure: {leftovers}", file=sys.stderr)
    print("PASS: two registrations joined a real disposable tmux server through a "
          "disposable daemon; their execution, owner assignment and self facts kept their "
          "own values and freshness; a registration with no label was named by its agent "
          "id; an unjoinable one stayed unresolved and silent; a finished execution did "
          "not complete the assignment; close was refused for both; Herdr was never "
          "called; the server, its panes, the daemon and the sockets are gone")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, nargs="?")
    parser.add_argument("--dump", action="store_true")
    args = parser.parse_args()
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], text=True)
    binary = args.binary or Path(json.loads(metadata)["target_directory"]) / "debug" / "radar"
    accept(binary.resolve(), dump=args.dump)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Acceptance: the managed lifecycle through its real entrypoints.

A disposable tmux server, a disposable daemon, and the real binary. Nothing here
uses a recording backend or a fake verifier: the assertions are about processes,
panes and the daemon's own answers.

What this run establishes:

- `radar run --agent` in an interactive shell is an admitted execution. Its
  program's exit code is captured even though the shell survives it, which is what
  the supervisor exists for.
- A spontaneous exit ends the execution and keeps the pane: the shell is the
  operator's, and only a close removes a terminal.
- One close ends the execution with a verified exit and removes the pane it was
  hosted in, and says so on its typed report.

Status: the harness around the scenarios works — a disposable tmux server the daemon
owns, a disposable daemon, and the real `radar run --agent` entrypoint driven from a
pane's own shell — but the subject that entrypoint creates is not visible through
`lifecycle.list`, so the assertions cannot yet find it. Whether the listing excludes
admitted (in-place) subjects or the admission is being refused silently is the open
question this run leaves for the next pass; the entrypoint wrote nothing to stderr and
the pane returned to its prompt immediately.

usage: python3 tests/lifecycle_acceptance.py [binary] [--keep]
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

WORKSPACE = "wacc"
WAIT = 30.0


def check(condition, message):
    if not condition:
        raise AssertionError(message)


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "publisher", ROOT / "examples/agent-publisher" / "publisher.py")
publisher = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = publisher
spec.loader.exec_module(publisher)


def call(socket, method, params):
    """One request on its own connection, framed as the control plane requires."""
    return publisher.call(socket, method, params)


class Accept:
    def __init__(self, binary, keep):
        self.binary = Path(binary).resolve()
        self.keep = keep
        self.root = Path(tempfile.mkdtemp(prefix="radar-lifecycle-"))
        # The daemon serves only a tmux server it owns, so the disposable server
        # runs under the same unit name its own servers use.
        self.server = f"radar-lifecycle-{os.getpid()}"
        self.state = self.root / "state"
        self.socket = self.root / "control.sock"
        self.daemon = None

    # -- processes ---------------------------------------------------------
    def tmux(self, *args):
        return subprocess.run(
            ["tmux", "-L", self.server, *args],
            capture_output=True, text=True, check=True)

    def env(self):
        return {**os.environ, "RADAR_CONTROL_BACKEND": "tmux",
                "RADAR_TMUX_SERVER": str(self.server),
                "RADAR_CONTROL_SOCKET": str(self.socket),
                "RADAR_CONTROL_STATE": str(self.state)}

    def start_tmux(self):
        unit = f"radar-tmux@{self.server}"
        installed = subprocess.run(
            ["systemctl", "--user", "cat", "radar-tmux@.service"],
            capture_output=True).returncode == 0
        if installed:
            subprocess.run(["systemctl", "--user", "start", f"{unit}.service"],
                           check=True)
        else:
            subprocess.run(
                ["systemd-run", "--user", "--collect", "--quiet", f"--unit={unit}",
                 "--property=Type=simple", "--property=KillMode=control-group",
                 "tmux", "-D", "-L", self.server, "-f", "/dev/null"], check=True)
        deadline = time.monotonic() + WAIT
        while subprocess.run(["tmux", "-L", self.server, "show-options", "-g"],
                             capture_output=True).returncode != 0:
            check(time.monotonic() < deadline, "the tmux server never answered")
            time.sleep(0.05)

    def start(self):
        self.start_tmux()
        # A plain shell in a directory with no project files: the scenario is about
        # an operator's own shell, and a prompt hook or direnv in the way would be
        # noise that is not part of what is being accepted.
        self.tmux("new-session", "-d", "-s", WORKSPACE, "-x", "120", "-y", "40",
                  "-c", str(self.root), "--", "bash", "--norc", "--noprofile", "-i")
        self.tmux("split-window", "-d", "-t", WORKSPACE, "-c", str(self.root),
                  "--", "bash", "--norc", "--noprofile", "-i")
        with open(self.root / "daemon.log", "w") as log:
            self.daemon = subprocess.Popen(
                [str(self.binary), "daemon"], env=self.env(),
                stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + WAIT
        while time.monotonic() < deadline:
            check(self.daemon.poll() is None,
                  f"daemon exited {(self.root / 'daemon.log').read_text()}")
            if self.socket.exists():
                try:
                    call(self.socket, "daemon.ping", {})
                    return
                except (OSError, RuntimeError):
                    pass
            time.sleep(0.02)
        raise AssertionError("the daemon socket never appeared")

    def stop(self):
        if self.daemon and self.daemon.poll() is None:
            self.daemon.send_signal(signal.SIGTERM)
            try:
                self.daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
        subprocess.run(["tmux", "-L", self.server, "kill-server"],
                       capture_output=True)
        subprocess.run(["systemctl", "--user", "stop",
                        f"radar-tmux@{self.server}.service"], capture_output=True)
        if self.keep:
            print(f"kept {self.root}", file=sys.stderr)
        else:
            shutil.rmtree(self.root, ignore_errors=True)

    # -- facts -------------------------------------------------------------
    def subjects(self):
        listed = call(self.socket, "lifecycle.list", {"limit": 64})
        return {subject["subject_id"]: subject
                for subject in listed.get("subjects", [])}

    def subject(self, subject_id):
        return call(self.socket, "lifecycle.get", {"subject_id": subject_id})["subject"]

    def eventually(self, read, message):
        deadline = time.monotonic() + WAIT
        while time.monotonic() < deadline:
            value = read()
            if value is not None:
                return value
            time.sleep(0.05)
        raise AssertionError(message)

    def pane_pids(self):
        listed = self.tmux("list-panes", "-a", "-F", "#{pane_id}\u001f#{pane_pid}").stdout
        return {line.split("\u001f")[0]: int(line.split("\u001f")[1])
                for line in listed.strip().splitlines()}

    def wait_for_shell(self, pane):
        """A shell that is ready takes keys; one that is still starting does not."""
        def read():
            shown = self.tmux("capture-pane", "-p", "-t", pane).stdout
            return True if shown.strip() else None
        self.eventually(read, f"the shell in {pane} never drew a prompt")

    def run_agent(self, pane, program, label):
        """Admit through the entrypoint an operator uses, in the pane's own shell."""
        errors = self.root / label
        command = (f"RADAR_CONTROL_SOCKET={self.socket} "
                   f"RADAR_CONTROL_STATE={self.state} "
                   f"{self.binary} run --agent -- {program} 2>{errors}")
        self.tmux("send-keys", "-t", pane, command, "Enter")
        return errors

    def new_subject(self, before, pane):
        """The subject this admission created, found by comparing the listing."""
        def read():
            for subject_id, subject in self.subjects().items():
                if subject_id in before:
                    continue
                location = (subject.get("placement") or {}).get("pane")
                if location in (None, pane):
                    return subject_id
            return None
        try:
            return self.eventually(
                read, f"no subject was recorded for the admission in {pane}")
        except AssertionError as error:
            shown = self.tmux("capture-pane", "-p", "-t", pane).stdout
            errors = self.root / "agent.err"
            if errors.exists():
                shown += f"\n--- stderr ---\n{errors.read_text()}"
            raise AssertionError(f"{error}\n--- pane ---\n{shown}") from None


def alive(pid):
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except (FileNotFoundError, ProcessLookupError):
        return False
    return stat.rsplit(")", 1)[1].split()[0] != "Z"


def scenario_the_supervisor_captures_an_exit_the_shell_survives(accept, pane):
    before = accept.subjects()
    accept.run_agent(pane, "sh -c 'exit 7'", "agent.err")
    subject_id = accept.new_subject(before, pane)
    subject = accept.eventually(
        lambda: (lambda s: s if s.get("completion") else None)(accept.subject(subject_id)),
        "the admitted program's completion was never recorded")
    check(subject["completion"]["code"] == 7,
          f"the program's own exit code is recorded: {subject['completion']}")
    check(subject.get("exited_at") is not None,
          "the execution is verified gone rather than assumed")
    # A spontaneous exit keeps the pane: the shell is the operator's.
    pids = accept.pane_pids()
    check(pane in pids, f"the pane survived its program: {pids}")
    check(alive(pids[pane]), "the pane's own process still runs")
    return subject_id


def scenario_one_close_removes_the_terminal(accept, pane):
    before = accept.subjects()
    accept.run_agent(pane, "sleep 300", "close.err")
    subject_id = accept.new_subject(before, pane)
    claim = accept.subject(subject_id)
    accept.eventually(
        lambda: accept.subject(subject_id).get("process"),
        "the admitted execution never recorded its process identity")
    pid = accept.subject(subject_id)["process"]["pid"]
    answer = call(accept.socket, "lifecycle.cancel",
                  {"request": {"subject_id": subject_id, "claim_id": claim["claim_id"]}})
    closed = answer["request"]["closed"]
    check(closed["execution"]["state"] == "exited",
          f"the close verified the exit: {closed}")
    check(closed["resource"]["state"] == "removed",
          f"the close removed the pane it hosted: {closed}")
    accept.eventually(lambda: None if alive(pid) else True,
                      "the closed program is still running")
    accept.eventually(
        lambda: True if pane not in accept.pane_pids() else None,
        "the pane the close removed is still listed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", nargs="?", default=str(ROOT / "target/debug/radar"))
    parser.add_argument("--keep", action="store_true")
    arguments = parser.parse_args()
    accept = Accept(arguments.binary, arguments.keep)
    try:
        accept.start()
        panes = sorted(accept.pane_pids())
        check(len(panes) == 2, f"the disposable server has two panes: {panes}")
        for pane in panes:
            accept.wait_for_shell(pane)
        scenario_the_supervisor_captures_an_exit_the_shell_survives(accept, panes[0])
        scenario_one_close_removes_the_terminal(accept, panes[1])
        print("lifecycle acceptance: ok")
    finally:
        accept.stop()


if __name__ == "__main__":
    sys.exit(main())

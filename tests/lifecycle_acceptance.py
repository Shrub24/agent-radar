#!/usr/bin/env python3
"""Real-scope acceptance for shell-hosted admission and pane-container closure.

Run after `cargo build --locked --bin radar`:
    python3 tests/lifecycle_acceptance.py [binary] [--keep]

This covers completion persistence, shell return and branch closure of an admitted
execution. Configured L, UI D, restart and nested workspace acceptance remain separate.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "publisher", ROOT / "examples/agent-publisher" / "publisher.py")
publisher = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = publisher
spec.loader.exec_module(publisher)
WAIT = 30


def eventually(read, message):
    deadline = time.monotonic() + WAIT
    while True:
        value = read()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise AssertionError(message)
        time.sleep(0.05)


class Acceptance:
    def __init__(self, binary, keep):
        self.binary = Path(binary).resolve()
        self.keep = keep
        self.root = Path(tempfile.mkdtemp(prefix="radar-lifecycle-"))
        self.server = f"radar-lifecycle-{os.getpid()}"
        self.state = self.root / "state"
        self.socket = self.root / "control.sock"
        self.daemon = None

    def tmux(self, *args):
        return subprocess.run(["tmux", "-L", self.server, *args],
                              capture_output=True, text=True, check=True).stdout

    def call(self, method, params):
        return publisher.call(self.socket, method, params)

    def events(self):
        # Read durable events directly: Registry::open deliberately reuses an
        # in-process log and cannot observe writes from this separate daemon.
        return [json.loads(path.read_text())
                for path in sorted((self.state / "events").glob("*.json"))]

    def start(self):
        unit = f"radar-tmux@{self.server}"
        installed = subprocess.run(
            ["systemctl", "--user", "cat", "radar-tmux@.service"],
            capture_output=True).returncode == 0
        if installed:
            subprocess.run(["systemctl", "--user", "start", f"{unit}.service"], check=True)
        else:
            subprocess.run(
                ["systemd-run", "--user", "--collect", "--quiet", f"--unit={unit}",
                 "--property=Type=simple", "--property=KillMode=control-group",
                 "tmux", "-D", "-L", self.server, "-f", "/dev/null"], check=True)
        eventually(
            lambda: subprocess.run(["tmux", "-L", self.server, "show-options", "-g"],
                                   capture_output=True).returncode == 0,
            "tmux did not start")
        self.tmux("new-session", "-d", "-s", "accept", "-x", "120", "-y", "40",
                  "-c", str(self.root), "bash", "--norc", "--noprofile", "-i")
        self.tmux("split-window", "-d", "-t", "accept", "-c", str(self.root),
                  "bash", "--norc", "--noprofile", "-i")
        env = {**os.environ, "RADAR_CONTROL_BACKEND": "tmux",
               "RADAR_TMUX_SERVER": self.server,
               "RADAR_CONTROL_SOCKET": str(self.socket),
               "RADAR_CONTROL_STATE": str(self.state)}
        with open(self.root / "daemon.log", "w") as log:
            self.daemon = subprocess.Popen([str(self.binary), "daemon"], env=env,
                                           stdout=log, stderr=subprocess.STDOUT)

        def ready():
            if self.daemon.poll() is not None:
                raise AssertionError((self.root / "daemon.log").read_text())
            if not self.socket.exists():
                return False
            self.call("daemon.ping", {})
            return True
        eventually(ready, "daemon did not start")
        panes = self.tmux("list-panes", "-a", "-F", "#{pane_id}").splitlines()
        assert len(panes) == 2, panes
        for pane in panes:
            eventually(lambda: "bash-" in self.tmux("capture-pane", "-p", "-t", pane),
                       f"shell in {pane} did not start")
        return panes

    def run_agent(self, pane, program):
        before = {event["event_id"] for event in self.events()}
        command = shlex.join([
            "env", f"RADAR_CONTROL_SOCKET={self.socket}",
            f"RADAR_CONTROL_STATE={self.state}", str(self.binary),
            "run", "--agent", "--", *program])
        command += "; printf 'RADAR_STATUS=%s\\n' $?"
        self.tmux("send-keys", "-t", pane, command, "Enter")

        def created():
            for event in self.events():
                transition = event["transition"]
                if event["event_id"] not in before and transition["type"] == "process_created":
                    return transition["data"]
            return None
        return eventually(created, f"admission in {pane} recorded no execution")

    def completion(self, registration):
        for event in self.events():
            if (event["subject_id"] == registration
                    and event["transition"]["type"] == "completed"):
                return event["transition"]["data"]
        return None

    def close(self):
        # An agent scope is outside the tmux server's cgroup. Clean up only this
        # run's recorded scopes, even when an assertion interrupts a live program.
        registrations = [event["transition"]["data"]["registration_id"]
                         for event in self.events()
                         if event["transition"]["type"] == "process_created"]
        for registration in registrations:
            unit = f"radar-{registration.replace('-', '')}-main.scope"
            subprocess.run(["systemctl", "--user", "kill", "--signal=KILL", unit],
                           capture_output=True, timeout=10)
            subprocess.run(["systemctl", "--user", "stop",
                            f"radar-{registration.replace('-', '')}.slice"],
                           capture_output=True, timeout=10)
        if self.daemon and self.daemon.poll() is None:
            self.daemon.send_signal(signal.SIGTERM)
            try:
                self.daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
                self.daemon.wait()
        subprocess.run(["systemctl", "--user", "stop",
                        f"radar-tmux@{self.server}.service"], capture_output=True, timeout=10)
        if self.keep:
            print(f"kept {self.root}", file=sys.stderr)
        else:
            shutil.rmtree(self.root)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", nargs="?", default=str(ROOT / "target/debug/radar"))
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    accept = Acceptance(args.binary, args.keep)
    try:
        first, second = accept.start()
        created = accept.run_agent(first, ["sh", "-c", "exit 7"])
        registration = created["registration_id"]
        completion = eventually(lambda: accept.completion(registration), "completion not persisted")
        assert completion["code"] == 7, completion
        eventually(lambda: "RADAR_STATUS=7" in accept.tmux("capture-pane", "-p", "-t", first),
                   "program status did not reach its surviving shell")

        ready = accept.root / "interrupt-ready"
        created = accept.run_agent(first, ["sh", "-c",
            f"trap 'exit 42' INT; : > {shlex.quote(str(ready))}; while :; do sleep 1; done"])
        registration = created["registration_id"]
        eventually(ready.exists, "program never installed its interrupt handler")
        accept.tmux("send-keys", "-t", first, "C-c")
        completion = eventually(lambda: accept.completion(registration),
                                "interrupt completion not persisted")
        assert completion["code"] == 42, completion
        eventually(lambda: "RADAR_STATUS=42" in accept.tmux("capture-pane", "-p", "-t", first),
                   "Ctrl-C did not return control to the surviving shell")

        created = accept.run_agent(second, ["sleep", "300"])
        registration = created["registration_id"]
        placement = created["facts"]["placement"]
        pane = placement["location"]["pane"]
        unit = f"radar-{registration.replace('-', '')}-main.scope"
        eventually(
            lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                   capture_output=True).returncode == 0,
            "admitted program never entered a real scope")
        containers = accept.call("topology.list", {})["containers"]
        container = next(item for item in containers
                         if (item.get("binding") or {}).get("handle") == pane)
        result = accept.call("topology.delete", {"request": {
            "container_id": container["container_id"],
            "frozen": {"target": {"pane": pane},
                       "identity": {"pane": {"pane_id": pane, "occupant": None}}}}})
        assert result["request"]["outcome"] == "completed", result
        eventually(lambda: second not in accept.tmux("list-panes", "-a", "-F", "#{pane_id}").splitlines(),
                   "container close did not remove its pane")
        eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                         capture_output=True).returncode != 0,
                   "closed execution scope is still active")
        print("lifecycle acceptance: completion, Ctrl-C, shell return and real-scope pane close passed")
    finally:
        accept.close()


if __name__ == "__main__":
    main()

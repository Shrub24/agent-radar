#!/usr/bin/env python3
"""Real-scope acceptance for shell-hosted admission and pane-container closure.

Run after `cargo build --locked --bin radar`:
    python3 tests/lifecycle_acceptance.py [binary] [--keep]

This drives real scopes and a disposable nested tmux view: completion, Ctrl-C,
configured L, UI restart/Close, native pane movement and multi-agent branch close.
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
import threading
import textwrap
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
        self.view_server = f"{self.server}-view"
        self.config = self.root / "config.toml"

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
        agent = self.root / "pi"
        agent.write_text(f"#!{sys.executable}\n" + textwrap.dedent(f'''\
            import json, os, runpy, sys, time, uuid
            from pathlib import Path
            call = runpy.run_path({str(Path(__file__).resolve())!r})["publisher"].call
            root = Path(__file__).parent
            session = sys.argv[sys.argv.index("--session") + 1] if "--session" in sys.argv else str(uuid.uuid4())
            identity = {{"pid": os.getpid(), "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                        "start_ticks": int(Path("/proc/self/stat").read_text().rsplit(") ", 1)[1].split()[19])}}
            report = {{"source": "herdsman-pi", "incarnation": str(uuid.uuid4()), "session_id": session,
                      "claim_id": str(uuid.uuid4()), "process": identity,
                      "registration_id": os.environ.pop("PI_RADAR_REGISTRATION_ID", None),
                      "label": "lifecycle-acceptance-agent",
                      "location": {{"backend": "tmux", "instance": {self.server!r},
                                   "pane": "tmux:{self.server}:" + os.environ["TMUX_PANE"]}}}}
            granted = call(root / "control.sock", "agent.presence", report)
            assert granted["standing"] == "granted", granted
            report["registration_id"] = granted["registration_id"]
            publisher = {{"source": report["source"], "incarnation": report["incarnation"]}}
            writer = call(root / "control.sock", "agent.acquire", {{"registration_id": report["registration_id"],
                          "channel": "execution", "publisher": publisher}})["writer"]
            facts = call(root / "control.sock", "agent.acquire", {{"registration_id": report["registration_id"],
                         "channel": "facts", "publisher": publisher}})["writer"]
            call(root / "control.sock", "agent.publish", {{"registration_id": report["registration_id"],
                 "channel": "facts", "writer_handle": facts["handle"], "sequence": 1,
                 "snapshot": {{"session_name": report["label"]}}}})
            sequence = 0
            while True:
                sequence += 1
                call(root / "control.sock", "agent.presence", report)
                call(root / "control.sock", "agent.publish", {{"registration_id": report["registration_id"],
                     "channel": "execution", "writer_handle": writer["handle"], "sequence": sequence,
                     "snapshot": {{"activity": "working"}}}})
                if sequence == 1:
                    (root / f"agent-{{os.getpid()}}.json").write_text(json.dumps({{"report": report, "granted": granted}}))
                time.sleep(1)
            '''))
        agent.chmod(0o700)
        self.config.write_text(f'[server]\nbackend = "tmux"\ntmux_server = {json.dumps(self.server)}\n'
                               f'[agents]\nlaunch_command = [{json.dumps(str(agent))}]\n')
        env = {**os.environ, "RADAR_CONFIG": str(self.config), "RADAR_CONTROL_BACKEND": "tmux",
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

    def branch_tab(self, count):
        """A recorded tab holding `count` POSIX shells, as (window, panes, container)."""
        before = set(self.tmux("list-windows", "-a", "-F", "#{window_id}").split())
        workspace = next(item for item in self.call("topology.list", {})["containers"]
                         if item["kind"] == "workspace")
        made = self.call("topology.create", {"request": {
            "kind": "tab", "workspace_id": workspace["binding"]["handle"], "focus": False}})
        assert made["request"]["outcome"] == "completed", made
        window = eventually(
            lambda: next(iter(set(self.tmux("list-windows", "-a", "-F", "#{window_id}").split())
                              - before), None), "the branch tab never appeared")
        lead = self.tmux("list-panes", "-t", window, "-F", "#{pane_id}").split()[0]
        for _ in range(count - 1):
            self.tmux("split-window", "-d", "-t", lead, "bash", "--norc", "--noprofile", "-i")
        panes = self.tmux("list-panes", "-t", window, "-F", "#{pane_id}").split()
        assert len(panes) == count, panes
        for pane in panes:
            # The tab starts the operator's own shell; the harness's command line is POSIX.
            self.tmux("send-keys", "-t", pane, "exec bash --norc --noprofile -i", "Enter")
            eventually(lambda: "bash-" in self.tmux("capture-pane", "-p", "-t", pane),
                       f"shell in {pane} did not start")
        tab = f"tmux:{self.server}:{window}"
        container = next(item for item in self.call("topology.list", {})["containers"]
                         if item["kind"] == "tab" and (item.get("binding") or {}).get("handle") == tab)
        return window, panes, container

    def delete_tab(self, window, panes, container):
        tab = f"tmux:{self.server}:{window}"
        members = [{"pane_id": f"tmux:{self.server}:{pane}", "occupant": None}
                   for pane in sorted(panes)]
        result = self.call("topology.delete", {"request": {
            "container_id": container["container_id"],
            "frozen": {"target": {"tab": tab},
                       "identity": {"tab": {"tab_id": tab, "members": members}}}}})
        assert result["request"]["outcome"] == "completed", result

    def completion(self, registration):
        for event in self.events():
            if (event["subject_id"] == registration
                    and event["transition"]["type"] == "completed"):
                return event["transition"]["data"]
        return None

    def view(self, *args):
        return subprocess.run(["tmux", "-L", self.view_server, "-f", "/dev/null", *args],
                              capture_output=True, text=True, check=True).stdout

    def start_view(self):
        self.view_pane = self.view("new-session", "-d", "-P", "-F", "#{pane_id}",
                                  "-s", "view", "-x", "160", "-y", "45", "sh").strip()
        self.view("set-option", "-t", self.view_pane, "remain-on-exit", "on")
        self.view("respawn-pane", "-k", "-t", self.view_pane,
                  "env", f"RADAR_CONFIG={self.config}", f"RADAR_CONTROL_SOCKET={self.socket}",
                  f"RADAR_CONTROL_STATE={self.state}", f"SHELL={shutil.which('bash')}",
                  str(self.binary), "sidebar")
        self.view("split-window", "-d", "-h", "-t", self.view_pane, "-p", "65",
                  "tmux", "-L", self.server, "attach", "-t", "accept")
        def ready():
            if self.view("display-message", "-p", "-t", self.view_pane, "#{pane_dead}").strip() == "1":
                raise AssertionError(f"sidebar exited: {self.screen()}")
            return "accept" in self.screen()
        eventually(ready, "sidebar never drew its workspace")

    def screen(self):
        return self.view("capture-pane", "-p", "-t", self.view_pane)

    def keys(self, *keys):
        self.view("send-keys", "-t", self.view_pane, *keys)

    def agents(self):
        return [json.loads(path.read_text()) for path in sorted(self.root.glob("agent-*.json"))]

    def working(self, pid):
        """Wait until the daemon reports this program working, so `w` has a target."""
        def ready():
            agents = self.call("observation.get", {})["snapshot"]["observation"]["inventory"]["agents"]
            return any(a["status"] == "working" and
                       (a["registry"]["agent"]["process"]["claim"] or {}).get("pid") == pid
                       for a in agents if a.get("registry"))
        eventually(ready, "agent never reported working")
        time.sleep(0.5)

    def confirmed(self, key, word):
        self.keys(key)
        eventually(lambda: word in self.screen().lower(), f"{key} did not open its confirmation")
        self.keys("Right", "Enter")

    def close(self):
        if self.daemon is not None and self.daemon.poll() is None:
            try:
                observation = self.call("observation.get", {})
            except Exception as error:
                observation = {"diagnostic_error": str(error)}
            (self.root / "observation.json").write_text(json.dumps(observation, indent=2))
        panes = {}
        for server in (self.server, self.view_server):
            listed = subprocess.run(["tmux", "-L", server, "list-panes", "-a", "-F", "#{pane_id}"],
                                    capture_output=True, text=True)
            panes[server] = {pane: subprocess.run(
                ["tmux", "-L", server, "capture-pane", "-p", "-t", pane], capture_output=True,
                text=True).stdout for pane in listed.stdout.splitlines()}
        (self.root / "panes.json").write_text(json.dumps(panes, indent=2))
        subprocess.run(["tmux", "-L", self.view_server, "kill-server"], capture_output=True)
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

        accept.start_view()
        accept.keys("L")
        first_agent = eventually(lambda: (accept.agents() or [None])[0],
                                 "configured L never produced a granted managed execution")
        eventually(lambda: "lifecycle-acceptance-agent" in accept.screen(),
                   "configured agent never appeared in the sidebar")
        accept.working(first_agent["report"]["process"]["pid"])
        accept.keys("w")
        accept.confirmed("r", "restart")
        agents = eventually(lambda: accept.agents() if len(accept.agents()) == 2 else None,
                            "UI restart never produced a successor")
        successor = next(agent for agent in agents if agent != first_agent)
        assert successor["granted"]["subject_id"] == first_agent["granted"]["subject_id"], agents
        assert successor["report"]["claim_id"] != first_agent["report"]["claim_id"], agents
        assert successor["report"]["location"]["pane"] == first_agent["report"]["location"]["pane"], \
            "restart did not reuse the agent's dedicated pane"
        old_pid = first_agent["report"]["process"]["pid"]
        eventually(lambda: not Path(f"/proc/{old_pid}").exists(), "restart left its predecessor alive")
        accept.working(successor["report"]["process"]["pid"])
        accept.keys("w")
        accept.confirmed("D", "close")
        new_pid = successor["report"]["process"]["pid"]
        eventually(lambda: not Path(f"/proc/{new_pid}").exists(), "UI D left the execution alive")
        pane = successor["report"]["location"]["pane"].rsplit(":", 1)[1]
        eventually(lambda: pane not in accept.tmux("list-panes", "-a", "-F", "#{pane_id}").splitlines(),
                   "UI D did not remove the selected agent's pane")
        print("lifecycle acceptance: configured L, subject-preserving UI restart and UI D passed", flush=True)

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
        # A branch holding several executions ends as one: every scope, every pane.
        window, panes, container = accept.branch_tab(2)
        units = []
        for pane in panes:
            created = accept.run_agent(pane, ["sleep", "300"])
            units.append(f"radar-{created['registration_id'].replace('-', '')}-main.scope")
        for unit in units:
            eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                              capture_output=True).returncode == 0,
                       "branch execution never entered a real scope")
        accept.delete_tab(window, panes, container)
        for unit in units:
            eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                              capture_output=True).returncode != 0,
                       "branch close left an execution scope active")
        eventually(lambda: window not in accept.tmux("list-windows", "-a", "-F", "#{window_id}").split(),
                   "branch close did not remove its tab")
        print("lifecycle acceptance: multi-agent branch close passed")

        # Three agents that ignore TERM share one shutdown window: the branch costs
        # one graceful wait and one escalation, not one of each per agent.
        window, panes, container = accept.branch_tab(3)
        units = []
        for pane in panes:
            created = accept.run_agent(pane, ["sh", "-c", "trap '' TERM; while :; do sleep 1; done"])
            units.append(f"radar-{created['registration_id'].replace('-', '')}-main.scope")
        for unit in units:
            eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                              capture_output=True).returncode == 0,
                       "stubborn execution never entered a real scope")
        started = time.monotonic()
        closer = threading.Thread(target=accept.delete_tab, args=(window, panes, container))
        closer.start()
        # While the stubborn agents hold the close open, the observation names the
        # branch as closing; afterwards it names nothing.
        seen = set()
        while closer.is_alive():
            observed = (accept.call("observation.get", {}).get("snapshot") or {}).get("observation") or {}
            seen |= set(observed.get("closing", []))
            time.sleep(0.1)
        closer.join()
        elapsed = time.monotonic() - started
        assert {f"tmux:{accept.server}:{pane}" for pane in panes} <= seen, f"closing never published: {seen}"
        eventually(lambda: not ((accept.call("observation.get", {}).get("snapshot") or {})
                                .get("observation") or {}).get("closing"),
                   "the closing state was never released")
        for unit in units:
            assert subprocess.run(["systemctl", "--user", "is-active", unit],
                                  capture_output=True).returncode != 0, f"{unit} survived the close"
        assert elapsed < 7, f"three stubborn agents took {elapsed:.1f}s: windows were not shared"
        print(f"lifecycle acceptance: stubborn branch escalated together in {elapsed:.1f}s")

        # Closing one admitted agent's pane ends its execution and removes only that pane.
        window, panes, container = accept.branch_tab(2)
        created = accept.run_agent(panes[0], ["sleep", "300"])
        unit = f"radar-{created['registration_id'].replace('-', '')}-main.scope"
        eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                          capture_output=True).returncode == 0,
                   "admitted execution never entered a real scope")
        target = f"tmux:{accept.server}:{panes[0]}"
        closed = accept.call("mux.close", {"request": {
            "target": {"pane": target},
            "identity": {"pane": {"pane_id": target, "occupant": None}}}, "requester": "acceptance"})
        assert closed["request"]["outcome"] == "completed", closed
        execution, = closed["request"]["closed"]["executions"]
        assert execution["registration_id"] == created["registration_id"], closed
        assert execution["end"] == {"state": "exited"}, closed
        assert execution["completion"]["signal"] == 15, closed
        assert subprocess.run(["systemctl", "--user", "is-active", unit],
                              capture_output=True).returncode != 0, "pane close left the scope active"
        remaining = accept.tmux("list-panes", "-a", "-F", "#{pane_id}").split()
        assert panes[0] not in remaining and panes[1] in remaining, remaining
        accept.delete_tab(window, panes[1:], container)
        print("lifecycle acceptance: admitted pane close passed")

        # A pane moved out of a tab by tmux is no longer inside it: closing the tab
        # must not end the execution that now lives elsewhere.
        window, panes, container = accept.branch_tab(2)
        created = accept.run_agent(panes[1], ["sleep", "300"])
        unit = f"radar-{created['registration_id'].replace('-', '')}-main.scope"
        eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                          capture_output=True).returncode == 0,
                   "moved execution never entered a real scope")
        accept.tmux("join-pane", "-d", "-s", panes[1], "-t", first)
        accept.delete_tab(window, panes[:1], container)
        eventually(lambda: window not in accept.tmux("list-windows", "-a", "-F", "#{window_id}").split(),
                   "closing the tab did not remove it")
        assert subprocess.run(["systemctl", "--user", "is-active", unit],
                              capture_output=True).returncode == 0, "closing a tab ended a pane moved out of it"
        print("lifecycle acceptance: native pane move passed")
        # A nested workspace ends with every tab and execution inside it.
        made = accept.call("topology.create", {"request": {
            "kind": "workspace", "directory": str(accept.root), "focus": False}})
        assert made["request"]["outcome"] == "completed", made
        def workspace():
            return next((item for item in accept.call("topology.list", {})["containers"]
                         if item["kind"] == "workspace" and item.get("removed_at") is None
                         and (item.get("binding") or {}).get("handle") != f"tmux:{accept.server}:$0"), None)
        nested = eventually(workspace, "the nested workspace was never recorded")
        handle = nested["binding"]["handle"]
        accept.call("topology.create", {"request": {
            "kind": "tab", "workspace_id": handle, "focus": False}})
        short = handle.rsplit(":", 1)[1]
        def inside():
            rows = accept.tmux("list-panes", "-a", "-F", "#{session_id} #{pane_id}").splitlines()
            return [pane for line in rows for sid, pane in [line.split()] if sid == short]
        eventually(lambda: len(inside()) == 2, "the nested workspace never held two panes")
        members = inside()
        units = []
        for pane in members:
            accept.tmux("send-keys", "-t", pane, "exec bash --norc --noprofile -i", "Enter")
            eventually(lambda: "bash-" in accept.tmux("capture-pane", "-p", "-t", pane),
                       f"shell in {pane} did not start")
            created = accept.run_agent(pane, ["sleep", "300"])
            units.append(f"radar-{created['registration_id'].replace('-', '')}-main.scope")
        result = accept.call("topology.delete", {"request": {
            "container_id": nested["container_id"],
            "frozen": {"target": {"workspace": handle},
                       "identity": {"workspace": {"workspace_id": handle, "members": [
                           {"pane_id": f"tmux:{accept.server}:{pane}", "occupant": None}
                           for pane in sorted(members)]}}}}})
        assert result["request"]["outcome"] == "completed", result
        for unit in units:
            eventually(lambda: subprocess.run(["systemctl", "--user", "is-active", unit],
                                              capture_output=True).returncode != 0,
                       "workspace close left an execution scope active")
        eventually(lambda: not inside(), "workspace close left panes behind")
        print("lifecycle acceptance: nested workspace close passed")
        print("lifecycle acceptance: completion, Ctrl-C, shell return and real-scope pane close passed")
    finally:
        accept.close()


if __name__ == "__main__":
    main()

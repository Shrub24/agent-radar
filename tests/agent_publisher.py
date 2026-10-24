#!/usr/bin/env python3
"""Exercise the documented direct publisher against a disposable Radar daemon."""

import importlib.util
import json
import os
import shutil
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/fixtures"))
import tmux_server  # noqa: E402

MODULE = ROOT / "examples/agent-publisher/publisher.py"
spec = importlib.util.spec_from_file_location("agent_publisher", MODULE)
publisher = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = publisher
spec.loader.exec_module(publisher)


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def call(path, method, params):
    return publisher.call(path, method, params)


def fixture_pairs():
    rows = [json.loads(line) for line in
            (ROOT / "docs/agent-registration.fixture.jsonl").read_text().splitlines() if line]
    check(len(rows) % 2 == 0, "fixture must contain request/response pairs")
    pairs = list(zip(rows[::2], rows[1::2]))
    expected = ["agent.register", "events.read", "events.read", "events.read",
                "agent.acquire", "agent.publish", "agent.acquire", "agent.publish",
                "agent.acquire", "agent.publish", "agent.publish", "agent.context",
                "agent.context", "agent.context", "agent.context", "agent.get", "agent.list",
                "agent.context", "agent.retire", "spawn.create", "agent.register", "spawn.get",
                "spawn.list", "spawn.create", "spawn.close", "spawn.close"]
    actual = []
    for request, response in pairs:
        check(request["kind"] == "request" and response["kind"] == "response",
              "fixture entries must be request/response pairs")
        check(request["version"] == publisher.PROTOCOL_VERSION, "protocol version mismatch")
        check(request["id"] == response["id"], "fixture response ID mismatch")
        check(set(request) == {"kind", "version", "id", "method", "params"},
              f"invalid request envelope: {request}")
        check(set(response) == {"kind", "id", "result"}, f"invalid response envelope: {response}")
        actual.append(request["method"])
    check(actual == expected, f"fixture methods are {actual}")
    events = [pair for pair in pairs if pair[0]["method"] == "events.read"]
    check(len(events) == 3, "fixture needs normal, empty-wait and expired event reads")
    close_pair = [pair for pair in pairs if pair[0]["method"] == "spawn.close"]
    check(len(close_pair) == 2, "fixture must contain close and identical replay")
    check(close_pair[0][0]["id"] == close_pair[1][0]["id"],
          "close replay must repeat the original request id")
    check(close_pair[0][0]["params"] == close_pair[1][0]["params"],
          "close replay must repeat identical content")
    check(close_pair[0][1]["result"]["close"] == close_pair[1][1]["result"]["close"],
          "close replay must return its recorded result")
    return pairs


def validate_fixture(path):
    """Run registry exchanges and compare stable response schema/content live."""
    pairs = fixture_pairs()
    handles, registration_id = {}, None  # handles are keyed by writer channel, or "context"
    previous_context = None
    event_epoch = None
    get_template = next(response["result"]["agent"] for request, response in pairs
                        if request["method"] == "agent.get")
    for request, response in pairs:
        method = request["method"]
        if method in ("spawn.create", "spawn.get", "spawn.list", "spawn.close"):
            continue
        if method == "events.read":
            params = json.loads(json.dumps(request["params"]))
            if params.get("epoch") == "<journal-epoch>":
                check(event_epoch is not None,
                      "fixture resumes from an epoch before reading one")
                params["epoch"] = event_epoch
            elif params.get("epoch") == "<old-journal-epoch>":
                params["epoch"] = "old-epoch-for-fixture"
            actual = call(path, method, params)
            expected = json.loads(json.dumps(response["result"]))
            if "cursor_expired" in expected:
                expired = actual["cursor_expired"]
                expected["cursor_expired"]["epoch"] = expired["epoch"]
                expected["cursor_expired"]["cursor"]["epoch"] = expired["epoch"]
            else:
                if event_epoch is None:
                    event_epoch = actual["epoch"]
                expected["epoch"] = actual["epoch"]
                expected["cursor"]["epoch"] = actual["epoch"]
                for event, served in zip(expected["events"], actual["events"]):
                    event["registration_id"] = registration_id
                    event["at"] = served["at"]
            check(actual == expected, f"fixture events.read differs:\nactual={actual}\nexpected={expected}")
            continue
        params = json.loads(json.dumps(request["params"]))
        if method == "agent.register" and "spawn_token" in params:
            continue
        if method in ("agent.acquire", "agent.publish", "agent.context", "agent.get", "agent.retire"):
            params["registration_id"] = registration_id
        if method == "agent.publish":
            params["writer_handle"] = handles[params["channel"]]
        if method == "agent.retire":
            params["writer_handle"] = handles["assignment"]
        if method == "agent.context" and params.get("writer_handle") == "<opaque-uuid>":
            params["writer_handle"] = handles["context"]
        if method == "agent.context" and "replace" in params:
            params["replace"]["handle"] = handles["context"]
            # The fixture uses a 1s incumbent lease so this validates the
            # documented expiry-gated replacement without inventing retire API.
            time.sleep(1.05)
        if method == "agent.acquire" and "replace" in params:
            # The facts incumbent has a 1s lease, so replacement waits for its
            # expiry. Each channel expires on its own clock: the assignment
            # channel, with a longer lease, stays fresh meanwhile.
            time.sleep(1.05)
            params["replace"]["handle"] = handles[params["channel"]]
            before = get_agent(path, registration_id)
            check(before["facts"]["snapshot"]["freshness"] == "stale", str(before))
            check(before["assignment"]["snapshot"]["freshness"] == "fresh", str(before))
        actual = call(path, method, params)
        expected = json.loads(json.dumps(response["result"]))
        if method == "agent.register":
            registration_id = actual["registration"]["registration_id"]
            expected["registration"]["registration_id"] = registration_id
            expected["registration"]["registered_at"] = actual["registration"]["registered_at"]
            expected["registration"]["session"] = actual["registration"]["session"]
        elif method == "agent.acquire":
            handles[params["channel"]] = actual["writer"]["handle"]
            expected["writer"]["handle"] = handles[params["channel"]]
        elif method == "agent.publish":
            channel = actual["channel"]
            handle = handles[params["channel"]]
            expected["channel"]["registration_id"] = registration_id
            expected["channel"]["writer"]["handle"] = handle
            expected["channel"]["snapshot"]["handle"] = handle
            expected["channel"]["snapshot"]["received_at"] = channel["snapshot"]["received_at"]
            expected["channel"]["snapshot"]["expires_at"] = channel["snapshot"]["expires_at"]
            check(channel["snapshot"]["lease_ms"] == params.get("lease_ms", publisher.DEFAULT_LEASE_MS),
                  str(channel))
        elif method == "agent.context":
            context = actual["context"]
            expected["context"]["writer"].pop("handle", None)
            if "context" not in handles or "replace" in params:
                handles["context"] = actual["writer"]["handle"]
            expected["writer"]["handle"] = handles["context"]
            expected["context"]["registration_id"] = registration_id
            expected["context"]["context"]["received_at"] = context["context"]["received_at"]
            expected["context"]["context"]["expires_at"] = context["context"]["expires_at"]
            check(context["context"]["session"] == params["context"]["session"], str(context))
            replay = previous_context == params
            if replay:
                check(isinstance(actual["warning"], str) and "not refreshed" in actual["warning"],
                      f"identical replay did not warn: {actual}")
            else:
                check(actual["warning"] is None, str(actual))
            previous_context = json.loads(json.dumps(params))
            check("handle" not in actual["context"]["writer"], str(actual))
            # Compare the whole exchange so the fixture exemplar stays authoritative
            # rather than being quietly corrected field by field.
            check(actual["writer"] == expected["writer"],
                  f"fixture context writer differs:\nactual={actual['writer']}\nexpected={expected['writer']}")
            check(actual["context"] == expected["context"],
                  f"fixture context projection differs:\nactual={actual['context']}\nexpected={expected['context']}")
            continue
        elif method == "agent.get":
            expected["agent"]["registration"]["registration_id"] = registration_id
            expected["agent"]["registration"]["registered_at"] = actual["agent"]["registration"]["registered_at"]
            expected["agent"]["assignment"]["registration_id"] = registration_id
            # get/list facts intentionally omit the private fencing handle.
            expected["agent"]["assignment"]["snapshot"].pop("handle", None)
            expected["agent"]["assignment"]["snapshot"]["received_at"] = actual["agent"]["assignment"]["snapshot"]["received_at"]
            expected["agent"]["assignment"]["snapshot"]["expires_at"] = actual["agent"]["assignment"]["snapshot"]["expires_at"]
            expected["agent"]["facts"]["registration_id"] = registration_id
            expected["agent"]["facts"]["snapshot"]["received_at"] = actual["agent"]["facts"]["snapshot"]["received_at"]
            expected["agent"]["facts"]["snapshot"]["expires_at"] = actual["agent"]["facts"]["snapshot"]["expires_at"]
            expected["agent"]["context"]["context"]["received_at"] = actual["agent"]["context"]["context"]["received_at"]
            expected["agent"]["context"]["context"]["expires_at"] = actual["agent"]["context"]["context"]["expires_at"]
            expected["agent"]["context"]["context"]["freshness"] = actual["agent"]["context"]["context"]["freshness"]
            expected["agent"]["context"]["registration_id"] = registration_id
        elif method == "agent.list":
            expected_agent = json.loads(json.dumps(get_template))
            expected_agent["registration"]["registration_id"] = registration_id
            expected_agent["registration"]["registered_at"] = actual["agents"][0]["registration"]["registered_at"]
            expected_agent["assignment"]["registration_id"] = registration_id
            expected_agent["assignment"]["snapshot"].pop("handle", None)
            expected_agent["assignment"]["snapshot"]["received_at"] = actual["agents"][0]["assignment"]["snapshot"]["received_at"]
            expected_agent["assignment"]["snapshot"]["expires_at"] = actual["agents"][0]["assignment"]["snapshot"]["expires_at"]
            expected_agent["context"]["context"]["received_at"] = actual["agents"][0]["context"]["context"]["received_at"]
            expected_agent["context"]["context"]["expires_at"] = actual["agents"][0]["context"]["context"]["expires_at"]
            expected_agent["context"]["context"]["freshness"] = actual["agents"][0]["context"]["context"]["freshness"]
            expected_agent["facts"]["registration_id"] = registration_id
            expected_agent["facts"]["snapshot"]["received_at"] = actual["agents"][0]["facts"]["snapshot"]["received_at"]
            expected_agent["facts"]["snapshot"]["expires_at"] = actual["agents"][0]["facts"]["snapshot"]["expires_at"]
            expected_agent["context"]["registration_id"] = registration_id
            check(actual["agents"] == [expected_agent], f"fixture agent.list mismatch: {actual}")
            check(actual["next"] is None, str(actual))
            continue
        elif method == "agent.retire":
            check(set(actual["channel"]) == {"version", "registration_id", "channel", "writer", "snapshot"},
                  f"retire response schema mismatch: {actual}")
            check(actual["channel"]["registration_id"] == registration_id, str(actual))
            check(actual["channel"]["writer"]["handle"] == handles["assignment"], str(actual))
            check(actual["channel"]["writer"]["retired_at"] is not None, str(actual))
            continue
        check(actual == expected,
              f"fixture {method} differs from served response:\nactual={actual}\nexpected={expected}")
    check(registration_id is not None, "fixture did not register an agent")
    # Facts publication appends identifier-only pointers, never the values it carries.
    events = call(path, "events.read", {"after": 1, "limit": 500})["events"]
    pointers = [event for event in events if event["kind"] == "facts.published"]
    check(len(pointers) == 2, f"expected one pointer per accepted facts report: {events}")
    check(all(set(event) == {"seq", "kind", "registration_id", "at"} and event["registration_id"] == registration_id
              for event in pointers), f"facts pointer carries more than identifiers: {pointers}")
    check("claude-opus-example" not in json.dumps(events), "facts value leaked into the journal")


def spawn_daemon(binary, socket_path, state_path, server):
    env = {**os.environ, "RADAR_CONTROL_SOCKET": str(socket_path),
           "RADAR_CONTROL_STATE": str(state_path), "RADAR_CONTROL_BACKEND": "tmux",
           "RADAR_TMUX_SERVER": server}
    child = subprocess.Popen([str(binary), "daemon"], stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, env=env)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if child.poll() is not None:
            out, err = child.communicate(timeout=1)
            raise RuntimeError(f"daemon exited {child.returncode}: {out}\n{err}")
        if socket_path.exists():
            try:
                call(socket_path, "daemon.ping", {})
                return child
            except (OSError, RuntimeError):
                pass
        time.sleep(0.02)
    child.terminate()
    child.wait(timeout=2)
    raise TimeoutError("daemon socket did not appear")


def stop_daemon(child):
    if child.poll() is None:
        child.send_signal(signal.SIGTERM)
    try:
        child.wait(timeout=3)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait(timeout=2)
        raise
    out, err = child.communicate(timeout=1)
    if child.returncode not in (0, -signal.SIGTERM):
        raise RuntimeError(f"daemon exit {child.returncode}: {out}\n{err}")


def get_agent(path, registration_id):
    return call(path, "agent.get", {"registration_id": registration_id})["agent"]


def process_birth_identity(pid):
    """Return this process's Linux boot ID and procfs start-tick identity."""
    boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rfind(")") + 2:].split()
    return {"pid": pid, "boot_id": boot_id, "start_ticks": int(fields[19])}


def call_with_id(path, request_id, method, params):
    """One call under a caller-chosen id, so a replay can repeat it."""
    request = {"version": publisher.PROTOCOL_VERSION, "id": request_id,
               "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.connect(str(path))
        connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        answer = json.loads(connection.makefile("rb").readline())
    if "error" in answer:
        raise publisher.RegistryError(answer["error"]["code"], answer["error"]["message"])
    return answer["result"]


def main():
    fixture_pairs()
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / "target/debug/radar").resolve()
    check(binary.is_file(), f"missing daemon binary: {binary}")
    with tempfile.TemporaryDirectory(prefix="radar-agent-publisher-") as temp:
        root = Path(temp)
        os.chmod(root, 0o700)
        socket_path, state_path = root / "control.sock", root / "state"
        sentinel = "/private/example-session/SENTINEL-DO-NOT-LEAK.json"
        incarnation = "1c2d3e4f-5678-4abc-9def-0123456789ab"
        registration = {
            "source": "publisher-smoke", "incarnation": incarnation,
            "session": "c1a2b3d4-e5f6-4a7b-8c9d-0e1a2b3c4d5d",
            "label": "publisher smoke",
            "launch": {"executable": "/usr/bin/pi", "argv": ["--resume", sentinel],
                       "cwd": "/tmp", "session": {"path": sentinel},
                       "provenance": "smoke", "revision": "r1"},
        }
        identity = {"source": "herdsman-owner-example", "incarnation": incarnation,
                    "reporting_owner": "owner-smoke"}
        snapshot = publisher.owner_snapshot(
            "owner-waiting-new-state", "custom-waiting-reason",
            {"result": "custom-unclassified-outcome"}, ["custom-action"])
        server = tmux_server.name("publisher")
        if not tmux_server.start(server):
            print("skipped: a disposable tmux server needs tmux and a user manager")
            return
        daemon = spawn_daemon(binary, socket_path, state_path, server)
        try:
            validate_fixture(socket_path)
            check("events.v1" in call(socket_path, "daemon.ping", {})["contracts"],
                  "events.v1 contract missing")
            ping = call(socket_path, "daemon.ping", {})
            check("registry.v1" in ping["contracts"], f"contract missing: {ping}")

            registered, channel = publisher.publish_assignment(
                socket_path, registration, identity, snapshot, 1, lease_ms=1000)
            event_read = call(socket_path, "events.read", {"after": 0, "limit": 100})
            check(any(event.get("registration_id") == registered["registration_id"] for event in event_read["events"]),
                  "events consumer fixture did not observe the registered agent")
            registration_id, old_writer = registered["registration_id"], channel["writer"]
            check(channel["snapshot"]["freshness"] == "fresh", str(channel))
            check(channel["snapshot"]["lease_ms"] == 1000, str(channel))
            check(channel["snapshot"]["snapshot"] == snapshot, str(channel))
            initial_context = call(socket_path, "agent.context", {
                "registration_id": registration_id, "publisher": identity, "sequence": 1,
                "lease_ms": 1000, "context": {"session": "c1a2b3d4-e5f6-4a7b-8c9d-0e1a2b3c4d5e"},
            })
            context_writer = initial_context["writer"]
            context_sequence = 1
            get = get_agent(socket_path, registration_id)
            listed = call(socket_path, "agent.list", {"limit": 10})["agents"]
            public = json.dumps([get, listed], sort_keys=True)
            check(sentinel not in public, "private launch sentinel leaked in public get/list")
            check(context_writer["handle"] not in public, "context writer handle leaked")
            check(get["context"]["context"]["session"] == initial_context["context"]["context"]["session"], str(get))
            check(get["registration"]["launch"] == {"available": True, "revision": "r1"}, str(get))
            check(get["assignment"]["snapshot"]["snapshot"] == snapshot, str(get))
            check(get["execution"] is None, "assignment merged into execution")
            # Self facts need no owner and never reach execution or assignment.
            facts = publisher.publish_facts(
                socket_path, registration_id, {"source": "pi", "incarnation": "2d3e4f50-6789-4abc-8def-0123456789ab"},
                {"model": "live-model", "session_name": "live"}, 1)
            check(facts["snapshot"]["freshness"] == "fresh", str(facts))
            live = get_agent(socket_path, registration_id)
            check(live["facts"]["snapshot"]["snapshot"]["model"] == "live-model", str(live))
            check(live["assignment"]["snapshot"]["snapshot"] == snapshot, "facts changed assignment")

            # Session switch uses the same context writer and generation, and
            # never republishes the immutable registration or private launch path.
            switched_session = "e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b"
            switched = call(socket_path, "agent.context", {
                "registration_id": registration_id, "publisher": identity,
                "writer_handle": context_writer["handle"], "sequence": 2,
                "lease_ms": 1000, "context": {"session": switched_session},
            })
            check(switched["writer"]["generation"] == context_writer["generation"], str(switched))
            check(get_agent(socket_path, registration_id)["context"]["context"]["session"] == switched_session,
                  "session switch was not visible in agent.get")
            cleared = call(socket_path, "agent.context", {
                "registration_id": registration_id, "publisher": identity,
                "writer_handle": context_writer["handle"], "sequence": 3,
                "lease_ms": 1000, "context": {"session": None},
            })
            check(cleared["context"]["context"]["session"] is None, str(cleared))
            public_clear = get_agent(socket_path, registration_id)
            check(public_clear["context"]["context"]["session"] is None, str(public_clear))
            check(context_writer["handle"] not in json.dumps(public_clear), "context writer handle leaked")
            check(sentinel not in json.dumps(public_clear), "launch data leaked after context update")

            registered2, heartbeat = publisher.publish_assignment(
                socket_path, registration, identity, snapshot, 2, lease_ms=1000)
            check(registered2["registration_id"] == registration_id, "registration retry changed identity")
            check(heartbeat["writer"]["handle"] == old_writer["handle"] and
                  heartbeat["writer"]["generation"] == old_writer["generation"],
                  "reconnect did not reacquire the same writer")
            replay = {"registration_id": registration_id, "channel": "assignment",
                      "writer_handle": old_writer["handle"], "sequence": 2,
                      "lease_ms": 1000, "snapshot": snapshot}
            one = call(socket_path, "agent.publish", replay)["channel"]
            time.sleep(0.05)
            two = call(socket_path, "agent.publish", replay)["channel"]
            check(one["snapshot"]["received_at"] == two["snapshot"]["received_at"],
                  "identical replay extended the lease")

            time.sleep(1.05)
            successor = {"source": "replacement-owner", "incarnation":
                         "9f8e7d6c-4321-4def-8abc-0123456789ab",
                         "reporting_owner": "owner-smoke"}
            replacement = call(socket_path, "agent.acquire", {
                "registration_id": registration_id, "channel": "assignment", "publisher": successor,
                "replace": {"generation": old_writer["generation"],
                            "handle": old_writer["handle"]},
            })["writer"]
            check(replacement["generation"] == old_writer["generation"] + 1, str(replacement))
            check(get_agent(socket_path, registration_id)["assignment"]["snapshot"]["freshness"] == "stale",
                  "replacement did not stale previous publication")
            try:
                call(socket_path, "agent.publish", replay)
                raise AssertionError("old writer unexpectedly published")
            except publisher.RegistryError as error:
                check(error.code == "refused", str(error))
            try:
                call(socket_path, "agent.acquire", {
                    "registration_id": registration_id, "channel": "assignment", "publisher": identity,
                })
                raise AssertionError("implicit takeover unexpectedly succeeded")
            except publisher.RegistryError as error:
                check(error.code == "refused", str(error))

            stop_daemon(daemon)
            daemon = spawn_daemon(binary, socket_path, state_path, server)
            restored = get_agent(socket_path, registration_id)["assignment"]["snapshot"]
            check(restored["restored"] is True and restored["freshness"] == "stale", str(restored))
            restored_facts = get_agent(socket_path, registration_id)["facts"]["snapshot"]
            check(restored_facts["restored"] is True and restored_facts["freshness"] == "stale",
                  str(restored_facts))
            check(restored_facts["snapshot"]["model"] == "live-model", str(restored_facts))
            registered3 = call(socket_path, "agent.register", registration)["registration"]
            check(registered3["registration_id"] == registration_id, "restart changed registration identity")
            binding = call(socket_path, "agent.acquire", {
                "registration_id": registration_id, "channel": "assignment", "publisher": successor,
            })["writer"]
            check(binding["handle"] == replacement["handle"], "restart changed current writer")
            refreshed = call(socket_path, "agent.publish", {
                "registration_id": registration_id, "channel": "assignment",
                "writer_handle": binding["handle"], "sequence": 1, "snapshot": snapshot,
            })["channel"]
            check(refreshed["snapshot"]["freshness"] == "fresh", str(refreshed))
            check(sentinel not in json.dumps(get_agent(socket_path, registration_id)),
                  "private launch sentinel leaked after restart")
            retired = call(socket_path, "agent.retire", {
                "registration_id": registration_id, "channel": "assignment", "writer_handle": binding["handle"],
            })["channel"]
            check(retired["writer"]["retired_at"] is not None, str(retired))
            print("PASS: live registry and event fixtures; trusted disposable daemon; register/acquire/publish/context/get/list/retire/events.read; "
                  "session switch/null; privacy; replay/heartbeat; replacement/fencing; restart freshness")
        finally:
            stop_daemon(daemon)
            tmux_server.stop(server)
        smoke = subprocess.run([sys.executable, str(ROOT / "tests/fixtures/events_consumer_smoke.py"),
                                str(binary)], capture_output=True, text=True, timeout=60)
        check(smoke.returncode == 0, f"events consumer smoke failed: {smoke.stdout} {smoke.stderr}")
        print(smoke.stdout.strip())


if __name__ == "__main__":
    main()

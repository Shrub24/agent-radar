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
    expected = ["agent.register", "agent.acquire", "agent.publish",
                "agent.context", "agent.context", "agent.context", "agent.context",
                "agent.get", "agent.list", "agent.context", "agent.retire",
                "spawn", "agent.register", "spawn.get", "spawn.list", "spawn",
                "child.close", "child.close"]
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
    close_pair = [pair for pair in pairs if pair[0]["method"] == "child.close"]
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
    handles, agent_id = {}, None
    previous_context = None
    get_template = next(response["result"]["agent"] for request, response in pairs
                        if request["method"] == "agent.get")
    for request, response in pairs:
        method = request["method"]
        if method in ("spawn", "spawn.get", "spawn.list", "child.close"):
            continue
        params = json.loads(json.dumps(request["params"]))
        if method == "agent.register" and "spawn_token" in params:
            continue
        if method in ("agent.acquire", "agent.publish", "agent.context", "agent.get", "agent.retire"):
            params["agent_id"] = agent_id
        if method == "agent.publish":
            params["writer_handle"] = handles["assignment"]
        if method == "agent.retire":
            params["writer_handle"] = handles["assignment"]
        if method == "agent.context" and params.get("writer_handle") == "<opaque-uuid>":
            params["writer_handle"] = handles["context"]
        if method == "agent.context" and "replace" in params:
            params["replace"]["handle"] = handles["context"]
            # The fixture uses a 1s incumbent lease so this validates the
            # documented expiry-gated replacement without inventing retire API.
            time.sleep(1.05)
        actual = call(path, method, params)
        expected = json.loads(json.dumps(response["result"]))
        if method == "agent.register":
            agent_id = actual["registration"]["agent_id"]
            expected["registration"]["agent_id"] = agent_id
            expected["registration"]["registered_at"] = actual["registration"]["registered_at"]
            expected["registration"]["session"] = actual["registration"]["session"]
        elif method == "agent.acquire":
            handles["assignment"] = actual["writer"]["handle"]
            expected["writer"]["handle"] = handles["assignment"]
        elif method == "agent.publish":
            channel = actual["channel"]
            expected["channel"]["agent_id"] = agent_id
            expected["channel"]["writer"]["handle"] = handles["assignment"]
            expected["channel"]["snapshot"]["handle"] = handles["assignment"]
            expected["channel"]["snapshot"]["received_at"] = channel["snapshot"]["received_at"]
            expected["channel"]["snapshot"]["expires_at"] = channel["snapshot"]["expires_at"]
            check(channel["snapshot"]["lease_ms"] == publisher.DEFAULT_LEASE_MS, str(channel))
        elif method == "agent.context":
            context = actual["context"]
            expected["context"]["writer"].pop("handle", None)
            if "context" not in handles or "replace" in params:
                handles["context"] = actual["writer"]["handle"]
            expected["writer"]["handle"] = handles["context"]
            expected["context"]["agent_id"] = agent_id
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
            expected["agent"]["registration"]["agent_id"] = agent_id
            expected["agent"]["registration"]["registered_at"] = actual["agent"]["registration"]["registered_at"]
            expected["agent"]["assignment"]["agent_id"] = agent_id
            # get/list facts intentionally omit the private fencing handle.
            expected["agent"]["assignment"]["snapshot"].pop("handle", None)
            expected["agent"]["assignment"]["snapshot"]["received_at"] = actual["agent"]["assignment"]["snapshot"]["received_at"]
            expected["agent"]["assignment"]["snapshot"]["expires_at"] = actual["agent"]["assignment"]["snapshot"]["expires_at"]
            expected["agent"]["context"]["context"]["received_at"] = actual["agent"]["context"]["context"]["received_at"]
            expected["agent"]["context"]["context"]["expires_at"] = actual["agent"]["context"]["context"]["expires_at"]
            expected["agent"]["context"]["context"]["freshness"] = actual["agent"]["context"]["context"]["freshness"]
            expected["agent"]["context"]["agent_id"] = agent_id
        elif method == "agent.list":
            expected_agent = json.loads(json.dumps(get_template))
            expected_agent["registration"]["agent_id"] = agent_id
            expected_agent["registration"]["registered_at"] = actual["agents"][0]["registration"]["registered_at"]
            expected_agent["assignment"]["agent_id"] = agent_id
            expected_agent["assignment"]["snapshot"].pop("handle", None)
            expected_agent["assignment"]["snapshot"]["received_at"] = actual["agents"][0]["assignment"]["snapshot"]["received_at"]
            expected_agent["assignment"]["snapshot"]["expires_at"] = actual["agents"][0]["assignment"]["snapshot"]["expires_at"]
            expected_agent["context"]["context"]["received_at"] = actual["agents"][0]["context"]["context"]["received_at"]
            expected_agent["context"]["context"]["expires_at"] = actual["agents"][0]["context"]["context"]["expires_at"]
            expected_agent["context"]["context"]["freshness"] = actual["agents"][0]["context"]["context"]["freshness"]
            expected_agent["context"]["agent_id"] = agent_id
            check(actual["agents"] == [expected_agent], f"fixture agent.list mismatch: {actual}")
            check(actual["next"] is None, str(actual))
            continue
        elif method == "agent.retire":
            check(set(actual["channel"]) == {"version", "agent_id", "channel", "writer", "snapshot"},
                  f"retire response schema mismatch: {actual}")
            check(actual["channel"]["agent_id"] == agent_id, str(actual))
            check(actual["channel"]["writer"]["handle"] == handles["assignment"], str(actual))
            check(actual["channel"]["writer"]["retired_at"] is not None, str(actual))
            continue
        check(actual == expected,
              f"fixture {method} differs from served response:\nactual={actual}\nexpected={expected}")
    check(agent_id is not None, "fixture did not register an agent")
    # The scripted-backend sequence below validates spawn wire shapes end to end.


def spawn_daemon(binary, socket_path, state_path, herdr_fake=None):
    env = {**os.environ, "RADAR_CONTROL_SOCKET": str(socket_path),
           "RADAR_CONTROL_STATE": str(state_path)}
    if herdr_fake is not None:
        env["PATH"] = f"{herdr_fake.parent}:{os.environ['PATH']}"
        env["RADAR_SPAWN_FAKE_STATE"] = str(herdr_fake.parent / "fake-state")
        env["RADAR_HERDR_FAKE_SOCKET"] = str(herdr_fake.parent / "fake-state" / "herdr.sock")
    child = subprocess.Popen([str(binary), "daemon"], stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, env=env)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if child.poll() is not None:
            out, err = child.communicate(timeout=1)
            raise RuntimeError(f"daemon exited {child.returncode}: {out}\n{err}")
        if socket_path.exists():
            try:
                call(socket_path, "ping", {})
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


def get_agent(path, agent_id):
    return call(path, "agent.get", {"agent_id": agent_id})["agent"]


def process_birth_identity(pid):
    """Return this process's Linux boot ID and procfs start-tick identity."""
    boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rfind(")") + 2:].split()
    return {"pid": pid, "boot_id": boot_id, "start_ticks": int(fields[19])}


def call_with_id(path, request_id, method, params):
    """One call under a caller-chosen id, so a replay can repeat it."""
    request = {"version": 1, "id": request_id, "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.connect(str(path))
        connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        answer = json.loads(connection.makefile("rb").readline())
    if "error" in answer:
        raise publisher.RegistryError(answer["error"]["code"], answer["error"]["message"])
    return answer["result"]


def validate_spawn_scripted(binary, socket_path, state_path, root, fake_backend):
    fake_bin = root / "fake-bin"
    fake_state = fake_bin / "fake-state"
    fake_state.mkdir(parents=True)
    fake = fake_bin / "herdr"
    fake.write_text(fake_backend)
    fake.chmod(0o755)
    server = subprocess.Popen([sys.executable, str(fake), "__serve"], env={**os.environ,
                                 "RADAR_SPAWN_FAKE_STATE": str(fake_state),
                                 "RADAR_HERDR_FAKE_SOCKET": str(fake_state / "herdr.sock")})
    deadline = time.monotonic() + 3
    while not (fake_state / "herdr.sock").exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    daemon = spawn_daemon(binary, socket_path, state_path, fake)
    try:
        ping = call(socket_path, "ping", {})
        check(ping["protocol"] == 1, str(ping))
        check({"creation", "launch"}.issubset(ping["capabilities"]), str(ping))
        parent = call(socket_path, "agent.register", {
            "source": "fixture-parent", "incarnation": "11111111-1111-4111-8111-111111111111",
            "location": {"backend": "herdr", "workspace": "wA", "tab": "wA:t1", "pane": "wA:p1"},
        })["registration"]["agent_id"]
        spawn_id = "56565656-5656-4656-8656-565656565656"
        response = call(socket_path, "spawn", {"request": {"parent": parent,
                            "executable": "/usr/bin/pi", "argv": ["--child", "row one"]},
                            "requester": "herdsman"})
        edge = response["spawn"]
        check(response["request"]["outcome"] == "completed", str(response))
        check(edge["created"] == "completed" and edge["launched"] == "completed", str(edge))
        check("bound" not in edge and "token" not in json.dumps(response), "spawn response leaked token")
        private_edge = next((state_path / "spawns").glob("*.json"))
        private_token = json.loads(private_edge.read_text())["token"]
        check(private_token, "edge did not persist token")
        launch = json.loads((fake_state / "launch.json").read_text())
        check(launch["token"] == private_token, "launched environment token differs from edge token")
        # The scripted pane's foreground occupant is this test process, so the
        # child's registered birth identity is the one the daemon verifies.
        (fake_state / "foreground-pid").write_text(str(os.getpid()))
        child = call(socket_path, "agent.register", {
            "source": "fixture-child", "incarnation": "34343434-3434-4343-8343-343434343434",
            "location": {"backend": "runtime", "workspace": "wA", "tab": "wA:t1", "pane": "wA:p2"},
            "process": process_birth_identity(os.getpid()),
            "spawn_token": private_token,
        })["registration"]
        check("spawn_token" not in json.dumps(child), "registration response leaked token")
        get = call(socket_path, "spawn.get", {"request_id": edge["request_id"]})["spawn"]
        check(get["state"] == "bound" and get["bound"]["incarnation"] == child["incarnation"], str(get))
        check(get["freshness"] == "fresh" and "token" not in json.dumps(get), str(get))
        listed = call(socket_path, "spawn.list", {"limit": 10})
        check(listed["spawns"] == [get] and listed["next"] is None, str(listed))
        for channel, publisher_identity in (
            ("assignment", {"source": "fixture-owner", "incarnation": "9f8e7d6c-4321-4def-8abc-0123456789ab", "reporting_owner": "owner-smoke"}),
            ("execution", {"source": "fixture-pi", "incarnation": "2f8e7d6c-4321-4def-8abc-0123456789ab"}),
        ):
            binding = call(socket_path, "agent.acquire", {
                "agent_id": child["agent_id"], "channel": channel, "publisher": publisher_identity,
            })["writer"]
            call(socket_path, "agent.publish", {
                "agent_id": child["agent_id"], "channel": channel,
                "writer_handle": binding["handle"], "sequence": 1,
                "snapshot": {"activity": f"{channel}-before-close", "actions": ["inspect"]},
            })
        before_agent = get_agent(socket_path, child["agent_id"])
        before_topology = call(socket_path, "spawn.get", {"request_id": edge["request_id"]})["spawn"]
        public = json.dumps([response, child, get, listed])
        check(private_token not in public, "token leaked through a public spawn projection")
        close_request, close_response = next(
            pair for pair in fixture_pairs() if pair[0]["method"] == "child.close")
        close_id = close_request["id"]
        close_params = {"spawn_request_id": edge["request_id"], "source": child["source"],
                        "incarnation": child["incarnation"], "intent": close_request["params"]["intent"]}
        expected_close = json.loads(json.dumps(close_response["result"]["close"]))
        # The fixture's close entry and its identical replay both answer from the
        # one recorded outcome; only the first of the two may dispatch.
        for _ in range(2):
            actual = call_with_id(socket_path, close_id, "child.close", close_params)
            check(actual == {"close": expected_close}, f"child.close fixture differs: {actual}")
        check((fake_state / "close-count").read_text() == "1",
              "same-ID close replay dispatched more than once")
        after_agent = get_agent(socket_path, child["agent_id"])
        after_topology = call(socket_path, "spawn.get", {"request_id": edge["request_id"]})["spawn"]
        for channel in ("assignment", "execution"):
            check(after_agent[channel] == before_agent[channel],
                  f"child.close changed {channel} publication")
        check(after_topology == before_topology, "child.close changed spawn topology")
        # A second registration with the consumed token refuses rather than rebinding.
        try:
            call(socket_path, "agent.register", {
                "source": "fixture-other-child", "incarnation": "44444444-4444-4444-8444-444444444444",
                "spawn_token": private_token,
            })
            raise AssertionError("spent token bound twice")
        except publisher.RegistryError as error:
            check(error.code == "refused", str(error))
    finally:
        stop_daemon(daemon)
        server.terminate()
        server.wait(timeout=2)


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
        daemon = spawn_daemon(binary, socket_path, state_path)
        try:
            validate_fixture(socket_path)
            ping = call(socket_path, "ping", {})
            check("agent_registry" in ping["capabilities"], f"capability missing: {ping}")

            registered, channel = publisher.publish_assignment(
                socket_path, registration, identity, snapshot, 1, lease_ms=1000)
            agent_id, old_writer = registered["agent_id"], channel["writer"]
            check(channel["snapshot"]["freshness"] == "fresh", str(channel))
            check(channel["snapshot"]["lease_ms"] == 1000, str(channel))
            check(channel["snapshot"]["snapshot"] == snapshot, str(channel))
            initial_context = call(socket_path, "agent.context", {
                "agent_id": agent_id, "publisher": identity, "sequence": 1,
                "lease_ms": 1000, "context": {"session": "c1a2b3d4-e5f6-4a7b-8c9d-0e1a2b3c4d5e"},
            })
            context_writer = initial_context["writer"]
            context_sequence = 1
            get = get_agent(socket_path, agent_id)
            listed = call(socket_path, "agent.list", {"limit": 10})["agents"]
            public = json.dumps([get, listed], sort_keys=True)
            check(sentinel not in public, "private launch sentinel leaked in public get/list")
            check(context_writer["handle"] not in public, "context writer handle leaked")
            check(get["context"]["context"]["session"] == initial_context["context"]["context"]["session"], str(get))
            check(get["registration"]["launch"] == {"available": True, "revision": "r1"}, str(get))
            check(get["assignment"]["snapshot"]["snapshot"] == snapshot, str(get))
            check(get["execution"] is None, "assignment merged into execution")

            # Session switch uses the same context writer and generation, and
            # never republishes the immutable registration or private launch path.
            switched_session = "e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b"
            switched = call(socket_path, "agent.context", {
                "agent_id": agent_id, "publisher": identity,
                "writer_handle": context_writer["handle"], "sequence": 2,
                "lease_ms": 1000, "context": {"session": switched_session},
            })
            check(switched["writer"]["generation"] == context_writer["generation"], str(switched))
            check(get_agent(socket_path, agent_id)["context"]["context"]["session"] == switched_session,
                  "session switch was not visible in agent.get")
            cleared = call(socket_path, "agent.context", {
                "agent_id": agent_id, "publisher": identity,
                "writer_handle": context_writer["handle"], "sequence": 3,
                "lease_ms": 1000, "context": {"session": None},
            })
            check(cleared["context"]["context"]["session"] is None, str(cleared))
            public_clear = get_agent(socket_path, agent_id)
            check(public_clear["context"]["context"]["session"] is None, str(public_clear))
            check(context_writer["handle"] not in json.dumps(public_clear), "context writer handle leaked")
            check(sentinel not in json.dumps(public_clear), "launch data leaked after context update")

            registered2, heartbeat = publisher.publish_assignment(
                socket_path, registration, identity, snapshot, 2, lease_ms=1000)
            check(registered2["agent_id"] == agent_id, "registration retry changed identity")
            check(heartbeat["writer"]["handle"] == old_writer["handle"] and
                  heartbeat["writer"]["generation"] == old_writer["generation"],
                  "reconnect did not reacquire the same writer")
            replay = {"agent_id": agent_id, "channel": "assignment",
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
                "agent_id": agent_id, "channel": "assignment", "publisher": successor,
                "replace": {"generation": old_writer["generation"],
                            "handle": old_writer["handle"]},
            })["writer"]
            check(replacement["generation"] == old_writer["generation"] + 1, str(replacement))
            check(get_agent(socket_path, agent_id)["assignment"]["snapshot"]["freshness"] == "stale",
                  "replacement did not stale previous publication")
            try:
                call(socket_path, "agent.publish", replay)
                raise AssertionError("old writer unexpectedly published")
            except publisher.RegistryError as error:
                check(error.code == "refused", str(error))
            try:
                call(socket_path, "agent.acquire", {
                    "agent_id": agent_id, "channel": "assignment", "publisher": identity,
                })
                raise AssertionError("implicit takeover unexpectedly succeeded")
            except publisher.RegistryError as error:
                check(error.code == "refused", str(error))

            stop_daemon(daemon)
            daemon = spawn_daemon(binary, socket_path, state_path)
            restored = get_agent(socket_path, agent_id)["assignment"]["snapshot"]
            check(restored["restored"] is True and restored["freshness"] == "stale", str(restored))
            registered3 = call(socket_path, "agent.register", registration)["registration"]
            check(registered3["agent_id"] == agent_id, "restart changed registration identity")
            binding = call(socket_path, "agent.acquire", {
                "agent_id": agent_id, "channel": "assignment", "publisher": successor,
            })["writer"]
            check(binding["handle"] == replacement["handle"], "restart changed current writer")
            refreshed = call(socket_path, "agent.publish", {
                "agent_id": agent_id, "channel": "assignment",
                "writer_handle": binding["handle"], "sequence": 1, "snapshot": snapshot,
            })["channel"]
            check(refreshed["snapshot"]["freshness"] == "fresh", str(refreshed))
            check(sentinel not in json.dumps(get_agent(socket_path, agent_id)),
                  "private launch sentinel leaked after restart")
            retired = call(socket_path, "agent.retire", {
                "agent_id": agent_id, "channel": "assignment", "writer_handle": binding["handle"],
            })["channel"]
            check(retired["writer"]["retired_at"] is not None, str(retired))
            print("PASS: live registry fixture; trusted disposable daemon; register/acquire/publish/context/get/list/retire; "
                  "session switch/null; privacy; replay/heartbeat; replacement/fencing; restart freshness")
        finally:
            stop_daemon(daemon)
        spawn_socket, spawn_state = root / "spawn.sock", root / "spawn-state"
        validate_spawn_scripted(binary, spawn_socket, spawn_state, root,
                                (ROOT / "tests/fixtures/fake-herdr.py").read_text())
        print("PASS: scripted Herdr spawn; child token binding; spawn.get/list freshness and redaction; "
              "verified child close with same-ID replay, unchanged publications and topology")


if __name__ == "__main__":
    main()

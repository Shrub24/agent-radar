#!/usr/bin/env python3
"""Exercise the documented direct publisher against a disposable Radar daemon."""

import importlib.util
import json
import os
from pathlib import Path
import signal
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
                "agent.get", "agent.list", "agent.retire"]
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
    return pairs


def validate_fixture(path):
    """Run fixture requests and compare stable response schema/content live."""
    pairs = fixture_pairs()
    ids, handles, agent_id = {}, {}, None
    get_template = pairs[3][1]["result"]["agent"]
    for request, response in pairs:
        method = request["method"]
        params = json.loads(json.dumps(request["params"]))
        if method in ("agent.acquire", "agent.publish", "agent.get", "agent.retire"):
            params["agent_id"] = agent_id
        if method == "agent.publish":
            params["writer_handle"] = handles["assignment"]
        if method == "agent.retire":
            params["writer_handle"] = handles["assignment"]
        actual = call(path, method, params)
        expected = json.loads(json.dumps(response["result"]))
        if method == "agent.register":
            agent_id = actual["registration"]["agent_id"]
            expected["registration"]["agent_id"] = agent_id
            expected["registration"]["registered_at"] = actual["registration"]["registered_at"]
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
        elif method == "agent.get":
            expected["agent"]["registration"]["agent_id"] = agent_id
            expected["agent"]["registration"]["registered_at"] = actual["agent"]["registration"]["registered_at"]
            expected["agent"]["assignment"]["agent_id"] = agent_id
            # get/list facts intentionally omit the private fencing handle.
            expected["agent"]["assignment"]["snapshot"].pop("handle", None)
            expected["agent"]["assignment"]["snapshot"]["received_at"] = actual["agent"]["assignment"]["snapshot"]["received_at"]
            expected["agent"]["assignment"]["snapshot"]["expires_at"] = actual["agent"]["assignment"]["snapshot"]["expires_at"]
        elif method == "agent.list":
            expected_agent = json.loads(json.dumps(get_template))
            expected_agent["registration"]["agent_id"] = agent_id
            expected_agent["registration"]["registered_at"] = actual["agents"][0]["registration"]["registered_at"]
            expected_agent["assignment"]["agent_id"] = agent_id
            expected_agent["assignment"]["snapshot"].pop("handle", None)
            expected_agent["assignment"]["snapshot"]["received_at"] = actual["agents"][0]["assignment"]["snapshot"]["received_at"]
            expected_agent["assignment"]["snapshot"]["expires_at"] = actual["agents"][0]["assignment"]["snapshot"]["expires_at"]
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


def spawn_daemon(binary, socket_path, state_path):
    env = {**os.environ, "RADAR_CONTROL_SOCKET": str(socket_path),
           "RADAR_CONTROL_STATE": str(state_path)}
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
            get = get_agent(socket_path, agent_id)
            listed = call(socket_path, "agent.list", {"limit": 10})["agents"]
            public = json.dumps([get, listed], sort_keys=True)
            check(sentinel not in public, "private launch sentinel leaked in public get/list")
            check(get["registration"]["launch"] == {"available": True, "revision": "r1"}, str(get))
            check(get["assignment"]["snapshot"]["snapshot"] == snapshot, str(get))
            check(get["execution"] is None, "assignment merged into execution")

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
            print("PASS: live fixture response schemas; trusted disposable daemon; "
                  "register/acquire/publish/get/list/retire; unknown vocabulary; launch privacy; "
                  "replay/heartbeat; expiry replacement; old-writer fencing; restart freshness; no Herdr")
        finally:
            stop_daemon(daemon)


if __name__ == "__main__":
    main()

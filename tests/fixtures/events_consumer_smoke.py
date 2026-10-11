#!/usr/bin/env python3
"""Run the documented events consumer against a disposable local daemon."""

import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
CONSUMER = ROOT / "examples/agent-publisher/events_consumer.py"


def call(path, method, params):
    request = {"version": 1, "id": str(uuid.uuid4()), "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(3)
        connection.connect(str(path))
        connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        response = json.loads(connection.makefile("rb").readline())
    if "error" in response:
        raise RuntimeError(response["error"])
    return response["result"]


def pointer_line(run, seconds):
    """The first event pointer line the consumer prints, within a bounded wait."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if select.select([run.stdout], [], [], deadline - time.monotonic())[0]:
            line = run.stdout.readline()
            if not line:
                break
            record = json.loads(line)
            if "event" in record:
                return record
    return None


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / "target/debug/radar").resolve()
    with tempfile.TemporaryDirectory(prefix="radar-events-consumer-") as directory:
        root = Path(directory)
        os.chmod(root, 0o700)
        sock, state = root / "daemon.sock", root / "state"
        env = {**os.environ, "RADAR_CONTROL_SOCKET": str(sock), "RADAR_CONTROL_STATE": str(state)}
        daemon = subprocess.Popen([str(binary), "daemon"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not sock.exists() and time.monotonic() < deadline:
                if daemon.poll() is not None:
                    raise RuntimeError(daemon.stderr.read())
                time.sleep(0.02)
            call(sock, "ping", {})
            run = subprocess.Popen([sys.executable, str(CONSUMER), str(sock)],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                registered = call(sock, "agent.register", {
                    "source": "consumer-smoke", "incarnation": "1c2d3e4f-5678-4abc-9def-0123456789ab",
                })["registration"]["agent_id"]
                observed = pointer_line(run, 5)
                assert run.poll() is None, "consumer exited instead of following the journal"
                run.send_signal(signal.SIGTERM)
                try:
                    run.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    run.kill()
                    raise
                assert observed is not None, f"consumer printed no event; stderr: {run.stderr.read()}"
                assert observed["event"]["kind"] == "agent.registered", observed
                assert observed["event"]["agent_id"] == registered, observed
                assert observed["agent"]["registration"]["agent_id"] == registered, observed
            finally:
                if run.poll() is None:
                    run.kill()
                    run.wait(timeout=2)
            print("PASS: reconnecting events consumer observed and re-read a live event")
        finally:
            daemon.send_signal(signal.SIGTERM)
            try:
                daemon.wait(timeout=3)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=2)


if __name__ == "__main__":
    main()

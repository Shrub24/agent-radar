#!/usr/bin/env python3
"""Reconnectable consumer for the Radar durable event journal."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import socket
import sys
import time
import uuid


def call(path: Path, method: str, params: dict) -> dict:
    request = {"version": 1, "id": str(uuid.uuid4()), "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(5)
        connection.connect(str(path))
        connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        answer = json.loads(connection.makefile("rb").readline())
    if answer.get("id") != request["id"]:
        raise RuntimeError("mismatched control-plane response id")
    if "error" in answer:
        raise RuntimeError(f"{answer['error']['code']}: {answer['error']['message']}")
    return answer["result"]


def full_read(path: Path) -> None:
    after = None
    while True:
        params = {"limit": 100}
        if after is not None:
            params["after"] = after
        page = call(path, "agent.list", params)
        for agent in page["agents"]:
            print(json.dumps({"kind": "agent", "value": agent}, separators=(",", ":")), flush=True)
        after = page["next"]
        if after is None:
            break
    after = None
    while True:
        params = {"limit": 100}
        if after is not None:
            params["after"] = after
        page = call(path, "spawn.list", params)
        for spawn in page["spawns"]:
            print(json.dumps({"kind": "spawn", "value": spawn}, separators=(",", ":")), flush=True)
        after = page["next"]
        if after is None:
            break
    for record in call(path, "requests", {"limit": 200})["requests"]:
        print(json.dumps({"kind": "request", "value": record}, separators=(",", ":")), flush=True)


def consume(path: Path) -> None:
    if "events_v1" not in call(path, "ping", {})["capabilities"]:
        raise RuntimeError("daemon does not advertise events_v1")
    epoch = None
    after = 0
    while True:
        params = {"after": after, "limit": 100, "wait_ms": 1000}
        if epoch is not None:
            params["epoch"] = epoch
        result = call(path, "events.read", params)
        expired = result.get("cursor_expired")
        if expired is not None:
            full_read(path)
            epoch = expired["epoch"]
            after = expired["cursor"]["sequence"]
            continue
        epoch = result["epoch"]
        for event in result["events"]:
            if event.get("agent_id") is not None:
                value = call(path, "agent.get", {"agent_id": event["agent_id"]})
                print(json.dumps({"event": event, "agent": value.get("agent")}, separators=(",", ":")), flush=True)
            elif event.get("spawn") is not None:
                value = call(path, "spawn.get", {"request_id": event["spawn"]})
                print(json.dumps({"event": event, "spawn": value.get("spawn")}, separators=(",", ":")), flush=True)
            elif event.get("request_id") is not None:
                value = call(path, "request", {"id": event["request_id"]})
                print(json.dumps({"event": event, "request": value.get("request")}, separators=(",", ":")), flush=True)
            else:
                print(json.dumps({"event": event}, separators=(",", ":")), flush=True)
        after = result["cursor"]["sequence"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path)
    parser.add_argument("--once", action="store_true", help="make one bounded events.read request")
    args = parser.parse_args()
    if args.once:
        result = call(args.socket, "events.read", {"after": 0, "limit": 100, "wait_ms": 0})
        print(json.dumps(result, separators=(",", ":")))
        return 0
    while True:
        try:
            consume(args.socket)
        except (OSError, TimeoutError, json.JSONDecodeError, RuntimeError) as error:
            print(f"events consumer reconnecting: {error}", file=sys.stderr, flush=True)
            time.sleep(0.2)


if __name__ == "__main__":
    raise SystemExit(main())

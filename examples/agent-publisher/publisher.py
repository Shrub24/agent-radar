#!/usr/bin/env python3
"""Minimal reconnecting publisher for the Radar direct agent registry.

This intentionally publishes only an owner assignment projection. Execution
facts are a separate channel with a separate writer identity and lifecycle.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import socket
import stat
import time
import uuid

PROTOCOL_VERSION = 1
MAX_LINE_BYTES = 1024 * 1024
DEFAULT_LEASE_MS = 30_000
MAX_LEASE_MS = 300_000


def trusted_socket(path: Path) -> None:
    """Check the daemon's read-only socket trust rule before connecting."""
    parent = path.parent
    parent_stat = parent.lstat()
    if stat.S_ISLNK(parent_stat.st_mode) or not stat.S_ISDIR(parent_stat.st_mode):
        raise RuntimeError(f"socket parent is not a real directory: {parent}")
    if parent_stat.st_uid != os.getuid() or stat.S_IMODE(parent_stat.st_mode) != 0o700:
        raise RuntimeError(f"socket parent must be uid {os.getuid()} mode 0700: {parent}")
    socket_stat = path.lstat()
    if stat.S_ISLNK(socket_stat.st_mode) or not stat.S_ISSOCK(socket_stat.st_mode):
        raise RuntimeError(f"socket path is not a real socket: {path}")
    if socket_stat.st_uid != os.getuid():
        raise RuntimeError(f"socket must be owned by uid {os.getuid()}: {path}")


def call(path: Path, method: str, params: dict) -> dict:
    trusted_socket(path)
    request = {"version": PROTOCOL_VERSION, "id": str(uuid.uuid4()),
               "method": method, "params": params}
    payload = (json.dumps(request, separators=(",", ":")) + "\n").encode()
    if len(payload) - 1 > MAX_LINE_BYTES:
        raise ValueError("request exceeds the control-plane line limit")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(5)
        connection.connect(str(path))
        connection.sendall(payload)
        stream = connection.makefile("rb")
        line = stream.readline(MAX_LINE_BYTES + 2)
    if not line.endswith(b"\n") or len(line) - 1 > MAX_LINE_BYTES:
        raise RuntimeError("missing, truncated, or oversized control-plane response")
    answer = json.loads(line)
    if answer.get("id") != request["id"]:
        raise RuntimeError("mismatched control-plane response id")
    if "error" in answer:
        raise RegistryError(answer["error"]["code"], answer["error"]["message"])
    return answer["result"]


class RegistryError(RuntimeError):
    def __init__(self, code: str, message: str):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


def publish_assignment(path: Path, registration: dict, publisher: dict,
                       snapshot: dict, sequence: int, *, lease_ms: int | None = None,
                       replace: dict | None = None) -> tuple[dict, dict]:
    """Idempotently register/acquire and publish one assignment snapshot.

    Call this again after reconnect with the same immutable registration,
    publisher identity, snapshot and sequence to retry a possibly lost response.
    Only increment sequence for new content or a heartbeat; identical sequence
    replay does not extend the lease.
    """
    registered = call(path, "agent.register", registration)["registration"]
    acquire = {"agent_id": registered["agent_id"], "channel": "assignment",
               "publisher": publisher}
    if replace is not None:
        acquire["replace"] = replace
    binding = call(path, "agent.acquire", acquire)["writer"]
    params = {"agent_id": registered["agent_id"], "channel": "assignment",
              "writer_handle": binding["handle"], "sequence": sequence,
              "snapshot": snapshot}
    if lease_ms is not None:
        params["lease_ms"] = lease_ms
    channel = call(path, "agent.publish", params)["channel"]
    return registered, channel


def publish_context(path: Path, agent_id: str, publisher: dict,
                    writer: dict | None, sequence: int, session: str | None,
                    *, lease_ms: int | None = None) -> tuple[dict, dict]:
    """Publish a changed current session; explicit null clears the association."""
    params = {"agent_id": agent_id, "publisher": publisher, "sequence": sequence,
              "context": {"session": session}}
    if writer is not None:
        params["writer_handle"] = writer["handle"]
    if lease_ms is not None:
        params["lease_ms"] = lease_ms
    result = call(path, "agent.context", params)
    return result["writer"], result["context"]


def owner_snapshot(activity: str, waiting_reason: str | None = None,
                   last_outcome: dict | None = None, actions: list[str] | None = None) -> dict:
    """Map Herdsman's existing owner projection without importing its tokens."""
    result = {"activity": activity}
    if waiting_reason is not None:
        result["waiting_reason"] = waiting_reason
    if last_outcome is not None:
        result["last_outcome"] = last_outcome
    if actions:
        result["actions"] = actions
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path, help="trusted Radar daemon socket")
    parser.add_argument("--owner", default="example-owner")
    parser.add_argument("--label", default="Herdsman owner projection")
    parser.add_argument("--session", help="current session UUID; omit to publish an explicit null")
    parser.add_argument("--once", action="store_true", help="publish one snapshot")
    parser.add_argument("--interval", type=float, default=10.0,
                        help="seconds between newer-sequence heartbeats")
    args = parser.parse_args()
    if args.interval <= 0:
        parser.error("--interval must be positive")

    # Immutable registration identity must be persisted by a real publisher if
    # it wants to reconnect as the same incarnation. This example keeps it
    # stable for one process lifetime; production extension state owns it.
    incarnation = str(uuid.uuid4())
    registration = {"source": "herdsman-owner-example", "incarnation": incarnation,
                    "owner": args.owner, "label": args.label}
    publisher = {"source": "herdsman-owner-example", "incarnation": incarnation,
                 "reporting_owner": args.owner}
    sequence = 0
    context_writer = None
    context_sequence = 0
    published_session = object()
    snapshot = owner_snapshot("waiting", "awaiting-child", actions=["assign", "cancel"])
    while True:
        attempted_sequence = sequence + 1
        try:
            registered, channel = publish_assignment(
                args.socket, registration, publisher, snapshot, attempted_sequence,
                lease_ms=DEFAULT_LEASE_MS)
            sequence = attempted_sequence
            current_session = args.session
            if current_session != published_session:
                context_writer, context = publish_context(
                    args.socket, registered["agent_id"], publisher, context_writer,
                    context_sequence + 1, current_session, lease_ms=DEFAULT_LEASE_MS)
                context_sequence += 1
                published_session = current_session
            print(json.dumps({"agent_id": registered["agent_id"],
                              "generation": channel["writer"]["generation"],
                              "sequence": sequence,
                              "freshness": channel.get("snapshot", {}).get("freshness"),
                              "context_session": context.get("context", {}).get("session")}),
                  flush=True)
        except RegistryError as error:
            # A successor has fenced this incarnation. Never silently replace it.
            if error.code == "refused" and ("writer" in error.message or
                                             "incumbent" in error.message):
                raise SystemExit(f"publisher fenced; stop and reconcile explicitly: {error}")
            raise
        except (OSError, TimeoutError, json.JSONDecodeError):
            # Reconnect by retrying identical immutable registration and
            # reacquiring the same current writer; no takeover is attempted.
            if args.once:
                raise
        if args.once:
            return 0
        time.sleep(args.interval)


if __name__ == "__main__":
    raise SystemExit(main())

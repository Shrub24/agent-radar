#!/usr/bin/env python3
"""Scripted Herdr CLI and socket server for the disposable spawn smoke."""
import json
import os
from pathlib import Path
import socket
import sys
import threading

state = Path(os.environ["RADAR_SPAWN_FAKE_STATE"])
sock_path = Path(os.environ.get("RADAR_HERDR_FAKE_SOCKET", state / "herdr.sock"))
if len(sys.argv) > 2 and sys.argv[1:3] == ["status", "--json"]:
    print(json.dumps({"server": {"socket": str(sock_path)}}))
    raise SystemExit(0)
elif sys.argv[1:3] == ["api", "snapshot"]:
    panes = [{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA"}]
    if (state / "created").exists():
        panes.append({"pane_id": "wA:p2", "tab_id": "wA:t1", "workspace_id": "wA"})
    print(json.dumps({"result": {"snapshot": {"workspaces": [], "tabs": [], "panes": panes, "agents": []}}}))
    raise SystemExit(0)
elif sys.argv[1:2] == ["__serve"]:
    pass
else:
    raise SystemExit(f"unexpected fake Herdr invocation: {sys.argv[1:]}")

def serve(connection):
    with connection:
        request = json.loads(connection.makefile("rb").readline())
        method, params = request["method"], request["params"]
        if method == "pane.split":
            (state / "created").write_text("yes")
            answer = {"result": {"pane": {"pane_id": "wA:p2"}}}
        elif method == "pane.send_input":
            line = params["text"]
            assert line.startswith("PI_RADAR_SPAWN_TOKEN="), line
            token = line.split("PI_RADAR_SPAWN_TOKEN=", 1)[1].split(" ", 1)[0].strip("'")
            (state / "launch.json").write_text(json.dumps({"line": line, "token": token,
                                                            "params": params}))
            answer = {"result": {"accepted": True}}
        else:
            answer = {"error": {"code": "fake_method", "message": method}}
        connection.sendall((json.dumps({"id": request["id"], **answer}) + "\n").encode())

listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
listener.bind(str(sock_path))
listener.listen(16)
while True:
    connection, _ = listener.accept()
    threading.Thread(target=serve, args=(connection,), daemon=True).start()

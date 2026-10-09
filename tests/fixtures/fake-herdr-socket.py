#!/usr/bin/env python3
"""Serve scripted Herdr API requests for the disposable spawn smoke."""
import json
from pathlib import Path
import socket
import sys

state = Path(sys.argv[1])
listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
listener.bind(str(state / "herdr.sock"))
listener.listen(16)
try:
    while True:
        conn, _ = listener.accept()
        with conn:
            request = json.loads(conn.makefile("rb").readline())
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
            conn.sendall((json.dumps({"id": request["id"], **answer}) + "\n").encode())
finally:
    listener.close()

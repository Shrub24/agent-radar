#!/bin/sh
# Executed in the pane created by the gated live child-close smoke.
set -eu

control_socket=$1
record_path=$2
pid=$$
boot_id=$(cat /proc/sys/kernel/random/boot_id)
stat=$(cat "/proc/$pid/stat")
rest=${stat##*) }
set -- $rest
# The remainder begins at stat field 3 (state); starttime is field 22.
start_ticks=${20}
token=${PI_RADAR_SPAWN_TOKEN:?missing Radar spawn token}

python3 - "$pid" "$boot_id" "$start_ticks" "$token" "$control_socket" "$record_path" <<'PY'
import json
import socket
import sys
import uuid

pid, boot_id, start_ticks, token, path, record_path = sys.argv[1:]
request = {
    "version": 1,
    "id": str(uuid.uuid4()),
    "method": "agent.register",
    "params": {
        "source": "radar-live-close-smoke",
        "incarnation": str(uuid.uuid4()),
        "process": {"pid": int(pid), "boot_id": boot_id, "start_ticks": int(start_ticks)},
        "spawn_token": token,
    },
}
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
    connection.connect(path)
    connection.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
    response = json.loads(connection.makefile("rb").readline())
if "error" in response:
    raise SystemExit(json.dumps(response["error"]))
registration = response["result"]["registration"]
with open(record_path, "w", encoding="utf-8") as record:
    json.dump({"agent_id": registration["agent_id"], "source": registration["source"],
               "incarnation": registration["incarnation"], "pid": int(pid),
               "boot_id": boot_id, "start_ticks": int(start_ticks)}, record)
PY

# Stay foregrounded so Herdr process-info sees this exact process.
exec sleep 600

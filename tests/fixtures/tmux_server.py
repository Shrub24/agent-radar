"""A disposable tmux server the daemon will serve.

The daemon serves only a server its own user unit started, so the helper starts
the server under the unit name the daemon checks rather than calling `tmux`
directly. `name` must be unique per run.
"""

import os
import subprocess
import time

WAIT = 5


def available():
    """Whether this machine can run the disposable server at all."""
    if subprocess.run(["tmux", "-V"], capture_output=True).returncode != 0:
        return False
    return subprocess.run(["systemctl", "--user", "show-environment"],
                          capture_output=True).returncode == 0


def start(name):
    """Starts `name` under its unit; reports whether it answered."""
    if not available():
        return False
    # Where the real template is installed a transient unit of the same name is
    # refused, so the template itself starts the server.
    if subprocess.run(["systemctl", "--user", "cat", "radar-tmux@.service"],
                      capture_output=True).returncode == 0:
        subprocess.run(["systemctl", "--user", "start", f"radar-tmux@{name}.service"],
                       capture_output=True)
    else:
        subprocess.run(["systemd-run", "--user", "--collect", "--quiet",
                        f"--unit=radar-tmux@{name}",
                        "--property=Type=simple", "--property=KillMode=control-group",
                        "tmux", "-D", "-L", name, "-f", "/dev/null"],
                       capture_output=True)
    deadline = time.monotonic() + WAIT
    while time.monotonic() < deadline:
        if subprocess.run(["tmux", "-L", name, "show-options", "-g"],
                          capture_output=True).returncode == 0:
            return True
        time.sleep(0.05)
    return False


def stop(name):
    """Removes the server and its unit, whatever state it is in."""
    subprocess.run(["systemctl", "--user", "stop", f"radar-tmux@{name}.service"],
                   capture_output=True)
    subprocess.run(["systemctl", "--user", "reset-failed",
                    f"radar-tmux@{name}.service"], capture_output=True)
    subprocess.run(["tmux", "-L", name, "kill-server"], capture_output=True)


def name(label):
    """A server name no other run of this script shares."""
    return f"radar-smoke-{os.getpid()}-{label}"

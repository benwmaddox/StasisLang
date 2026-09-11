"""Run a bounded Unix desktop host/guest package smoke without invite disclosure."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import urllib.request


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--result", required=True, type=Path)
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    result_path = args.result.resolve()
    result_path.parent.mkdir(parents=True, exist_ok=True)
    if result_path.parent == executable.parent:
        parser.error("result must be outside the executable directory to test asset discovery")
    environment = dict(os.environ)
    environment["STASIS_NETWORK_ADVERTISE_IPV4"] = "127.0.0.1"
    if sys.platform == "linux":
        environment.update(SDL_VIDEODRIVER="offscreen", SDL_AUDIODRIVER="dummy",
                           LIBGL_ALWAYS_SOFTWARE="1")
    process = subprocess.Popen([str(executable)], cwd=executable.parent.parent, env=environment,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    port = None
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"packaged host exited early: {process.returncode}")
            if sys.platform == "darwin":
                listeners = subprocess.run(
                    ["lsof", "-a", "-p", str(process.pid), "-iTCP", "-sTCP:LISTEN", "-Fn"],
                    capture_output=True, text=True, timeout=3)
                if listeners.returncode not in (0, 1):
                    raise RuntimeError("could not inspect packaged host listeners")
                for row in listeners.stdout.splitlines():
                    if row.startswith("n"):
                        port = int(row.rsplit(":", 1)[1])
            else:
                inodes = set()
                for descriptor in Path(f"/proc/{process.pid}/fd").iterdir():
                    try:
                        link = descriptor.readlink().as_posix()
                    except FileNotFoundError:
                        continue
                    if link.startswith("socket:["):
                        inodes.add(link[8:-1])
                for row in Path(f"/proc/{process.pid}/net/tcp").read_text().splitlines()[1:]:
                    fields = row.split()
                    if fields[3] == "0A" and fields[9] in inodes:
                        port = int(fields[1].split(":")[1], 16)
            if port is not None:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=3) as response:
                    assert response.status == 200
                    assert b"<!doctype html" in response.read().lower()
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("packaged host did not serve its browser guest within 20 seconds")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    if port is not None:
        with socket.socket() as probe:
            probe.settimeout(1)
            assert probe.connect_ex(("127.0.0.1", port)) != 0
    result = {"native_executable_started": True, "staged_browser_guest_http_200": True,
              "process_exit_released_listener": True, "unrelated_working_directory": True}
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()

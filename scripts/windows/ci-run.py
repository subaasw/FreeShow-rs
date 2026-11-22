"""Publish the tail of a failed native command to the public check."""
import argparse
import collections
import subprocess
import sys
import threading

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--timeout", type=float)
parser.add_argument("command", nargs=argparse.REMAINDER)
args = parser.parse_args()

tail = collections.deque(maxlen=200)
with subprocess.Popen(args.command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                      text=True, encoding="utf-8", errors="replace") as command:
    def read_output():
        for line in command.stdout:
            print(line, end="", flush=True)
            tail.append(line)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    try:
        status = command.wait(timeout=args.timeout)
    except subprocess.TimeoutExpired:
        print("::error::Native command timed out", flush=True)
        if sys.platform == "win32":
            subprocess.run(["taskkill", "/PID", str(command.pid), "/T", "/F"], check=False)
        else:
            command.kill()
        status = 124
    reader.join(timeout=5)
if status:
    report = "".join(tail)[-9000:]
    for start in range(0, len(report), 1800):
        chunk = report[start:start + 1800]
        print("::error::" + chunk.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A"))
    print(f"::error::Native command exited with status {status}")
sys.exit(status)

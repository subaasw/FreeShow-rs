#!/usr/bin/env bash
# Stream command output and attach its tail to the check when it fails.
set -uo pipefail

log=$(mktemp) || exit 1
trap 'rm -f -- "$log"' EXIT

if "$@" 2>&1 | tee "$log"; then
    exit 0
else
    status=$?
fi

python3 - "$log" <<'PY'
import pathlib
import sys

tail = pathlib.Path(sys.argv[1]).read_text(errors="replace")[-9000:]
for start in range(0, len(tail), 2000):
    chunk = tail[start:start+2000].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print("::error::" + chunk)
PY
echo "::error::Command exited with status $status"
exit "$status"

#!/usr/bin/env bash
# Stream command output and attach test failures and the tail to the check.
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
import re
import sys

output = pathlib.Path(sys.argv[1]).read_text(errors="replace")
failures = re.findall(r"^failures:\n\n---- .*?(?=^failures:|\Z)", output, re.M | re.S)
for report in ("\n".join(failures)[:5000], output[-9000:]):
    for start in range(0, len(report), 2000):
        chunk = report[start:start+2000].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print("::error::" + chunk)
PY
echo "::error::Command exited with status $status"
exit "$status"

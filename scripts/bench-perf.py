#!/usr/bin/env python3
"""Measure the plan §4 performance budgets with `freeshow --check-perf`.

Builds a release binary when needed, runs the headless checker, writes JSON,
and exits 1 if a budget is missed. The compositor paints 1920×1080 by default, or the active output's stored
bounds. Slide layout stays 1920×1080.
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, text=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/freeshow")
    parser.add_argument("--library", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "dist/perf.json")
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if not args.skip_build or not args.binary.is_file():
        run("cargo", "build", "--release", "--locked", "-p", "freeshow", cwd=ROOT)
    command = [args.binary, "--check-perf"]
    if args.library:
        command.append(args.library)
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    # The tool prints a JSON object; keep the last object if logs precede it.
    payload = result.stdout.strip()
    start = payload.rfind("{")
    if start < 0:
        sys.exit(result.returncode or 1)
    report = json.loads(payload[start:])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()

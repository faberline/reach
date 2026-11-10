#!/usr/bin/env python3
"""Explicit live test. Sends one AX notification only from a headless child to itself."""
import json
import pathlib
import subprocess
import sys

helper = pathlib.Path(__file__).resolve().parent / "target/debug/mac-focus-notifications"
try:
    result = subprocess.run([str(helper), "--test-own-notification"], capture_output=True, text=True, timeout=15)
    lines = [json.loads(line) for line in result.stdout.splitlines()]
except (OSError, ValueError, subprocess.TimeoutExpired) as error:
    print(json.dumps({"status": "test_incomplete", "reason": type(error).__name__, "action_allowed": False}))
    sys.exit(1)

ok = (result.returncode == 3 and len(lines) == 3 and lines[0].get("status") == "ready"
      and lines[0].get("scope") == "own_child_test"
      and lines[1].get("status") == "focus_changed" and lines[1].get("kind") == "focused_element"
      and lines[2].get("termination") == "focus_changed" and lines[2].get("test_child_stopped") is True
      and lines[2].get("counts", {}).get("focused_element") == 1
      and all(line.get("action_allowed") is False and line.get("input_posted") is False for line in lines))
print(json.dumps({"status": "test_passed" if ok else "test_incomplete", "action_allowed": False,
                  "exit_code": result.returncode, "records": lines}, sort_keys=True))
sys.exit(0 if ok else 1)

#!/bin/sh
set -eu
cd "$(dirname "$0")"
sh build-headless-event-fixture.sh
bin=target/debug/mac-headless-event-experiment
out=$(mktemp "${TMPDIR:-/private/tmp}/headless-args.XXXXXX")
trap 'rm -f "$out"' EXIT HUP INT TERM
for test_case in empty external_pid malformed_child; do
    set --
    case "$test_case" in
        external_pid) set -- --run-experiment --pid 42 ;;
        malformed_child) set -- --child 42 invalid ;;
    esac
    if "$bin" "$@" > /dev/null 2> "$out"; then
        echo "unexpected acceptance: $test_case" >&2
        exit 1
    fi
    python3 -c 'import json,sys; value=json.load(open(sys.argv[1])); assert value["status"] == "stopped" and value["general_input_authorized"] is False' "$out"
done
python3 - "$bin" <<'PY'
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import uuid

binary = str(pathlib.Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="headless-foreign-parent-") as tmp:
    # A full-looking child invocation must fail before any tap or permission check.
    nonce = str(uuid.uuid4()).upper()
    native_tmp = subprocess.check_output(["getconf", "DARWIN_USER_TEMP_DIR"], text=True).strip()
    directory = pathlib.Path(native_tmp, "headless-event-" + nonce)
    directory.mkdir(mode=0o700)
    try:
        handshake = {"purpose": "headless_event_experiment", "nonce": nonce, "marker": 99,
                     "parent_pid": os.getpid(), "parent_start_seconds": 1,
                     "parent_start_microseconds": 1, "parent_executable": sys.executable}
        (directory / "parent-handshake.json").write_text(json.dumps(handshake))
        env = os.environ.copy()
        env["TMPDIR"] = tmp  # Deliberately no trailing slash.
        env["HEADLESS_EVENT_PRIVATE_DIR"] = str(directory)
        result = subprocess.run([binary, "--child", str(os.getpid()), nonce, "99"],
                                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=5, check=False)
        assert result.returncode != 0, result
        body = json.loads(result.stderr)
        assert body["status"] == "stopped" and body["general_input_authorized"] is False, body
        assert body["reason"] == "parent and child executables differ", body
        assert sorted(p.name for p in directory.iterdir()) == ["parent-handshake.json"]
    finally:
        shutil.rmtree(directory)
print("argument and foreign-parent rejection tests passed; experiment not run")
PY

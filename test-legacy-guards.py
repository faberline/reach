#!/usr/bin/env python3
"""Check disabled fixture entrypoints without creating any GUI or input."""
import json
import pathlib
import subprocess

base = pathlib.Path(__file__).resolve().parent / 'target' / 'debug'
cases = {
    'mac-ax-press-experiment': ['--run-fixture'],
    'mac-target-event-experiment': [
        '--run-fixture', '--observe-fixture', '--observe-passive-fixture'
    ],
}
for name, args in cases.items():
    binary = base / name
    result = subprocess.run([str(binary), '--self-test'], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0 and json.loads(result.stdout)['live_ui_access'] is False
    for arg in args:
        result = subprocess.run([str(binary), arg], capture_output=True, text=True, timeout=10)
        report = json.loads(result.stderr)
        assert result.returncode != 0 and not result.stdout
        assert report['status'] == 'stopped' and report['general_input_authorized'] is False
        assert 'disabled' in report['reason']
        print(f'{name} {arg}: stopped')

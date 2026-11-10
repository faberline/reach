#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache CursorOverlay.swift -o target/debug/mac-cursor-overlay
target/debug/mac-cursor-overlay --self-test

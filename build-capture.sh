#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -parse-as-library -O -module-cache-path /private/tmp/sw-computer-use-swift-cache WindowCapture.swift -o target/debug/mac-window-capture
target/debug/mac-window-capture --self-test

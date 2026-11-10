#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -parse-as-library -module-cache-path /private/tmp/sw-computer-use-swift-cache FrameStatusProbe.swift -o target/debug/mac-frame-status

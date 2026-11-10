#!/bin/sh
set -eu
cd "$(dirname "$0")"
fixture=target/debug/AXPressFixture.app/Contents
mkdir -p "$fixture/MacOS" /private/tmp/sw-computer-use-swift-cache
cp fixtures/AXPressInfo.plist "$fixture/Info.plist"
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache fixtures/AXPressFixture.swift -o "$fixture/MacOS/AXPressFixture"
swiftc -parse-as-library -module-cache-path /private/tmp/sw-computer-use-swift-cache AXPressExperiment.swift -o target/debug/mac-ax-press-experiment
target/debug/mac-ax-press-experiment --self-test

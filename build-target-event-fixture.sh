#!/bin/sh
set -eu
cd "$(dirname "$0")"
fixture=target/debug/TargetEventFixture.app/Contents
mkdir -p "$fixture/MacOS" /private/tmp/sw-computer-use-swift-cache
cp fixtures/TargetEventInfo.plist "$fixture/Info.plist"
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache fixtures/TargetEventFixture.swift -o "$fixture/MacOS/TargetEventFixture"
swiftc -parse-as-library -module-cache-path /private/tmp/sw-computer-use-swift-cache TargetEventExperiment.swift -o target/debug/mac-target-event-experiment
target/debug/mac-target-event-experiment --self-test

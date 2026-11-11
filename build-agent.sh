#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -parse-as-library -O -module-cache-path /private/tmp/sw-computer-use-swift-cache ReachAgent.swift ReachAgentCapture.swift ReachAgentAccessibility.swift ReachAgentInput.swift ReachAgentInputSystem.swift -o target/debug/ReachAgent
target/debug/ReachAgent --self-test

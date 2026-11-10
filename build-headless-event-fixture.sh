#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -parse-as-library -module-cache-path /private/tmp/sw-computer-use-swift-cache HeadlessEventExperiment.swift -o target/debug/mac-headless-event-experiment
# This branch uses pure data only. It does not create a tap, process, window, or event.
target/debug/mac-headless-event-experiment --self-test

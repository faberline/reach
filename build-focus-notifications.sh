#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p target/debug /private/tmp/sw-computer-use-swift-cache
swiftc -module-cache-path /private/tmp/sw-computer-use-swift-cache FocusNotifications.swift -o target/debug/mac-focus-notifications

target/debug/mac-focus-notifications --self-test

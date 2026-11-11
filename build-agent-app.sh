#!/bin/sh
# Build ReachAgent.app and sign it.
#   sh build-agent-app.sh           build, sign with REACH_SIGN_IDENTITY, install to ~/Applications
#   sh build-agent-app.sh --adhoc   build and ad-hoc sign in a temporary directory, no install (CI)
# This script never creates a certificate and never changes trust settings or permissions.
set -eu
cd "$(dirname "$0")"

usage() {
    echo "usage: sh build-agent-app.sh [--adhoc]" >&2
    exit 2
}

mode=install
if [ "$#" -gt 1 ]; then
    usage
fi
if [ "$#" -eq 1 ]; then
    if [ "$1" = "--adhoc" ]; then
        mode=adhoc
    else
        usage
    fi
fi

if [ "$mode" = install ]; then
    if [ -z "${REACH_SIGN_IDENTITY-}" ]; then
        {
            echo "REACH_SIGN_IDENTITY is not set, so nothing was built or installed."
            echo "Permissions are tied to a stable signing identity, and an ad-hoc signature changes on every build."
            echo "Create a local code signing certificate once (see docs/BUILD.md), then run:"
            echo "  REACH_SIGN_IDENTITY=\"<certificate name>\" sh build-agent-app.sh"
            echo "To only check that the app builds, run: sh build-agent-app.sh --adhoc"
        } >&2
        exit 2
    fi
    identity=$REACH_SIGN_IDENTITY
    out=target/agent-app
else
    identity=-
    out=$(mktemp -d "${TMPDIR:-/tmp}/reach-agent-app.XXXXXX")
fi

app="$out/ReachAgent.app"
mkdir -p "$app/Contents/MacOS" /private/tmp/sw-computer-use-swift-cache
swiftc -parse-as-library -O -module-cache-path /private/tmp/sw-computer-use-swift-cache ReachAgent.swift ReachAgentCapture.swift ReachAgentAccessibility.swift ReachAgentInput.swift ReachAgentInputSystem.swift ReachAgentObservation.swift -o "$app/Contents/MacOS/ReachAgent"
cp ReachAgent-Info.plist "$app/Contents/Info.plist"
codesign --force --sign "$identity" --identifier dev.faberline.reach.agent "$app"
codesign --verify --strict "$app"
codesign -d -r- "$app" 2>&1
"$app/Contents/MacOS/ReachAgent" --self-test

if [ "$mode" = adhoc ]; then
    echo "ad-hoc build at $app (not installed)"
    exit 0
fi

mkdir -p "$HOME/Applications"
ditto "$app" "$HOME/Applications/ReachAgent.app"
codesign -d -r- "$HOME/Applications/ReachAgent.app" 2>&1
echo "installed $HOME/Applications/ReachAgent.app"
echo "Grant the permissions to ReachAgent yourself in System Settings; this script does not change them."

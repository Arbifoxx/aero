#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [[ "$(uname -s)" != Darwin ]]; then echo "error: aero-macos is supported on macOS only" >&2; exit 1; fi
echo "building on $(uname -m) macOS $(sw_vers -productVersion)"
cargo build --locked -p aero-macos
echo "built: target/debug/aero-macos"

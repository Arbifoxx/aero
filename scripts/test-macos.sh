#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [[ "$(uname -s)" != Darwin ]]; then echo "error: this test script requires macOS/Metal" >&2; exit 1; fi
cargo test --locked -p aero-machine
cargo test --locked -p aero-devices-gpu --features wgpu-backend --test aerogpu_native_smoke
cargo check --locked -p aero-macos
cargo run --locked -p aero-macos -- --list-gpu

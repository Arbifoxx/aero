#!/usr/bin/env bash
# Prepare the repository-local Rust dependency cache for the native macOS frontend.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v cargo >/dev/null || { echo "error: Rust/Cargo is required (install via rustup)" >&2; exit 1; }
command -v xcode-select >/dev/null || { echo "error: Xcode command-line tools are required" >&2; exit 1; }
xcode-select -p >/dev/null
echo "host: $(uname -m) $(sw_vers -productVersion)"
rustc --version
cargo --version
cargo fetch --locked
echo "bootstrap complete; run scripts/build-macos.sh"

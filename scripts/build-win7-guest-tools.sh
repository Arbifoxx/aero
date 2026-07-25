#!/usr/bin/env bash
# The WDDM driver requires the Windows WDK; deliberately do not emulate a successful build on macOS.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [[ "${OS:-}" != "Windows_NT" ]]; then
  echo "error: AeroGPU Win7 binaries must be built on Windows 10/11 x64 with WDK 10 + MSBuild." >&2
  echo "See drivers/aerogpu/README.md and docs/WINDOWS7_GUEST_SETUP.md." >&2
  exit 2
fi
pwsh -NoProfile -ExecutionPolicy Bypass -File ci/install-wdk.ps1
pwsh -NoProfile -ExecutionPolicy Bypass -File ci/build-drivers.ps1 -ToolchainJson out/toolchain.json -Drivers aerogpu

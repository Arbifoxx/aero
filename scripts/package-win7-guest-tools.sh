#!/usr/bin/env bash
# Package already-built Windows outputs; signing/certificate handling remains an explicit Windows action.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [[ "${OS:-}" != "Windows_NT" ]]; then
  echo "error: Win7 package catalog generation/signing requires Windows WDK tools (Inf2Cat, signtool)." >&2
  exit 2
fi
[[ -f out/toolchain.json ]] || { echo "error: missing out/toolchain.json; run build-win7-guest-tools.sh first" >&2; exit 1; }
pwsh -NoProfile -ExecutionPolicy Bypass -File ci/make-catalogs.ps1 -ToolchainJson out/toolchain.json
pwsh -NoProfile -ExecutionPolicy Bypass -File ci/sign-drivers.ps1 -ToolchainJson out/toolchain.json
pwsh -NoProfile -ExecutionPolicy Bypass -File ci/package-guest-tools.ps1 -InputRoot out/packages -CertPath out/certs/aero-test.cer -OutDir out/artifacts -SpecPath tools/packaging/specs/win7-signed.json

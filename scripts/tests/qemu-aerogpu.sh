#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
QEMU_BIN="${AERO_QEMU_BIN:-$(bash "$REPO_ROOT/scripts/build-qemu-aerogpu.sh" print-bin)}"

[[ -x "$QEMU_BIN" ]] || {
  echo "error: QEMU binary is not runnable: $QEMU_BIN" >&2
  exit 1
}

"$QEMU_BIN" -device help | grep 'name "aerogpu"' >/dev/null

QMP_OUTPUT="$(
  "$QEMU_BIN" \
    -machine q35 \
    -display none \
    -nodefaults \
    -device aerogpu \
    -S \
    -qmp stdio <<'EOF'
{"execute":"qmp_capabilities"}
{"execute":"query-pci"}
{"execute":"quit"}
EOF
)"

grep -F '"vendor": 41888' <<<"$QMP_OUTPUT" >/dev/null
grep -F '"device": 1' <<<"$QMP_OUTPUT" >/dev/null
grep -F '"size": 65536' <<<"$QMP_OUTPUT" >/dev/null
grep -F '"size": 67108864' <<<"$QMP_OUTPUT" >/dev/null

echo "QEMU AeroGPU PCI smoke test passed"

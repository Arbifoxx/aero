#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aero-vm-test.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT

export AERO_VM_HOME="$TEST_ROOT/vms"
export AERO_MACOS_BIN=/usr/bin/true
export AERO_MACHINE_BIN=/usr/bin/true

ISO="$TEST_ROOT/install.iso"
touch "$ISO"

"$REPO_ROOT/scripts/aero-vm.sh" create test-vm \
  --ram 1024 \
  --cpus 1 \
  --disk-size 1G \
  --iso "$ISO" \
  --acceleration on

[[ -f "$AERO_VM_HOME/test-vm/disk.raw" ]]
[[ "$(<"$AERO_VM_HOME/test-vm/ram_mib")" == 1024 ]]
[[ "$(<"$AERO_VM_HOME/test-vm/cpus")" == 1 ]]
[[ "$(<"$AERO_VM_HOME/test-vm/acceleration")" == on ]]

"$REPO_ROOT/scripts/aero-vm.sh" show test-vm >/dev/null
"$REPO_ROOT/scripts/aero-vm.sh" list | grep -q test-vm
"$REPO_ROOT/scripts/aero-vm.sh" set test-vm --ram 2048 --acceleration off >/dev/null
[[ "$(<"$AERO_VM_HOME/test-vm/ram_mib")" == 2048 ]]
[[ "$(<"$AERO_VM_HOME/test-vm/acceleration")" == off ]]

GUI_COMMAND="$("$REPO_ROOT/scripts/aero-vm.sh" start test-vm --install --dry-run 2>/dev/null)"
[[ "$GUI_COMMAND" == *"--memory 2048"* ]]
[[ "$GUI_COMMAND" == *"--cpus 1"* ]]
[[ "$GUI_COMMAND" == *"--boot cd-first"* ]]
[[ "$GUI_COMMAND" == *"--no-aerogpu"* ]]

HEADLESS_COMMAND="$("$REPO_ROOT/scripts/aero-vm.sh" start test-vm --install --headless --dry-run 2>/dev/null)"
[[ "$HEADLESS_COMMAND" == *"--ram 2048"* ]]
[[ "$HEADLESS_COMMAND" == *"--max-ms 120000"* ]]

"$REPO_ROOT/scripts/aero-vm.sh" trash test-vm --yes >/dev/null
[[ ! -d "$AERO_VM_HOME/test-vm" ]]
find "$AERO_VM_HOME/.trash" -mindepth 1 -maxdepth 1 -type d | grep -q .

echo "aero-vm manager tests passed"

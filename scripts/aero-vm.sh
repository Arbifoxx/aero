#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
VM_ROOT="${AERO_VM_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/aero/vms}"

die() {
  echo "error: $*" >&2
  exit 1
}

warn() {
  echo "warning: $*" >&2
}

usage() {
  cat <<'EOF'
Manage local Aero macOS virtual machines.

Usage:
  scripts/aero-vm.sh create NAME [--ram MiB] [--cpus N] [--disk-size SIZE]
                                  [--iso PATH] [--acceleration on|off]
  scripts/aero-vm.sh list
  scripts/aero-vm.sh show NAME
  scripts/aero-vm.sh set NAME [--ram MiB] [--cpus N] [--iso PATH|none]
                               [--acceleration on|off]
  scripts/aero-vm.sh start NAME [--install] [--headless] [--max-ms MS]
                                [--trace] [--dry-run] [-- EXTRA_ARGS...]
  scripts/aero-vm.sh doctor
  scripts/aero-vm.sh trash NAME [--yes]

Environment:
  AERO_VM_HOME       VM storage root (default: $XDG_DATA_HOME/aero/vms or
                     $HOME/.local/share/aero/vms)
  AERO_MACOS_BIN     Override the aero-macos executable
  AERO_MACHINE_BIN   Override the headless aero-machine executable

Notes:
  - Windows 7 reaches its file-loading screen; a complete install is not validated.
  - One vCPU is the supported bring-up setting. More than one enables incomplete SMP.
  - VM disks and Windows media remain outside the repository and are never bundled.
EOF
}

validate_name() {
  local name="$1"
  [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] ||
    die "invalid VM name '$name' (use 1-64 letters, numbers, dots, underscores, or dashes)"
}

validate_uint() {
  local label="$1"
  local value="$2"
  [[ "$value" =~ ^[0-9]+$ ]] || die "$label must be an integer"
}

validate_ram() {
  local value="$1"
  validate_uint "RAM MiB" "$value"
  local numeric=$((10#$value))
  ((numeric >= 256 && numeric <= 16384)) ||
    die "RAM must be between 256 and 16384 MiB"
}

validate_cpus() {
  local value="$1"
  validate_uint "vCPU count" "$value"
  local numeric=$((10#$value))
  ((numeric >= 1 && numeric <= 64)) || die "vCPU count must be between 1 and 64"
}

validate_acceleration() {
  case "$1" in
    on | off) ;;
    *) die "acceleration must be 'on' or 'off'" ;;
  esac
}

validate_disk_size() {
  local value
  value="$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')"
  [[ "$value" =~ ^[1-9][0-9]*[MGT]$ ]] ||
    die "disk size must look like 40960M, 40G, or 1T"
}

absolute_existing_file() {
  local path="$1"
  [[ -f "$path" ]] || die "file does not exist: $path"
  local dir
  dir="$(cd "$(dirname "$path")" && pwd -P)"
  printf '%s/%s\n' "$dir" "$(basename "$path")"
}

vm_dir() {
  local name="$1"
  validate_name "$name"
  printf '%s/%s\n' "$VM_ROOT" "$name"
}

require_vm() {
  local dir
  dir="$(vm_dir "$1")"
  [[ -d "$dir" && -f "$dir/version" ]] || die "VM does not exist: $1"
  printf '%s\n' "$dir"
}

read_setting() {
  local dir="$1"
  local key="$2"
  [[ -f "$dir/$key" ]] || die "VM metadata is missing '$key': $dir"
  IFS= read -r REPLY <"$dir/$key" || true
  printf '%s\n' "$REPLY"
}

write_setting() {
  local dir="$1"
  local key="$2"
  local value="$3"
  printf '%s\n' "$value" >"$dir/$key"
}

create_sparse_disk() {
  local path="$1"
  local size
  size="$(printf '%s' "$2" | tr '[:lower:]' '[:upper:]')"
  if command -v mkfile >/dev/null 2>&1; then
    mkfile -n "$size" "$path"
    return
  fi
  if command -v truncate >/dev/null 2>&1; then
    truncate -s "$size" "$path"
    return
  fi
  die "neither mkfile nor truncate is available to create a sparse disk"
}

find_binary() {
  local override="$1"
  local release_path="$2"
  local debug_path="$3"
  if [[ -n "$override" ]]; then
    [[ -x "$override" ]] || die "configured executable is not runnable: $override"
    printf '%s\n' "$override"
  elif [[ -x "$release_path" ]]; then
    printf '%s\n' "$release_path"
  elif [[ -x "$debug_path" ]]; then
    printf '%s\n' "$debug_path"
  else
    die "Aero executable is not built; run scripts/build-macos.sh first"
  fi
}

print_command() {
  printf 'command:'
  printf ' %q' "$@"
  printf '\n'
}

cmd_create() {
  (($# >= 1)) || die "create requires a VM name"
  local name="$1"
  shift
  validate_name "$name"

  local ram=2048
  local cpus=1
  local disk_size=40G
  local iso=""
  local acceleration=on
  while (($#)); do
    case "$1" in
      --ram)
        (($# >= 2)) || die "--ram requires MiB"
        ram="$2"
        shift 2
        ;;
      --cpus)
        (($# >= 2)) || die "--cpus requires a count"
        cpus="$2"
        shift 2
        ;;
      --disk-size)
        (($# >= 2)) || die "--disk-size requires a size"
        disk_size="$2"
        shift 2
        ;;
      --iso)
        (($# >= 2)) || die "--iso requires a path"
        iso="$(absolute_existing_file "$2")"
        shift 2
        ;;
      --acceleration)
        (($# >= 2)) || die "--acceleration requires on or off"
        acceleration="$2"
        shift 2
        ;;
      *) die "unknown create option: $1" ;;
    esac
  done

  validate_ram "$ram"
  validate_cpus "$cpus"
  validate_disk_size "$disk_size"
  validate_acceleration "$acceleration"
  ((10#$cpus == 1)) || warn "SMP is incomplete; Windows boot testing should use one vCPU"

  mkdir -p "$VM_ROOT"
  local final_dir
  final_dir="$(vm_dir "$name")"
  [[ ! -e "$final_dir" ]] || die "VM already exists: $name"

  local temp_dir
  temp_dir="$(mktemp -d "$VM_ROOT/.create-${name}.XXXXXX")"
  local cleanup_dir="$temp_dir"
  trap 'if [[ -n "${cleanup_dir:-}" && -d "$cleanup_dir" ]]; then rm -rf -- "$cleanup_dir"; fi' EXIT

  create_sparse_disk "$temp_dir/disk.raw" "$disk_size"
  write_setting "$temp_dir" version 1
  write_setting "$temp_dir" ram_mib "$ram"
  write_setting "$temp_dir" cpus "$cpus"
  write_setting "$temp_dir" acceleration "$acceleration"
  write_setting "$temp_dir" install_iso "$iso"
  write_setting "$temp_dir" disk_size "$disk_size"
  mv "$temp_dir" "$final_dir"
  cleanup_dir=""
  trap - EXIT

  echo "created VM '$name'"
  echo "  directory: $final_dir"
  echo "  disk: $final_dir/disk.raw ($disk_size sparse)"
  echo "  RAM/vCPUs: ${ram} MiB / $cpus"
  if [[ -n "$iso" ]]; then
    echo "  install media: $iso"
    echo "next: scripts/aero-vm.sh start '$name' --install"
  else
    echo "next: attach an ISO with scripts/aero-vm.sh set '$name' --iso /path/to/windows.iso"
  fi
}

cmd_list() {
  mkdir -p "$VM_ROOT"
  printf '%-24s %-10s %-6s %-13s %s\n' "NAME" "RAM(MiB)" "vCPUs" "ACCELERATION" "INSTALL ISO"
  local found=0
  local dir
  shopt -s nullglob
  for dir in "$VM_ROOT"/*; do
    [[ -d "$dir" && -f "$dir/version" ]] || continue
    found=1
    printf '%-24s %-10s %-6s %-13s %s\n' \
      "$(basename "$dir")" \
      "$(read_setting "$dir" ram_mib)" \
      "$(read_setting "$dir" cpus)" \
      "$(read_setting "$dir" acceleration)" \
      "$(read_setting "$dir" install_iso)"
  done
  shopt -u nullglob
  ((found == 1)) || echo "(no VMs under $VM_ROOT)"
}

cmd_show() {
  (($# == 1)) || die "show requires exactly one VM name"
  local dir
  dir="$(require_vm "$1")"
  local disk="$dir/disk.raw"
  echo "name: $(basename "$dir")"
  echo "directory: $dir"
  echo "RAM MiB: $(read_setting "$dir" ram_mib)"
  echo "vCPUs: $(read_setting "$dir" cpus)"
  echo "acceleration: $(read_setting "$dir" acceleration)"
  echo "configured disk size: $(read_setting "$dir" disk_size)"
  echo "disk: $disk"
  if [[ -f "$disk" ]]; then
    echo "disk bytes: $(stat -f '%z' "$disk" 2>/dev/null || stat -c '%s' "$disk")"
    echo "disk allocated: $(du -h "$disk" | awk '{print $1}')"
  fi
  echo "install ISO: $(read_setting "$dir" install_iso)"
}

cmd_set() {
  (($# >= 1)) || die "set requires a VM name"
  local name="$1"
  shift
  local dir
  dir="$(require_vm "$name")"
  (($# > 0)) || die "set requires at least one option"

  while (($#)); do
    case "$1" in
      --ram)
        (($# >= 2)) || die "--ram requires MiB"
        validate_ram "$2"
        write_setting "$dir" ram_mib "$2"
        shift 2
        ;;
      --cpus)
        (($# >= 2)) || die "--cpus requires a count"
        validate_cpus "$2"
        ((10#$2 == 1)) || warn "SMP is incomplete; Windows boot testing should use one vCPU"
        write_setting "$dir" cpus "$2"
        shift 2
        ;;
      --iso)
        (($# >= 2)) || die "--iso requires a path or 'none'"
        if [[ "$2" == none ]]; then
          write_setting "$dir" install_iso ""
        else
          write_setting "$dir" install_iso "$(absolute_existing_file "$2")"
        fi
        shift 2
        ;;
      --acceleration)
        (($# >= 2)) || die "--acceleration requires on or off"
        validate_acceleration "$2"
        write_setting "$dir" acceleration "$2"
        shift 2
        ;;
      *) die "unknown set option: $1" ;;
    esac
  done
  cmd_show "$name"
}

cmd_start() {
  (($# >= 1)) || die "start requires a VM name"
  local name="$1"
  shift
  local dir
  dir="$(require_vm "$name")"

  local install=0
  local headless=0
  local trace=0
  local dry_run=0
  local max_ms=""
  local -a extra=()
  while (($#)); do
    case "$1" in
      --install)
        install=1
        shift
        ;;
      --headless)
        headless=1
        shift
        ;;
      --trace)
        trace=1
        shift
        ;;
      --dry-run)
        dry_run=1
        shift
        ;;
      --max-ms)
        (($# >= 2)) || die "--max-ms requires milliseconds"
        validate_uint "maximum runtime" "$2"
        max_ms="$2"
        shift 2
        ;;
      --)
        shift
        extra=("$@")
        break
        ;;
      *) die "unknown start option: $1 (put frontend-specific options after --)" ;;
    esac
  done

  local ram cpus acceleration iso disk
  ram="$(read_setting "$dir" ram_mib)"
  cpus="$(read_setting "$dir" cpus)"
  acceleration="$(read_setting "$dir" acceleration)"
  iso="$(read_setting "$dir" install_iso)"
  disk="$dir/disk.raw"
  [[ -f "$disk" ]] || die "VM disk is missing: $disk"
  validate_ram "$ram"
  validate_cpus "$cpus"
  validate_acceleration "$acceleration"
  ((10#$cpus == 1)) || warn "this VM uses incomplete SMP; use 'set $name --cpus 1' for boot work"
  if ((install)); then
    [[ -n "$iso" && -f "$iso" ]] || die "--install requires an attached ISO"
  fi

  warn "Windows 7 reaches its file-loading screen, but a complete install is not validated"

  local -a command
  if ((headless)); then
    local binary
    binary="$(find_binary "${AERO_MACHINE_BIN:-}" \
      "$REPO_ROOT/target/release/aero-machine" \
      "$REPO_ROOT/target/debug/aero-machine")"
    [[ -n "$max_ms" ]] || max_ms=120000
    command=("$binary" --disk "$disk" --ram "$ram" --cpus "$cpus" --max-ms "$max_ms")
    if ((install)); then
      command+=(--install-iso "$iso" --boot cd-first)
    else
      command+=(--boot hdd)
    fi
    if ((trace)); then
      command+=(--debugcon-out stdout)
    fi
  else
    [[ "$(uname -s)" == Darwin ]] || die "the native Metal frontend requires macOS"
    local binary
    binary="$(find_binary "${AERO_MACOS_BIN:-}" \
      "$REPO_ROOT/target/release/aero-macos" \
      "$REPO_ROOT/target/debug/aero-macos")"
    command=("$binary" --disk "$disk" --memory "$ram" --cpus "$cpus")
    if ((install)); then
      command+=(--install-iso "$iso" --boot cd-first)
    else
      command+=(--boot hdd)
    fi
    if [[ "$acceleration" == on ]]; then
      command+=(--aerogpu-wgpu)
    else
      command+=(--no-aerogpu)
    fi
    [[ -z "$max_ms" ]] || command+=(--max-ms "$max_ms")
    if ((trace)); then
      command+=(--trace-pci --trace-scanout --log-level debug)
    fi
  fi
  command+=("${extra[@]}")
  print_command "${command[@]}"
  ((dry_run)) && return 0
  exec "${command[@]}"
}

cmd_doctor() {
  echo "repository: $REPO_ROOT"
  echo "VM root: $VM_ROOT"
  echo "host: $(uname -s) $(uname -m)"
  if [[ "$(uname -s)" != Darwin ]]; then
    warn "native Metal VM execution requires macOS"
    return 1
  fi
  local binary
  binary="$(find_binary "${AERO_MACOS_BIN:-}" \
    "$REPO_ROOT/target/release/aero-macos" \
    "$REPO_ROOT/target/debug/aero-macos")"
  echo "frontend: $binary"
  "$binary" --list-gpu
}

cmd_trash() {
  (($# >= 1)) || die "trash requires a VM name"
  local name="$1"
  shift
  local yes=0
  if (($#)); then
    [[ "$1" == --yes && $# == 1 ]] || die "trash accepts only --yes"
    yes=1
  fi
  local dir
  dir="$(require_vm "$name")"
  if ((yes == 0)); then
    printf "Move VM '%s' to Aero's recoverable trash? [y/N] " "$name" >&2
    local answer
    IFS= read -r answer
    [[ "$answer" == y || "$answer" == Y || "$answer" == yes ]] || {
      echo "cancelled"
      return 0
    }
  fi
  local trash_root="$VM_ROOT/.trash"
  mkdir -p "$trash_root"
  local destination="$trash_root/${name}-$(date +%Y%m%d-%H%M%S)"
  [[ ! -e "$destination" ]] || die "trash destination already exists: $destination"
  mv "$dir" "$destination"
  echo "moved VM to recoverable trash: $destination"
}

main() {
  (($# > 0)) || {
    usage
    exit 2
  }
  local command="$1"
  shift
  case "$command" in
    create) cmd_create "$@" ;;
    list) cmd_list "$@" ;;
    show) cmd_show "$@" ;;
    set) cmd_set "$@" ;;
    start) cmd_start "$@" ;;
    doctor) cmd_doctor "$@" ;;
    trash) cmd_trash "$@" ;;
    help | --help | -h) usage ;;
    *) die "unknown command: $command (try --help)" ;;
  esac
}

main "$@"

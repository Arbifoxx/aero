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
                                  [--disk-format raw|qcow2] [--iso PATH]
                                  [--backend qemu|aero] [--acceleration on|off]
                                  [--renderer native|noop]
  scripts/aero-vm.sh list
  scripts/aero-vm.sh show NAME
  scripts/aero-vm.sh set NAME [--ram MiB] [--cpus N] [--iso PATH|none]
                               [--backend qemu|aero] [--acceleration on|off]
                               [--renderer native|noop]
  scripts/aero-vm.sh resize NAME --disk-size SIZE
  scripts/aero-vm.sh start NAME [--install] [--headless] [--max-ms MS]
                                [--trace] [--dry-run] [-- EXTRA_ARGS...]
  scripts/aero-vm.sh doctor
  scripts/aero-vm.sh menu
  scripts/aero-vm.sh trash NAME [--yes]

Environment:
  AERO_VM_HOME       VM storage root (default: $XDG_DATA_HOME/aero/vms or
                     $HOME/.local/share/aero/vms)
  AERO_MACOS_BIN     Override the aero-macos executable
  AERO_MACHINE_BIN   Override the headless aero-machine executable
  AERO_QEMU_BIN      Override the AeroGPU-enabled qemu-system-x86_64
  AERO_QEMU_BRIDGE   Override libaero_qemu_bridge.dylib

Notes:
  - QEMU is the recommended native backend. The native renderer connects AeroGPU
    command rings to Aero's experimental wgpu/Metal D3D9 executor.
  - The Aero backend remains available for device-model bring-up.
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

validate_renderer() {
  case "$1" in
    native | noop) ;;
    *) die "renderer must be 'native' or 'noop'" ;;
  esac
}

validate_backend() {
  case "$1" in
    qemu | aero) ;;
    *) die "backend must be 'qemu' or 'aero'" ;;
  esac
}

validate_disk_format() {
  case "$1" in
    raw | qcow2) ;;
    *) die "disk format must be 'raw' or 'qcow2'" ;;
  esac
}

validate_disk_size() {
  local value
  value="$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')"
  [[ "$value" =~ ^[1-9][0-9]*[MGT]$ ]] ||
    die "disk size must look like 40960M, 40G, or 1T"
}

size_to_bytes() {
  local value
  value="$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')"
  validate_disk_size "$value"
  local number="${value%?}"
  case "${value: -1}" in
    M) printf '%s\n' "$((10#$number * 1024 * 1024))" ;;
    G) printf '%s\n' "$((10#$number * 1024 * 1024 * 1024))" ;;
    T) printf '%s\n' "$((10#$number * 1024 * 1024 * 1024 * 1024))" ;;
  esac
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

read_setting_default() {
  local dir="$1"
  local key="$2"
  local default="$3"
  if [[ -f "$dir/$key" ]]; then
    read_setting "$dir" "$key"
  else
    printf '%s\n' "$default"
  fi
}

write_setting() {
  local dir="$1"
  local key="$2"
  local value="$3"
  printf '%s\n' "$value" >"$dir/$key"
}

create_disk() {
  local path="$1"
  local size format
  size="$(printf '%s' "$2" | tr '[:lower:]' '[:upper:]')"
  format="$3"
  if [[ "$format" == qcow2 ]]; then
    command -v qemu-img >/dev/null 2>&1 ||
      die "qemu-img is required to create qcow2 disks"
    qemu-img create -q -f qcow2 "$path" "$size"
    return
  fi
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

vm_disk_path() {
  local dir="$1"
  local format
  format="$(read_setting_default "$dir" disk_format raw)"
  case "$format" in
    raw) printf '%s/disk.raw\n' "$dir" ;;
    qcow2) printf '%s/disk.qcow2\n' "$dir" ;;
    *) die "unsupported disk format in VM metadata: $format" ;;
  esac
}

find_qemu_binary() {
  if [[ -n "${AERO_QEMU_BIN:-}" ]]; then
    [[ -x "$AERO_QEMU_BIN" ]] || die "AERO_QEMU_BIN is not runnable: $AERO_QEMU_BIN"
    printf '%s\n' "$AERO_QEMU_BIN"
    return
  fi
  local tag
  tag="$(sed -n '1p' "$REPO_ROOT/qemu/supported-version")"
  local bundled="$REPO_ROOT/target/qemu-aerogpu/$tag/install/bin/qemu-system-x86_64"
  if [[ -x "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return
  fi
  die "AeroGPU QEMU is not built; run scripts/build-qemu-aerogpu.sh build"
}

find_qemu_bridge() {
  if [[ -n "${AERO_QEMU_BRIDGE:-}" ]]; then
    [[ -f "$AERO_QEMU_BRIDGE" ]] ||
      die "AERO_QEMU_BRIDGE does not exist: $AERO_QEMU_BRIDGE"
    printf '%s\n' "$AERO_QEMU_BRIDGE"
    return
  fi
  local tag
  tag="$(sed -n '1p' "$REPO_ROOT/qemu/supported-version")"
  local bundled="$REPO_ROOT/target/qemu-aerogpu/$tag/install/lib/libaero_qemu_bridge.dylib"
  if [[ -f "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return
  fi
  die "AeroGPU QEMU bridge is not built; run scripts/build-qemu-aerogpu.sh build"
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
  local disk_format=auto
  local iso=""
  local backend=qemu
  local acceleration=on
  local renderer=native
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
      --disk-format)
        (($# >= 2)) || die "--disk-format requires raw or qcow2"
        disk_format="$2"
        shift 2
        ;;
      --iso)
        (($# >= 2)) || die "--iso requires a path"
        iso="$(absolute_existing_file "$2")"
        shift 2
        ;;
      --backend)
        (($# >= 2)) || die "--backend requires qemu or aero"
        backend="$2"
        shift 2
        ;;
      --acceleration)
        (($# >= 2)) || die "--acceleration requires on or off"
        acceleration="$2"
        shift 2
        ;;
      --renderer)
        (($# >= 2)) || die "--renderer requires native or noop"
        renderer="$2"
        shift 2
        ;;
      *) die "unknown create option: $1" ;;
    esac
  done

  validate_ram "$ram"
  validate_cpus "$cpus"
  validate_disk_size "$disk_size"
  validate_backend "$backend"
  if [[ "$disk_format" == auto ]]; then
    [[ "$backend" == qemu ]] && disk_format=qcow2 || disk_format=raw
  fi
  validate_disk_format "$disk_format"
  [[ "$backend" == qemu || "$disk_format" == raw ]] ||
    die "the Aero backend currently requires a raw disk"
  validate_acceleration "$acceleration"
  validate_renderer "$renderer"
  if [[ "$backend" == aero ]] && ((10#$cpus != 1)); then
    warn "the Aero backend's SMP support is incomplete; QEMU is recommended for multiple vCPUs"
  fi

  mkdir -p "$VM_ROOT"
  local final_dir
  final_dir="$(vm_dir "$name")"
  [[ ! -e "$final_dir" ]] || die "VM already exists: $name"

  local temp_dir
  temp_dir="$(mktemp -d "$VM_ROOT/.create-${name}.XXXXXX")"
  local cleanup_dir="$temp_dir"
  trap 'if [[ -n "${cleanup_dir:-}" && -d "$cleanup_dir" ]]; then rm -rf -- "$cleanup_dir"; fi' EXIT

  local disk_name="disk.$disk_format"
  create_disk "$temp_dir/$disk_name" "$disk_size" "$disk_format"
  write_setting "$temp_dir" version 2
  write_setting "$temp_dir" ram_mib "$ram"
  write_setting "$temp_dir" cpus "$cpus"
  write_setting "$temp_dir" backend "$backend"
  write_setting "$temp_dir" acceleration "$acceleration"
  write_setting "$temp_dir" renderer "$renderer"
  write_setting "$temp_dir" install_iso "$iso"
  write_setting "$temp_dir" disk_size "$disk_size"
  write_setting "$temp_dir" disk_format "$disk_format"
  mv "$temp_dir" "$final_dir"
  cleanup_dir=""
  trap - EXIT

  echo "created VM '$name'"
  echo "  directory: $final_dir"
  echo "  backend: $backend"
  echo "  AeroGPU: $acceleration ($renderer renderer)"
  echo "  disk: $final_dir/$disk_name ($disk_size $disk_format)"
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
  printf '%-24s %-8s %-10s %-6s %-8s %-13s %s\n' \
    "NAME" "BACKEND" "RAM(MiB)" "vCPUs" "DISK" "AEROGPU" "INSTALL ISO"
  local found=0
  local dir
  shopt -s nullglob
  for dir in "$VM_ROOT"/*; do
    [[ -d "$dir" && -f "$dir/version" ]] || continue
    found=1
    printf '%-24s %-8s %-10s %-6s %-8s %-13s %s\n' \
      "$(basename "$dir")" \
      "$(read_setting_default "$dir" backend aero)" \
      "$(read_setting "$dir" ram_mib)" \
      "$(read_setting "$dir" cpus)" \
      "$(read_setting_default "$dir" disk_format raw)" \
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
  local disk
  disk="$(vm_disk_path "$dir")"
  echo "name: $(basename "$dir")"
  echo "directory: $dir"
  echo "backend: $(read_setting_default "$dir" backend aero)"
  echo "RAM MiB: $(read_setting "$dir" ram_mib)"
  echo "vCPUs: $(read_setting "$dir" cpus)"
  echo "acceleration: $(read_setting "$dir" acceleration)"
  echo "AeroGPU renderer: $(read_setting_default "$dir" renderer native)"
  echo "configured disk size: $(read_setting "$dir" disk_size)"
  echo "disk format: $(read_setting_default "$dir" disk_format raw)"
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
        write_setting "$dir" cpus "$2"
        shift 2
        ;;
      --backend)
        (($# >= 2)) || die "--backend requires qemu or aero"
        validate_backend "$2"
        if [[ "$2" == aero && "$(read_setting_default "$dir" disk_format raw)" != raw ]]; then
          die "the Aero backend currently requires a raw disk"
        fi
        write_setting "$dir" backend "$2"
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
      --renderer)
        (($# >= 2)) || die "--renderer requires native or noop"
        validate_renderer "$2"
        write_setting "$dir" renderer "$2"
        shift 2
        ;;
      *) die "unknown set option: $1" ;;
    esac
  done
  cmd_show "$name"
}

cmd_resize() {
  (($# == 3)) || die "resize usage: resize NAME --disk-size SIZE"
  local name="$1"
  [[ "$2" == --disk-size ]] || die "resize accepts only --disk-size"
  local requested="$3"
  validate_disk_size "$requested"
  local dir
  dir="$(require_vm "$name")"
  local current
  current="$(read_setting "$dir" disk_size)"
  local requested_bytes current_bytes
  requested_bytes="$(size_to_bytes "$requested")"
  current_bytes="$(size_to_bytes "$current")"
  ((requested_bytes > current_bytes)) ||
    die "disk resize must grow the disk (current: $current, requested: $requested)"
  command -v qemu-img >/dev/null 2>&1 || die "qemu-img is required to resize disks"
  local format disk
  format="$(read_setting_default "$dir" disk_format raw)"
  disk="$(vm_disk_path "$dir")"
  [[ -f "$disk" ]] || die "VM disk is missing: $disk"
  qemu-img resize -f "$format" "$disk" "$requested"
  write_setting "$dir" disk_size "$(printf '%s' "$requested" | tr '[:lower:]' '[:upper:]')"
  echo "resized '$name' disk from $current to $requested"
  echo "note: grow the Windows partition inside the guest to use the new space"
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

  local ram cpus backend acceleration renderer iso disk disk_format
  ram="$(read_setting "$dir" ram_mib)"
  cpus="$(read_setting "$dir" cpus)"
  backend="$(read_setting_default "$dir" backend aero)"
  acceleration="$(read_setting "$dir" acceleration)"
  renderer="$(read_setting_default "$dir" renderer native)"
  iso="$(read_setting "$dir" install_iso)"
  disk_format="$(read_setting_default "$dir" disk_format raw)"
  disk="$(vm_disk_path "$dir")"
  [[ -f "$disk" ]] || die "VM disk is missing: $disk"
  validate_ram "$ram"
  validate_cpus "$cpus"
  validate_backend "$backend"
  validate_acceleration "$acceleration"
  validate_renderer "$renderer"
  if [[ "$backend" == aero ]] && ((10#$cpus != 1)); then
    warn "the Aero backend uses incomplete SMP; switch to QEMU or use one vCPU"
  fi
  if ((install)); then
    [[ -n "$iso" && -f "$iso" ]] || die "--install requires an attached ISO"
  fi

  local -a command
  if [[ "$backend" == qemu ]]; then
    local binary
    binary="$(find_qemu_binary)"
    command=(
      "$binary"
      -name "$name"
      -machine q35
      -accel tcg,thread=multi
      -cpu Nehalem
      -smp "$cpus"
      -m "$ram"
      -drive "file=$disk,if=ide,format=$disk_format,cache=writeback"
      -vga std
      -device e1000,netdev=net0
      -netdev user,id=net0
    )
    if [[ "$acceleration" == on ]]; then
      local bridge
      bridge="$(find_qemu_bridge)"
      command+=(-device "aerogpu,bridge-path=$bridge,renderer=$renderer")
      if [[ "$renderer" == native ]]; then
        warn "AeroGPU native rendering is experimental; keep the standard VGA adapter enabled as a recovery display"
      else
        warn "AeroGPU noop mode validates the driver/ring/fence path without rendering"
      fi
    fi
    if ((install)); then
      command+=(-cdrom "$iso" -boot order=d,menu=on)
    else
      command+=(-boot order=c,menu=on)
    fi
    if ((headless)); then
      command+=(-display none -serial stdio)
    else
      [[ "$(uname -s)" == Darwin ]] || die "the Cocoa display requires macOS"
      command+=(-display cocoa)
    fi
    if [[ -n "$max_ms" ]]; then
      warn "--max-ms is not implemented for the QEMU backend and will be ignored"
    fi
    if ((trace)); then
      command+=(-d guest_errors,unimp)
    fi
  elif ((headless)); then
    warn "Windows 7 reaches its file-loading screen, but a complete Aero-backend install is not validated"
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
    warn "Windows 7 reaches its file-loading screen, but a complete Aero-backend install is not validated"
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
    warn "the supported native host is macOS"
    return 1
  fi
  echo
  echo "QEMU integration:"
  if bash "$REPO_ROOT/scripts/build-qemu-aerogpu.sh" status; then
    :
  else
    warn "build it with: bash scripts/build-qemu-aerogpu.sh build"
  fi
  echo
  echo "Aero frontend:"
  local release="$REPO_ROOT/target/release/aero-macos"
  local debug="$REPO_ROOT/target/debug/aero-macos"
  if [[ -n "${AERO_MACOS_BIN:-}" && -x "$AERO_MACOS_BIN" ]]; then
    echo "frontend: $AERO_MACOS_BIN"
    "$AERO_MACOS_BIN" --list-gpu
  elif [[ -x "$release" ]]; then
    echo "frontend: $release"
    "$release" --list-gpu
  elif [[ -x "$debug" ]]; then
    echo "frontend: $debug"
    "$debug" --list-gpu
  else
    echo "frontend: not built (optional when using QEMU)"
  fi
}

prompt_default() {
  local prompt="$1"
  local default="$2"
  local answer
  if [[ -n "$default" ]]; then
    printf '%s [%s]: ' "$prompt" "$default" >&2
  else
    printf '%s: ' "$prompt" >&2
  fi
  IFS= read -r answer || return 1
  printf '%s\n' "${answer:-$default}"
}

menu_create() {
  local name backend ram cpus disk_size disk_format iso renderer
  name="$(prompt_default "VM name" "")" || return
  backend="$(prompt_default "Backend (qemu/aero)" qemu)" || return
  ram="$(prompt_default "RAM in MiB" 4096)" || return
  cpus="$(prompt_default "vCPU count" 2)" || return
  disk_size="$(prompt_default "Disk size (for example 40G)" 40G)" || return
  if [[ "$backend" == qemu ]]; then
    disk_format="$(prompt_default "Disk format (qcow2/raw)" qcow2)" || return
  else
    disk_format=raw
  fi
  iso="$(prompt_default "Windows ISO path (blank to attach later)" "")" || return
  renderer="$(prompt_default "AeroGPU renderer (native/noop)" native)" || return
  local -a args=(
    "$name" --backend "$backend" --ram "$ram" --cpus "$cpus"
    --disk-size "$disk_size" --disk-format "$disk_format"
    --renderer "$renderer"
  )
  [[ -z "$iso" ]] || args+=(--iso "$iso")
  cmd_create "${args[@]}"
}

menu_start() {
  local install="$1"
  local name
  name="$(prompt_default "VM name" "")" || return
  if [[ "$install" == 1 ]]; then
    cmd_start "$name" --install
  else
    cmd_start "$name"
  fi
}

menu_settings() {
  local name dir value
  name="$(prompt_default "VM name" "")" || return
  dir="$(require_vm "$name")"
  echo "Press Enter to keep the current value."
  value="$(prompt_default "Backend" "$(read_setting_default "$dir" backend aero)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --backend "$value" >/dev/null
  value="$(prompt_default "RAM MiB" "$(read_setting "$dir" ram_mib)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --ram "$value" >/dev/null
  value="$(prompt_default "vCPUs" "$(read_setting "$dir" cpus)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --cpus "$value" >/dev/null
  value="$(prompt_default "AeroGPU (on/off)" "$(read_setting "$dir" acceleration)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --acceleration "$value" >/dev/null
  value="$(prompt_default "AeroGPU renderer (native/noop)" "$(read_setting_default "$dir" renderer native)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --renderer "$value" >/dev/null
  value="$(prompt_default "ISO path, 'none' to detach" "$(read_setting "$dir" install_iso)")" || return
  [[ -z "$value" ]] || cmd_set "$name" --iso "$value" >/dev/null
  cmd_show "$name"
}

cmd_menu() {
  (($# == 0)) || die "menu accepts no arguments"
  while true; do
    echo
    echo "Aero VM Manager"
    echo "  1) List VMs"
    echo "  2) Create VM"
    echo "  3) Start VM"
    echo "  4) Start Windows installer"
    echo "  5) Change VM settings"
    echo "  6) Grow VM disk"
    echo "  7) Show VM details"
    echo "  8) Check host / QEMU"
    echo "  9) Build AeroGPU QEMU"
    echo "  t) Move VM to recoverable trash"
    echo "  q) Quit"
    local choice name size
    printf "Choice: " >&2
    IFS= read -r choice || return 0
    case "$choice" in
      1) cmd_list ;;
      2) menu_create ;;
      3) menu_start 0 ;;
      4) menu_start 1 ;;
      5) menu_settings ;;
      6)
        name="$(prompt_default "VM name" "")" || continue
        size="$(prompt_default "New disk size" "")" || continue
        cmd_resize "$name" --disk-size "$size"
        ;;
      7)
        name="$(prompt_default "VM name" "")" || continue
        cmd_show "$name"
        ;;
      8) cmd_doctor || true ;;
      9) bash "$REPO_ROOT/scripts/build-qemu-aerogpu.sh" build ;;
      t | T)
        name="$(prompt_default "VM name" "")" || continue
        cmd_trash "$name"
        ;;
      q | Q) return 0 ;;
      *) warn "choose one of the displayed options" ;;
    esac
  done
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
    resize) cmd_resize "$@" ;;
    start) cmd_start "$@" ;;
    doctor) cmd_doctor "$@" ;;
    menu) cmd_menu "$@" ;;
    trash) cmd_trash "$@" ;;
    help | --help | -h) usage ;;
    *) die "unknown command: $command (try --help)" ;;
  esac
}

main "$@"

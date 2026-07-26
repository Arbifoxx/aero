#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
VERSION_FILE="$REPO_ROOT/qemu/supported-version"

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Build the pinned QEMU fork containing the AeroGPU PCI device.

Usage:
  scripts/build-qemu-aerogpu.sh build
  scripts/build-qemu-aerogpu.sh status
  scripts/build-qemu-aerogpu.sh print-bin
  scripts/build-qemu-aerogpu.sh print-bridge

Environment:
  AERO_QEMU_SOURCE   QEMU checkout (default: $XDG_CACHE_HOME/aero/qemu-TAG)
  AERO_QEMU_BUILD    Build directory (default: repository target/qemu-aerogpu/TAG/build)
  AERO_QEMU_PREFIX   Install directory (default: repository target/qemu-aerogpu/TAG/install)
  AERO_QEMU_JOBS     Ninja parallelism (default: host CPU count, capped at 8)
  AERO_QEMU_RENDERER Build the native Metal renderer (default: 1; set 0 for protocol-only)
EOF
}

[[ -f "$VERSION_FILE" ]] || die "missing $VERSION_FILE"
QEMU_TAG="$(sed -n '1p' "$VERSION_FILE")"
QEMU_COMMIT="$(sed -n '2p' "$VERSION_FILE")"
[[ "$QEMU_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "invalid QEMU tag in $VERSION_FILE"
[[ "$QEMU_COMMIT" =~ ^[0-9a-f]{40}$ ]] || die "invalid QEMU commit in $VERSION_FILE"

CACHE_BASE="${XDG_CACHE_HOME:-$HOME/.cache}"
SOURCE_DIR="${AERO_QEMU_SOURCE:-$CACHE_BASE/aero/qemu-$QEMU_TAG}"
BUILD_ROOT="${AERO_QEMU_BUILD:-$REPO_ROOT/target/qemu-aerogpu/$QEMU_TAG/build}"
INSTALL_ROOT="${AERO_QEMU_PREFIX:-$REPO_ROOT/target/qemu-aerogpu/$QEMU_TAG/install}"
PATCH_DIR="$REPO_ROOT/qemu/patches/$QEMU_TAG"
QEMU_BIN="$INSTALL_ROOT/bin/qemu-system-x86_64"
BRIDGE_TARGET="$REPO_ROOT/target/qemu-aerogpu/bridge"
BRIDGE_BUILD="$BRIDGE_TARGET/release/libaero_qemu_bridge.dylib"
BRIDGE_LIB="$INSTALL_ROOT/lib/libaero_qemu_bridge.dylib"

host_jobs() {
  local jobs=2
  if command -v sysctl >/dev/null 2>&1; then
    jobs="$(sysctl -n hw.logicalcpu 2>/dev/null || echo 2)"
  elif command -v getconf >/dev/null 2>&1; then
    jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)"
  fi
  [[ "$jobs" =~ ^[1-9][0-9]*$ ]] || jobs=2
  ((jobs > 8)) && jobs=8
  printf '%s\n' "$jobs"
}

ensure_source() {
  if [[ ! -e "$SOURCE_DIR" ]]; then
    mkdir -p "$(dirname "$SOURCE_DIR")"
    git clone --depth 1 --branch "$QEMU_TAG" --single-branch \
      https://gitlab.com/qemu-project/qemu.git "$SOURCE_DIR"
  fi
  [[ -d "$SOURCE_DIR/.git" ]] || die "QEMU source is not a git checkout: $SOURCE_DIR"

  local actual
  actual="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
  [[ "$actual" == "$QEMU_COMMIT" ]] ||
    die "QEMU checkout is at $actual; expected $QEMU_COMMIT ($QEMU_TAG)"
}

apply_patches() {
  [[ -d "$PATCH_DIR" ]] || die "missing patch directory: $PATCH_DIR"
  local patch
  shopt -s nullglob
  local patches=("$PATCH_DIR"/*.patch)
  shopt -u nullglob
  ((${#patches[@]} > 0)) || die "no patches found under $PATCH_DIR"

  for patch in "${patches[@]}"; do
    if git -C "$SOURCE_DIR" apply --check "$patch" >/dev/null 2>&1; then
      git -C "$SOURCE_DIR" apply "$patch"
      echo "applied: $(basename "$patch")"
    elif git -C "$SOURCE_DIR" apply --reverse --check "$patch" >/dev/null 2>&1; then
      echo "already applied: $(basename "$patch")"
    else
      die "patch neither applies nor appears applied: $patch"
    fi
  done
}

configure_and_build() {
  [[ "$(uname -s)" == Darwin ]] || die "this initial build script targets macOS"
  [[ "$(uname -m)" == arm64 ]] ||
    echo "warning: the supported host is Apple Silicon arm64" >&2
  command -v git >/dev/null || die "git is required"
  command -v ninja >/dev/null || die "ninja is required (install with: brew install ninja)"

  local -a bridge_features=()
  if [[ "${AERO_QEMU_RENDERER:-1}" == 1 ]]; then
    bridge_features=(--features native-renderer)
  elif [[ "${AERO_QEMU_RENDERER:-1}" != 0 ]]; then
    die "AERO_QEMU_RENDERER must be 0 or 1"
  fi
  if [[ "$(uname -s)" == Darwin ]]; then
    # Apple's linker can emit a malformed LINKEDIT string pool when Rust's
    # release dead stripping is used for this large wgpu cdylib. Keeping the
    # bridge's exported/dependency code produces a valid, loadable dylib.
    CARGO_TARGET_DIR="$BRIDGE_TARGET" cargo rustc --release --locked \
      -p aero-qemu-bridge "${bridge_features[@]}" -- -C link-dead-code=yes
    xcrun dyld_info -validate_only "$BRIDGE_BUILD" >/dev/null
  else
    CARGO_TARGET_DIR="$BRIDGE_TARGET" cargo build --release --locked \
      -p aero-qemu-bridge "${bridge_features[@]}"
  fi
  [[ -f "$BRIDGE_BUILD" ]] || die "bridge build completed without $BRIDGE_BUILD"
  mkdir -p "$INSTALL_ROOT/lib"
  install -m 755 "$BRIDGE_BUILD" "$BRIDGE_LIB"

  mkdir -p "$BUILD_ROOT" "$INSTALL_ROOT"
  local -a configure_args=(
    --target-list=x86_64-softmmu
    "--prefix=$INSTALL_ROOT"
    "--extra-cflags=-I$REPO_ROOT/qemu/include"
    --enable-cocoa
    --disable-docs
    --disable-werror
  )
  if command -v brew >/dev/null 2>&1; then
    local brew_prefix
    brew_prefix="$(brew --prefix)"
    configure_args+=("--extra-ldflags=-Wl,-rpath,$brew_prefix/lib")
  fi
  (
    cd "$BUILD_ROOT"
    "$SOURCE_DIR/configure" "${configure_args[@]}"
  )

  local jobs="${AERO_QEMU_JOBS:-$(host_jobs)}"
  [[ "$jobs" =~ ^[1-9][0-9]*$ ]] || die "AERO_QEMU_JOBS must be a positive integer"
  ninja -C "$BUILD_ROOT" -j "$jobs"
  ninja -C "$BUILD_ROOT" install

  [[ -x "$QEMU_BIN" ]] || die "build completed without $QEMU_BIN"
  if command -v brew >/dev/null 2>&1; then
    local runtime_lib
    runtime_lib="$(brew --prefix)/lib"
    if ! otool -l "$QEMU_BIN" | grep -F "path $runtime_lib " >/dev/null; then
      install_name_tool -add_rpath "$runtime_lib" "$QEMU_BIN"
      codesign --force --sign - "$QEMU_BIN" >/dev/null
    fi
  fi
  "$QEMU_BIN" -device help | grep 'name "aerogpu"' >/dev/null ||
    die "built QEMU does not contain the AeroGPU device"
  [[ -f "$BRIDGE_LIB" ]] || die "installed AeroGPU bridge is missing"
  "$QEMU_BIN" --version | head -1
  echo "AeroGPU QEMU: $QEMU_BIN"
  echo "AeroGPU bridge: $BRIDGE_LIB"
}

cmd_status() {
  echo "tag: $QEMU_TAG"
  echo "commit: $QEMU_COMMIT"
  echo "source: $SOURCE_DIR"
  echo "build: $BUILD_ROOT"
  echo "install: $INSTALL_ROOT"
  echo "bridge: $BRIDGE_LIB"
  if [[ -x "$QEMU_BIN" ]]; then
    "$QEMU_BIN" --version | head -1
    if "$QEMU_BIN" -device help | grep 'name "aerogpu"' >/dev/null; then
      echo "AeroGPU device: available"
    else
      echo "AeroGPU device: missing"
      return 1
    fi
  else
    echo "AeroGPU QEMU: not built"
  fi
  if [[ -f "$BRIDGE_LIB" ]]; then
    echo "AeroGPU bridge: available"
  else
    echo "AeroGPU bridge: missing"
  fi
}

case "${1:-build}" in
  build)
    (($# == 1 || $# == 0)) || die "build accepts no arguments"
    ensure_source
    apply_patches
    configure_and_build
    ;;
  status)
    (($# == 1)) || die "status accepts no arguments"
    cmd_status
    ;;
  print-bin)
    (($# == 1)) || die "print-bin accepts no arguments"
    [[ -x "$QEMU_BIN" ]] || die "AeroGPU QEMU is not built; run $0 build"
    printf '%s\n' "$QEMU_BIN"
    ;;
  print-bridge)
    (($# == 1)) || die "print-bridge accepts no arguments"
    [[ -f "$BRIDGE_LIB" ]] || die "AeroGPU bridge is not built; run $0 build"
    printf '%s\n' "$BRIDGE_LIB"
    ;;
  help | --help | -h)
    usage
    ;;
  *)
    die "unknown command: ${1:-}"
    ;;
esac

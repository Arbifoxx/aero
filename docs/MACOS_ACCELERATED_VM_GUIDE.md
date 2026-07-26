# Windows 7 with AeroGPU on Apple Silicon

The recommended native architecture is QEMU for the x86 PC and CPU, plus the
AeroGPU PCI device and Rust-to-Metal bridge maintained in this repository.
This avoids the browser/Wasm runtime while retaining Aero's Windows graphics
protocol and renderer.

For implementation details and the latest validation boundary, see
`qemu/README.md`.

## What acceleration means

There are two independent performance paths:

1. QEMU TCG translates the Windows 7 guest's x86/x86-64 CPU instructions to
   ARM64. It supports multiple vCPUs but is software CPU emulation, not
   Hypervisor.framework acceleration.
2. After the Windows AeroGPU driver loads, supported D3D commands travel through
   the `A3A0:0001` PCI device to Aero's Rust renderer and wgpu/Metal. This
   accelerates guest graphics; it does not accelerate guest CPU execution.

Standard VGA is deliberately retained as a boot and recovery adapter. AeroGPU
has a separate QEMU display console.

## Build and verify the host

```bash
bash scripts/build-qemu-aerogpu.sh build
bash scripts/build-qemu-aerogpu.sh status
bash scripts/tests/qemu-aerogpu.sh
bash scripts/aero-vm.sh doctor
```

The generated QEMU binary and bridge are kept beneath
`target/qemu-aerogpu/`. The exact upstream QEMU version is pinned in
`qemu/supported-version`.

## Create a Windows 7 VM

Windows 7 SP1 x64, 4-8 GiB RAM, 2-6 vCPUs, and a 40 GiB qcow2 disk are
reasonable starting values on a modern Apple Silicon Mac:

```bash
bash scripts/aero-vm.sh create win7 \
  --backend qemu \
  --ram 8192 \
  --cpus 5 \
  --disk-size 40G \
  --disk-format qcow2 \
  --iso /path/to/windows-7-sp1-x64.iso \
  --acceleration on \
  --renderer native

bash scripts/aero-vm.sh start win7 --install
```

After installation, omit `--install` so the hard disk is selected:

```bash
bash scripts/aero-vm.sh start win7
```

The interactive alternative is:

```bash
bash scripts/aero-vm.sh menu
```

VMs are stored outside the repository under
`${AERO_VM_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/aero/vms}`. Media,
product keys, VM disks, certificates, generated drivers, and memory dumps must
not be committed.

## VM management

```bash
bash scripts/aero-vm.sh list
bash scripts/aero-vm.sh show win7
bash scripts/aero-vm.sh set win7 --ram 6144 --cpus 4
bash scripts/aero-vm.sh set win7 --renderer noop
bash scripts/aero-vm.sh resize win7 --disk-size 60G
bash scripts/aero-vm.sh start win7 --dry-run
```

Disk resizing changes only virtual disk capacity; it does not resize the
Windows partition or filesystem. Shut down and back up the VM before resizing
or installing experimental display drivers. `trash` is recoverable and moves
the VM under the VM root's `.trash` directory.

Renderer modes:

- `native` runs supported guest GPU commands on Metal.
- `noop` completes the protocol path without drawing.
- `--acceleration off` removes AeroGPU and leaves standard VGA.

If a driver install breaks the display:

```bash
bash scripts/aero-vm.sh set win7 --acceleration off
bash scripts/aero-vm.sh start win7
```

## Install and validate the Windows driver

Build the WDDM package on a Windows 10/11 x64 machine with WDK 10 and MSBuild;
macOS cannot produce the driver binaries:

```powershell
pwsh ci/install-wdk.ps1
pwsh ci/build-drivers.ps1 -ToolchainJson out/toolchain.json -Drivers aerogpu
pwsh ci/build-aerogpu-dbgctl.ps1 -ToolchainJson out/toolchain.json
pwsh ci/make-catalogs.ps1 -ToolchainJson out/toolchain.json
pwsh ci/sign-drivers.ps1 -ToolchainJson out/toolchain.json
pwsh ci/package-drivers.ps1
```

Transfer `out/packages/aerogpu/x64/` and `out/certs/aero-test.cer` to the
test VM. Then follow `docs/WINDOWS7_GUEST_SETUP.md`, including test-signing and
certificate steps.

Validate incrementally:

1. Confirm hardware ID `PCI\VEN_A3A0&DEV_0001`.
2. Install the signed x64 package and reboot.
3. Confirm Device Manager reports no Code 43/52.
4. Run `aerogpu_dbgctl.exe --status` and check rings/fences/errors.
5. Run `d3d9ex_triangle.exe`; retain its log and bitmap.
6. Switch to the AeroGPU graphical console with QEMU's View menu or
   `Ctrl+Alt+2`.
7. Test DWM/Aero Glass only after the triangle passes.

## Current boundary

As of 2026-07-26:

- Windows 7 installation and boot succeed under the managed QEMU VM.
- The QEMU PCI device, Rust bridge, DMA, ring, fence, IRQ, native Metal
  initialization, and second scanout console are implemented and host-tested.
- The Windows AeroGPU driver has not yet been validated end-to-end in this
  QEMU path.
- D3D9/D3D9Ex is the initial native renderer target. D3D10/11 is incomplete.
- AeroGPU migration and snapshots are disabled.

Do not use boot success, window smoothness, or Metal adapter logs as proof of
guest graphics acceleration. Require a loaded guest driver, advancing fences,
no protocol error, and a passing D3D guest test.

## Legacy in-process frontend

`crates/aero-macos` and `aero-machine` remain useful for deterministic device
and CPU bring-up, but they are not the recommended Windows VM runtime. Their
interpreter timing, one-vCPU constraints, and browser-derived machine structure
do not apply to the QEMU backend described above.

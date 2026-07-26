# AeroGPU on QEMU for Apple Silicon

This integration uses stock QEMU's x86_64 TCG engine and PC platform, with one
project-specific PCI display device. QEMU owns CPUs, RAM, storage, firmware,
PCI, and guest DMA. A versioned dynamic-library bridge connects that PCI device
to Aero's Rust AeroGPU protocol implementation and native wgpu/Metal renderer.

This is the recommended macOS path. It does not use the browser/Wasm machine
and does not require a separate x86 JIT.

## Current status

As of 2026-07-26:

- Windows 7 installation and normal boot work under QEMU TCG on Apple Silicon.
- QEMU exposes canonical AeroGPU hardware ID `PCI\VEN_A3A0&DEV_0001`.
- BAR0 command rings, guest-memory DMA, fences, vblank, and interrupts are
  connected to the Rust device core.
- `renderer=native` initializes Aero's D3D9 command executor on Metal.
- QEMU exposes the AeroGPU scanout as a second graphical console while keeping
  standard VGA available for boot and recovery.
- The host bridge and PCI path pass automated smoke tests, including native
  Metal initialization.
- Loading the Windows driver and rendering `d3d9ex_triangle` in this QEMU path
  remain the next guest-side validation milestone.

Do not call the VM accelerated merely because it boots or because the native
renderer initializes. Acceleration is established only after the Windows
AeroGPU driver loads and a guest D3D test completes through the command ring.

## Build

From the repository root:

```bash
bash scripts/build-qemu-aerogpu.sh build
bash scripts/build-qemu-aerogpu.sh status
bash scripts/tests/qemu-aerogpu.sh
```

The build script checks out the exact QEMU tag and commit recorded in
`qemu/supported-version`, applies the matching patch, builds the Rust bridge,
and installs both under `target/qemu-aerogpu/`.

The default bridge contains the Metal renderer. For protocol-only diagnostics:

```bash
AERO_QEMU_RENDERER=0 bash scripts/build-qemu-aerogpu.sh build
```

## Create and manage VMs

Launch the interactive menu:

```bash
bash scripts/aero-vm.sh menu
```

Or use the CLI:

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
bash scripts/aero-vm.sh start win7
```

Use `--install` only while booting installation media. A normal start boots the
installed disk. `show`, `set`, `resize`, `list`, and recoverable `trash`
operations are available; run `scripts/aero-vm.sh help` for their arguments.

Useful renderer settings:

- `native`: execute supported AeroGPU commands through wgpu/Metal.
- `noop`: exercise PCI discovery, rings, fences, DMA, and interrupts without
  rendering. This is useful when separating driver/protocol bugs from renderer
  bugs.
- `--acceleration off`: omit the AeroGPU PCI device entirely.

## Displays and recovery

Standard VGA remains the boot display. AeroGPU is a second display console that
becomes meaningful after the Windows driver configures scanout. In QEMU's Cocoa
window, use the View menu or `Ctrl+Alt+1` / `Ctrl+Alt+2` to switch between
graphical consoles.

Keep standard VGA enabled until the AeroGPU driver is proven stable. Before
installing an experimental display driver, shut down the VM and make a copy of
its disk or VM directory. If the guest becomes unusable, turn acceleration off:

```bash
bash scripts/aero-vm.sh set win7 --acceleration off
bash scripts/aero-vm.sh start win7
```

## Guest driver milestone

The macOS host cannot build Windows WDK drivers. Build and sign the package on a
Windows 10/11 x64 build host, then transfer the generated x64 package and test
certificate to the test VM. Follow:

- `docs/WINDOWS7_GUEST_SETUP.md`
- `drivers/aerogpu/packaging/win7/README.md`
- `drivers/aerogpu/tests/win7/README.md`

Validate in this order:

1. Device Manager shows `PCI\VEN_A3A0&DEV_0001` with no error.
2. `aerogpu_dbgctl.exe --status` reports valid protocol and ring state.
3. Fences advance and no AeroGPU error IRQ is reported.
4. `d3d9ex_triangle.exe` reports `PASS`.
5. The AeroGPU QEMU console displays the expected frame.
6. Only then test DWM/Aero Glass and broader applications.

## Known limitations

- x86/x86-64 guest code still runs under QEMU TCG. Metal accelerates supported
  GPU commands, not the guest CPU.
- The native renderer is initially focused on D3D9/D3D9Ex; D3D10/11 coverage
  is incomplete.
- Only 32-bit BGRA/RGBA scanout formats are presently displayed by QEMU.
- AeroGPU device migration and snapshots are disabled because Rust renderer
  state is not serializable yet.
- Guest driver loading, first triangle, DWM, and Aero Glass are not yet
  validated end-to-end on this path.

QEMU is GPL-licensed. Distributing a combined build requires compliance with
QEMU's license and corresponding-source obligations.

# AeroGPU on QEMU: Native macOS ARM64 Integration Plan

Status: proposed  
Target host: Apple Silicon macOS  
Target guest: Windows 7 SP1 x64  
Initial QEMU base: `v11.0.3` (latest stable release as of 2026-07-25)

## 1. Goal

Run Windows 7 under QEMU's existing x86_64 TCG CPU emulation while presenting the
existing AeroGPU PCI device to the guest. AeroGPU command streams should execute through
the existing Rust `wgpu` backend on Metal.

The finished data path is:

```text
Windows 7 application
  -> D3D9Ex/D3D11 AeroGPU user-mode driver
  -> AeroGPU WDDM kernel-mode driver
  -> AeroGPU PCI BARs, DMA ring, fences, and INTx
  -> QEMU AeroGPU PCI device
  -> stable C ABI
  -> AeroGpuBar0MmioDevice
  -> aero-devices-gpu command executor
  -> NativeAeroGpuBackend
  -> wgpu
  -> Metal
```

QEMU remains responsible for the PC platform, x86-to-ARM64 translation, firmware,
storage, networking, timers, PCI routing, and the VM lifecycle. The Aero project remains
responsible for the guest driver, AeroGPU protocol, device semantics, and graphics
translation.

## 2. Decisions

### 2.1 Use a small QEMU fork

Build a normal QEMU binary with an in-tree QOM/qdev PCI device. Do not fork QEMU's CPU
emulator or machine model.

An unmodified QEMU binary cannot instantiate a device it does not contain. QEMU's plugin
API is not a general PCI-device API, and the locally installed macOS QEMU build does not
include the `vfio-user-pci` client. A compiled-in device is therefore the shortest,
most testable path.

Keep the QEMU changes as a small branch based on `v11.0.3`. The Aero repository should
record the supported QEMU tag or commit and provide build automation, but should not
vendor the full QEMU source tree.

### 2.2 Reuse the transport-independent Rust device core

Use `aero_devices_gpu::device::AeroGpuBar0MmioDevice` as the source of truth for:

- BAR0 register behavior
- ring validation and submission decoding
- DMA and fence semantics
- interrupt status, enable, acknowledge, and level calculation
- scanout, cursor, and vblank state
- dispatch to `AeroGpuCommandBackend`

This type is already the canonical register, vblank, and ring-executor state machine without
a PCI configuration-space wrapper. QEMU should own the one real PCI configuration and BAR
mappings. The existing `AeroGpuPciDevice` remains the Aero machine's PCI frontend and a
behavioral test oracle.

Do not port these behaviors into a second C implementation. The QEMU C device should own
only QEMU-specific integration.

### 2.3 Add a narrow, versioned C ABI

Add an `aerogpu-qemu-ffi` Rust crate that builds a native static library and C header.
The ABI should expose an opaque device handle and explicit functions for:

- create, reset, and destroy
- BAR0 read and write
- BAR1 read and write, or access to its authoritative backing memory
- service pending ring work
- advance the virtual vblank clock
- query the current interrupt level
- query scanout and cursor state
- copy the current scanout to a host buffer
- save and load device state

The Rust side receives a host callback table for guest DMA reads/writes, logging, and
work notification. Every exported function must:

- validate pointers, lengths, and ABI versions
- prevent Rust unwinding from crossing the C boundary
- return explicit status codes
- document thread affinity and ownership
- avoid retaining QEMU-owned pointers beyond their documented lifetime

### 2.4 Keep all QEMU APIs on QEMU threads

The initial bring-up may execute submissions synchronously for simplicity. Before running
general workloads, move Metal execution to a dedicated Rust worker.

The worker must not call QEMU APIs directly. It should publish completions to a
thread-safe queue and notify a QEMU bottom half or event source. The QEMU thread then
updates fences, raises or lowers INTx, and refreshes the display.

### 2.5 Preserve the current guest ABI

The QEMU device must initially expose exactly:

| Property | Value |
| --- | --- |
| Vendor/device ID | `A3A0:0001` |
| Subsystem ID | `A3A0:0001` |
| PCI class | `03:00:00`, VGA-compatible display controller |
| BAR0 | 64 KiB MMIO register window |
| BAR1 | 64 MiB prefetchable VRAM aperture |
| VBE LFB offset in BAR1 | `0x40000` |
| MMIO magic | `AGPU` |
| Protocol ABI | 1.4 |
| Initial interrupt mechanism | legacy level-triggered INTx |

The PCI slot is assigned by QEMU. The driver must bind by hardware ID and must not depend
on Aero's current `00:07.0` placement.

### 2.6 Bring AeroGPU up as a secondary adapter first

Use QEMU standard VGA for firmware, installation, Safe Mode, and recovery during early
development. Add AeroGPU as a second PCI display adapter:

```text
-vga std -device aerogpu
```

This separates ordinary Windows/QEMU boot problems from AeroGPU driver problems. Once
the WDDM driver, scanout, and mode setting are stable, test AeroGPU as the primary
VGA-compatible adapter and remove standard VGA if Windows' display-stack behavior
requires it.

### 2.7 Use QEMU virtual time

Drive vblank with `QEMU_CLOCK_VIRTUAL`, normally at 60 Hz. Pausing the VM must pause
vblank, and snapshots must not capture host wall-clock deadlines.

## 3. Component Boundaries

### QEMU fork

Expected files:

```text
hw/display/aerogpu-pci.c
hw/display/Kconfig
hw/display/meson.build
include/hw/display/aerogpu-pci.h
tests/qtest/aerogpu-test.c
```

The QEMU device owns:

- QOM/QDev lifecycle and properties
- `PCIDevice` configuration
- BAR registration with `MemoryRegion`
- PCI DMA through `pci_dma_read()` and `pci_dma_write()`
- INTx delivery through `pci_set_irq()`
- virtual timers and bottom halves
- `QemuConsole`/`GraphicHwOps` display integration
- `VMState` integration or an explicit snapshot blocker

### Aero repository

Expected additions:

```text
crates/aerogpu-qemu-ffi/
qemu/README.md
qemu/supported-version
scripts/build-qemu-aerogpu.sh
```

The Aero side owns:

- the stable C ABI and generated or checked C header
- construction of `AeroGpuBar0MmioDevice`
- the existing protocol executor and validation
- `NativeAeroGpuBackend`
- worker-thread command execution
- scanout readback and format conversion
- Rust unit, integration, and FFI tests

## 4. Memory, DMA, and Display Design

### BAR0

Register BAR0 with `memory_region_init_io()` and forward valid accesses through the C
ABI. Preserve QEMU's PCI command-register gating. Reject or return all-ones for invalid
access sizes and offsets consistently with the canonical device tests.

### BAR1 and VRAM

There must be exactly one authoritative 64 MiB VRAM allocation.

The preferred final design is a QEMU RAM `MemoryRegion` registered as prefetchable BAR1,
with a stable host mapping made available to Rust for the lifetime of the realized
device. This avoids a byte-at-a-time callback path and lets QEMU account for dirty pages.

If that ownership model delays initial bring-up, use BAR1 I/O callbacks as a correctness
prototype, then replace them before performance testing. Do not maintain mirrored QEMU
and Rust VRAM copies.

The first 256 KiB retains the existing planar VGA reservation. The VBE linear
framebuffer begins at BAR1 offset `0x40000`.

### Guest DMA

The AeroGPU command ring, fence page, command buffers, and resources use guest physical
addresses. Route them through `pci_dma_read()`/`pci_dma_write()` callbacks rather than
direct host-RAM pointers. This preserves QEMU's address-space and IOMMU semantics.

Do not service DMA while PCI bus mastering is disabled. Invalid, overflowing, or
unmapped ranges must fail the submission without crashing QEMU.

### Interrupts

Start with the driver's existing INTx contract:

1. Rust updates `IRQ_STATUS`.
2. The QEMU shim observes whether `IRQ_STATUS & IRQ_ENABLE` is nonzero.
3. QEMU calls `pci_set_irq()` with the resulting level.
4. The line remains asserted until the guest acknowledges all enabled causes.

Add MSI/MSI-X only after the existing Windows 7 path is stable and only with a compatible
guest ABI extension.

### Scanout

For the correctness milestone, use `GraphicHwOps` and a QEMU display surface:

1. Query the active AeroGPU scanout.
2. Ask the Rust backend for an RGBA/BGRA8 image.
3. Copy or convert into the QEMU surface.
4. Call `dpy_gfx_update()` for changed regions.

This includes a GPU-to-CPU readback, so it is not the final low-latency presentation
path. It still provides real GPU acceleration for rendering and is much easier to
validate. Optimize scanout only after command execution is correct.

## 5. Milestones and Exit Gates

### M0 — Reproducible QEMU baseline

Tasks:

- Build QEMU `v11.0.3` natively on Apple Silicon with
  `x86_64-softmmu`, TCG, Cocoa display, and the required storage/network backends.
- Record configure options, compiler versions, and the exact QEMU commit.
- Boot the Windows 7 installer using standard VGA and conservative IDE/SATA storage.
- Install and reboot Windows 7 without any AeroGPU device present.

Exit gate:

- An installed Windows 7 x64 VM boots reliably under upstream QEMU TCG.

This gate proves that Windows boot is a QEMU configuration concern, not an AeroGPU
device concern.

### M1 — QEMU PCI skeleton

Tasks:

- Add the `aerogpu` QOM/qdev PCI type, Kconfig entry, and Meson wiring.
- Implement the canonical PCI identity, BAR sizes/flags, reset, and INTx line.
- Initially return magic, ABI version, and feature registers from a minimal backend.
- Add QTests for configuration space, BAR sizing, MMIO decode, reset, and invalid access.

Exit gate:

- `qemu-system-x86_64 -device aerogpu` starts.
- Linux `lspci` and Windows Device Manager report `A3A0:0001`.
- The canonical Windows driver package recognizes the device hardware ID.

### M2 — Rust FFI and canonical register model

Tasks:

- Add `aerogpu-qemu-ffi` and its checked C header.
- Instantiate `AeroGpuBar0MmioDevice` through the ABI.
- Forward BAR0 and BAR1 accesses.
- Implement safe create/reset/destroy behavior and failure injection tests.
- Reuse protocol constants from one generated or mechanically checked source.
- Run existing `aero-devices-gpu` tests unchanged.

Exit gate:

- QTest register results match the existing Rust PCI/MMIO tests.
- Repeated QEMU realize, reset, and unrealize cycles are leak- and crash-free.
- No duplicate register implementation exists in QEMU C.

### M3 — Ring, DMA, fence, and INTx

Tasks:

- Implement QEMU-to-Rust DMA callbacks with bounds and bus-master checks.
- Service doorbells and ring reset.
- Write the shared fence page and completed-fence registers.
- Raise, hold, acknowledge, and reassert INTx correctly.
- Add a QTest that builds a ring in guest RAM and submits a no-op command.
- Test malformed descriptors, wrapping arithmetic, missing pages, and disabled BME.

Exit gate:

- A synthetic guest submission advances the ring head, completes a fence, updates the
  fence page, raises INTx, and deasserts only after acknowledgement.

### M4 — Windows 7 kernel driver bring-up

Tasks:

- Boot with standard VGA plus secondary AeroGPU.
- Enable Windows test signing and install the canonical x64 AeroGPU package.
- Capture QEMU logs, kernel-driver logs, Device Manager status, BAR assignments, and
  interrupt resources.
- Run the existing diagnostic control tool and a no-op submission.
- Remove any fixed-BDF assumptions discovered in the guest code.

Exit gate:

- Device Manager reports the AeroGPU adapter without an error code.
- The KMD creates the device and context, submits a command, and observes its fence and
  interrupt.

### M5 — Metal command execution

Tasks:

- Enable the existing `aerogpu-native` feature and construct `NativeAeroGpuBackend`.
- Require the Metal backend for the macOS integration build; fail clearly if unavailable.
- First prove one synchronous clear/triangle submission.
- Move command execution to a dedicated worker with a completion queue and QEMU bottom
  half notification.
- Add queue limits and deterministic device-lost/error propagation.

Exit gate:

- The in-guest D3D9Ex triangle test produces a Metal command buffer and completes its
  fence.
- The QEMU vCPU/main loop is not blocked waiting for routine GPU work.

### M6 — QEMU display, cursor, and vblank

Tasks:

- Add a QEMU graphical console and surface update path.
- Implement mode changes, resize, scanout enable/disable, and cursor composition.
- Drive vblank from a virtual-clock timer.
- Switch the visible console from standard VGA when AeroGPU scanout becomes valid.
- Avoid full-surface refresh when nothing changed.

Exit gate:

- The D3D9Ex triangle is visible in the QEMU Cocoa window.
- Mode changes, cursor movement, pause/resume, and vblank interrupts remain stable.

### M7 — Windows 7 acceleration validation

Tasks:

- Run the existing Win7 D3D9Ex test suite.
- Validate DWM composition and Aero Glass.
- Add D3D11 coverage after D3D9Ex is stable.
- Stress resource creation/destruction, dynamic buffers, shader translation, resize,
  suspend/resume, and device reset.
- Compare traces with the existing Aero machine backend.
- Add snapshot state or explicitly block snapshots while AeroGPU is active.

Exit gate:

- Windows 7 boots repeatedly with the driver installed.
- D3D9Ex tests and DWM composition run without fence stalls, corrupt scanout, or QEMU
  crashes.
- A screenshot and logs demonstrate Aero Glass rendered through Metal.

### M8 — Packaging and maintenance

Tasks:

- Make `scripts/build-qemu-aerogpu.sh` fetch or validate the pinned QEMU source, apply the
  maintained patch series, build the Rust library, and build QEMU.
- Extend the VM management script to select the AeroGPU QEMU binary and add
  `-device aerogpu`.
- Record QEMU, Aero, Rust, `wgpu`, and macOS versions in diagnostic output.
- Add Linux compile/QTest CI with a non-Metal test backend.
- Compile periodically against QEMU master to detect API drift without making master the
  supported runtime.
- Document source and license obligations for distributed builds.

Exit gate:

- A clean checkout can produce a versioned, diagnosable QEMU+AeroGPU build and boot an
  existing VM using one documented command.

## 6. Test Matrix

| Layer | Required tests |
| --- | --- |
| Rust device | Existing PCI, BAR0, BAR1, ring, DMA, INTx, scanout, and snapshot tests |
| FFI | null pointers, length overflow, panic containment, repeated lifecycle, ABI mismatch |
| QTest | PCI identity, BAR probing, MEM/BME gating, reset, DMA, fence, INTx, malformed rings |
| Host | Apple Silicon Metal adapter selection, device loss, pause/resume, window resize |
| Guest | Win7 x64 driver load, diagnostic tool, D3D9Ex tests, DWM/Aero, reboot |
| Regression | VM without `-device aerogpu` behaves like unmodified QEMU |
| Security | fuzz MMIO sequences, ring headers, descriptor lengths, and guest DMA ranges |

Do not place Windows installation media, WDK files, signed driver artifacts, or other
proprietary binaries in either repository.

## 7. Known Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Windows rejects or mishandles two VGA-class WDDM adapters | Bring up as secondary; test alternate class/subclass only if necessary; then make AeroGPU primary |
| Rust worker calls QEMU from the wrong thread | Use a completion queue plus QEMU bottom half/event source |
| BAR1 exists in two copies | Define one authoritative allocation before M3 |
| Synchronous Metal work stalls the guest | Allow only for the first triangle; require a worker before general tests |
| Scanout readback is slow | Accept for correctness; measure and optimize after M7 |
| QEMU snapshot captures incomplete GPU state | Block snapshot initially, then serialize protocol state and reconstruct host resources |
| Guest-controlled lengths cause overflow or excessive allocation | Preserve current caps; validate at FFI and DMA boundaries; add fuzzing |
| Test-signed Windows 7 driver is operationally awkward | Document test-signing setup; defer production signing |
| Custom PCI vendor ID is not PCI-SIG assigned | Keep it private/experimental; revisit identity before proposing upstream |
| QEMU and Rust licensing are mixed in one distributed executable | Publish the corresponding QEMU fork source and comply with GPL requirements |

## 8. First Implementation Sprint

The first change set should stop at a visible, testable PCI skeleton:

1. Pin and build QEMU `v11.0.3`.
2. Add `CONFIG_AEROGPU`, Meson wiring, and `hw/display/aerogpu-pci.c`.
3. Expose `A3A0:0001`, class `03:00:00`, BAR0 64 KiB, and BAR1 64 MiB.
4. Implement reset and fixed magic/ABI/features reads.
5. Add QTests for PCI identity, BAR probing, reset, and MMIO access.
6. Boot Windows 7 with standard VGA plus AeroGPU.
7. Record Device Manager detection and resource assignments.

Do not add Metal, scanout, or asynchronous execution to this first patch. Its purpose is
to establish that the QEMU build, device lifecycle, PCI contract, and guest enumeration
are correct. The next change set should add the Rust ABI and replace the fixed register
stub with `AeroGpuBar0MmioDevice`.

## 9. Illustrative Bring-up Command

The exact machine and storage flags should come from the successful M0 baseline, but the
intended shape is:

```bash
qemu-system-x86_64 \
  -machine q35 \
  -accel tcg,thread=multi \
  -cpu Nehalem \
  -smp 2 \
  -m 4096 \
  -drive file=win7.qcow2,if=ide,format=qcow2 \
  -vga std \
  -device aerogpu \
  -display cocoa
```

There is intentionally no guest CPU-Hz option. QEMU TCG translates and schedules guest
instructions according to its own execution and virtual-clock model.

## 10. Upstream References

- [QEMU downloads](https://www.qemu.org/download/)
- [QEMU release tags](https://gitlab.com/qemu-project/qemu/-/tags?sort=updated_desc)
- [QEMU qdev/QOM device API](https://www.qemu.org/docs/master/devel/qdev-api.html)
- [QEMU memory API](https://www.qemu.org/docs/master/devel/memory.html)
- [QEMU load/store and PCI DMA APIs](https://www.qemu.org/docs/master/devel/loads-stores.html)
- [QEMU vfio-user documentation](https://www.qemu.org/docs/master/system/devices/vfio-user.html)

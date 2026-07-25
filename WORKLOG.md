# macOS native frontend worklog

## 2026-07-25 — baseline

- Cloned upstream at `4f9b19ac1` into this isolated workspace.
- Host: `arm64`, macOS `27.0`, Rust toolchain pinned by the repository to `1.92.0`.
- Ran `cargo build --locked -p aero-machine-cli`, `cargo test --locked -p aero-machine-cli`, and `cargo test --locked -p aero-devices-gpu --features wgpu-backend --test aerogpu_native_smoke`; all completed successfully.

## 2026-07-25 — native bring-up

- Added `crates/aero-macos`, which uses `winit` plus a Metal-only `wgpu` instance, host triangle, RGBA framebuffer blit, native input routing, bounded machine execution, and optional existing native AeroGPU executor.
- `cargo run --locked -p aero-macos -- --list-gpu` selected `Apple M1 Pro` through Metal and reported timestamp-query support. A bounded `--host-triangle` window and bounded no-disk firmware run both completed without `wgpu` validation errors.
- Windows driver binary build/package remains intentionally delegated to Windows WDK scripts; no placeholder artifacts were produced.

Next: run the host triangle interactively, then attempt a bounded guest boot with a writable Win7 disk image and capture first device initialization traces.

## 2026-07-25 — install-media and real-ISO bring-up

- Added native `--install-iso` and `--boot hdd|cdrom|cd-first` support. The
  frontend now attaches media, applies BIOS policy, and resets after
  `Machine::new`; without that reset, POST had already run against empty media.
- Added guest-reset handling to the native event loop. CD-first changes to HDD
  after the first CD boot, while unresolved CPU assists/exceptions now stop with
  diagnostics instead of being logged and retried forever.
- Validated a bounded native run on Apple M1 Pro/Metal with the reference Win7
  x64 ISO. The canonical machine exposed AeroGPU at `00:07.0`, BAR0
  `0xe0010000`, BAR1 `0xe4000000`, a 720×400 firmware scanout, and repeated
  Metal surface presentation.
- The first headless ISO run stopped on real-mode `ENTER`. Successive
  instruction snapshots exposed and fixed:
  - ENTER/LEAVE, including nested frame construction;
  - PUSHFD/POPFD width in a 16-bit code segment;
  - real-mode LES;
  - indirect real-mode far JMP with a segment override;
  - RETFD operand and selector stack-slot widths;
  - iced-x86 offset memory sizes used by indirect near jumps/calls.
- `cargo test --locked -p aero-cpu-core --test interp_integer` now passes 18
  tests, including focused cases for each of those paths.
- The optimized reference run now reaches protected-mode bootloader code. An
  instruction-count breakpoint shows the current failure is an uninitialized
  global at physical `0x00495e08`, followed by `call [eax]` with `eax=0`.
  Physical address zero contains the IVT far pointer `F000:EF00`, which becomes
  the invalid flat target `0xf000ef00`. This needs an initialization/handoff
  trace; the terminal `0xff` opcode is only a symptom.
- The final VGA dump is still black 720×400 with a cursor, and serial/debugcon
  are empty. No Windows UI, PnP, AeroGPU KMD, or guest D3D command was reached.

## 2026-07-25 — protocol and package audit

- Compared the canonical C headers, Rust protocol mirror, canonical machine
  device, PCI wrapper, KMD expectations, and Win7 INFs.
- Confirmed ABI 1.4, `A3A0:0001`, BAR sizes/layout, packed ring/descriptor and
  allocation-table offsets, 56-byte fence structure, little-endian split-`u64`
  MMIO, and legacy INTA routing to default GSI/IRQ 13.
- Ran `cargo test --locked -p aero-protocol -p aero-devices-gpu`; all protocol,
  PCI, BAR, ring, bounds, fence, interrupt, vblank, package, and device-contract
  tests passed.
- Confirmed no built Windows driver payload exists locally. Windows KMD/UMD
  build, catalog creation, and signing still require the documented WDK host
  workflow; no placeholder or proprietary artifact was created.

Next: instrument or breakpoint writes to guest physical `0x00495e08` and compare
the loader initialization branch with a known-good PC BIOS boot. After the
guest reaches PnP, capture KMD BAR discovery, ABI/features, ring enable,
doorbells, fence IRQ/DPC completion, and finally run `d3d9ex_triangle`.

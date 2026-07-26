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

## 2026-07-25 — callback initialization trace and VM workflow

- Added bounded guest-physical write watchpoints to the canonical machine and
  headless runner. The CLI can inspect/dump physical ranges, single-step after
  an instruction threshold, stop on a watched write, and apply an explicitly
  labeled debug-only u32 patch for controlled counterfactual experiments.
- The first write to `0x00495e08` occurs at instruction 19,059,872. The prior
  state is a `REP STOSD` zero-fill from `0x0046d000` through `0x00496060`;
  all 16 watched bytes remain zero, and no later write occurs before the null
  call at instruction 20,376,806.
- A black-box QEMU/TCG boot of the same locally supplied media reaches the same
  call site with a low-memory callback pointer installed. Applying that value
  to Aero as an experiment advances 263 instructions and then diverges at a
  different low address. This disproves a one-value workaround and points back
  to missing callback registration or broader handoff semantics.
- The reference comparison exposed `CR0.ET=1` versus Aero's incorrect zero.
  Added the fixed ET bit to canonical reset state, MOV CR0/LMSW writes, and the
  legacy helper, with a focused regression test. The real ISO now reports
  `CR0=0x11` at the failure boundary, but the callback remains null.
- Local memory comparison showed the low transition table itself is intact:
  both executions contain the callback-table pointer at physical `0x2530c`
  and the same transition stub at `0x23dc0`. The missing behavior is therefore
  the registration/propagation step into protected global `0x495e08`, not
  generation of the low thunk.
- Added native `--cpus` plumbing with an explicit warning for counts above one.
  One vCPU remains the recommended Windows bring-up configuration.
- Added `scripts/aero-vm.sh` and a shell integration test. It creates sparse
  VM disks outside the repository, tracks RAM/vCPUs/ISO/acceleration settings,
  launches GUI or headless flows, prints dry-run commands, and moves removed
  VMs to recoverable trash.
- Added `docs/MACOS_ACCELERATED_VM_GUIDE.md` covering the CPU/Metal/AeroGPU
  layers, lifecycle, driver requirements, evidence required before claiming
  guest acceleration, and current limitations.

Next: trace reads of the low callback table and the code path that should copy
`0x2530c` into `0x495e08`, starting from handoff data passed from the 16-bit
boot environment into the protected loader. Do not convert the observed
pointer into a product workaround.

## 2026-07-25 — fixed mixed-width transition stack

- Added bounded guest-physical read watchpoints and used them to locate the
  first consumption of the low callback data at instruction 20,372,390.
- The earlier low-table comparison was incomplete: QEMU populated
  `0x252f8..0x2530f`, while Aero left it zero. The protected registration
  routine at `0x0040674b` was not at fault; Aero skipped its caller because
  the `"BOOT APP"` handoff pointer was invalid.
- At Boot Manager entry, both executions had `EDX=0x25398` and
  `ESP=0x61ff4`. QEMU's stack contained return `0x20a9a` and argument
  `0x25398`; Aero's intended values were instead found at the low-16-bit
  aliases `0x1ff4` and `0x1ff8`.
- Fixed canonical stack pointer selection. Outside long mode the stack address
  size now follows `SS.B`, independently of the code width selected by
  `CS.D`. Added a protected-16-code/32-bit-stack regression matching the
  Windows transition thunk.
- The real ISO now fills `[0x2530c]=0x252f8` and
  `[0x495e08]=0x252f8`, passes the former 20,376,806-instruction null call,
  and reaches 100,000,000 instructions without an exception.
- The framebuffer at that limit is still black and execution is in the low
  real-mode callback path. No installer UI or guest acceleration is claimed.

Next: measure callback/I/O progress across longer runs and compare the
real-mode callback sequence with QEMU to distinguish slow loading from the
next deterministic loop.

## 2026-07-25 — first visible Windows boot UI

- Snapshot comparisons showed the apparent real-mode loop changed only its
  transition stack and INT 1Ah result buffer.
- The request block at `0x30000` contained BIOS interrupt vector `0x1a`.
  Aero's BDA tick remained zero through 100,000,000 instructions because the
  deterministic 3 GHz clock maps one retired instruction to one cycle.
- The first BDA tick arrived after roughly 165,000,000 total instructions and
  Boot Manager immediately left the polling loop. This proved the callback was
  working and exposed virtual-time scaling as the apparent stall.
- Added diagnostic `--guest-cpu-hz HZ` overrides to the headless and native
  macOS frontends. The override is reapplied after native guest resets and is
  explicitly reported as non-representative timing.
- Continuing from a real snapshot at 3 MHz virtual TSC produced the first
  visible Windows output: “Windows is loading files…” with its progress bar.
  The screen later cleared while the guest continued protected-mode work in
  newly populated high memory.
- An additional 320,000,000 instructions under accelerated diagnostic time
  completed without an exception. The graphical setup UI and 64-bit kernel
  transition are not yet reached.

Next: continue the protected-mode loading/decompression path to the long-mode
transition, then compare that boundary with the QEMU reference.

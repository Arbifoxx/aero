# macOS native status

Validated on 2026-07-25 on ARM64 macOS 27.0 (build `26A5378n`), Apple M1 Pro,
Rust/Cargo 1.92.0. The current native frontend commit is `13c826be1`; the CPU
bootloader bring-up commit is `f70373d91`.

| Milestone | Status | Evidence |
|---|---|---|
| Native Rust build | validated | `cargo check --locked -p aero-macos` |
| Metal discovery | validated | Apple M1 Pro selected through the Metal backend |
| Native host triangle | validated | Metal window and surface presentation complete without validation errors |
| Canonical machine framebuffer presentation | validated | `Machine::display_present()` uploads RGBA output to the native Metal surface |
| HDD/install ISO attachment and BIOS selection | implemented and host validated | `--disk`, `--install-iso`, and `--boot hdd|cdrom|cd-first`; media is attached before a required machine reset |
| Guest reset/install transition | implemented | CD-first disables itself after the first CD-booted guest reset and selects HDD |
| Keyboard/mouse injection | implemented | winit physical input maps to canonical PS/2 machine APIs |
| AeroGPU PCI identity | validated on host | `A3A0:0001` at `00:07.0`; observed BAR0 `0xe0010000`, BAR1 `0xe4000000` |
| AeroGPU protocol/device suites | validated on host | complete `aero-protocol` and `aero-devices-gpu` test suites pass |
| Native AeroGPU wgpu executor | builds/tests | native smoke test passes on Metal |
| Real Windows 7 ISO boot | partial, visible boot UI | renders “Windows is loading files…” and continues protected-mode loading; setup UI not reached |
| Win7 KMD load / BAR-ring-fence traffic | not reached | extended boot run has not reached PnP |
| Guest D3D9 triangle | not reached | in-tree `d3d9ex_triangle` is ready but cannot run until guest boot and driver install |
| DWM / Aero Glass | not validated | depends on the same guest milestones |

## Current boot boundary

Reference media:

- `Win7_Ult_SP1_English_x64.iso`
- SHA-256
  `36f4fa2416d0982697ab106e3a72d2e120dbcdb6cc54fd3906d06120d0653808`

The former deterministic crash at instruction 20,376,806 is fixed. The root
cause was independent code and stack address sizes: Windows ran 16-bit code
(`CS.D=0`) with a 32-bit stack (`SS.B=1`), but Aero selected SP from the code
width. Four 32-bit transition pushes updated ESP while writing through its low
16-bit alias at `0x00001fec..0x00001ffb`, leaving the intended
`0x00061fec..0x00061ffb` call frame uninitialized. Boot Manager consequently
received `0xffff79af` instead of the valid `0x00025398` `"BOOT APP"` handoff
pointer and skipped callback registration.

Stack address size now follows `SS.B` outside long mode. The real ISO produces
the same transition frame, low callback table, and protected globals as the
QEMU reference:

- handoff argument: `0x00025398`
- callback table: `0x000252f8`
- callback-table owner pointer: `[0x0002530c] = 0x000252f8`
- protected callback global: `[0x00495e08] = 0x000252f8`

An optimized run now passes the old crash. It initially appeared to remain in
the low real-mode transition/BIOS-callback path, but that code was polling BIOS
INT 1Ah for a timer tick. Aero deterministically models one retired instruction
as one 3 GHz TSC cycle, so 100,000,000 interpreted instructions advance only
33 ms of guest time—less than one 54.9 ms BIOS tick—even when they take roughly
90 seconds on the host. At the first virtual tick the loader immediately
continues.

Both native frontends now accept a diagnostic `--guest-cpu-hz HZ` override.
With `--guest-cpu-hz 3000000`, the same unmodified ISO renders
“Windows is loading files…”, completes that text-mode loading screen, clears
the display, and continues sustained protected-mode processing in populated
high memory. A further 320,000,000-instruction accelerated-time continuation
has not faulted, but has not yet entered the 64-bit kernel or displayed the
graphical setup UI. The override changes guest-visible timing and is a
bring-up tool, not a performance or compatibility fix.

The comparison also found Aero incorrectly allowed `CR0.ET` to read as zero.
The reset state and CR0 write paths now keep this modern-CPU fixed bit set,
matching the reference `CR0=0x11` at the boundary. This is an architectural
correction with a regression test. The later stack-address fix is what moved
the boot boundary.

The earliest next task is to finish the current protected-mode file/image
processing, reach the 64-bit kernel transition, and determine whether the next
boundary is CPU correctness, a missing device, or simply interpreter
throughput. QEMU reaches the 64-bit kernel with the same ISO, so any later
architectural divergence remains Aero-specific.

The run now passes the earlier deterministic loader gaps for ENTER/LEAVE,
PUSHFD/POPFD operand overrides, LES, indirect far JMP, RETFD stack width, and
indirect jump-table offset widths. Each has a focused interpreter regression
test.

Therefore this status claims a visible Windows bootloader screen, but not the
graphical installer, driver initialization, a D3D9 triangle, or Aero Glass.

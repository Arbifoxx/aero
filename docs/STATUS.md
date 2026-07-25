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
| Real Windows 7 ISO boot | partial, blocked before UI | reaches 32-bit protected bootloader code; no Windows Boot Manager pixels yet |
| Win7 KMD load / BAR-ring-fence traffic | not reached | current CPU/bootloader blocker occurs before PnP |
| Guest D3D9 triangle | not reached | in-tree `d3d9ex_triangle` is ready but cannot run until guest boot and driver install |
| DWM / Aero Glass | not validated | depends on the same guest milestones |

## Current boot boundary

Reference media:

- `Win7_Ult_SP1_English_x64.iso`
- SHA-256
  `36f4fa2416d0982697ab106e3a72d2e120dbcdb6cc54fd3906d06120d0653808`

An optimized ISO-only run executes 20,376,806 instructions and reaches
32-bit protected code. At `0020:0040695f`, `mov eax,[0x00495e08]` loads a null
bootloader global. The following `call [eax]` reads physical address zero,
interprets the IVT bytes for `F000:EF00` as a flat target (`0xf000ef00`), and
then faults on unmapped `0xff` bytes. The earliest next task is to trace why
`0x00495e08` was not initialized—CPU semantics versus firmware/loader handoff—
not to add another opcode at the final address.

The run now passes the earlier deterministic loader gaps for ENTER/LEAVE,
PUSHFD/POPFD operand overrides, LES, indirect far JMP, RETFD stack width, and
indirect jump-table offset widths. Each has a focused interpreter regression
test.

The dumped framebuffer remains a black 720×400 boot surface with a top-left
cursor. Therefore this status does not claim a visible Windows boot screen,
driver initialization, a D3D9 triangle, or Aero Glass.

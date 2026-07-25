# AeroGPU protocol audit

The protocol source of truth is `drivers/aerogpu/protocol/`. Rust mirrors live in
`emulator/protocol/aerogpu/`; `crates/aero-devices-gpu` and
`crates/aero-machine/src/aerogpu.rs` consume those mirrors. The native frontend
does not define a second wire format.

Audit date: 2026-07-25. “Host validated” below means the Rust protocol/device
tests passed. It does not mean the Windows 7 KMD has initialized the device in a
real guest.

| Contract | Canonical value | Host/guest compatibility |
|---|---|---|
| PCI identity | `A3A0:0001`, subsystem `A3A0:0001`, class `03:00:00` | Host validated; both primary Win7 INFs match the same canonical HWID |
| PCI location | `00:07.0` | Host validated in the canonical machine profile |
| BAR0 | 64 KiB MMIO register aperture | Host validated |
| BAR1 | 64 MiB prefetchable VRAM; VBE LFB at offset `0x40000` | Host validated, including VGA/VBE alias tests |
| Discovery | magic `0x55504741` (`AGPU`), ABI `1.4` | C/Rust layout and register tests pass |
| Feature bits | fence page, cursor, scanout, vblank, transfer, error info | Advertised by the host and covered by feature-gating tests |
| Byte/word rules | little-endian; MMIO registers are 32-bit; `u64` values use adjacent LO/HI registers | Sub-dword merge, 32-bit access, and split-`u64` host tests pass |
| Submit descriptor | packed 64 bytes; `cmd_gpa@16`, `alloc_table_gpa@32`, `signal_fence@48` | C/Rust layout tests pass |
| Ring header | packed 64 bytes; monotonic `u32` head/tail at offsets 24/28; power-of-two entry count | Layout, wrap, size, stride, overflow, and ABI-version tests pass |
| Allocation table | 24-byte header; 32-byte entries; unique nonzero IDs; checked GPA/size ranges | Decode, extension-stride, duplicate-ID, readonly, and overflow tests pass |
| Fence page | 56-byte structure in a 4 KiB guest page; completed fence at offset 8 | Write/layout tests pass |
| Interrupts | legacy level INTx INTA; device 7 swizzles to PIRQ D and default GSI/IRQ 13; W1C ACK | PCI INTx gating and IRQ status/enable/ack tests pass; Win7 ISR/DPC path is unvalidated |
| Scanout/cursor/vblank | BAR0 register blocks plus guest-memory readback | Bounds, format, atomic GPA update, vblank pacing, and stale-IRQ tests pass |
| Backend handoff | validated submissions enter `AeroGpuCommandBackend`; fences return through the device model | Immediate/deferred and native-wgpu smoke tests pass; no real guest submission observed |

The command and allocation ranges are validated before guest memory is touched.
Malformed submissions advance or reset the ring according to the device rules
and latch structured error state. Frontends must not dereference guest pointers,
decode ACMD packets, or synthesize fence completion independently.

The Windows package relationship is consistent:

- Service name and KMD binary: `aerogpu` / `aerogpu.sys`.
- D3D9-only `aerogpu.inf`: feature score `0xF8`; x86
  `aerogpu_d3d9`, x64 `aerogpu_d3d9_x64`, WOW64
  `aerogpu_d3d9`.
- DX11-capable `aerogpu_dx11.inf`: feature score `0xF7` and the
  same D3D9 UMDs plus the architecture-matched D3D10/11 UMD.

Remaining compatibility boundary:

- The real Windows 7 install ISO currently stops before KMD discovery, so BAR
  reads, ring programming, interrupt registration, and D3D9 command submission
  have not been observed from the guest.
- Native trace switches currently provide periodic summaries and point to the
  shared-layer tracing targets. They are not yet a structured per-MMIO or
  per-packet capture facility; do not treat their presence as guest protocol
  evidence.

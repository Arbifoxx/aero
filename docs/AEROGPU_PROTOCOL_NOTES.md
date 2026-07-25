# AeroGPU protocol notes

There is one protocol source of truth: `drivers/aerogpu/protocol/`. Rust and TypeScript mirrors live in `emulator/protocol/aerogpu/`; the native frontend does not define a duplicate wire format.

The canonical machine device is `crates/aero-machine/src/aerogpu.rs`: PCI `A3A0:0001` at `00:07.0`, BAR0 for MMIO/ring/fences/interrupts/vblank and BAR1 for VRAM plus legacy VGA/VBE compatibility. The backend boundary is `aero_devices_gpu::AeroGpuCommandBackend`; the existing native implementation decodes commands through `aero-gpu` and reports fence completions back to the device model.

All ring, allocation table, guest-memory and command validation must stay in those shared layers. Frontends may observe trace categories, but must never interpret guest pointers or construct ACMD packets independently.

# Native macOS debugging

Start by isolating the layer:

1. `aero-macos --list-gpu` confirms a Metal adapter and limits.
2. `aero-macos --host-triangle` proves native surface, shader and present without the VM.
3. Run without `--aerogpu-wgpu` to validate firmware/VGA/VBE framebuffer presentation.
4. Add `--aerogpu-wgpu --trace-pci --trace-mmio --trace-scanout` and `--log-level aero_machine=trace,aero_gpu=trace` to isolate PCI/device/decoder/backend failures.

For a native crash, run under LLDB with `RUST_BACKTRACE=1`. Validation errors must remain visible; do not use a fallback adapter unless `--allow-fallback-adapter` was explicitly supplied. `wgpu` 0.20 reports whether fallback was requested but does not expose a selected-adapter fallback flag, so logs record the requested policy.

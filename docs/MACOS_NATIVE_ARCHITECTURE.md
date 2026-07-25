# macOS native architecture

`crates/aero-macos` is the native Apple Silicon bring-up frontend. It deliberately reuses the canonical Rust machine rather than the browser host:

```text
winit window/input → aero-macos → aero_machine::Machine → AeroGPU BAR0/BAR1
                                  ↘ display_present RGBA8 → wgpu Metal surface
                                  ↘ optional NativeAeroGpuBackend → aero_gpu → Metal
```

The executable uses a Metal-only `wgpu::Instance`; adapter selection is logged and a non-Metal adapter is rejected. `--host-triangle` isolates the host graphics path before any guest command is involved. Normal mode runs bounded machine slices from the event loop, uploads `Machine::display_framebuffer()` to a native texture, and presents it. This is cooperative rather than a dedicated VM thread because `Machine` intentionally contains `Rc` device state and is not `Send`; no unsafe cross-thread wrapper is used.

Useful commands:

```bash
scripts/bootstrap-macos.sh
scripts/build-macos.sh
target/debug/aero-macos --list-gpu
target/debug/aero-macos --host-triangle
target/debug/aero-macos --install-iso /path/to/win7.iso --boot cdrom --memory 2048 --trace-pci --trace-scanout
target/debug/aero-macos --disk /path/to/win7.img --install-iso /path/to/win7.iso --boot cd-first --memory 2048 --aerogpu-wgpu
```

`--aerogpu-wgpu` installs the existing feature-gated in-process executor. The frontend's own Metal surface remains independent from that executor; `Machine::display_present()` bridges backend scanout readback into the presentation texture.

`Machine::new` performs POST immediately, so the frontend resets the machine
after attaching HDD/CD media and applying the boot policy. On a guest reset,
`cd-first` is disabled after a CD boot and the next POST selects HDD. HLT keeps
the event loop alive for device interrupts; unresolved assists, exceptions, and
fatal CPU exits terminate the frontend with mode/CS/RIP diagnostics.

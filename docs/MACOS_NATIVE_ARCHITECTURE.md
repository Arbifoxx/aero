# macOS native architecture

For a complete Windows 7 VM, the primary architecture is now:

```text
Windows 7 x86_64 → QEMU 11 TCG/PC platform → AeroGPU PCI device
                                               ↕ versioned C ABI + guest DMA
                                        Rust AeroGPU core → wgpu → Metal
```

QEMU supplies the mature CPU, SMP, firmware, storage, and platform emulation.
The repository patch adds only the AeroGPU PCI adapter. See `qemu/README.md`
and `docs/MACOS_ACCELERATED_VM_GUIDE.md` for the supported operator workflow.

`crates/aero-macos` is the native Apple Silicon bring-up frontend. It deliberately reuses the canonical Rust machine rather than the browser host:

```text
winit window/input → aero-macos → aero_machine::Machine → AeroGPU BAR0/BAR1
                                  ↘ display_present RGBA8 → wgpu Metal surface
                                  ↘ optional NativeAeroGpuBackend → aero_gpu → Metal
```

The executable uses a Metal-only `wgpu::Instance`; adapter selection is logged and a non-Metal adapter is rejected. `--host-triangle` isolates the host graphics path before any guest command is involved. Normal mode runs bounded machine slices from the event loop, uploads `Machine::display_framebuffer()` to a native texture, and presents it. This is cooperative rather than a dedicated VM thread because `Machine` intentionally contains `Rc` device state and is not `Send`; no unsafe cross-thread wrapper is used.

Useful legacy bring-up commands:

```bash
scripts/bootstrap-macos.sh
scripts/build-macos.sh
target/debug/aero-macos --list-gpu
target/debug/aero-macos --host-triangle
target/debug/aero-macos --install-iso /path/to/win7.iso --boot cdrom --memory 2048 --cpus 1 --trace-pci --trace-scanout
target/debug/aero-macos --disk /path/to/win7.img --install-iso /path/to/win7.iso --boot cd-first --memory 2048 --cpus 1 --aerogpu-wgpu
scripts/aero-vm.sh create win7-lab --ram 2048 --cpus 1 --disk-size 40G --iso /path/to/win7.iso
scripts/aero-vm.sh start win7-lab --install --trace
```

`--aerogpu-wgpu` installs the existing feature-gated in-process executor. The frontend's own Metal surface remains independent from that executor; `Machine::display_present()` bridges backend scanout readback into the presentation texture.

`--cpus` publishes the selected topology and enables the canonical machine's
cooperative AP loop. Counts above one are still SMP bring-up only; use one vCPU
for Windows boot work. `scripts/aero-vm.sh` stores sparse disks and small
configuration files outside the repository. See
`docs/MACOS_ACCELERATED_VM_GUIDE.md` for the operator workflow and the
distinction between host Metal presentation and validated guest acceleration.

`Machine::new` performs POST immediately, so the frontend resets the machine
after attaching HDD/CD media and applying the boot policy. On a guest reset,
`cd-first` is disabled after a CD boot and the next POST selects HDD. HLT keeps
the event loop alive for device interrupts; unresolved assists, exceptions, and
fatal CPU exits terminate the frontend with mode/CS/RIP diagnostics.

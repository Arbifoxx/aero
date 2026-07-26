# Native macOS debugging

Start by isolating the layer:

1. `aero-macos --list-gpu` confirms a Metal adapter and limits.
2. `aero-macos --host-triangle` proves native surface, shader and present without the VM.
3. Run without `--aerogpu-wgpu` to validate firmware/VGA/VBE framebuffer presentation.
4. Add `--aerogpu-wgpu --trace-pci --trace-mmio --trace-scanout` and `--log-level aero_machine=trace,aero_gpu=trace` to isolate PCI/device/decoder/backend failures.

For a native crash, run under LLDB with `RUST_BACKTRACE=1`. Validation errors must remain visible; do not use a fallback adapter unless `--allow-fallback-adapter` was explicitly supplied. `wgpu` 0.20 reports whether fallback was requested but does not expose a selected-adapter fallback flag, so logs record the requested policy.

For CPU/firmware boot failures, use the headless CLI first:

```bash
target/release/aero-machine \
  --install-iso /path/to/win7.iso \
  --boot cdrom \
  --ram 1024 \
  --max-ms 120000 \
  --vga-png /tmp/aero-win7.png
```

Failure reports include mode, segment selectors/bases, RIP/linear IP, flags,
control registers, descriptor-table bases/limits, GPRs, instruction bytes, and
stack bytes while paging is disabled. Once a failure is deterministic,
`--max-insts N` can stop immediately before it; this is how the former null
callback at `0020:0040695f` was separated from its later `0xf000ef00` symptom.

For provenance, use bounded physical read/write watchpoints. The runner can
switch to single-instruction slices only near the suspected interval:

```bash
target/release/aero-machine \
  --install-iso /path/to/win7.iso \
  --boot cdrom \
  --ram 1024 \
  --max-insts 20376806 \
  --watch-phys 0x495e08:4 \
  --watch-after-insts 19000000 \
  --watch-granularity-insts 1 \
  --watch-stop \
  --inspect-phys 0x495e08:16
```

Use `--watch-read-phys ADDRESS:LENGTH` with the same threshold, granularity,
and stop options to identify the instruction that consumes a handoff field.
Read events contain only bytes returned by the original access; tracing does
not perform a second MMIO read.

`--dump-phys ADDRESS:LENGTH:PATH` writes a local byte dump capped at 256 MiB.
Guest dumps can contain proprietary material and must not be committed.
`--patch-phys-u32-at INSTRUCTIONS:ADDRESS:VALUE` is a counterfactual debugging
tool only: it deliberately changes guest state and cannot establish a fix.

The native `--trace-*` switches currently emit periodic summaries or direct
users to shared-layer tracing. They are not a complete per-MMIO/per-command
capture. Guest protocol claims require KMD/debug-control output or shared-layer
instrumentation, not only those frontend switches.

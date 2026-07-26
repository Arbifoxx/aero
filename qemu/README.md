# AeroGPU QEMU integration

This directory maintains the small QEMU patch series used by the native Apple Silicon
runtime. QEMU handles x86_64 TCG emulation and the PC platform; AeroGPU remains a
project-specific PCI display device.

## Build

```bash
bash scripts/build-qemu-aerogpu.sh build
bash scripts/build-qemu-aerogpu.sh status
```

The script clones the exact tag and commit recorded in `qemu/supported-version`, applies
the matching patch series, builds `x86_64-softmmu`, and installs it under
`target/qemu-aerogpu/`.

The initial patch exposes the canonical `A3A0:0001` PCI function, a 64 KiB BAR0, a
64 MiB prefetchable BAR1, and the ABI discovery registers. It deliberately advertises
no GPU command features yet. The next integration step replaces the fixed BAR0 reads
with the Rust `AeroGpuBar0MmioDevice` through a C ABI.

## VM manager

Run the interactive menu:

```bash
bash scripts/aero-vm.sh menu
```

Or use the command-line interface:

```bash
bash scripts/aero-vm.sh create win7 --backend qemu --ram 4096 --cpus 2 \
  --disk-size 40G --iso /path/to/windows-7.iso
bash scripts/aero-vm.sh start win7 --install
```

The QEMU backend uses standard VGA for the boot/install display and adds AeroGPU as a
secondary adapter. At this stage it provides PCI enumeration only, not accelerated
rendering.

QEMU is GPL-licensed. Distributing a combined build requires compliance with QEMU's
license and corresponding-source obligations.

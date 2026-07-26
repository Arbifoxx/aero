# Accelerated Aero VMs on macOS

This guide describes the native Apple Silicon frontend in `crates/aero-macos`.
It is an experimental emulator, not a production replacement for QEMU,
VirtualBox, VMware, or Parallels. As of 2026-07-25, Windows 7 passes the former
32-bit bootloader callback crash, but an extended run still has not displayed
the installer UI. The commands below create durable VM storage and launch the
implemented path; they do not imply that a Windows installation can complete
yet.

## The important mental model

An Aero VM has three distinct performance layers:

1. **Guest CPU:** x86/x86-64 instructions are emulated on Apple Silicon. This
   is not Apple Hypervisor.framework acceleration. Use one vCPU for boot work;
   Aero's multi-vCPU scheduler is still experimental.
2. **Boot display:** BIOS VGA/VBE output is copied to a native Metal window.
   Seeing this surface does not mean a Windows graphics driver is active.
3. **Accelerated guest graphics:** the Windows AeroGPU WDDM driver submits
   commands through the emulated `A3A0:0001` PCI device. With
   `--aerogpu-wgpu`, the native backend executes those commands through wgpu
   on Metal. This final path is implemented and host-tested, but has not run
   inside a booted Windows 7 guest.

The first reliable accelerated milestone will be the in-tree
`d3d9ex_triangle` test. Aero Glass and general application compatibility come
later.

## Recommended first VM

- Windows 7 SP1 x64 media that you legally own
- 2048 MiB RAM
- 1 vCPU
- 40 GiB sparse raw disk
- AeroGPU acceleration enabled

Keep the ISO, product key, VM disk, certificates, generated drivers, and
memory dumps outside the repository. None of those artifacts may be committed.

## Build and check the host

```bash
scripts/bootstrap-macos.sh
scripts/build-macos.sh
scripts/aero-vm.sh doctor
```

`doctor` lists the selected Metal adapter. You can independently validate host
presentation with:

```bash
target/debug/aero-macos --host-triangle
```

That triangle is host-only; it does not exercise the Windows driver.

## Create and manage a VM

The manager stores VMs under
`${AERO_VM_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/aero/vms}`. Override
`AERO_VM_HOME` when a different volume is preferable.

```bash
scripts/aero-vm.sh create win7-lab \
  --ram 2048 \
  --cpus 1 \
  --disk-size 40G \
  --iso /path/to/windows-7-sp1-x64.iso \
  --acceleration on

scripts/aero-vm.sh list
scripts/aero-vm.sh show win7-lab
scripts/aero-vm.sh set win7-lab --ram 3072
scripts/aero-vm.sh start win7-lab --install --trace
```

Use `--dry-run` to inspect the exact frontend command. Use `--headless` for a
bounded diagnostic run:

```bash
scripts/aero-vm.sh start win7-lab --install --headless --max-ms 120000 --dry-run
```

`trash NAME` moves a VM into a recoverable `.trash` directory under the VM
root instead of permanently deleting its disk.

The disk is sparse: its virtual capacity is immediately visible to the guest,
while host blocks are allocated as data is written. The manager deliberately
does not resize installed filesystems or partitions. Back up the disk before
using external storage tools.

## Expected install lifecycle

Today, `start win7-lab --install` is expected to hit the documented early
bootloader blocker. Once boot is fixed, the intended lifecycle is:

1. Start with the ISO and disk using `--boot cd-first`.
2. Install Windows onto the VM's `disk.raw`.
3. The first guest reset disables CD-first and selects the HDD.
4. Subsequent normal starts use `--boot hdd`.
5. Build the AeroGPU drivers on Windows with WDK 10.
6. Enable Windows test signing, trust the local test certificate, and install
   the x64 AeroGPU package.
7. Confirm Device Manager, PCI BARs, interrupts, rings, fences, and scanout.
8. Run `drivers\aerogpu\tests\win7\d3d9ex_triangle`.

Driver build and guest installation commands are in
`docs/WINDOWS7_GUEST_SETUP.md`. macOS cannot build the WDK driver package.

## Acceleration switches

- `--acceleration on` makes the manager pass `--aerogpu-wgpu`. This selects
  the real Metal command executor, but Windows must still load the AeroGPU
  driver before guest rendering is accelerated.
- `--acceleration off` passes `--no-aerogpu` and uses the legacy VGA device.
  This is useful for isolating boot/display problems, not for Aero Glass.
- The frontend rejects non-Metal backends. Software fallback is disabled
  unless explicitly requested.

Do not judge acceleration from window smoothness or the host triangle. Require
guest evidence: AeroGPU PnP success, driver logs, BAR/ring/fence activity, and
the D3D9Ex test result.

## Current limitations that affect VM choices

- Windows setup has not reached a visible UI; the former null-callback crash
  is fixed, and the current extended-run boundary is in low real-mode callback
  traffic.
- One vCPU is the only recommended boot configuration.
- Guest CPU execution is emulated and may remain slow even after GPU
  acceleration works.
- AeroGPU guest driver loading, D3D9Ex, DWM, and Aero Glass are unvalidated.
- Snapshots exist in the headless machine tooling but are not yet integrated
  into the native VM manager lifecycle.
- Network, audio, USB, suspend/resume, and long-running disk durability are
  not validated as a complete native Windows VM product.

## Useful diagnostics

The native frontend accepts `--trace-pci`, `--trace-scanout`, and related trace
switches. These currently provide summaries and logging guidance rather than a
complete per-access protocol trace.

The headless runner supports deterministic instruction limits, register
diagnostics, physical-memory inspection/dumps, and bounded write watchpoints:

```bash
target/release/aero-machine \
  --install-iso /path/to/windows.iso \
  --boot cdrom \
  --ram 1024 \
  --max-insts 20376806 \
  --watch-phys 0x495e08:4 \
  --inspect-phys 0x495e08:16
```

`--dump-phys` output can contain proprietary guest bytes and must remain a
local debugging artifact. `--patch-phys-u32-at` exists only for controlled
experiments; it changes guest state and is never evidence of a real fix.

See `docs/STATUS.md` for the exact current boundary and `docs/DEBUGGING.md` for
the debugging workflow.

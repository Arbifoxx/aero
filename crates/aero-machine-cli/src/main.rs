#![forbid(unsafe_code)]

// This crate is a native-only CLI tool, but the workspace CI builds `--target wasm32-unknown-unknown
// --workspace --tests --no-run` to ensure the emulator crates remain wasm-compatible.
//
// Provide a tiny wasm32 stub `main` so the workspace continues to compile for wasm targets. The CLI
// is not expected to run in the browser.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::fmt;
    use std::fs::File;
    use std::io::{self, BufWriter, Write};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::time::{Duration, Instant};

    use aero_machine::{BootDevice, Machine, MachineConfig, RunExit};
    use aero_storage::{AeroCowDisk, DiskImage, StdFileBackend, VirtualDisk, SECTOR_SIZE};
    use anyhow::{anyhow, bail, Context, Result};
    use clap::{ArgGroup, Parser, ValueEnum};

    const SLICE_INST_BUDGET: u64 = 100_000;

    #[derive(Debug, Parser)]
    #[command(
        about = "Native runner for aero_machine::Machine (boot/integration debugging)",
        group(
            ArgGroup::new("stop")
                .required(true)
                .args(["max_insts", "max_ms"])
        ),
        group(
            ArgGroup::new("media")
                .required(true)
                .args(["disk", "install_iso"])
        )
    )]
    pub struct Args {
        /// Disk image to attach (raw/qcow2/vhd/aerospar; auto-detected).
        ///
        /// The virtual capacity must be a multiple of 512 bytes.
        #[arg(long)]
        disk: Option<PathBuf>,

        /// Open the disk image read-only (guest writes will fail).
        #[arg(long, conflicts_with = "disk_overlay", requires = "disk")]
        disk_ro: bool,

        /// Optional copy-on-write overlay image (AEROSPAR).
        ///
        /// If provided, the base `--disk` is opened read-only and guest writes go to the overlay.
        #[arg(long, requires = "disk")]
        disk_overlay: Option<PathBuf>,

        /// Allocation unit (block size) used when creating a new `--disk-overlay` (bytes).
        ///
        /// Must be a power of two and a multiple of 512.
        #[arg(long, default_value_t = 1024 * 1024, requires = "disk_overlay")]
        disk_overlay_block_size: u32,

        /// Guest RAM size in MiB.
        #[arg(long, default_value_t = 64)]
        ram: u64,

        /// Number of guest vCPUs. Keep this at 1 for Windows boots; SMP is still experimental.
        #[arg(long, default_value_t = 1)]
        cpus: u8,

        /// Stop after executing at most N guest instructions.
        #[arg(long)]
        max_insts: Option<u64>,

        /// Stop after running for at most N milliseconds of host time.
        #[arg(long)]
        max_ms: Option<u64>,

        /// Override the deterministic guest TSC frequency in Hz.
        ///
        /// Debugging only: lower values make guest timers advance faster per retired instruction
        /// and change guest-visible timing.
        #[arg(long, value_name = "HZ")]
        guest_cpu_hz: Option<u64>,

        /// Where to write accumulated COM1 output bytes (`stdout` or a file path).
        #[arg(long, default_value = "stdout")]
        serial_out: String,

        /// Where to write accumulated DebugCon output bytes (I/O port `0xE9`).
        ///
        /// Use `none` to disable (default), `stdout` to write to stdout, or a file path.
        #[arg(long, default_value = "none")]
        debugcon_out: String,

        /// Dump the last VGA framebuffer to a PNG file on exit.
        #[arg(long)]
        vga_png: Option<PathBuf>,

        /// Save a snapshot (aero_snapshot format) on exit.
        #[arg(long)]
        snapshot_save: Option<PathBuf>,

        /// Load a snapshot (aero_snapshot format) before running.
        #[arg(long, requires = "disk")]
        snapshot_load: Option<PathBuf>,

        /// Optional install/recovery ISO to attach as an ATAPI CD-ROM (IDE secondary master).
        ///
        /// This uses the canonical Win7 install-media slot (`disk_id=1`).
        #[arg(long)]
        install_iso: Option<PathBuf>,

        /// BIOS boot selection policy.
        ///
        /// Defaults to:
        /// - `hdd` when no `--install-iso` is provided
        /// - `cd-first` when both `--disk` and `--install-iso` are provided (install flow: boot CD once, then reboot into HDD)
        /// - `cdrom` when `--install-iso` is provided without `--disk` (ISO-only boots)
        ///
        /// Note: when `--snapshot-load` is used, this only affects future guest resets. The current
        /// CPU state is restored from the snapshot and the VM is not reset.
        #[arg(long, value_enum)]
        boot: Option<BootMode>,

        /// Print guest physical memory on exit (`ADDRESS:LENGTH`; decimal or `0x` hex).
        ///
        /// May be repeated. Each range is capped at 1 MiB.
        #[arg(long, value_name = "ADDRESS:LENGTH")]
        inspect_phys: Vec<PhysicalRange>,

        /// Dump guest physical memory on exit (`ADDRESS:LENGTH:PATH`; decimal or `0x` hex).
        ///
        /// May be repeated. Dumps are local debugging artifacts and must not be committed when
        /// they contain proprietary guest bytes. Each dump is capped at 256 MiB.
        #[arg(long, value_name = "ADDRESS:LENGTH:PATH")]
        dump_phys: Vec<PhysicalDump>,

        /// Record writes overlapping guest physical memory (`ADDRESS:LENGTH`).
        ///
        /// The watched range is capped at 4 KiB. Events report the instruction-count interval in
        /// which the write occurred, plus previous and current bytes.
        #[arg(long, value_name = "ADDRESS:LENGTH")]
        watch_phys: Option<PhysicalRange>,

        /// Record reads overlapping guest physical memory (`ADDRESS:LENGTH`).
        ///
        /// The watched range is capped at 4 KiB. Events report the instruction-count interval in
        /// which the read occurred and the bytes returned.
        #[arg(long, value_name = "ADDRESS:LENGTH")]
        watch_read_phys: Option<PhysicalRange>,

        /// Begin using `--watch-granularity-insts` after this many instructions.
        #[arg(long, default_value_t = 0)]
        watch_after_insts: u64,

        /// Runner slice size after `--watch-after-insts`.
        ///
        /// Use 1 to identify the instruction immediately responsible for a watched write.
        #[arg(long, default_value_t = SLICE_INST_BUDGET)]
        watch_granularity_insts: u64,

        /// Stop immediately after the first slice that records a watched read or write.
        #[arg(long)]
        watch_stop: bool,

        /// Debug-only guest mutation (`INSTRUCTIONS:ADDRESS:VALUE`, decimal or `0x` hex).
        ///
        /// Writes a little-endian u32 after exactly the requested number of retired instructions.
        /// This alters guest behavior and must never be mistaken for an emulator fix.
        #[arg(long, value_name = "INSTRUCTIONS:ADDRESS:VALUE")]
        patch_phys_u32_at: Option<PhysicalU32Patch>,
    }

    #[derive(Debug, Clone)]
    struct PhysicalRange {
        address: u64,
        length: usize,
    }

    impl FromStr for PhysicalRange {
        type Err = String;

        fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
            let (address, length) = value
                .split_once(':')
                .ok_or_else(|| "expected ADDRESS:LENGTH".to_owned())?;
            let address = parse_integer(address)?;
            let length_u64 = parse_integer(length)?;
            let length = usize::try_from(length_u64)
                .map_err(|_| format!("length does not fit this host: {length}"))?;
            if length == 0 {
                return Err("length must be greater than zero".to_owned());
            }
            address
                .checked_add(length_u64)
                .ok_or_else(|| "physical range overflows u64".to_owned())?;
            Ok(Self { address, length })
        }
    }

    #[derive(Debug, Clone)]
    struct PhysicalDump {
        range: PhysicalRange,
        path: PathBuf,
    }

    #[derive(Debug, Clone)]
    struct PhysicalU32Patch {
        instructions: u64,
        address: u64,
        value: u32,
    }

    impl FromStr for PhysicalU32Patch {
        type Err = String;

        fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
            let mut fields = value.split(':');
            let instructions = parse_integer(fields.next().unwrap_or_default())?;
            let address = parse_integer(
                fields
                    .next()
                    .ok_or_else(|| "expected INSTRUCTIONS:ADDRESS:VALUE".to_owned())?,
            )?;
            let raw_value = parse_integer(
                fields
                    .next()
                    .ok_or_else(|| "expected INSTRUCTIONS:ADDRESS:VALUE".to_owned())?,
            )?;
            if fields.next().is_some() {
                return Err("expected INSTRUCTIONS:ADDRESS:VALUE".to_owned());
            }
            address
                .checked_add(4)
                .ok_or_else(|| "physical patch range overflows u64".to_owned())?;
            let value = u32::try_from(raw_value)
                .map_err(|_| format!("patch value does not fit u32: {raw_value:#x}"))?;
            Ok(Self {
                instructions,
                address,
                value,
            })
        }
    }

    impl FromStr for PhysicalDump {
        type Err = String;

        fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
            let mut fields = value.splitn(3, ':');
            let address = fields.next().unwrap_or_default();
            let length = fields
                .next()
                .ok_or_else(|| "expected ADDRESS:LENGTH:PATH".to_owned())?;
            let path = fields
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| "expected ADDRESS:LENGTH:PATH".to_owned())?;
            let range = format!("{address}:{length}").parse()?;
            Ok(Self {
                range,
                path: PathBuf::from(path),
            })
        }
    }

    fn parse_integer(value: &str) -> std::result::Result<u64, String> {
        let value = value.trim();
        let parsed = if let Some(hex) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            u64::from_str_radix(hex, 16)
        } else {
            value.parse()
        };
        parsed.map_err(|err| format!("invalid integer {value:?}: {err}"))
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
    enum BootMode {
        /// Boot from the primary HDD (`DL=0x80`).
        Hdd,
        /// Boot from the install-media CD-ROM (`DL=0xE0`).
        Cdrom,
        /// Enable firmware "CD-first when present" policy (try `DL=0xE0` when ISO is present, otherwise fall back to HDD).
        CdFirst,
    }

    impl fmt::Display for BootMode {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                BootMode::Hdd => f.write_str("hdd"),
                BootMode::Cdrom => f.write_str("cdrom"),
                BootMode::CdFirst => f.write_str("cd-first"),
            }
        }
    }

    pub fn main() -> Result<()> {
        let args = Args::parse();

        let ram_bytes = args
            .ram
            .checked_mul(1024 * 1024)
            .context("RAM size overflow")?;

        // By default:
        // - If install media + HDD are both present, boot CD once (CD-first policy) and then allow
        //   the guest to reboot into HDD without host-side boot-drive toggling (mirrors the browser
        //   runtime's `vmRuntime="machine"` install flow).
        // - If only install media is present, boot from CD directly (no CD-first toggling).
        let boot_mode = args.boot.unwrap_or_else(|| {
            if args.install_iso.is_some() && args.disk.is_some() {
                BootMode::CdFirst
            } else if args.install_iso.is_some() {
                BootMode::Cdrom
            } else {
                BootMode::Hdd
            }
        });
        if matches!(boot_mode, BootMode::Cdrom | BootMode::CdFirst) && args.install_iso.is_none() {
            bail!("--boot={boot_mode} requires --install-iso");
        }
        if matches!(boot_mode, BootMode::Hdd | BootMode::CdFirst) && args.disk.is_none() {
            bail!("--boot={boot_mode} requires --disk");
        }

        // Use the canonical PC platform defaults so the CLI is useful for full-system boot images.
        if args.cpus == 0 {
            bail!("--cpus must be at least 1");
        }
        if args.guest_cpu_hz == Some(0) {
            bail!("--guest-cpu-hz must be greater than zero");
        }
        if args.cpus > 1 {
            eprintln!(
                "warning: SMP is experimental; use --cpus 1 for Windows 7 boot compatibility"
            );
        }
        let mut cfg = MachineConfig::win7_storage_defaults(ram_bytes);
        cfg.cpu_count = args.cpus;
        let mut machine = Machine::new(cfg).map_err(|e| anyhow!("{e}"))?;
        for (kind, watch) in [
            ("read", args.watch_read_phys.as_ref()),
            ("write", args.watch_phys.as_ref()),
        ] {
            let Some(watch) = watch else {
                continue;
            };
            const MAX_WATCH_BYTES: usize = 4096;
            if watch.length > MAX_WATCH_BYTES {
                bail!(
                    "refusing to watch {kind}s over {} bytes at {:#x}; maximum is {} bytes",
                    watch.length,
                    watch.address,
                    MAX_WATCH_BYTES
                );
            }
            let accepted = match kind {
                "read" => machine.set_physical_read_watchpoint(watch.address, watch.length),
                "write" => machine.set_physical_write_watchpoint(watch.address, watch.length),
                _ => unreachable!(),
            };
            if !accepted {
                bail!(
                    "invalid physical {kind} watch range at {:#x} with length {}",
                    watch.address,
                    watch.length
                );
            }
        }
        if args.watch_phys.is_some() || args.watch_read_phys.is_some() {
            if args.watch_granularity_insts == 0 {
                bail!("--watch-granularity-insts must be greater than zero");
            }
        }

        // Record the host's chosen disk paths in the machine's snapshot overlay refs so snapshots
        // produced by this CLI remain self-describing (even when no explicit COW overlay is used).
        //
        // Note: these refs are metadata only; disk bytes always remain external to the snapshot
        // blob.
        let base_image = args
            .disk
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if let Some(disk) = &args.disk {
            if let Some(overlay) = &args.disk_overlay {
                machine.set_ahci_port0_disk_overlay_ref(
                    base_image.clone(),
                    overlay.display().to_string(),
                );
            } else {
                machine.set_ahci_port0_disk_overlay_ref(base_image.clone(), "");
            }

            let disk_backend = if let Some(overlay) = &args.disk_overlay {
                open_disk_backend_with_overlay(disk, overlay, args.disk_overlay_block_size)?
            } else {
                open_disk_backend(disk, args.disk_ro)?
            };
            machine
                .set_disk_backend(disk_backend)
                .map_err(|e| anyhow!("{e}"))?;
        }

        // Track whether the firmware "CD-first when present" policy is enabled so we can disable it
        // after the first guest-initiated reset (Windows setup reboots into the installed HDD while
        // leaving install media inserted).
        let mut cd_first_enabled: bool;

        if let Some(path) = &args.snapshot_load {
            let mut f = File::open(path)
                .with_context(|| format!("failed to open snapshot for load: {}", path.display()))?;
            machine
                .restore_snapshot_from_checked(&mut f)
                .map_err(|e| anyhow!("{e}"))?;

            // Snapshot disk refs are host-managed metadata. Warn if the snapshot was produced for a
            // different base/overlay path than the current CLI flags.
            if let Some(restored) = machine.restored_disk_overlays() {
                if let Some(primary) = restored
                    .disks
                    .iter()
                    .find(|d| d.disk_id == Machine::DISK_ID_PRIMARY_HDD)
                {
                    if !primary.base_image.is_empty() && primary.base_image != base_image {
                        eprintln!(
                            "warning: snapshot base_image differs from --disk: snapshot={} cli={}",
                            primary.base_image, base_image
                        );
                    }
                    if !primary.overlay_image.is_empty() {
                        match &args.disk_overlay {
                            Some(cli_overlay) => {
                                let cli_overlay = cli_overlay.display().to_string();
                                if primary.overlay_image != cli_overlay {
                                    eprintln!(
                                        "warning: snapshot overlay_image differs from --disk-overlay: snapshot={} cli={}",
                                        primary.overlay_image, cli_overlay
                                    );
                                }
                            }
                            None => {
                                eprintln!(
                                    "warning: snapshot specifies overlay_image {} but CLI did not provide --disk-overlay",
                                    primary.overlay_image
                                );
                            }
                        }
                    }
                }

                if let Some(install_iso) = &args.install_iso {
                    let cli_iso = install_iso.display().to_string();
                    if let Some(cd) = restored
                        .disks
                        .iter()
                        .find(|d| d.disk_id == Machine::DISK_ID_INSTALL_MEDIA)
                    {
                        if !cd.base_image.is_empty() && cd.base_image != cli_iso {
                            eprintln!(
                                "warning: snapshot install-media base_image differs from --install-iso: snapshot={} cli={}",
                                cd.base_image, cli_iso
                            );
                        }
                        if !cd.overlay_image.is_empty() {
                            eprintln!(
                                "warning: snapshot install-media overlay_image is non-empty (expected read-only ISO): {}",
                                cd.overlay_image
                            );
                        }
                    }
                }
            }

            // Storage controller snapshots intentionally drop host backends. Reattach the shared
            // disk so the guest can continue booting after restore.
            machine
                .attach_shared_disk_to_ahci_port0()
                .context("failed to reattach shared disk to AHCI port0")?;
            machine
                .attach_shared_disk_to_virtio_blk()
                .map_err(|e| anyhow!("{e}"))?;

            // If install media is provided, reattach it without changing guest-visible tray state.
            if let Some(iso_path) = &args.install_iso {
                let iso = open_disk_image(iso_path, true)?;
                machine
                    .attach_install_media_iso_for_restore(Box::new(iso))
                    .with_context(|| {
                        format!(
                            "failed to attach install ISO for restore: {}",
                            iso_path.display()
                        )
                    })?;
                machine
                    .set_ide_secondary_master_atapi_overlay_ref(iso_path.display().to_string(), "");
            }

            // Only override the snapshot's boot policy when explicitly requested. Otherwise we keep
            // the restored BIOS config intact.
            if args.boot.is_some() {
                match boot_mode {
                    BootMode::Hdd => {
                        machine.set_boot_from_cd_if_present(false);
                        machine.set_boot_drive(0x80);
                    }
                    BootMode::Cdrom => {
                        machine.set_boot_from_cd_if_present(false);
                        machine.set_boot_drive(0xE0);
                    }
                    BootMode::CdFirst => {
                        machine.set_cd_boot_drive(0xE0);
                        machine.set_boot_from_cd_if_present(true);
                        machine.set_boot_drive(0x80);
                    }
                }
            }
            cd_first_enabled = machine.boot_from_cd_if_present();
        } else {
            // No snapshot restore: attach optional install media and apply boot policy, then reset.
            if let Some(iso_path) = &args.install_iso {
                let iso = open_disk_image(iso_path, true)?;
                machine
                    .attach_install_media_iso_and_set_overlay_ref(
                        Box::new(iso),
                        iso_path.display().to_string(),
                    )
                    .with_context(|| {
                        format!("failed to attach install ISO: {}", iso_path.display())
                    })?;
            }

            match boot_mode {
                BootMode::Hdd => {
                    machine.set_boot_from_cd_if_present(false);
                    machine.set_boot_drive(0x80);
                }
                BootMode::Cdrom => {
                    machine.set_boot_from_cd_if_present(false);
                    machine.set_boot_drive(0xE0);
                }
                BootMode::CdFirst => {
                    machine.set_cd_boot_drive(0xE0);
                    machine.set_boot_from_cd_if_present(true);
                    machine.set_boot_drive(0x80);
                }
            }
            cd_first_enabled = machine.boot_from_cd_if_present();

            // `Machine::new` performs an initial BIOS POST + boot attempt. Re-run POST after
            // attaching disks and configuring boot policy so the guest starts executing from the
            // selected boot device.
            machine.reset();
        }

        if let Some(hz) = args.guest_cpu_hz {
            for cpu_index in 0..machine.cpu_count() {
                machine.cpu_core_mut_by_index(cpu_index).time.set_tsc_hz(hz);
            }
            eprintln!(
                "warning: diagnostic guest TSC frequency override active: {hz} Hz; timing is not representative"
            );
        }

        let mut serial_sink = open_serial_sink(&args.serial_out)?;
        let mut debugcon_sink = open_optional_sink(&args.debugcon_out)?;

        let start = Instant::now();
        let mut total_executed: u64 = 0;
        let mut watch_event_count: u64 = 0;
        let mut dropped_watch_event_count: u64 = 0;
        let mut physical_patch_applied = false;
        let mut run_error: Option<anyhow::Error> = None;
        report_physical_writes(
            &mut machine,
            0,
            0,
            &mut watch_event_count,
            &mut dropped_watch_event_count,
        );
        report_physical_reads(
            &mut machine,
            0,
            0,
            &mut watch_event_count,
            &mut dropped_watch_event_count,
        );

        loop {
            if let Some(patch) = &args.patch_phys_u32_at {
                if !physical_patch_applied && total_executed == patch.instructions {
                    machine.write_physical_u32(patch.address, patch.value);
                    physical_patch_applied = true;
                    eprintln!(
                        "debug physical patch applied: instructions={} paddr={:#x} value={:#010x}",
                        total_executed, patch.address, patch.value
                    );
                }
            }
            let interval_start = total_executed;
            let exit = if let Some(max_insts) = args.max_insts {
                if total_executed >= max_insts {
                    break;
                }
                let budget = debug_slice_budget(
                    &args,
                    total_executed,
                    max_insts - total_executed,
                    physical_patch_applied,
                );
                machine.run_slice(budget)
            } else {
                let max_ms = args
                    .max_ms
                    .expect("clap enforces that one of max_insts/max_ms is set");
                if start.elapsed() >= Duration::from_millis(max_ms) {
                    break;
                }
                let budget = debug_slice_budget(
                    &args,
                    total_executed,
                    SLICE_INST_BUDGET,
                    physical_patch_applied,
                );
                machine.run_slice(budget)
            };

            total_executed = total_executed.saturating_add(exit.executed());
            let previous_watch_event_count = watch_event_count;
            report_physical_writes(
                &mut machine,
                interval_start,
                total_executed,
                &mut watch_event_count,
                &mut dropped_watch_event_count,
            );
            report_physical_reads(
                &mut machine,
                interval_start,
                total_executed,
                &mut watch_event_count,
                &mut dropped_watch_event_count,
            );
            if args.watch_stop && watch_event_count != previous_watch_event_count {
                eprintln!("physical watch requested stop after {total_executed} instructions");
                break;
            }
            stream_serial(&mut machine, &mut serial_sink)?;
            if let Some(out) = debugcon_sink.as_mut() {
                stream_debugcon(&mut machine, out)?;
            }

            match handle_exit(&mut machine, exit, total_executed, &mut cd_first_enabled) {
                Ok(LoopControl::Continue) => continue,
                Ok(LoopControl::Break) => break,
                Err(e) => {
                    run_error = Some(e);
                    break;
                }
            }
        }

        eprintln!(
            "run summary: instructions={} elapsed_ms={} cpus={} configured_boot={:?} active_boot={:?} {}",
            total_executed,
            start.elapsed().as_millis(),
            machine.cpu_count(),
            machine.boot_device(),
            machine.active_boot_device(),
            cpu_diagnostic(&mut machine)
        );
        inspect_physical_ranges(&mut machine, &args.inspect_phys)?;
        dump_physical_ranges(&mut machine, &args.dump_phys)?;
        if args.watch_phys.is_some() || args.watch_read_phys.is_some() {
            eprintln!(
                "physical watch summary: events={} dropped={}",
                watch_event_count, dropped_watch_event_count
            );
        }

        // Flush any remaining serial bytes.
        stream_serial(&mut machine, &mut serial_sink)?;
        if let Err(e) = serial_sink.flush() {
            if run_error.is_some() {
                eprintln!("warning: failed to flush serial output: {e}");
            } else {
                return Err(e.into());
            }
        }
        if let Some(out) = debugcon_sink.as_mut() {
            stream_debugcon(&mut machine, out)?;
            if let Err(e) = out.flush() {
                if run_error.is_some() {
                    eprintln!("warning: failed to flush debugcon output: {e}");
                } else {
                    return Err(e.into());
                }
            }
        }

        if let Some(path) = &args.snapshot_save {
            let mut f = File::create(path).with_context(|| {
                format!(
                    "failed to create snapshot file for save: {}",
                    path.display()
                )
            })?;
            if let Err(e) = machine
                .save_snapshot_full_to(&mut f)
                .map_err(|e| anyhow!("{e}"))
            {
                if run_error.is_some() {
                    eprintln!(
                        "warning: failed to save snapshot to {}: {e}",
                        path.display()
                    );
                } else {
                    return Err(e);
                }
            }
        }

        if let Some(path) = &args.vga_png {
            if let Err(e) = dump_vga_png(&mut machine, path) {
                if run_error.is_some() {
                    eprintln!("warning: failed to dump VGA PNG to {}: {e}", path.display());
                } else {
                    return Err(e);
                }
            }
        }

        if let Some(e) = run_error {
            return Err(e);
        }

        Ok(())
    }

    fn open_disk_image(path: &Path, read_only: bool) -> Result<DiskImage<StdFileBackend>> {
        let backend = if read_only {
            StdFileBackend::open_read_only(path)
        } else {
            StdFileBackend::open_rw(path)
        }
        .map_err(|e| anyhow!("failed to open disk image {}: {e}", path.display()))?;

        let disk = DiskImage::open_auto(backend)
            .map_err(|e| anyhow!("failed to open disk image {}: {e}", path.display()))?;

        let capacity = disk.capacity_bytes();
        if capacity == 0 {
            bail!(
                "disk image is empty (expected at least one {}-byte sector)",
                SECTOR_SIZE
            );
        }
        if capacity % SECTOR_SIZE as u64 != 0 {
            bail!(
                "disk image capacity {} is not a multiple of {} bytes",
                capacity,
                SECTOR_SIZE
            );
        }

        Ok(disk)
    }

    fn open_disk_backend(path: &Path, read_only: bool) -> Result<Box<dyn VirtualDisk>> {
        let disk = open_disk_image(path, read_only)?;
        Ok(Box::new(disk))
    }

    fn open_disk_backend_with_overlay(
        base_path: &Path,
        overlay_path: &Path,
        create_block_size: u32,
    ) -> Result<Box<dyn VirtualDisk>> {
        let base = open_disk_image(base_path, true)?;

        let overlay_exists = overlay_path.exists();
        let overlay_backend = if overlay_exists {
            StdFileBackend::open_rw(overlay_path)
        } else {
            StdFileBackend::create(overlay_path, 0)
        }
        .map_err(|e| {
            anyhow!(
                "failed to open overlay image {}: {e}",
                overlay_path.display()
            )
        })?;

        let cow = if overlay_exists {
            AeroCowDisk::open(base, overlay_backend)
        } else {
            AeroCowDisk::create(base, overlay_backend, create_block_size)
        }
        .map_err(|e| anyhow!("failed to initialize COW overlay disk: {e}"))?;

        Ok(Box::new(cow))
    }

    fn open_serial_sink(serial_out: &str) -> Result<Box<dyn Write>> {
        if serial_out == "stdout" {
            return Ok(Box::new(io::stdout()));
        }
        let f = File::create(serial_out)
            .with_context(|| format!("failed to create serial output file: {serial_out}"))?;
        Ok(Box::new(BufWriter::new(f)))
    }

    fn open_optional_sink(dest: &str) -> Result<Option<Box<dyn Write>>> {
        if dest == "none" {
            return Ok(None);
        }
        Ok(Some(open_serial_sink(dest)?))
    }

    fn stream_serial(machine: &mut Machine, out: &mut dyn Write) -> Result<()> {
        let bytes = machine.take_serial_output();
        if !bytes.is_empty() {
            out.write_all(&bytes)?;
        }
        Ok(())
    }

    fn stream_debugcon(machine: &mut Machine, out: &mut dyn Write) -> Result<()> {
        let bytes = machine.take_debugcon_output();
        if !bytes.is_empty() {
            out.write_all(&bytes)?;
        }
        Ok(())
    }

    fn inspect_physical_ranges(machine: &mut Machine, ranges: &[PhysicalRange]) -> Result<()> {
        const MAX_INSPECT_BYTES: usize = 1024 * 1024;
        for range in ranges {
            if range.length > MAX_INSPECT_BYTES {
                bail!(
                    "refusing to inspect {} bytes at {:#x}; maximum is {} bytes",
                    range.length,
                    range.address,
                    MAX_INSPECT_BYTES
                );
            }
            let bytes = machine.read_physical_bytes(range.address, range.length);
            let formatted = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!(
                "physical memory [{:#x}..{:#x}) = [{}]",
                range.address,
                range.address + range.length as u64,
                formatted
            );
        }
        Ok(())
    }

    fn dump_physical_ranges(machine: &mut Machine, dumps: &[PhysicalDump]) -> Result<()> {
        const MAX_DUMP_BYTES: usize = 256 * 1024 * 1024;
        for dump in dumps {
            if dump.range.length > MAX_DUMP_BYTES {
                bail!(
                    "refusing to dump {} bytes at {:#x}; maximum is {} bytes",
                    dump.range.length,
                    dump.range.address,
                    MAX_DUMP_BYTES
                );
            }
            let bytes = machine.read_physical_bytes(dump.range.address, dump.range.length);
            let mut file = File::create(&dump.path).with_context(|| {
                format!(
                    "failed to create physical memory dump {}",
                    dump.path.display()
                )
            })?;
            file.write_all(&bytes).with_context(|| {
                format!(
                    "failed to write physical memory dump {}",
                    dump.path.display()
                )
            })?;
            eprintln!(
                "dumped physical memory [{:#x}..{:#x}) to {}",
                dump.range.address,
                dump.range.address + dump.range.length as u64,
                dump.path.display()
            );
        }
        Ok(())
    }

    fn report_physical_writes(
        machine: &mut Machine,
        interval_start: u64,
        interval_end: u64,
        event_count: &mut u64,
        dropped_event_count: &mut u64,
    ) {
        for event in machine.take_physical_write_events() {
            *event_count = event_count.saturating_add(1);
            eprintln!(
                "physical write event: instructions=({interval_start},{interval_end}] paddr={:#x} previous=[{}] current=[{}]",
                event.paddr,
                format_bytes(&event.previous),
                format_bytes(&event.current)
            );
        }
        let dropped = machine.take_dropped_physical_write_event_count();
        if dropped != 0 {
            *dropped_event_count = dropped_event_count.saturating_add(dropped);
            eprintln!(
                "warning: dropped {dropped} physical write events in instruction interval ({interval_start},{interval_end}]"
            );
        }
    }

    fn report_physical_reads(
        machine: &mut Machine,
        interval_start: u64,
        interval_end: u64,
        event_count: &mut u64,
        dropped_event_count: &mut u64,
    ) {
        for event in machine.take_physical_read_events() {
            *event_count = event_count.saturating_add(1);
            eprintln!(
                "physical read event: instructions=({interval_start},{interval_end}] paddr={:#x} bytes=[{}]",
                event.paddr,
                format_bytes(&event.bytes)
            );
        }
        let dropped = machine.take_dropped_physical_read_event_count();
        if dropped != 0 {
            *dropped_event_count = dropped_event_count.saturating_add(dropped);
            eprintln!(
                "warning: dropped {dropped} physical read events in instruction interval ({interval_start},{interval_end}]"
            );
        }
    }

    fn format_bytes(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn debug_slice_budget(
        args: &Args,
        total_executed: u64,
        default_budget: u64,
        physical_patch_applied: bool,
    ) -> u64 {
        let mut budget = default_budget;
        if let Some(patch) = &args.patch_phys_u32_at {
            if !physical_patch_applied && total_executed < patch.instructions {
                budget = budget.min(patch.instructions - total_executed);
            }
        }
        if args.watch_phys.is_none() && args.watch_read_phys.is_none() {
            return budget;
        }
        if total_executed < args.watch_after_insts {
            return budget.min(args.watch_after_insts - total_executed);
        }
        budget.min(args.watch_granularity_insts)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LoopControl {
        Continue,
        Break,
    }

    fn handle_exit(
        machine: &mut Machine,
        exit: RunExit,
        total_executed: u64,
        cd_first_enabled: &mut bool,
    ) -> Result<LoopControl> {
        match exit {
            RunExit::Completed { .. } => Ok(LoopControl::Continue),
            RunExit::Halted { .. } => {
                eprintln!("guest halted after {total_executed} instructions");
                Ok(LoopControl::Break)
            }
            RunExit::ResetRequested { kind, .. } => {
                // When using the "CD-first when present" policy, Windows setup commonly boots from
                // CD once, then reboots into the installed HDD while leaving the ISO attached.
                // Disable the policy after the first guest reset so setup does not loop back into
                // install media.
                if *cd_first_enabled && machine.active_boot_device() == BootDevice::Cdrom {
                    eprintln!(
                        "guest requested reset: {kind:?} (disabling CD-first policy; booting HDD next)"
                    );
                    machine.set_boot_from_cd_if_present(false);
                    machine.set_boot_drive(0x80);
                    *cd_first_enabled = false;
                } else {
                    eprintln!("guest requested reset: {kind:?} (continuing)");
                }
                machine.reset();
                Ok(LoopControl::Continue)
            }
            RunExit::Assist { reason, .. } => {
                bail!(
                    "execution stopped: assist required: {reason:?}; {}",
                    cpu_diagnostic(machine)
                )
            }
            RunExit::Exception { exception, .. } => {
                bail!(
                    "execution stopped: exception: {exception:?}; {}",
                    cpu_diagnostic(machine)
                )
            }
            RunExit::CpuExit { exit, .. } => bail!(
                "execution stopped: cpu exit: {exit:?}; {}",
                cpu_diagnostic(machine)
            ),
        }
    }

    fn cpu_diagnostic(machine: &mut Machine) -> String {
        let state = machine.cpu().clone();
        let linear_ip = state.segments.cs.base.wrapping_add(state.rip());
        let linear_sp = state.segments.ss.base.wrapping_add(state.stack_ptr());
        let instruction_bytes = if state.control.cr0 & (1 << 31) == 0 {
            let bytes = machine.read_physical_bytes(linear_ip, 16);
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::from("<paging enabled; linear-to-physical translation unavailable>")
        };
        let stack_bytes = if state.control.cr0 & (1 << 31) == 0 {
            let bytes = machine.read_physical_bytes(linear_sp, 32);
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::from("<paging enabled; linear-to-physical translation unavailable>")
        };
        format!(
            "cpu={{mode={:?} cs={:#06x} cs_base={:#018x} rip={:#018x} linear_ip={:#018x} \
             ss={:#06x} ss_base={:#018x} ds={:#06x} ds_base={:#018x} \
             es={:#06x} es_base={:#018x} fs={:#06x} fs_base={:#018x} \
             gs={:#06x} gs_base={:#018x} \
             rflags={:#018x} cr0={:#018x} cr3={:#018x} cr4={:#018x} efer={:#018x} \
             gdtr_base={:#018x} gdtr_limit={:#06x} idtr_base={:#018x} idtr_limit={:#06x} \
             rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x} rsi={:#018x} \
             rdi={:#018x} rbp={:#018x} rsp={:#018x} linear_sp={:#018x} \
             bytes=[{}] stack=[{}]}}",
            state.mode,
            state.segments.cs.selector,
            state.segments.cs.base,
            state.rip(),
            linear_ip,
            state.segments.ss.selector,
            state.segments.ss.base,
            state.segments.ds.selector,
            state.segments.ds.base,
            state.segments.es.selector,
            state.segments.es.base,
            state.segments.fs.selector,
            state.segments.fs.base,
            state.segments.gs.selector,
            state.segments.gs.base,
            state.rflags_snapshot(),
            state.control.cr0,
            state.control.cr3,
            state.control.cr4,
            state.msr.efer,
            state.tables.gdtr.base,
            state.tables.gdtr.limit,
            state.tables.idtr.base,
            state.tables.idtr.limit,
            state.gpr[0],
            state.gpr[3],
            state.gpr[1],
            state.gpr[2],
            state.gpr[6],
            state.gpr[7],
            state.gpr[5],
            state.gpr[4],
            linear_sp,
            instruction_bytes,
            stack_bytes
        )
    }

    fn dump_vga_png(machine: &mut Machine, path: &Path) -> Result<()> {
        machine.display_present();
        let (w, h) = machine.display_resolution();
        if w == 0 || h == 0 {
            bail!("no VGA framebuffer available (resolution was {w}x{h})");
        }

        let fb = machine.display_framebuffer();
        let expected_len = (w as usize)
            .checked_mul(h as usize)
            .context("framebuffer size overflow")?;
        if fb.len() != expected_len {
            bail!(
                "unexpected framebuffer length: got {}, expected {} ({w}x{h})",
                fb.len(),
                expected_len
            );
        }

        // `aero_gpu_vga` framebuffer pixels are u32 with little-endian RGBA byte order:
        //   value = R | (G<<8) | (B<<16) | (A<<24)
        // Convert to an explicit RGBA byte buffer for the `image` crate.
        let mut rgba = Vec::with_capacity(fb.len() * 4);
        for &p in fb {
            rgba.push((p & 0xFF) as u8); // R
            rgba.push(((p >> 8) & 0xFF) as u8); // G
            rgba.push(((p >> 16) & 0xFF) as u8); // B
            rgba.push(((p >> 24) & 0xFF) as u8); // A
        }

        let img =
            image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| anyhow!("invalid image data"))?;
        img.save(path)
            .with_context(|| format!("failed to write PNG: {}", path.display()))?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{parse_integer, PhysicalDump, PhysicalRange, PhysicalU32Patch};

        #[test]
        fn physical_range_accepts_decimal_and_hex() {
            let decimal: PhysicalRange = "4096:16".parse().unwrap();
            assert_eq!(decimal.address, 4096);
            assert_eq!(decimal.length, 16);

            let hex: PhysicalRange = "0x495e08:0x10".parse().unwrap();
            assert_eq!(hex.address, 0x495e08);
            assert_eq!(hex.length, 16);
        }

        #[test]
        fn physical_range_rejects_invalid_or_overflowing_values() {
            assert!("4096".parse::<PhysicalRange>().is_err());
            assert!("0x1000:0".parse::<PhysicalRange>().is_err());
            assert!("0xffffffffffffffff:2".parse::<PhysicalRange>().is_err());
            assert!(parse_integer("0xnot-hex").is_err());
        }

        #[test]
        fn physical_dump_keeps_colons_in_output_path() {
            let dump: PhysicalDump = "0x1000:32:/tmp/a:b.bin".parse().unwrap();
            assert_eq!(dump.range.address, 0x1000);
            assert_eq!(dump.range.length, 32);
            assert_eq!(dump.path.to_string_lossy(), "/tmp/a:b.bin");
        }

        #[test]
        fn physical_u32_patch_parses_and_checks_width() {
            let patch: PhysicalU32Patch = "20376805:0x495e08:0x252f8".parse().unwrap();
            assert_eq!(patch.instructions, 20_376_805);
            assert_eq!(patch.address, 0x495e08);
            assert_eq!(patch.value, 0x252f8);
            assert!("1:2:0x100000000".parse::<PhysicalU32Patch>().is_err());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    native::main()
}

//! Native macOS frontend for the canonical Aero machine.
//!
//! This binary deliberately owns presentation rather than reusing the browser presenter.  It is
//! therefore a small, independently-testable proof that the host renderer is native `wgpu` on
//! Metal.  The machine remains on the UI thread for now because `Machine` contains `Rc` device
//! state and is intentionally not `Send`; bounded `run_slice` calls keep the event loop pumping.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aero_machine::{BootDevice, Machine, MachineConfig, Ps2MouseButton, RunExit};
use aero_storage::{DiskImage, StdFileBackend, VirtualDisk, SECTOR_SIZE};
use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowBuilder};

const MACHINE_SLICE_INSTRUCTIONS: u64 = 20_000;

#[derive(Debug, Parser)]
#[command(about = "Native ARM64 macOS frontend for Aero (Metal-only wgpu)")]
struct Args {
    /// Disk image to attach as the primary HDD (raw/qcow2/vhd/aerospar; auto-detected).
    #[arg(long)]
    disk: Option<PathBuf>,

    /// Windows install/recovery ISO to attach as the canonical ATAPI CD-ROM.
    #[arg(long)]
    install_iso: Option<PathBuf>,

    /// BIOS boot policy. Defaults to HDD, CD-ROM, or CD-first based on attached media.
    #[arg(long, value_enum)]
    boot: Option<BootMode>,

    /// Guest memory in MiB.
    #[arg(long, default_value_t = 512)]
    memory: u64,

    /// Number of guest vCPUs. Keep this at 1 for Windows boots; SMP is still experimental.
    #[arg(long, default_value_t = 1)]
    cpus: u8,

    /// Only render the host Metal triangle; do not start the emulated machine.
    #[arg(long)]
    host_triangle: bool,

    /// Enumerate Metal adapters and exit without creating a window.
    #[arg(long)]
    list_gpu: bool,

    /// Permit adapter fallback. Disabled by default so software rendering cannot be mistaken for Metal.
    #[arg(long)]
    allow_fallback_adapter: bool,

    /// Install Aero's existing native wgpu command executor into the AeroGPU PCI device.
    #[arg(long)]
    aerogpu_wgpu: bool,

    /// Disable the AeroGPU PCI device and use the legacy VGA device instead.
    #[arg(long)]
    no_aerogpu: bool,

    /// Maximum host runtime in milliseconds; useful for reproducible smoke boots.
    #[arg(long)]
    max_ms: Option<u64>,

    /// Rust tracing filter (for example, `debug` or `aero_machine=trace`).
    #[arg(long, default_value = "info")]
    log_level: String,

    #[arg(long)]
    trace_pci: bool,
    #[arg(long)]
    trace_mmio: bool,
    #[arg(long)]
    trace_gpu_commands: bool,
    #[arg(long)]
    trace_fences: bool,
    #[arg(long)]
    trace_vblank: bool,
    #[arg(long)]
    trace_scanout: bool,
    #[arg(long)]
    trace_shared_surfaces: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum BootMode {
    /// Boot from the primary HDD.
    Hdd,
    /// Boot directly from the install-media CD-ROM.
    Cdrom,
    /// Try the CD-ROM when present, then fall back to the HDD.
    CdFirst,
}

#[derive(Clone, Copy, Debug, Default)]
struct TraceOptions {
    pci: bool,
    mmio: bool,
    gpu_commands: bool,
    fences: bool,
    vblank: bool,
    scanout: bool,
    shared_surfaces: bool,
}

impl From<&Args> for TraceOptions {
    fn from(args: &Args) -> Self {
        Self {
            pci: args.trace_pci,
            mmio: args.trace_mmio,
            gpu_commands: args.trace_gpu_commands,
            fences: args.trace_fences,
            vblank: args.trace_vblank,
            scanout: args.trace_scanout,
            shared_surfaces: args.trace_shared_surfaces,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&args.log_level)?)
        .with_target(false)
        .init();

    if args.list_gpu {
        list_metal_adapters(args.allow_fallback_adapter);
        return Ok(());
    }
    run(args)
}

fn list_metal_adapters(allow_fallback: bool) {
    let instance = metal_instance();
    let adapters = instance.enumerate_adapters(wgpu::Backends::METAL);
    if adapters.is_empty() {
        println!("No Metal adapters found (software fallback allowed: {allow_fallback}).");
        return;
    }
    for (idx, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        let limits = adapter.limits();
        let features = adapter.features();
        println!("adapter[{idx}]: {}", info.name);
        println!(
            "  backend={:?} type={:?} vendor=0x{:04x} device=0x{:04x}",
            info.backend, info.device_type, info.vendor, info.device
        );
        println!("  driver={} ({})", info.driver, info.driver_info);
        println!(
            "  fallback_requested={allow_fallback} (wgpu 0.20 does not expose adapter fallback status)"
        );
        println!(
            "  max_texture_dimension_2d={} max_buffer_size={}",
            limits.max_texture_dimension_2d, limits.max_buffer_size
        );
        println!(
            "  timestamp_query={}",
            features.contains(wgpu::Features::TIMESTAMP_QUERY)
        );
        println!("  features={features:?}");
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            println!(
                "  format={format:?} usages={:?}",
                adapter.get_texture_format_features(format).allowed_usages
            );
        }
    }
}

fn metal_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..Default::default()
    })
}

fn run(args: Args) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let window = std::sync::Arc::new(
        WindowBuilder::new()
            .with_title("Aero macOS — Metal")
            .with_inner_size(PhysicalSize::new(1280, 800))
            .build(&event_loop)?,
    );
    let mut renderer =
        pollster::block_on(Renderer::new(window.clone(), args.allow_fallback_adapter))?;
    let trace = TraceOptions::from(&args);
    let mut machine = (!args.host_triangle)
        .then(|| create_machine(&args, trace))
        .transpose()?;
    let mut cd_first_enabled = machine
        .as_ref()
        .is_some_and(|machine| machine.boot_from_cd_if_present());
    let started = Instant::now();
    let deadline = args.max_ms.map(|ms| started + Duration::from_millis(ms));
    let mut last_cursor = None;
    let mut last_trace = Instant::now();
    let mut exiting = false;

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => renderer.resize(size),
                WindowEvent::RedrawRequested => {
                    let result = if let Some(machine) = machine.as_mut() {
                        machine.display_present();
                        let (width, height) = machine.display_resolution();
                        renderer.render_framebuffer(width, height, machine.display_framebuffer())
                    } else {
                        renderer.render_triangle()
                    };
                    if let Err(err) = result {
                        tracing::error!("presentation failed: {err:#}");
                        target.exit();
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let (Some(machine), Some(code)) =
                        (machine.as_mut(), browser_code(&event.physical_key))
                    {
                        machine.inject_browser_key(code, event.state == ElementState::Pressed);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Some(machine) = machine.as_mut() {
                        if let Some((x, y)) = last_cursor.replace((position.x, position.y)) {
                            machine.inject_mouse_motion(
                                (position.x - x) as i32,
                                (position.y - y) as i32,
                                0,
                            );
                        }
                    }
                }
                WindowEvent::CursorLeft { .. } => last_cursor = None,
                WindowEvent::MouseInput { state, button, .. } => {
                    if let (Some(machine), Some(button)) = (machine.as_mut(), map_button(button)) {
                        machine.inject_mouse_button(button, state == ElementState::Pressed);
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if let Some(machine) = machine.as_mut() {
                        let wheel = match delta {
                            MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                            MouseScrollDelta::PixelDelta(p) => (p.y / 16.0).round() as i32,
                        };
                        machine.inject_mouse_motion(0, 0, wheel);
                    }
                }
                _ => {}
            },
            Event::AboutToWait => {
                if deadline.is_some_and(|end| Instant::now() >= end) {
                    if !exiting {
                        tracing::info!("maximum runtime reached");
                        exiting = true;
                    }
                    target.exit();
                    return;
                }
                if let Some(machine) = machine.as_mut() {
                    let exit = machine.run_slice(MACHINE_SLICE_INSTRUCTIONS);
                    match exit {
                        RunExit::Completed { .. } => {}
                        RunExit::Halted { .. } => {
                            // Keep polling devices: a pending timer or input interrupt can wake HLT.
                            tracing::trace!("guest halted; continuing device polling");
                        }
                        RunExit::ResetRequested { kind, .. } => {
                            if cd_first_enabled
                                && machine.active_boot_device() == BootDevice::Cdrom
                            {
                                tracing::info!(
                                    ?kind,
                                    "guest reset after CD boot; disabling CD-first policy and booting HDD"
                                );
                                machine.set_boot_from_cd_if_present(false);
                                machine.set_boot_drive(0x80);
                                cd_first_enabled = false;
                            } else {
                                tracing::info!(?kind, "guest requested reset");
                            }
                            machine.reset();
                        }
                        RunExit::Assist { reason, .. } => {
                            let cpu = machine.cpu();
                            tracing::error!(
                                ?reason,
                                mode = ?cpu.mode,
                                cs = cpu.segments.cs.selector,
                                rip = cpu.rip(),
                                "unhandled CPU assist"
                            );
                            target.exit();
                            return;
                        }
                        RunExit::Exception { exception, .. } => {
                            let cpu = machine.cpu();
                            tracing::error!(
                                ?exception,
                                mode = ?cpu.mode,
                                cs = cpu.segments.cs.selector,
                                rip = cpu.rip(),
                                "guest execution stopped on CPU exception"
                            );
                            target.exit();
                            return;
                        }
                        RunExit::CpuExit { exit, .. } => {
                            let cpu = machine.cpu();
                            tracing::error!(
                                ?exit,
                                mode = ?cpu.mode,
                                cs = cpu.segments.cs.selector,
                                rip = cpu.rip(),
                                "guest execution stopped on fatal CPU exit"
                            );
                            target.exit();
                            return;
                        }
                    }
                    trace_machine_state(machine, trace, &mut last_trace);
                }
                window.request_redraw();
            }
            _ => {}
        }
    })?;
    Ok(())
}

fn create_machine(args: &Args, trace: TraceOptions) -> Result<Machine> {
    let boot_mode = resolve_boot_mode(args.disk.is_some(), args.install_iso.is_some(), args.boot)?;
    if args.cpus == 0 {
        bail!("--cpus must be at least 1");
    }
    if args.cpus > 1 {
        tracing::warn!(
            cpus = args.cpus,
            "SMP is experimental; use --cpus 1 for Windows 7 boot compatibility"
        );
    }
    let ram_bytes = args
        .memory
        .checked_mul(1024 * 1024)
        .context("memory size overflow")?;
    let mut cfg = if args.no_aerogpu {
        MachineConfig::win7_storage_defaults(ram_bytes)
    } else {
        MachineConfig::win7_graphics(ram_bytes)
    };
    cfg.cpu_count = args.cpus;
    let mut machine = Machine::new(cfg).map_err(|err| anyhow!(err))?;
    if let Some(path) = &args.disk {
        machine
            .set_disk_backend(open_disk(path, false)?)
            .map_err(|err| anyhow!(err))?;
    } else if args.install_iso.is_none() {
        tracing::warn!("no --disk supplied; starting firmware with an empty primary disk");
    }
    if let Some(path) = &args.install_iso {
        machine
            .attach_install_media_iso_and_set_overlay_ref(
                open_disk(path, true)?,
                path.display().to_string(),
            )
            .with_context(|| format!("failed to attach install ISO {}", path.display()))?;
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
    // Machine::new performs BIOS POST immediately. Attached media and the selected boot policy are
    // therefore not visible until reset re-runs POST.
    machine.reset();
    tracing::info!(
        configured_boot = ?machine.boot_device(),
        active_boot = ?machine.active_boot_device(),
        cpus = machine.cpu_count(),
        disk = ?args.disk,
        install_iso = ?args.install_iso,
        "machine boot media configured"
    );
    if args.aerogpu_wgpu {
        if args.no_aerogpu {
            bail!("--aerogpu-wgpu conflicts with --no-aerogpu");
        }
        machine
            .aerogpu_set_backend_wgpu()
            .map_err(|err| anyhow!(err))?;
        tracing::info!("installed existing AeroGPU native wgpu command backend");
    } else if !args.no_aerogpu {
        machine.aerogpu_set_backend_immediate();
    }
    if trace.pci {
        tracing::info!(
            "AeroGPU PCI: bdf={:?} bar0={:#x?} bar1={:#x?}",
            machine.aerogpu_bdf(),
            machine.aerogpu_bar0_base(),
            machine.aerogpu_vram_bar_base()
        );
    }
    Ok(machine)
}

fn resolve_boot_mode(
    has_disk: bool,
    has_install_iso: bool,
    requested: Option<BootMode>,
) -> Result<BootMode> {
    let mode = requested.unwrap_or(match (has_disk, has_install_iso) {
        (true, true) => BootMode::CdFirst,
        (false, true) => BootMode::Cdrom,
        _ => BootMode::Hdd,
    });
    if matches!(mode, BootMode::Cdrom | BootMode::CdFirst) && !has_install_iso {
        bail!(
            "--boot {} requires --install-iso",
            mode.to_possible_value().unwrap().get_name()
        );
    }
    if matches!(mode, BootMode::Hdd | BootMode::CdFirst) && !has_disk {
        bail!(
            "--boot {} requires --disk",
            mode.to_possible_value().unwrap().get_name()
        );
    }
    Ok(mode)
}

fn open_disk(path: &Path, read_only: bool) -> Result<Box<dyn VirtualDisk>> {
    let backend = if read_only {
        StdFileBackend::open_read_only(path)
    } else {
        StdFileBackend::open_rw(path).or_else(|_| StdFileBackend::open_read_only(path))
    }
    .map_err(|err| anyhow!("failed to open disk {}: {err}", path.display()))?;
    let disk = DiskImage::open_auto(backend)
        .map_err(|err| anyhow!("failed to inspect disk {}: {err}", path.display()))?;
    let capacity = disk.capacity_bytes();
    if capacity == 0 || capacity % SECTOR_SIZE as u64 != 0 {
        bail!(
            "disk {} has invalid capacity {capacity}; expected a non-zero 512-byte multiple",
            path.display()
        );
    }
    Ok(Box::new(disk))
}

#[cfg(test)]
mod tests {
    use super::{create_machine, resolve_boot_mode, Args, BootMode, TraceOptions};

    #[test]
    fn boot_mode_defaults_follow_attached_media() {
        assert_eq!(resolve_boot_mode(true, false, None).unwrap(), BootMode::Hdd);
        assert_eq!(
            resolve_boot_mode(false, true, None).unwrap(),
            BootMode::Cdrom
        );
        assert_eq!(
            resolve_boot_mode(true, true, None).unwrap(),
            BootMode::CdFirst
        );
    }

    #[test]
    fn zero_vcpus_is_rejected_before_machine_creation() {
        let args = Args {
            disk: None,
            install_iso: Some("/does/not/matter.iso".into()),
            boot: Some(BootMode::Cdrom),
            memory: 512,
            cpus: 0,
            host_triangle: false,
            list_gpu: false,
            allow_fallback_adapter: false,
            aerogpu_wgpu: false,
            no_aerogpu: false,
            max_ms: Some(1),
            log_level: "info".into(),
            trace_pci: false,
            trace_mmio: false,
            trace_gpu_commands: false,
            trace_fences: false,
            trace_vblank: false,
            trace_scanout: false,
            trace_shared_surfaces: false,
        };
        let err = match create_machine(&args, TraceOptions::default()) {
            Ok(_) => panic!("zero vCPUs unexpectedly accepted"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("--cpus must be at least 1"));
    }

    #[test]
    fn boot_mode_rejects_missing_required_media() {
        assert!(resolve_boot_mode(true, false, Some(BootMode::Cdrom)).is_err());
        assert!(resolve_boot_mode(false, true, Some(BootMode::Hdd)).is_err());
        assert!(resolve_boot_mode(false, false, Some(BootMode::CdFirst)).is_err());
    }
}

fn trace_machine_state(machine: &Machine, trace: TraceOptions, last: &mut Instant) {
    if *last + Duration::from_secs(1) > Instant::now() {
        return;
    }
    *last = Instant::now();
    if trace.mmio {
        tracing::debug!(bar1_reads = ?machine.aerogpu_bar1_mmio_read_count(), "aerogpu MMIO summary");
    }
    if trace.scanout {
        tracing::debug!(resolution = ?machine.display_resolution(), "scanout summary");
    }
    if trace.gpu_commands {
        tracing::debug!(
            "AeroGPU command tracing is owned by the command backend; set RUST_LOG=aero_gpu=trace"
        );
    }
    if trace.fences {
        tracing::debug!(
            "fence state is exposed by the AeroGPU MMIO model; set RUST_LOG=aero_machine=trace"
        );
    }
    if trace.vblank {
        tracing::debug!(
            "vblank state is exposed by the AeroGPU MMIO model; set RUST_LOG=aero_machine=trace"
        );
    }
    if trace.shared_surfaces {
        tracing::debug!(
            "shared-surface protocol tracing is not yet emitted by the native frontend"
        );
    }
}

fn map_button(button: MouseButton) -> Option<Ps2MouseButton> {
    match button {
        MouseButton::Left => Some(Ps2MouseButton::Left),
        MouseButton::Right => Some(Ps2MouseButton::Right),
        MouseButton::Middle => Some(Ps2MouseButton::Middle),
        MouseButton::Back => Some(Ps2MouseButton::Side),
        MouseButton::Forward => Some(Ps2MouseButton::Extra),
        MouseButton::Other(_) => None,
    }
}

fn browser_code(key: &PhysicalKey) -> Option<&'static str> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        KeyCode::KeyA => "KeyA",
        KeyCode::KeyB => "KeyB",
        KeyCode::KeyC => "KeyC",
        KeyCode::KeyD => "KeyD",
        KeyCode::KeyE => "KeyE",
        KeyCode::KeyF => "KeyF",
        KeyCode::KeyG => "KeyG",
        KeyCode::KeyH => "KeyH",
        KeyCode::KeyI => "KeyI",
        KeyCode::KeyJ => "KeyJ",
        KeyCode::KeyK => "KeyK",
        KeyCode::KeyL => "KeyL",
        KeyCode::KeyM => "KeyM",
        KeyCode::KeyN => "KeyN",
        KeyCode::KeyO => "KeyO",
        KeyCode::KeyP => "KeyP",
        KeyCode::KeyQ => "KeyQ",
        KeyCode::KeyR => "KeyR",
        KeyCode::KeyS => "KeyS",
        KeyCode::KeyT => "KeyT",
        KeyCode::KeyU => "KeyU",
        KeyCode::KeyV => "KeyV",
        KeyCode::KeyW => "KeyW",
        KeyCode::KeyX => "KeyX",
        KeyCode::KeyY => "KeyY",
        KeyCode::KeyZ => "KeyZ",
        KeyCode::Digit0 => "Digit0",
        KeyCode::Digit1 => "Digit1",
        KeyCode::Digit2 => "Digit2",
        KeyCode::Digit3 => "Digit3",
        KeyCode::Digit4 => "Digit4",
        KeyCode::Digit5 => "Digit5",
        KeyCode::Digit6 => "Digit6",
        KeyCode::Digit7 => "Digit7",
        KeyCode::Digit8 => "Digit8",
        KeyCode::Digit9 => "Digit9",
        KeyCode::Enter => "Enter",
        KeyCode::Escape => "Escape",
        KeyCode::Space => "Space",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::ShiftLeft => "ShiftLeft",
        KeyCode::ShiftRight => "ShiftRight",
        KeyCode::ControlLeft => "ControlLeft",
        KeyCode::ControlRight => "ControlRight",
        KeyCode::AltLeft => "AltLeft",
        KeyCode::AltRight => "AltRight",
        KeyCode::ArrowUp => "ArrowUp",
        KeyCode::ArrowDown => "ArrowDown",
        KeyCode::ArrowLeft => "ArrowLeft",
        KeyCode::ArrowRight => "ArrowRight",
        KeyCode::Delete => "Delete",
        KeyCode::Insert => "Insert",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        _ => return None,
    })
}

struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    triangle: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    framebuffer: Option<Framebuffer>,
}

struct Framebuffer {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl Renderer {
    async fn new(window: std::sync::Arc<Window>, allow_fallback: bool) -> Result<Self> {
        let instance = metal_instance();
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create macOS wgpu surface")?;
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: allow_fallback }).await.context("Metal adapter unavailable; install/use a Metal-capable macOS system or pass --allow-fallback-adapter explicitly")?;
        let info = adapter.get_info();
        if info.backend != wgpu::Backend::Metal {
            bail!(
                "Metal-only frontend selected unexpected backend {:?}",
                info.backend
            );
        }
        tracing::info!(adapter = %info.name, backend = ?info.backend, vendor = info.vendor, device = info.device, fallback_requested = allow_fallback, features = ?adapter.features(), limits = ?adapter.limits(), "selected native GPU");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Aero macOS Metal device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                },
                None,
            )
            .await
            .context("failed to create Metal device")?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        tracing::info!(?format, present_modes = ?caps.present_modes, alpha_modes = ?caps.alpha_modes, "Metal surface capabilities");
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let triangle = create_triangle_pipeline(&device, format);
        let (blit, blit_layout, sampler) = create_blit_pipeline(&device, format);
        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            triangle,
            blit,
            blit_layout,
            sampler,
            framebuffer: None,
        })
    }
    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }
    fn render_triangle(&mut self) -> Result<()> {
        self.render(false)
    }
    fn render_framebuffer(&mut self, width: u32, height: u32, pixels: &[u32]) -> Result<()> {
        if width == 0 || height == 0 || pixels.len() != width as usize * height as usize {
            return self.render(false);
        }
        let recreate = self
            .framebuffer
            .as_ref()
            .is_none_or(|f| f.width != width || f.height != height);
        if recreate {
            self.framebuffer = Some(self.create_framebuffer(width, height));
        }
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        let frame = self.framebuffer.as_ref().expect("created above");
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.render(true)
    }
    fn create_framebuffer(&self, width: u32, height: u32) -> Framebuffer {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Aero guest framebuffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Aero guest framebuffer bindings"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Framebuffer {
            width,
            height,
            texture,
            bind_group,
        }
    }
    fn render(&mut self, framebuffer: bool) -> Result<()> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        let output = match self.surface.get_current_texture() {
            Ok(texture) => texture,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(wgpu::SurfaceError::OutOfMemory) => bail!("Metal surface out of memory"),
        };
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Aero macOS present encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Aero macOS present pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if framebuffer {
                let group = &self
                    .framebuffer
                    .as_ref()
                    .expect("framebuffer requested")
                    .bind_group;
                pass.set_pipeline(&self.blit);
                pass.set_bind_group(0, group, &[]);
            } else {
                pass.set_pipeline(&self.triangle);
            }
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

fn create_triangle_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Aero Metal triangle"), source: wgpu::ShaderSource::Wgsl("@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> { if (i == 0u) { return vec4(-0.65,-0.55,0.0,1.0); } if (i == 1u) { return vec4(0.65,-0.55,0.0,1.0); } return vec4(0.0,0.70,0.0,1.0); } @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return vec4(0.15 + p.x / 1800.0, 0.55, 0.95, 1.0); }".into()) });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Aero Metal triangle pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
    })
}

fn create_blit_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Aero framebuffer layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Aero framebuffer pipeline layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Aero framebuffer blit"), source: wgpu::ShaderSource::Wgsl("struct V { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> }; @vertex fn vs(@builtin(vertex_index) i: u32) -> V { var o: V; if (i == 0u) { o.p=vec4(-1.,-3.,0.,1.); o.uv=vec2(0.,2.); } else if (i == 1u) { o.p=vec4(3.,1.,0.,1.); o.uv=vec2(2.,0.); } else { o.p=vec4(-1.,1.,0.,1.); o.uv=vec2(0.,0.); } return o; } @group(0) @binding(0) var tex: texture_2d<f32>; @group(0) @binding(1) var smp: sampler; @fragment fn fs(v: V) -> @location(0) vec4<f32> { return textureSample(tex,smp,v.uv); }".into()) });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Aero framebuffer pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Aero framebuffer sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (pipeline, layout, sampler)
}

#![deny(unsafe_op_in_unsafe_fn)]

//! Versioned C ABI between QEMU and Aero's portable AeroGPU device model.
//!
//! QEMU remains responsible for PCI configuration space, BAR placement, guest
//! physical memory, display presentation, and interrupt delivery. This library
//! owns the AeroGPU BAR0 protocol state machine and optional native wgpu
//! renderer.

use std::ffi::{c_char, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use aero_devices::pci::PciDevice;
#[cfg(feature = "native-renderer")]
use aero_devices_gpu::NativeAeroGpuBackend;
use aero_devices_gpu::{
    AeroGpuDeviceConfig, AeroGpuExecutorConfig, AeroGpuFenceCompletionMode, AeroGpuPciDevice,
    ImmediateAeroGpuBackend,
};
use memory::{MemoryBus, MmioHandler};

pub const AEROGPU_QEMU_BRIDGE_ABI_VERSION: u32 = 1;
pub const AEROGPU_QEMU_RENDERER_NOOP: u32 = 0;
pub const AEROGPU_QEMU_RENDERER_NATIVE: u32 = 1;

const PCI_COMMAND_MEM_ENABLE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER_ENABLE: u16 = 1 << 2;
const DEFAULT_VBLANK_HZ: u32 = 60;

pub type AeroGpuReadMemoryFn =
    unsafe extern "C" fn(opaque: *mut c_void, gpa: u64, dst: *mut u8, len: usize) -> i32;
pub type AeroGpuWriteMemoryFn =
    unsafe extern "C" fn(opaque: *mut c_void, gpa: u64, src: *const u8, len: usize) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AeroGpuBridgeMemory {
    pub opaque: *mut c_void,
    pub read: Option<AeroGpuReadMemoryFn>,
    pub write: Option<AeroGpuWriteMemoryFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AeroGpuBridgeScanout {
    pub enabled: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pitch_bytes: u32,
    pub fb_gpa: u64,
}

#[repr(C)]
pub struct AeroGpuBridgeApi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub create: unsafe extern "C" fn(
        renderer: u32,
        vblank_hz: u32,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut c_void,
    pub destroy: unsafe extern "C" fn(bridge: *mut c_void),
    pub reset: unsafe extern "C" fn(bridge: *mut c_void),
    pub sync_pci_command: unsafe extern "C" fn(bridge: *mut c_void, command: u16),
    pub mmio_read: unsafe extern "C" fn(bridge: *mut c_void, offset: u64, size: u32) -> u64,
    pub mmio_write: unsafe extern "C" fn(
        bridge: *mut c_void,
        memory: *const AeroGpuBridgeMemory,
        now_ns: u64,
        offset: u64,
        value: u64,
        size: u32,
    ) -> i32,
    pub tick: unsafe extern "C" fn(
        bridge: *mut c_void,
        memory: *const AeroGpuBridgeMemory,
        now_ns: u64,
    ) -> i32,
    pub irq_level: unsafe extern "C" fn(bridge: *mut c_void) -> i32,
    pub scanout: unsafe extern "C" fn(bridge: *mut c_void, out: *mut AeroGpuBridgeScanout) -> i32,
}

struct Bridge {
    device: AeroGpuPciDevice,
}

impl Bridge {
    fn new(renderer: u32, vblank_hz: u32) -> Result<Self, String> {
        let mut device = AeroGpuPciDevice::new(AeroGpuDeviceConfig {
            executor: AeroGpuExecutorConfig {
                verbose: false,
                keep_last_submissions: 0,
                fence_completion: AeroGpuFenceCompletionMode::Deferred,
            },
            vblank_hz: Some(if vblank_hz == 0 {
                DEFAULT_VBLANK_HZ
            } else {
                vblank_hz
            }),
        });

        match renderer {
            AEROGPU_QEMU_RENDERER_NOOP => {
                device.set_backend(Box::new(ImmediateAeroGpuBackend::new()));
            }
            AEROGPU_QEMU_RENDERER_NATIVE => {
                #[cfg(feature = "native-renderer")]
                {
                    let backend = NativeAeroGpuBackend::new_headless()
                        .map_err(|err| format!("failed to initialize native renderer: {err}"))?;
                    device.set_backend(Box::new(backend));
                }
                #[cfg(not(feature = "native-renderer"))]
                {
                    return Err(
                        "native renderer support was not compiled into the QEMU bridge".into(),
                    );
                }
            }
            other => return Err(format!("unknown AeroGPU renderer mode {other}")),
        }

        // QEMU performs the actual PCI decode gating. Start enabled so discovery
        // reads work before the first explicit command-register synchronization.
        device
            .config_mut()
            .set_command(PCI_COMMAND_MEM_ENABLE | PCI_COMMAND_BUS_MASTER_ENABLE);
        Ok(Self { device })
    }

    fn sync_pci_command(&mut self, command: u16) {
        self.device.config_mut().set_command(command);
    }
}

struct CallbackMemory {
    callbacks: AeroGpuBridgeMemory,
}

impl CallbackMemory {
    fn from_ptr(callbacks: *const AeroGpuBridgeMemory) -> Option<Self> {
        if callbacks.is_null() {
            return None;
        }
        // SAFETY: The caller promises that `callbacks` points to a readable C ABI
        // struct for the duration of this synchronous bridge call.
        let callbacks = unsafe { ptr::read(callbacks) };
        Some(Self { callbacks })
    }
}

impl MemoryBus for CallbackMemory {
    fn read_physical(&mut self, paddr: u64, buf: &mut [u8]) {
        // Match an unmapped PCI/DMA read if the host callback is absent or fails.
        buf.fill(0xff);
        let Some(read) = self.callbacks.read else {
            return;
        };
        // SAFETY: `buf` is valid and writable for `buf.len()` bytes for the
        // duration of this synchronous callback.
        let _ = unsafe { read(self.callbacks.opaque, paddr, buf.as_mut_ptr(), buf.len()) };
    }

    fn write_physical(&mut self, paddr: u64, buf: &[u8]) {
        let Some(write) = self.callbacks.write else {
            return;
        };
        // SAFETY: `buf` is valid and readable for `buf.len()` bytes for the
        // duration of this synchronous callback.
        let _ = unsafe { write(self.callbacks.opaque, paddr, buf.as_ptr(), buf.len()) };
    }
}

fn with_bridge_mut<T>(bridge: *mut c_void, fallback: T, f: impl FnOnce(&mut Bridge) -> T) -> T {
    if bridge.is_null() {
        return fallback;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Handles returned by `bridge_create` point to a live `Bridge`
        // until the matching `bridge_destroy` call.
        let bridge = unsafe { &mut *(bridge.cast::<Bridge>()) };
        f(bridge)
    }))
    .unwrap_or(fallback)
}

unsafe fn copy_error(error: *mut c_char, error_len: usize, message: &str) {
    if error.is_null() || error_len == 0 {
        return;
    }
    let bytes = message.as_bytes();
    let copy_len = bytes.len().min(error_len - 1);
    // SAFETY: The caller supplied a writable buffer of `error_len` bytes.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), error.cast::<u8>(), copy_len);
        *error.add(copy_len) = 0;
    }
}

unsafe extern "C" fn bridge_create(
    renderer: u32,
    vblank_hz: u32,
    error: *mut c_char,
    error_len: usize,
) -> *mut c_void {
    match catch_unwind(AssertUnwindSafe(|| Bridge::new(renderer, vblank_hz))) {
        Ok(Ok(bridge)) => Box::into_raw(Box::new(bridge)).cast(),
        Ok(Err(message)) => {
            // SAFETY: Forwarding the caller-provided error buffer.
            unsafe { copy_error(error, error_len, &message) };
            ptr::null_mut()
        }
        Err(_) => {
            // SAFETY: Forwarding the caller-provided error buffer.
            unsafe { copy_error(error, error_len, "panic while creating AeroGPU bridge") };
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn bridge_destroy(bridge: *mut c_void) {
    if bridge.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Ownership of this handle is transferred back exactly once.
        drop(unsafe { Box::from_raw(bridge.cast::<Bridge>()) });
    }));
}

unsafe extern "C" fn bridge_reset(bridge: *mut c_void) {
    with_bridge_mut(bridge, (), |bridge| bridge.device.reset());
}

unsafe extern "C" fn bridge_sync_pci_command(bridge: *mut c_void, command: u16) {
    with_bridge_mut(bridge, (), |bridge| bridge.sync_pci_command(command));
}

unsafe extern "C" fn bridge_mmio_read(bridge: *mut c_void, offset: u64, size: u32) -> u64 {
    let Ok(size) = usize::try_from(size) else {
        return u64::MAX;
    };
    with_bridge_mut(bridge, u64::MAX, |bridge| bridge.device.read(offset, size))
}

unsafe extern "C" fn bridge_mmio_write(
    bridge: *mut c_void,
    memory: *const AeroGpuBridgeMemory,
    now_ns: u64,
    offset: u64,
    value: u64,
    size: u32,
) -> i32 {
    let Some(mut memory) = CallbackMemory::from_ptr(memory) else {
        return 0;
    };
    let Ok(size) = usize::try_from(size) else {
        return 0;
    };
    with_bridge_mut(bridge, 0, |bridge| {
        bridge.device.write(offset, size, value);
        // Process doorbells immediately; the periodic QEMU timer handles later
        // completions and vblank pacing.
        bridge.device.tick(&mut memory, now_ns);
        1
    })
}

unsafe extern "C" fn bridge_tick(
    bridge: *mut c_void,
    memory: *const AeroGpuBridgeMemory,
    now_ns: u64,
) -> i32 {
    let Some(mut memory) = CallbackMemory::from_ptr(memory) else {
        return 0;
    };
    with_bridge_mut(bridge, 0, |bridge| {
        bridge.device.tick(&mut memory, now_ns);
        1
    })
}

unsafe extern "C" fn bridge_irq_level(bridge: *mut c_void) -> i32 {
    with_bridge_mut(bridge, 0, |bridge| i32::from(bridge.device.irq_level()))
}

unsafe extern "C" fn bridge_scanout(bridge: *mut c_void, out: *mut AeroGpuBridgeScanout) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_bridge_mut(bridge, 0, |bridge| {
        let scanout = &bridge.device.regs.scanout0;
        let value = AeroGpuBridgeScanout {
            enabled: u32::from(scanout.enable),
            width: scanout.width,
            height: scanout.height,
            format: scanout.format as u32,
            pitch_bytes: scanout.pitch_bytes,
            fb_gpa: scanout.fb_gpa,
        };
        // SAFETY: The caller supplied a writable output structure for this
        // synchronous call.
        unsafe { ptr::write(out, value) };
        1
    })
}

static API: AeroGpuBridgeApi = AeroGpuBridgeApi {
    abi_version: AEROGPU_QEMU_BRIDGE_ABI_VERSION,
    struct_size: std::mem::size_of::<AeroGpuBridgeApi>() as u32,
    create: bridge_create,
    destroy: bridge_destroy,
    reset: bridge_reset,
    sync_pci_command: bridge_sync_pci_command,
    mmio_read: bridge_mmio_read,
    mmio_write: bridge_mmio_write,
    tick: bridge_tick,
    irq_level: bridge_irq_level,
    scanout: bridge_scanout,
};

/// Return the immutable versioned AeroGPU/QEMU bridge function table.
#[no_mangle]
pub extern "C" fn aerogpu_qemu_bridge_get_api() -> *const AeroGpuBridgeApi {
    &API
}

#[cfg(test)]
mod tests {
    use super::*;
    use aero_devices_gpu::mmio;

    #[test]
    fn api_and_discovery_registers_are_stable() {
        let api = unsafe { &*aerogpu_qemu_bridge_get_api() };
        assert_eq!(api.abi_version, AEROGPU_QEMU_BRIDGE_ABI_VERSION);
        assert_eq!(
            api.struct_size as usize,
            std::mem::size_of::<AeroGpuBridgeApi>()
        );

        let mut error = [0i8; 128];
        let bridge = unsafe {
            (api.create)(
                AEROGPU_QEMU_RENDERER_NOOP,
                60,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert!(!bridge.is_null());

        unsafe {
            (api.sync_pci_command)(
                bridge,
                PCI_COMMAND_MEM_ENABLE | PCI_COMMAND_BUS_MASTER_ENABLE,
            );
        }
        assert_eq!(
            unsafe { (api.mmio_read)(bridge, mmio::MAGIC, 4) } as u32,
            aero_devices_gpu::regs::AEROGPU_MMIO_MAGIC
        );
        assert_ne!(unsafe { (api.mmio_read)(bridge, mmio::FEATURES_LO, 4) }, 0);

        let memory = AeroGpuBridgeMemory {
            opaque: ptr::null_mut(),
            read: None,
            write: None,
        };
        for (offset, value) in [
            (mmio::SCANOUT0_WIDTH, 800),
            (mmio::SCANOUT0_HEIGHT, 600),
            (
                mmio::SCANOUT0_FORMAT,
                aero_devices_gpu::AeroGpuFormat::B8G8R8X8Unorm as u64,
            ),
            (mmio::SCANOUT0_PITCH_BYTES, 3200),
            (mmio::SCANOUT0_FB_GPA_LO, 0x1234_5000),
            (mmio::SCANOUT0_FB_GPA_HI, 0),
            (mmio::SCANOUT0_ENABLE, 1),
        ] {
            assert_eq!(
                unsafe { (api.mmio_write)(bridge, &memory, 0, offset, value, 4) },
                1
            );
        }

        let mut scanout = AeroGpuBridgeScanout::default();
        assert_eq!(unsafe { (api.scanout)(bridge, &mut scanout) }, 1);
        assert_eq!(
            scanout,
            AeroGpuBridgeScanout {
                enabled: 1,
                width: 800,
                height: 600,
                format: aero_devices_gpu::AeroGpuFormat::B8G8R8X8Unorm as u32,
                pitch_bytes: 3200,
                fb_gpa: 0x1234_5000,
            }
        );

        unsafe { (api.destroy)(bridge) };
    }

    #[test]
    fn native_mode_reports_a_build_time_error_when_disabled() {
        if cfg!(feature = "native-renderer") {
            return;
        }
        let mut error = [0i8; 128];
        let bridge = unsafe {
            bridge_create(
                AEROGPU_QEMU_RENDERER_NATIVE,
                60,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert!(bridge.is_null());
        let nul = error.iter().position(|&byte| byte == 0).unwrap();
        let bytes: Vec<u8> = error[..nul].iter().map(|&byte| byte as u8).collect();
        assert!(String::from_utf8(bytes).unwrap().contains("not compiled"));
    }
}

/*
 * Versioned C ABI between QEMU and Aero's Rust AeroGPU device core.
 *
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
#ifndef AEROGPU_QEMU_BRIDGE_H
#define AEROGPU_QEMU_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#define AEROGPU_QEMU_BRIDGE_ABI_VERSION 1
#define AEROGPU_QEMU_RENDERER_NOOP 0
#define AEROGPU_QEMU_RENDERER_NATIVE 1

typedef int32_t (*AeroGpuReadMemoryFn)(void *opaque, uint64_t gpa,
                                      uint8_t *dst, size_t len);
typedef int32_t (*AeroGpuWriteMemoryFn)(void *opaque, uint64_t gpa,
                                       const uint8_t *src, size_t len);

typedef struct AeroGpuBridgeMemory {
    void *opaque;
    AeroGpuReadMemoryFn read;
    AeroGpuWriteMemoryFn write;
} AeroGpuBridgeMemory;

typedef struct AeroGpuBridgeScanout {
    uint32_t enabled;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t pitch_bytes;
    uint64_t fb_gpa;
} AeroGpuBridgeScanout;

typedef struct AeroGpuBridgeApi {
    uint32_t abi_version;
    uint32_t struct_size;
    void *(*create)(uint32_t renderer, uint32_t vblank_hz,
                    char *error, size_t error_len);
    void (*destroy)(void *bridge);
    void (*reset)(void *bridge);
    void (*sync_pci_command)(void *bridge, uint16_t command);
    uint64_t (*mmio_read)(void *bridge, uint64_t offset, uint32_t size);
    int32_t (*mmio_write)(void *bridge, const AeroGpuBridgeMemory *memory,
                          uint64_t now_ns, uint64_t offset, uint64_t value,
                          uint32_t size);
    int32_t (*tick)(void *bridge, const AeroGpuBridgeMemory *memory,
                    uint64_t now_ns);
    int32_t (*irq_level)(void *bridge);
    int32_t (*scanout)(void *bridge, AeroGpuBridgeScanout *out);
} AeroGpuBridgeApi;

typedef const AeroGpuBridgeApi *(*AeroGpuBridgeGetApiFn)(void);

#endif

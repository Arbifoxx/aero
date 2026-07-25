# QEMU port plan (deferred)

Do not port AeroGPU to QEMU until the native frontend demonstrates driver installation, a D3D9 triangle, textures, D3D9Ex shared surfaces and basic DWM composition. Reuse the canonical protocol decoder and Rust `wgpu` renderer; candidate transports are an in-process Rust static library, an external process, shared memory, or a local Unix socket. No QEMU implementation is included in this change.

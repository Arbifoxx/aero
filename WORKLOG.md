# macOS native frontend worklog

## 2026-07-25 — baseline

- Cloned upstream at `4f9b19ac1` into this isolated workspace.
- Host: `arm64`, macOS `27.0`, Rust toolchain pinned by the repository to `1.92.0`.
- Ran `cargo build --locked -p aero-machine-cli`, `cargo test --locked -p aero-machine-cli`, and `cargo test --locked -p aero-devices-gpu --features wgpu-backend --test aerogpu_native_smoke`; all completed successfully.

## 2026-07-25 — native bring-up

- Added `crates/aero-macos`, which uses `winit` plus a Metal-only `wgpu` instance, host triangle, RGBA framebuffer blit, native input routing, bounded machine execution, and optional existing native AeroGPU executor.
- `cargo run --locked -p aero-macos -- --list-gpu` selected `Apple M1 Pro` through Metal and reported timestamp-query support. A bounded `--host-triangle` window and bounded no-disk firmware run both completed without `wgpu` validation errors.
- Windows driver binary build/package remains intentionally delegated to Windows WDK scripts; no placeholder artifacts were produced.

Next: run the host triangle interactively, then attempt a bounded guest boot with a writable Win7 disk image and capture first device initialization traces.

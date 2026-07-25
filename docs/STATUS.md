# macOS native status

Validated on 2026-07-25 at commit `383f8c760`, ARM64 macOS 27.0 on Apple M1 Pro:

| Milestone | Status | Evidence |
|---|---|---|
| Native Rust build | validated | `cargo check --locked -p aero-macos` |
| Metal discovery | validated | `cargo run --locked -p aero-macos -- --list-gpu` selected `Apple M1 Pro`, `Metal` |
| Native host triangle | validated | 1.2-second native Metal window completed without validation errors |
| Native framebuffer present | validated for empty firmware boot; guest boot pending | 1.2-second no-disk machine run completed via `Machine::display_present()` → RGBA texture upload |
| Keyboard/mouse injection | implemented | winit physical key/mouse mapping → `Machine` PS/2 APIs |
| AeroGPU PCI/ring/device model | existing partial implementation | canonical `aero-machine` paths/tests |
| Existing native AeroGPU executor | builds/tests | `cargo test -p aero-devices-gpu --features wgpu-backend --test aerogpu_native_smoke` |
| Win7 driver install / D3D9 guest triangle | blocked by unvalidated guest boot/driver workflow | no claim made |
| DWM / Aero Glass | not implemented/validated | no claim made |

Reproduce the validated host checks with `scripts/test-macos.sh`. The next experiment is a bounded boot with a writable Win7 disk image, then use the installed driver to collect BAR/ring/fence logs. Escalate to a deeper systems pass only after a reproducible driver initialization failure is captured.

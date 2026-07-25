You are working on an experimental systems project whose long-term goal is:

> Run a fully emulated x86 Windows 7 guest on an Apple Silicon Mac with genuine GPU-accelerated Windows Aero desktop composition, using the host Apple GPU through Metal.

The immediate project is **not** to add support directly to QEMU. The initial objective is to make the existing Aero emulator and AeroGPU stack run natively on macOS, prove that the Windows 7 AeroGPU WDDM driver can submit accelerated graphics through native `wgpu`/Metal, and establish a known-good reference implementation.

Primary upstream repository:

```text
https://github.com/wilsonzlin/aero
```

Relevant comparison/reference repositories may include:

```text
https://github.com/startergo/qemu-3dfx-macos
https://github.com/virtio-win/kvm-guest-drivers-windows
https://github.com/Keenuts/virtio-gpu-win-icd
```

Use those comparison projects only for architectural reference where useful. Do not change the project into a VirtIO-GPU or qemu-3dfx implementation.

# Core objective

Create a native ARM64 macOS frontend for Aero that can eventually:

1. Run Aero’s existing x86 machine emulator.
2. Boot a Windows 7 x86 or x64 disk image.
3. Present VGA output in a native macOS window.
4. Accept keyboard and mouse input.
5. Expose Aero’s existing AeroGPU PCI device to the guest.
6. Execute AeroGPU commands through native Rust `wgpu`.
7. Use the Metal backend on Apple Silicon.
8. Present AeroGPU scanout in the native window.
9. Support the existing Windows 7 AeroGPU guest driver.
10. Produce detailed diagnostic traces for driver initialization, command submission, fences, vblank, scanout, shared surfaces, and presentation.

The first graphics success criterion is not Aero Glass. It is:

> Render one accelerated D3D9 triangle from a Windows 7 guest through AeroGPU, native `wgpu`, and Metal.

After that, advance incrementally toward:

```text
D3D9 triangle
→ textured rendering
→ D3D9Ex
→ cross-process shared surfaces
→ DWM composition
→ Aero Glass
```

## Model-aware execution strategy

This strategy is meant explicitly for GPT-5.6 Terra. Work aggressively and autonomously, but prioritize bounded, testable progress over speculative rewrites.

For this run, focus first on the parts most suitable for Terra:

* repository and architecture analysis
* reproducing the upstream build
* macOS ARM64 compatibility
* native frontend scaffolding
* VGA framebuffer presentation
* keyboard and mouse input
* native `wgpu` initialization
* explicit Metal backend selection
* host-side triangle rendering
* AeroGPU tracing and integration groundwork
* reproducible scripts, tests, and documentation

Do not spend excessive effort attempting to solve deeply ambiguous Windows 7 WDDM, D3D9Ex, shared-surface, or DWM failures before the native frontend and host Metal path are validated.

When reaching a difficult cross-layer blocker:

1. reduce it to the smallest reproducible case
2. collect precise logs and traces
3. identify the exact failing layer
4. add assertions or tests
5. make the smallest defensible fix
6. continue while measurable progress is possible

Do not make broad architectural changes merely because the first approach fails.

If a blocker remains unresolved after thorough investigation, leave the repository in a clean, reproducible state and document:

* the exact validated milestone
* the failing command or test
* relevant logs
* the suspected subsystem
* evidence supporting that diagnosis
* files and functions most likely involved
* the next concrete experiment
* whether the issue is best escalated to GPT-5.6 Sol

The preferred Terra milestone is:

> A native ARM64 macOS build of Aero boots or begins booting a guest, displays its legacy framebuffer, accepts input, initializes `wgpu` through Metal, renders a host-side triangle, and has sufficient AeroGPU tracing and architecture documentation for a later Sol run to continue directly.

Do not stop early merely because full Aero acceleration is not achieved during this run. Complete all prerequisite and diagnostic work that can be validated independently.


# Important working rules

Work autonomously and continue through implementation, building, testing, diagnosis, and repair without stopping at artificial checkpoints.

Do not pause merely to report progress or ask whether to continue.

Do not require the user to perform routine actions that you can perform yourself.

You may use:

* Git
* Homebrew
* Cargo
* Rust toolchains
* CMake
* Ninja
* Python
* shell scripts
* LLDB
* macOS system utilities
* QEMU only as a reference or testing aid
* Windows build tooling where available
* Wine or CrossOver only where useful for inspecting Windows binaries
* Existing test images or downloadable development dependencies, subject to licensing and repository instructions

Operate in an isolated project directory. Do not modify unrelated system files or repositories.

Prefer project-local dependencies, virtual environments, Cargo-managed tools, and reproducible scripts.

Do not disable macOS security mechanisms globally.

Do not make destructive changes to the user’s machine.

Before running potentially destructive commands, inspect their effects and constrain them to the working directory.

# Initial repository investigation

Clone the Aero repository and inspect it comprehensively before making architectural changes.

Determine:

1. Which crates implement the x86 machine.
2. Which crates implement the AeroGPU PCI device.
3. Which crates define the AeroGPU protocol.
4. Which crates decode AeroGPU command streams.
5. Which crates implement GPU execution.
6. Whether a native `wgpu` backend already exists.
7. Which code is browser-specific.
8. Which code is WASM-specific.
9. Which frontend or display abstractions already exist.
10. How scanout textures are currently exposed.
11. How keyboard and mouse input are injected.
12. How disks and firmware are loaded.
13. How PCI BARs, DMA, interrupts, queues, fences, and vblank are modeled.
14. How the Windows 7 guest driver is built and packaged.
15. What test programs already exist for D3D9, D3D9Ex, DXGI, or shared surfaces.
16. What the repository documentation says is complete, partial, stubbed, or unimplemented.

Read all graphics-related documentation, especially files concerning:

* AeroGPU architecture
* Windows 7 guest tools
* WDDM support
* shared surfaces
* share tokens
* D3D9Ex
* DXGI
* native `wgpu`
* graphics status
* scanout
* fences
* command transport

Do not assume documentation is current. Confirm statements against the source code.

Create a concise internal architecture map before editing.

# Development strategy

Work in phases. Complete and test each phase before moving to the next, but do not stop and wait for approval between phases.

## Phase 0: Reproducible build and baseline

Establish a clean baseline.

Tasks:

1. Record the host architecture and macOS version.
2. Install only required build dependencies.
3. Build the unmodified repository.
4. Run its existing tests.
5. Record all failures.
6. Determine whether the native GPU-related crates compile on ARM64 macOS.
7. Identify browser-only assumptions.
8. Add a reproducible bootstrap script.

Create:

```text
scripts/bootstrap-macos.sh
scripts/build-macos.sh
scripts/test-macos.sh
```

Requirements:

* Scripts must be idempotent.
* Scripts must fail clearly.
* Scripts must avoid global modifications where possible.
* Scripts must log toolchain versions.
* Scripts must work from the repository root.

Do not suppress build errors or warnings without understanding them.

## Phase 1: Native macOS machine frontend

Create a native frontend executable for macOS.

Prefer Rust and reuse the project’s existing crates.

Use an appropriate window/input library, preferably in this order:

1. Existing Aero frontend abstraction, if suitable.
2. `winit`
3. SDL2
4. Another mature cross-platform Rust frontend library

The native frontend must:

* Create a resizable native window.
* Run the emulated machine on an appropriate execution thread.
* Display VGA or legacy framebuffer output.
* Handle keyboard input.
* Handle relative and absolute mouse input as appropriate.
* Support graceful shutdown.
* Accept a disk image path through CLI arguments.
* Accept configurable memory size.
* Accept configurable logging verbosity.
* Avoid blocking the UI event loop.
* Avoid unsafe shared-state patterns unless required and documented.

Suggested CLI:

```text
aero-macos \
  --disk /path/to/windows7.img \
  --memory 2048 \
  --log-level debug
```

Initially, VGA output is sufficient.

Add a headless mode where practical for automated testing.

## Phase 2: Native `wgpu` and Metal initialization

Integrate the existing GPU executor with native `wgpu`.

Requirements:

* Explicitly request a native `wgpu` adapter.
* Prefer the Metal backend on macOS.
* Log the selected adapter, backend, vendor, device, limits, and features.
* Fail clearly if Metal is unavailable.
* Do not silently fall back to a software renderer unless explicitly enabled by a CLI option.
* Handle surface resize and swapchain reconfiguration.
* Handle device-loss conditions.
* Keep the graphics backend independent from browser APIs.

Add a diagnostic command such as:

```text
aero-macos --list-gpu
```

It should print:

* available adapters
* selected backend
* supported texture formats
* relevant feature flags
* maximum buffer and texture limits
* timestamp-query support
* adapter fallback status

Create a host-only rendering test that draws a triangle through native `wgpu` before involving the guest.

Success criterion:

> A native ARM64 macOS executable draws a triangle through `wgpu` using Metal.

## Phase 3: AeroGPU PCI device integration

Wire the existing AeroGPU virtual PCI device into the native machine frontend.

Do not invent a new protocol.

Reuse the existing implementation wherever possible.

Verify and instrument:

* PCI enumeration
* vendor and device IDs
* BAR layout
* BAR mapping
* MMIO reads and writes
* command ring setup
* submission queues
* DMA or guest-memory mappings
* interrupt delivery
* fence creation
* fence completion
* vblank generation
* cursor state
* scanout configuration
* resource creation
* resource destruction
* error handling

Add categorized tracing controlled by environment variables or CLI flags:

```text
--trace-pci
--trace-mmio
--trace-gpu-commands
--trace-fences
--trace-vblank
--trace-scanout
--trace-shared-surfaces
```

Tracing must be rate-limited or filterable so that logs remain usable.

Do not flood logs with every framebuffer update unless explicitly requested.

## Phase 4: Guest driver packaging and installation workflow

Inspect the existing Windows 7 AeroGPU driver.

Determine:

* supported Windows versions
* supported architectures
* test-signing requirements
* INF names
* service names
* device IDs
* KMD and UMD relationships
* D3D9 and D3D9Ex support
* D3D10/11 support
* DLL installation paths
* driver-store requirements
* expected registry entries
* required guest services
* debug logging mechanisms

Create a reproducible guest-tools packaging process.

Create:

```text
scripts/build-win7-guest-tools.sh
scripts/package-win7-guest-tools.sh
docs/WINDOWS7_GUEST_SETUP.md
```

Where native Windows build tools are unavailable on macOS, support one or more of:

* a Windows VM build environment
* an existing CI workflow
* a documented Visual Studio/WDK build process
* use of existing upstream driver binaries for early testing

Do not fake successful driver builds.

Do not replace missing binaries with placeholders.

Clearly distinguish:

* source-built binaries
* upstream prebuilt binaries
* locally modified binaries
* unsigned binaries
* test-signed binaries

## Phase 5: Device enumeration in Windows 7

Boot Windows 7 with AeroGPU exposed.

Initial success criteria:

1. Windows boots with the device present.
2. Device Manager sees AeroGPU.
3. The driver installs.
4. The device starts without Code 10, Code 12, Code 31, Code 39, or Code 43.
5. BARs are mapped correctly.
6. Interrupts are delivered.
7. The driver creates its expected queues.
8. The driver submits at least one valid command.
9. The host acknowledges at least one fence.

Collect:

* emulator logs
* Windows setup logs
* driver installation logs
* kernel debug output where available
* device status
* PCI configuration
* crash dumps if generated

Add scripts to extract and organize diagnostic logs.

Do not proceed to complex rendering while the device cannot initialize reliably.

## Phase 6: Minimal accelerated guest rendering

Find or create the smallest possible Windows 7 graphics test.

Preferred order:

1. Existing AeroGPU test application.
2. Existing D3D9 triangle test.
3. A minimal custom D3D9 application.
4. A minimal custom D3D9Ex application.

The test must:

* create a device
* allocate a render target
* clear the target
* draw one triangle
* present it
* report HRESULT values
* produce deterministic logs
* avoid unrelated framework dependencies

Add a known visual reference or output hash where practical.

Instrument both sides of every command:

```text
Guest UMD call
→ guest command encoding
→ ring submission
→ host command decoding
→ wgpu resource operation
→ Metal execution
→ fence completion
→ scanout/present
```

Success criterion:

> A D3D9 triangle rendered by the Windows 7 guest appears in the native macOS window and is executed by the Apple GPU through Metal.

Verify that it is not software rasterization.

Use host GPU diagnostics, Metal capture where practical, and timing evidence.

## Phase 7: Textures, formats, and presentation

After the triangle works, add support incrementally for:

* vertex buffers
* index buffers
* constant buffers
* textures
* texture uploads
* samplers
* shaders
* render states
* blending
* depth/stencil
* common Windows 7 surface formats
* pitch handling
* resource copies
* presentation
* resizing
* fullscreen/windowed transitions

Create a test matrix.

Track each feature as:

```text
unsupported
stubbed
partially working
working
validated
```

Do not mark features as working based only on successful compilation.

## Phase 8: D3D9Ex and shared surfaces

This phase is required for Windows 7 DWM.

Focus on:

* D3D9Ex device creation
* cross-process shared-resource handles
* AeroGPU share tokens
* allocation identity
* resource lifetime
* opening shared resources from another process
* synchronization
* scanout ownership
* alpha-capable surfaces
* correct pitch and format metadata
* DWM producer/consumer behavior

Use the repository’s existing shared-surface design.

Do not substitute process-local handles where stable cross-process tokens are required.

Create dedicated guest tests:

1. Process A creates a shared surface.
2. Process B opens it.
3. Process A renders into it.
4. Process B reads or presents it.
5. Both processes synchronize correctly.
6. Resource destruction does not cause stale references.

Success criterion:

> A D3D9Ex surface created in one Windows process can be opened and consumed by another through AeroGPU.

## Phase 9: Windows Desktop Window Manager

Only after shared surfaces work, test DWM.

Validate:

* Desktop Window Manager service starts.
* Desktop composition can be enabled.
* `dwm.exe` creates its D3D9Ex device.
* DWM allocations succeed.
* shared surfaces open correctly.
* composition frames are submitted.
* scanout displays composed output.
* vblank timing is reasonable.
* resizing and moving windows do not crash the driver.
* device loss is handled.
* explorer remains usable.

Start with basic composition.

Do not focus on glass transparency until non-glass desktop composition is stable.

Then validate:

* window thumbnails
* taskbar previews
* animations
* transparency
* blur
* Aero Glass
* Flip 3D, if feasible

Success criterion:

> Windows 7 reports desktop composition enabled and visibly renders the composed desktop through AeroGPU on Metal.

Final visual criterion:

> Aero Glass transparency and composition are working without software rasterization.

# Debugging requirements

Use systematic fault isolation.

When a failure occurs, identify which layer failed:

```text
Windows application
Windows D3D runtime
AeroGPU UMD
AeroGPU KMD
WDDM scheduler/memory interaction
PCI transport
guest-memory mapping
command-ring protocol
host decoder
wgpu validation
Metal backend
scanout/presentation
```

Do not randomly patch multiple layers simultaneously.

For each significant bug:

1. Reproduce it reliably.
2. Reduce it to the smallest test.
3. Add logging or assertions.
4. Identify the violated invariant.
5. Fix the root cause.
6. Add a regression test.
7. Document the result.

Use LLDB for native crashes.

Use Rust backtraces.

Enable `wgpu` validation.

Use AddressSanitizer or equivalent where applicable.

Use Windows kernel debugging only when necessary and document the setup.

Never hide protocol errors merely to keep the guest running.

# Architecture requirements

Keep the implementation modular.

Prefer an architecture resembling:

```text
aero-machine
aerogpu-protocol
aerogpu-device
aerogpu-command-decoder
aerogpu-wgpu
aero-native-frontend
aero-macos
```

Do not duplicate protocol definitions between components.

Keep wire-format structures explicitly sized and endian-safe.

Add compile-time or runtime checks for:

* structure sizes
* field offsets
* alignment
* ring boundaries
* integer overflow
* guest-memory bounds
* malformed commands
* stale handles
* invalid resource IDs

Treat all guest-provided data as untrusted.

Do not permit arbitrary guest pointers to become unchecked host pointers.

# Testing requirements

Create tests at several levels.

## Unit tests

Test:

* protocol serialization
* protocol parsing
* command validation
* resource-handle tables
* fence progression
* ring wraparound
* pitch calculations
* texture-format conversion
* shared-token lifetime
* invalid command rejection

## Host rendering tests

Test:

* adapter initialization
* Metal backend selection
* triangle rendering
* texture upload
* render-to-texture
* resource copy
* alpha blending
* scanout presentation
* resize handling

## Machine integration tests

Test:

* PCI enumeration
* BAR behavior
* MMIO semantics
* queue setup
* interrupt generation
* fence completion
* vblank scheduling
* guest-memory reads and writes

## Windows guest tests

Test:

* driver installation
* device startup
* D3D9 device creation
* triangle rendering
* D3D9Ex creation
* shared surfaces
* repeated present
* process restart
* resolution change
* DWM startup

Whenever practical, make tests automated and headless.

# Performance requirements

Correctness comes first.

After correctness:

* avoid copying full framebuffers unnecessarily
* reuse textures and buffers
* batch uploads
* avoid synchronous GPU waits in the UI thread
* use fences correctly
* avoid polling at excessive frequency
* maintain responsive input
* measure frame times
* measure command-decoding overhead
* measure TCG/emulator CPU utilization
* log dropped or delayed frames

Do not perform premature optimization before the triangle path works.

# CPU emulation scope

Do not replace Aero’s CPU emulator during the initial GPU bring-up unless it prevents Windows 7 from booting at all.

The initial goal is graphics correctness.

After graphics are proven, evaluate CPU performance separately.

Possible future options may include:

* optimizing Aero’s existing CPU core
* adding an ARM64 JIT
* integrating a dynamic binary translator
* integrating QEMU TCG
* porting AeroGPU to QEMU

Do not begin those major projects during the first graphics milestone.

# QEMU integration is explicitly deferred

Do not port AeroGPU into QEMU until all of the following work in Aero’s own native macOS frontend:

* Windows 7 driver installation
* successful device initialization
* D3D9 triangle
* textures
* D3D9Ex
* shared surfaces
* basic DWM composition

After those milestones, prepare a design document for a QEMU port.

The QEMU design should consider:

```text
Windows 7 AeroGPU driver
→ QEMU AeroGPU PCI device
→ reusable AeroGPU protocol decoder
→ external or in-process Rust wgpu renderer
→ Metal
```

Potential implementation models:

1. QEMU C device with a Rust static-library renderer.
2. QEMU C device connected to an external Rust GPU process.
3. Shared-memory command transport.
4. Local Unix-domain socket transport for early development.
5. Later in-process integration after protocol stabilization.

Do not rewrite the existing renderer in C without a strong reason.

# Documentation requirements

Maintain:

```text
docs/MACOS_NATIVE_ARCHITECTURE.md
docs/AEROGPU_PROTOCOL_NOTES.md
docs/WINDOWS7_GUEST_SETUP.md
docs/DEBUGGING.md
docs/STATUS.md
docs/QEMU_PORT_PLAN.md
```

`docs/STATUS.md` must include:

* current build status
* current boot status
* current GPU status
* known failures
* completed milestones
* blocked milestones
* exact reproduction commands
* latest validated commit
* whether rendering is hardware accelerated
* whether DWM works
* whether Aero Glass works

Do not use vague statements such as “mostly works.”

Use precise statuses.

# Git workflow

Create small, coherent commits.

Each commit should:

* build where possible
* have a descriptive message
* avoid unrelated formatting changes
* include tests for functional changes
* document important architectural changes

Do not rewrite upstream history.

Do not commit:

* Windows disk images
* proprietary SDKs
* generated build directories
* private certificates
* large logs
* user-specific absolute paths
* licensed Microsoft binaries unless redistribution is explicitly permitted

# Reporting format during work

Keep an ongoing local engineering log:

```text
WORKLOG.md
```

For each meaningful step, record:

* what was attempted
* commands used
* result
* failure mode
* diagnosis
* files changed
* next technical action

Do not stop merely to produce the log.

Continue implementing.

# Final deliverables

Produce as much of the working implementation as technically possible.

At minimum, leave the repository in a reproducible and well-documented state with:

1. A native ARM64 macOS build path.
2. A native windowed frontend.
3. VGA framebuffer presentation.
4. Keyboard and mouse input.
5. Native `wgpu` Metal initialization.
6. A host-side Metal triangle test.
7. AeroGPU device integration progress.
8. Detailed tracing.
9. Reproducible scripts.
10. Automated tests.
11. Windows 7 guest-driver packaging instructions.
12. A clear statement of the furthest validated milestone.
13. A concrete list of remaining blockers.
14. A QEMU port design deferred until the reference implementation works.

The preferred final result is:

> Windows 7 boots in a native ARM64 macOS build of Aero, installs AeroGPU, renders an accelerated D3D9 triangle through Metal, and progresses toward D3D9Ex shared surfaces and DWM.

# Decision-making guidance

When choosing between approaches:

* Prefer reusing existing AeroGPU code over creating new implementations.
* Prefer small validated milestones over broad speculative rewrites.
* Prefer observable tests over assumptions.
* Prefer explicit errors over silent fallback.
* Prefer native `wgpu`/Metal over OpenGL compatibility layers.
* Prefer a known-good Aero reference implementation before QEMU integration.
* Prefer fixing root causes over bypassing validation.
* Prefer architectural separation that will make a later QEMU port possible.

Do not conclude that the project is impossible merely because a subsystem is incomplete.

When blocked:

1. inspect the implementation
2. inspect protocol definitions
3. inspect upstream history
4. inspect open issues and branches
5. search for related tests
6. construct a minimal reproduction
7. implement the smallest missing piece
8. validate it
9. continue

Begin by cloning the repository, building it unmodified, reading all relevant architecture and graphics documentation, identifying the current native execution paths, and producing a concrete source-backed implementation plan. Then immediately proceed into Phase 0 and Phase 1 without waiting for user approval.

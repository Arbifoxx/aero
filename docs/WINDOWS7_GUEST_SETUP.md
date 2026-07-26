# Windows 7 AeroGPU guest setup

The in-tree driver targets Windows 7 SP1 x86 and x64, WDDM 1.1. Its canonical hardware identity is `PCI\VEN_A3A0&DEV_0001`; package metadata, INF selection, UMD/KMD relationships and debug tooling are documented in `drivers/aerogpu/README.md` and `drivers/aerogpu/packaging/win7/README.md`.

For native VM creation, resource settings, and the distinction between host
Metal presentation and guest AeroGPU acceleration, first read
`docs/MACOS_ACCELERATED_VM_GUIDE.md`.

Builds are not performed on macOS. Use a Windows 10/11 x64 build environment
with WDK 10 and MSBuild:

```powershell
pwsh ci/install-wdk.ps1
pwsh ci/build-drivers.ps1 -ToolchainJson out/toolchain.json -Drivers aerogpu
pwsh ci/build-aerogpu-dbgctl.ps1 -ToolchainJson out/toolchain.json
pwsh ci/make-catalogs.ps1 -ToolchainJson out/toolchain.json
pwsh ci/sign-drivers.ps1 -ToolchainJson out/toolchain.json
pwsh ci/package-drivers.ps1
```

The installable x64 package is staged under
`out/packages/aerogpu/x64/`; the test certificate is
`out/certs/aero-test.cer`. The scripts intentionally fail without the Windows
toolchain rather than creating placeholders. Do not commit certificates, PFX
files, generated packages, Microsoft binaries, or Windows media.

For installation, attach the generated Guest Tools media, trust the test
certificate in the test guest, reboot with test signing enabled, and install:

```bat
pnputil -i -a aerogpu_dx11.inf
packaging\win7\verify_umd_registration.cmd
```

Then inspect `%WINDIR%\inf\setupapi.dev.log`, Device Manager status,
`aerogpu_dbgctl`, and the adapter registry values. The first rendering target is
`drivers\aerogpu\tests\win7\d3d9ex_triangle`; it reports `PASS:`/`FAIL:` and
supports the guest validation workflow documented in that directory.

Current limitation: the native machine has not yet reached Windows PnP with the
reference ISO, so none of these guest steps has been validated on macOS.

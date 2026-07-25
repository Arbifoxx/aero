# Windows 7 AeroGPU guest setup

The in-tree driver targets Windows 7 SP1 x86 and x64, WDDM 1.1. Its canonical hardware identity is `PCI\VEN_A3A0&DEV_0001`; package metadata, INF selection, UMD/KMD relationships and debug tooling are documented in `drivers/aerogpu/README.md` and `drivers/aerogpu/packaging/win7/README.md`.

Builds are not performed on macOS. Use a Windows 10/11 x64 build environment with WDK 10 and MSBuild:

```text
scripts/build-win7-guest-tools.sh
scripts/package-win7-guest-tools.sh
```

The scripts intentionally fail elsewhere rather than creating placeholders. Packaging creates catalogs and test-signs them; use only test-signed/local artifacts for development and enable Windows test signing only in the guest test VM. Do not commit certificates, PFX files, generated packages, or Microsoft binaries.

For installation, attach the generated Guest Tools media, install the matching architecture package via `pnputil -i -a <package>\aerogpu_dx11.inf`, then inspect `%WINDIR%\inf\setupapi.dev.log`, Device Manager status, and `aerogpu_dbgctl` output. A successful install has not yet been validated in the macOS frontend.

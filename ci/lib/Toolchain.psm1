Set-StrictMode -Version Latest

function Write-ToolchainLog {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Message,
    [ValidateSet('INFO', 'WARN', 'ERROR')]
    [string]$Level = 'INFO'
  )

  $timestamp = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss.fffK')
  Write-Host "[$timestamp] [$Level] $Message"
}

function Resolve-ExistingPath {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$LiteralPath
  )

  return (Resolve-Path -LiteralPath $LiteralPath).Path
}

function Get-VsWhereExe {
  [CmdletBinding()]
  param()

  $candidate = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (Test-Path -LiteralPath $candidate) {
    return (Resolve-ExistingPath -LiteralPath $candidate)
  }

  return $null
}

function Get-VsInstallationPath {
  [CmdletBinding()]
  param()

  $vswhere = Get-VsWhereExe
  if ($null -ne $vswhere) {
    $installPath = & $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -property installationPath 2>$null
    $installPath = [string]$installPath
    $installPath = $installPath.Trim()
    if (-not [string]::IsNullOrWhiteSpace($installPath) -and (Test-Path -LiteralPath $installPath)) {
      return (Resolve-ExistingPath -LiteralPath $installPath)
    }
  }

  if (-not [string]::IsNullOrWhiteSpace($env:VSINSTALLDIR) -and (Test-Path -LiteralPath $env:VSINSTALLDIR)) {
    return (Resolve-ExistingPath -LiteralPath $env:VSINSTALLDIR)
  }

  return $null
}

function Get-VsDevCmdBat {
  [CmdletBinding()]
  param()

  $installPath = Get-VsInstallationPath
  if ($null -eq $installPath) {
    return $null
  }

  $candidate = Join-Path $installPath 'Common7\Tools\VsDevCmd.bat'
  if (Test-Path -LiteralPath $candidate) {
    return (Resolve-ExistingPath -LiteralPath $candidate)
  }

  return $null
}

function Get-VcVarsAllBat {
  [CmdletBinding()]
  param()

  $installPath = Get-VsInstallationPath
  if ($null -eq $installPath) {
    return $null
  }

  $candidate = Join-Path $installPath 'VC\Auxiliary\Build\vcvarsall.bat'
  if (Test-Path -LiteralPath $candidate) {
    return (Resolve-ExistingPath -LiteralPath $candidate)
  }

  return $null
}

function Get-MSBuildExe {
  [CmdletBinding()]
  param()

  $vswhere = Get-VsWhereExe
  if ($null -ne $vswhere) {
    $findPatterns = @(
      'MSBuild\Current\Bin\amd64\MSBuild.exe',
      'MSBuild\Current\Bin\MSBuild.exe',
      'MSBuild\**\Bin\amd64\MSBuild.exe',
      'MSBuild\**\Bin\MSBuild.exe'
    )

    foreach ($pattern in $findPatterns) {
      $paths = & $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -find $pattern 2>$null
      foreach ($path in @($paths)) {
        if (-not [string]::IsNullOrWhiteSpace($path) -and (Test-Path -LiteralPath $path)) {
          return (Resolve-ExistingPath -LiteralPath $path)
        }
      }
    }
  }

  $cmd = Get-Command msbuild.exe -ErrorAction SilentlyContinue
  if ($null -ne $cmd -and -not [string]::IsNullOrWhiteSpace($cmd.Source) -and (Test-Path -LiteralPath $cmd.Source)) {
    return (Resolve-ExistingPath -LiteralPath $cmd.Source)
  }

  throw @"
msbuild.exe was not found.

Remediation:
  - Install Visual Studio 2022 or the Visual Studio 2022 Build Tools (MSBuild).
  - Verify that vswhere.exe is available and that MSBuild is installed.
"@
}

function ConvertTo-VersionSafe {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$VersionText
  )

  try {
    return [Version]$VersionText
  } catch {
    return $null
  }
}

function Get-WindowsKitsRoot {
  [CmdletBinding()]
  param()

  $regRoots = @(
    'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots'
  )

  foreach ($regRoot in $regRoots) {
    try {
      $props = Get-ItemProperty -Path $regRoot -ErrorAction Stop
      foreach ($propName in @('KitsRoot10', 'KitsRoot81')) {
        $prop = $props.PSObject.Properties[$propName]
        if ($null -eq $prop) {
          continue
        }
        $value = $prop.Value
        if ([string]::IsNullOrWhiteSpace($value)) {
          continue
        }

        $kitRoot = [string]$value
        $kitRoot = $kitRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
        if ([string]::IsNullOrWhiteSpace($kitRoot)) {
          continue
        }

        $base = Split-Path -Parent $kitRoot
        if (-not [string]::IsNullOrWhiteSpace($base) -and (Test-Path -LiteralPath $base)) {
          return (Resolve-ExistingPath -LiteralPath $base)
        }
      }
    } catch {
      # ignore and continue
    }
  }

  foreach ($pf in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
    if ([string]::IsNullOrWhiteSpace($pf)) {
      continue
    }

    $candidate = Join-Path $pf 'Windows Kits'
    if (Test-Path -LiteralPath $candidate) {
      return (Resolve-ExistingPath -LiteralPath $candidate)
    }
  }

  return $null
}

function Test-Inf2CatSupportsWin7 {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Inf2CatExe
  )

  if (-not (Test-Path -LiteralPath $Inf2CatExe)) {
    return $false
  }

  try {
    $help = & $Inf2CatExe '/?' 2>&1 | Out-String
  } catch {
    Write-ToolchainLog -Level WARN -Message "Failed to execute Inf2Cat.exe to verify Windows 7 support: $($_.Exception.Message)"
    return $false
  }

  return ($help -match '\b7_X86\b' -and $help -match '\b7_X64\b')
}

function Get-WindowsKitVersionBins {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$BinRoot
  )

  $bins = @()

  if (-not (Test-Path -LiteralPath $BinRoot)) {
    return $bins
  }

  $versionDirs =
    Get-ChildItem -LiteralPath $BinRoot -Directory -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
      Sort-Object { [Version]$_.Name } -Descending

  foreach ($dir in $versionDirs) {
    $bins += [pscustomobject]@{
      Version = [Version]$dir.Name
      Path    = $dir.FullName
      Source  = 'versioned'
    }
  }

  # Some installations expose tools under bin\x64, bin\x86 without a version folder.
  $bins += [pscustomobject]@{
    Version = [Version]'0.0.0.0'
    Path    = $BinRoot
    Source  = 'unversioned'
  }

  return $bins
}

function Find-KitTool {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$BinDir,
    [Parameter(Mandatory = $true)]
    [string]$ToolName,
    [string[]]$Architectures = @('x64', 'x86')
  )

  foreach ($arch in $Architectures) {
    $candidate = Join-Path $BinDir (Join-Path $arch $ToolName)
    if (Test-Path -LiteralPath $candidate) {
      return (Resolve-ExistingPath -LiteralPath $candidate)
    }
  }

  return $null
}

function Resolve-WindowsKitTool {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$ToolName,
    [string[]]$Architectures = @('x64', 'x86'),
    [string[]]$KitVersions = @('10', '8.1'),
    [switch]$RequireWin7Inf2Cat
  )

  $kitsRoot = Get-WindowsKitsRoot
  if ([string]::IsNullOrWhiteSpace($kitsRoot) -or -not (Test-Path -LiteralPath $kitsRoot)) {
    return $null
  }

  foreach ($kitVersion in $KitVersions) {
    $binRoot = Join-Path $kitsRoot (Join-Path $kitVersion 'bin')
    if (-not (Test-Path -LiteralPath $binRoot)) {
      continue
    }

    $bins = Get-WindowsKitVersionBins -BinRoot $binRoot
    foreach ($bin in $bins) {
      $exe = Find-KitTool -BinDir $bin.Path -ToolName $ToolName -Architectures $Architectures
      if ($null -eq $exe) {
        continue
      }

      if ($RequireWin7Inf2Cat -and -not (Test-Inf2CatSupportsWin7 -Inf2CatExe $exe)) {
        Write-ToolchainLog -Level WARN -Message "Ignoring Windows Kit candidate (tool=$ToolName, kit=$kitVersion, bin=$($bin.Path)) because it does not advertise 7_X86/7_X64 support."
        continue
      }

      return [pscustomobject]@{
        Exe = $exe
        KitFamily = $kitVersion
        KitBinDir = $bin.Path
        KitBinSource = $bin.Source
        KitToolVersion = $bin.Version.ToString()
      }
    }
  }

  return $null
}

function Resolve-WindowsKitToolchain {
  [CmdletBinding()]
  param(
    [Parameter()]
    [string[]]$KitVersions = @('10', '8.1'),
    [Parameter()]
    [switch]$RequireWin7Inf2Cat
  )

  $kitsRoot = Get-WindowsKitsRoot
  if ([string]::IsNullOrWhiteSpace($kitsRoot) -or -not (Test-Path -LiteralPath $kitsRoot)) {
    return $null
  }

  $inf2cat = Resolve-WindowsKitTool -ToolName 'Inf2Cat.exe' -Architectures @('x64', 'x86') -KitVersions $KitVersions -RequireWin7Inf2Cat:$RequireWin7Inf2Cat
  $signtool = Resolve-WindowsKitTool -ToolName 'signtool.exe' -Architectures @('x64', 'x86') -KitVersions $KitVersions
  $stampinf = Resolve-WindowsKitTool -ToolName 'stampinf.exe' -Architectures @('x64', 'x86') -KitVersions $KitVersions

  if ($null -eq $inf2cat -or $null -eq $signtool) {
    return $null
  }

  return [pscustomobject]@{
    Inf2CatExe = $inf2cat.Exe
    SignToolExe = $signtool.Exe
    StampInfExe = if ($null -ne $stampinf) { $stampinf.Exe } else { $null }
    WindowsKits = @{
      Inf2Cat = $inf2cat
      SignTool = $signtool
      StampInf = $stampinf
    }
  }
}

function Resolve-VsDriverPlatformToolset {
  [CmdletBinding()]
  param()

  $vsInstallPath = Get-VsInstallationPath
  if ([string]::IsNullOrWhiteSpace($vsInstallPath)) {
    return $null
  }

  $vcMsBuildRoot = Join-Path $vsInstallPath 'MSBuild\Microsoft\VC'
  if (-not (Test-Path -LiteralPath $vcMsBuildRoot)) {
    return $null
  }

  # WDK driver projects are selected through PlatformToolset, not through the
  # optional BuildCustomizations\Driver.props/targets pair. Current Visual
  # Studio/WDK releases install per-platform Toolset.props/targets files here:
  #
  #   ...\Platforms\<arch>\PlatformToolsets\
  #       WindowsKernelModeDriver10.0\Toolset.{props,targets}
  #
  # Search beneath the active VS installation because the v170 path component
  # and product edition are intentionally version-dependent.
  $toolsetProps = @(
    Get-ChildItem -LiteralPath $vcMsBuildRoot -Filter 'Toolset.props' -File -Recurse -ErrorAction SilentlyContinue |
      Where-Object {
        $_.Directory.Name -eq 'WindowsKernelModeDriver10.0' -and
        $_.Directory.Parent.Name -eq 'PlatformToolsets'
      }
  )

  $platforms = @('Win32', 'x64')
  $entries = @()
  foreach ($platform in $platforms) {
    $props = $toolsetProps |
      Where-Object {
        $relativePath = $_.FullName.Substring($vcMsBuildRoot.Length).TrimStart('\', '/')
        $relativePath -match "(?i)(^|[\\/])Platforms[\\/]$([Regex]::Escape($platform))[\\/]PlatformToolsets[\\/]WindowsKernelModeDriver10\.0[\\/]Toolset\.props$"
      } |
      Select-Object -First 1
    if ($null -eq $props) {
      return $null
    }

    $targets = Join-Path $props.DirectoryName 'Toolset.targets'
    if (-not (Test-Path -LiteralPath $targets)) {
      return $null
    }

    $entries += [pscustomobject]@{
      Platform = $platform
      Props = (Resolve-ExistingPath -LiteralPath $props.FullName)
      Targets = (Resolve-ExistingPath -LiteralPath $targets)
    }
  }

  if ($entries.Count -ne $platforms.Count) {
    return $null
  }

  return [pscustomobject]@{
    Name = 'WindowsKernelModeDriver10.0'
    Platforms = $entries
  }
}

function Get-VsInstallerSetupExe {
  [CmdletBinding()]
  param()

  $candidate = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\setup.exe'
  if (Test-Path -LiteralPath $candidate) {
    return (Resolve-ExistingPath -LiteralPath $candidate)
  }

  return $null
}

function Resolve-WindowsDriverKitBuildSupport {
  [CmdletBinding()]
  param(
    [Parameter()]
    [string]$PreferredKitVersion = '10.0.22621.0'
  )

  $kitsRoot = Get-WindowsKitsRoot
  if ([string]::IsNullOrWhiteSpace($kitsRoot)) {
    return $null
  }

  $includeRoot = Join-Path $kitsRoot '10\Include'
  $buildRoot = Join-Path $kitsRoot '10\build'
  if (-not (Test-Path -LiteralPath $includeRoot) -or -not (Test-Path -LiteralPath $buildRoot)) {
    return $null
  }

  $versions = @()
  if (-not [string]::IsNullOrWhiteSpace($PreferredKitVersion)) {
    $versions += $PreferredKitVersion
  } else {
    $versions += @(
      Get-ChildItem -LiteralPath $includeRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object { [Version]$_.Name } -Descending |
        ForEach-Object { $_.Name }
    )
  }
  $versions = @($versions | Select-Object -Unique)

  $driverPlatformToolset = Resolve-VsDriverPlatformToolset
  if ($null -eq $driverPlatformToolset) {
    return $null
  }

  foreach ($version in $versions) {
    $versionIncludeRoot = Join-Path $includeRoot $version
    $versionBuildRoot = Join-Path $buildRoot $version
    $requiredPaths = @(
      (Join-Path $versionIncludeRoot 'km\ntddk.h'),
      (Join-Path $versionIncludeRoot 'km\ndis.h'),
      (Join-Path $versionBuildRoot 'WindowsDriver.Common.props'),
      (Join-Path $versionBuildRoot 'WindowsDriver.Default.props')
    )
    if (@($requiredPaths | Where-Object { -not (Test-Path -LiteralPath $_) }).Count -ne 0) {
      continue
    }

    $requiredDdiHeaders = @(
      'd3d10umddi.h',
      'd3dumddi.h',
      'd3dkmthk.h'
    )
    $missingDdiHeader = $false
    foreach ($header in $requiredDdiHeaders) {
      $found = $false
      foreach ($includeKind in @('um', 'shared')) {
        if (Test-Path -LiteralPath (Join-Path $versionIncludeRoot (Join-Path $includeKind $header))) {
          $found = $true
          break
        }
      }
      if (-not $found) {
        $missingDdiHeader = $true
        break
      }
    }
    if ($missingDdiHeader) {
      continue
    }

    return [pscustomobject]@{
      KitVersion = $version
      IncludeRoot = (Resolve-ExistingPath -LiteralPath $versionIncludeRoot)
      BuildRoot = (Resolve-ExistingPath -LiteralPath $versionBuildRoot)
      DriverPlatformToolset = $driverPlatformToolset
    }
  }

  return $null
}

function Get-WindowsKitPayloadState {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$KitVersion,
    [switch]$RequireDriverPayload,
    [switch]$RequireSdkTools
  )

  $missing = @()
  $kitsRoot = Get-WindowsKitsRoot
  if ([string]::IsNullOrWhiteSpace($kitsRoot)) {
    return [pscustomobject]@{
      Ready = $false
      Missing = @('Windows Kits installation root')
    }
  }

  $kit10Root = Join-Path $kitsRoot '10'
  $versionIncludeRoot = Join-Path $kit10Root (Join-Path 'Include' $KitVersion)
  $versionBuildRoot = Join-Path $kit10Root (Join-Path 'build' $KitVersion)
  $versionBinRoot = Join-Path $kit10Root (Join-Path 'bin' $KitVersion)

  if ($RequireDriverPayload) {
    $requiredPaths = @(
      (Join-Path $versionIncludeRoot 'km\ntddk.h'),
      (Join-Path $versionIncludeRoot 'km\ndis.h'),
      (Join-Path $versionBuildRoot 'WindowsDriver.Common.props'),
      (Join-Path $versionBuildRoot 'WindowsDriver.Default.props')
    )
    foreach ($path in $requiredPaths) {
      if (-not (Test-Path -LiteralPath $path)) {
        $missing += $path
      }
    }

    foreach ($header in @('d3d10umddi.h', 'd3dumddi.h', 'd3dkmthk.h')) {
      $found = $false
      foreach ($includeKind in @('um', 'shared')) {
        if (Test-Path -LiteralPath (Join-Path $versionIncludeRoot (Join-Path $includeKind $header))) {
          $found = $true
          break
        }
      }
      if (-not $found) {
        $missing += (Join-Path $versionIncludeRoot "<um|shared>\$header")
      }
    }

    if ($null -eq (Find-KitTool -BinDir $versionBinRoot -ToolName 'Inf2Cat.exe' -Architectures @('x64', 'x86'))) {
      $missing += (Join-Path $versionBinRoot '<x64|x86>\Inf2Cat.exe')
    }
    if ($null -eq (Find-KitTool -BinDir $versionBinRoot -ToolName 'stampinf.exe' -Architectures @('x64', 'x86'))) {
      $missing += (Join-Path $versionBinRoot '<x64|x86>\stampinf.exe')
    }
  }

  if ($RequireSdkTools) {
    if ($null -eq (Find-KitTool -BinDir $versionBinRoot -ToolName 'signtool.exe' -Architectures @('x64', 'x86'))) {
      $missing += (Join-Path $versionBinRoot '<x64|x86>\signtool.exe')
    }
  }

  return [pscustomobject]@{
    Ready = ($missing.Count -eq 0)
    Missing = @($missing)
  }
}

function Wait-WindowsKitPayload {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$KitVersion,
    [switch]$RequireDriverPayload,
    [switch]$RequireSdkTools,
    [int]$TimeoutSeconds = 600
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $pollCount = 0
  do {
    $state = Get-WindowsKitPayloadState `
      -KitVersion $KitVersion `
      -RequireDriverPayload:$RequireDriverPayload `
      -RequireSdkTools:$RequireSdkTools
    if ($state.Ready) {
      Write-ToolchainLog -Message "Windows Kit $KitVersion payload is ready."
      return
    }

    if (($pollCount % 6) -eq 0) {
      Write-ToolchainLog -Level WARN -Message "Waiting for the detached Windows Kit $KitVersion installation to finish; missing $($state.Missing.Count) item(s)."
    }
    $pollCount += 1
    Start-Sleep -Seconds 5
  } while ([DateTime]::UtcNow -lt $deadline)

  $missingText = ($state.Missing | ForEach-Object { "  - $_" }) -join "`n"
  throw @"
Timed out waiting for the Windows Kit $KitVersion payload after $TimeoutSeconds seconds.

Still missing:
$missingText
"@
}

function Install-VsDriverKitComponent {
  [CmdletBinding()]
  param()

  $setupExe = Get-VsInstallerSetupExe
  $vsInstallPath = Get-VsInstallationPath
  if ([string]::IsNullOrWhiteSpace($setupExe) -or [string]::IsNullOrWhiteSpace($vsInstallPath)) {
    throw 'Visual Studio Installer setup.exe or the Visual Studio installation path could not be resolved.'
  }

  Invoke-ExternalCommand `
    -FilePath $setupExe `
    -Arguments @(
      'modify',
      '--installPath', $vsInstallPath,
      '--add', 'Component.Microsoft.Windows.DriverKit',
      '--quiet',
      '--norestart'
    ) `
    -AcceptedExitCodes @(0, 1641, 3010) `
    -FailureHint @"
Visual Studio 2022 version 17.11 and newer package WDK MSBuild integration as the
Component.Microsoft.Windows.DriverKit individual component.
"@
}

function Wait-VsDriverKitIntegration {
  [CmdletBinding()]
  param(
    [int]$TimeoutSeconds = 600
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $pollCount = 0
  do {
    $driverPlatformToolset = Resolve-VsDriverPlatformToolset
    if ($null -ne $driverPlatformToolset) {
      $platformNames = @($driverPlatformToolset.Platforms | ForEach-Object { $_.Platform }) -join ', '
      Write-ToolchainLog -Message "Visual Studio WDK platform toolset is ready for: $platformNames"
      return
    }
    if (($pollCount % 6) -eq 0) {
      Write-ToolchainLog -Level WARN -Message 'Waiting for the Visual Studio WindowsKernelModeDriver10.0 platform toolset...'
    }
    $pollCount += 1
    Start-Sleep -Seconds 5
  } while ([DateTime]::UtcNow -lt $deadline)

  throw "Timed out waiting for the Visual Studio WindowsKernelModeDriver10.0 platform toolset after $TimeoutSeconds seconds."
}

function Wait-Win7Inf2Cat {
  [CmdletBinding()]
  param(
    [int]$TimeoutSeconds = 600
  )

  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $pollCount = 0
  do {
    $inf2cat = Resolve-WindowsKitTool `
      -ToolName 'Inf2Cat.exe' `
      -Architectures @('x64', 'x86') `
      -KitVersions @('10', '8.1') `
      -RequireWin7Inf2Cat
    if ($null -ne $inf2cat) {
      Write-ToolchainLog -Message "Windows 7-capable Inf2Cat is ready: $($inf2cat.Exe)"
      return
    }
    if (($pollCount % 6) -eq 0) {
      Write-ToolchainLog -Level WARN -Message 'Waiting for the Windows 7-capable Inf2Cat installation to finish...'
    }
    $pollCount += 1
    Start-Sleep -Seconds 5
  } while ([DateTime]::UtcNow -lt $deadline)

  throw "Timed out waiting for an Inf2Cat.exe that supports 7_X86 and 7_X64 after $TimeoutSeconds seconds."
}

function Get-WingetExe {
  [CmdletBinding()]
  param()

  $cmd = Get-Command winget.exe -ErrorAction SilentlyContinue
  if ($null -ne $cmd -and -not [string]::IsNullOrWhiteSpace($cmd.Source) -and (Test-Path -LiteralPath $cmd.Source)) {
    return (Resolve-ExistingPath -LiteralPath $cmd.Source)
  }

  return $null
}

function Get-WingetPackageVersionsForKit {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$WingetExe,
    [Parameter(Mandatory = $true)]
    [string]$WingetId,
    [Parameter(Mandatory = $true)]
    [string]$PreferredKitVersion
  )

  $preferredVersion = ConvertTo-VersionSafe -VersionText $PreferredKitVersion
  if ($null -eq $preferredVersion) {
    return @()
  }

  try {
    $output = & $WingetExe show --id $WingetId --exact --versions --source winget --accept-source-agreements 2>&1
    if ($LASTEXITCODE -ne 0) {
      return @()
    }
  } catch {
    Write-ToolchainLog -Level WARN -Message "Failed to query winget versions for '$WingetId': $($_.Exception.Message)"
    return @()
  }

  $versions = @()
  foreach ($line in @($output)) {
    $text = ([string]$line).Trim()
    if ($text -notmatch '^\d+\.\d+\.\d+\.\d+$') {
      continue
    }
    $version = ConvertTo-VersionSafe -VersionText $text
    if ($null -ne $version -and $version.Build -eq $preferredVersion.Build) {
      $versions += $version
    }
  }

  return @(
    $versions |
      Sort-Object -Descending |
      ForEach-Object { $_.ToString() } |
      Select-Object -Unique
  )
}

function Test-IsAdministrator {
  [CmdletBinding()]
  param()

  try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
  } catch {
    return $false
  }
}

function Invoke-ExternalCommand {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [Parameter(Mandatory = $true)]
    [string[]]$Arguments,
    [string]$FailureHint,
    [int[]]$AcceptedExitCodes = @(0)
  )

  $prettyArgs = ($Arguments | ForEach-Object {
    if ($_ -match '\s') { '"' + $_.Replace('"', '\"') + '"' } else { $_ }
  }) -join ' '

  Write-ToolchainLog -Message "Running: $FilePath $prettyArgs"

  $stdoutFile = [System.IO.Path]::GetTempFileName()
  $stderrFile = [System.IO.Path]::GetTempFileName()

  try {
    $proc = Start-Process `
      -FilePath $FilePath `
      -ArgumentList $prettyArgs `
      -NoNewWindow `
      -Wait `
      -PassThru `
      -RedirectStandardOutput $stdoutFile `
      -RedirectStandardError $stderrFile

    if ($AcceptedExitCodes -notcontains $proc.ExitCode) {
      $stdout = Get-Content -LiteralPath $stdoutFile -Raw -ErrorAction SilentlyContinue
      $stderr = Get-Content -LiteralPath $stderrFile -Raw -ErrorAction SilentlyContinue

      $details = @()
      if (-not [string]::IsNullOrWhiteSpace($stdout)) { $details += "stdout:`n$stdout" }
      if (-not [string]::IsNullOrWhiteSpace($stderr)) { $details += "stderr:`n$stderr" }

      $hint = ''
      if (-not [string]::IsNullOrWhiteSpace($FailureHint)) {
        $hint = "`n`n$FailureHint"
      }

      $detailText = ''
      if ($details.Count -gt 0) {
        $detailText = "`n`n" + ($details -join "`n`n")
      }

      throw "Command failed with exit code $($proc.ExitCode): $FilePath $prettyArgs$detailText$hint"
    }
  } finally {
    Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
  }
}

function Get-MicrosoftKitBootstrapper {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Uri,
    [Parameter(Mandatory = $true)]
    [string]$FileName,
    [Parameter(Mandatory = $true)]
    [string]$DisplayName,
    [string]$DownloadDirectory
  )

  $downloadUri = [Uri]$Uri
  if ($downloadUri.Scheme -ne 'https' -or $downloadUri.Host -notin @('go.microsoft.com', 'download.microsoft.com')) {
    throw "Refusing to download $DisplayName from a non-Microsoft HTTPS URL: $Uri"
  }

  if ([string]::IsNullOrWhiteSpace($DownloadDirectory)) {
    $DownloadDirectory = Join-Path ([System.IO.Path]::GetTempPath()) 'aero-wdk-download-cache'
  } elseif (-not [System.IO.Path]::IsPathRooted($DownloadDirectory)) {
    $DownloadDirectory = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $DownloadDirectory))
  }

  if (-not (Test-Path -LiteralPath $DownloadDirectory)) {
    New-Item -ItemType Directory -Force -Path $DownloadDirectory | Out-Null
  }

  $installerPath = Join-Path $DownloadDirectory $FileName

  if (Test-Path -LiteralPath $installerPath) {
    $signature = Get-AuthenticodeSignature -LiteralPath $installerPath
    if ($signature.Status -eq [System.Management.Automation.SignatureStatus]::Valid -and
        $null -ne $signature.SignerCertificate -and
        $signature.SignerCertificate.Subject -match 'Microsoft') {
      Write-ToolchainLog -Message "Using cached Microsoft-signed $DisplayName bootstrapper: $installerPath"
      return (Resolve-ExistingPath -LiteralPath $installerPath)
    }

    Write-ToolchainLog -Level WARN -Message "Discarding cached $DisplayName bootstrapper because its Microsoft Authenticode signature is not valid (status=$($signature.Status)): $installerPath"
    Remove-Item -LiteralPath $installerPath -Force
  }

  $partialPath = "$installerPath.download"
  Remove-Item -LiteralPath $partialPath -Force -ErrorAction SilentlyContinue

  try {
    Write-ToolchainLog -Message "Downloading the official $DisplayName bootstrapper from Microsoft: $Uri"
    Invoke-WebRequest -Uri $Uri -OutFile $partialPath -UseBasicParsing

    $signature = Get-AuthenticodeSignature -LiteralPath $partialPath
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
        $null -eq $signature.SignerCertificate -or
        $signature.SignerCertificate.Subject -notmatch 'Microsoft') {
      throw "Downloaded $DisplayName bootstrapper does not have a valid Microsoft Authenticode signature (status=$($signature.Status))."
    }

    Move-Item -LiteralPath $partialPath -Destination $installerPath -Force
  } finally {
    Remove-Item -LiteralPath $partialPath -Force -ErrorAction SilentlyContinue
  }

  return (Resolve-ExistingPath -LiteralPath $installerPath)
}

function Install-MicrosoftKitBootstrapper {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Uri,
    [Parameter(Mandatory = $true)]
    [string]$FileName,
    [Parameter(Mandatory = $true)]
    [string]$DisplayName,
    [string]$DownloadDirectory
  )

  $installer = Get-MicrosoftKitBootstrapper `
    -Uri $Uri `
    -FileName $FileName `
    -DisplayName $DisplayName `
    -DownloadDirectory $DownloadDirectory

  Invoke-ExternalCommand `
    -FilePath $installer `
    -Arguments @('/features', '+', '/quiet', '/norestart') `
    -AcceptedExitCodes @(0, 1641, 3010) `
    -FailureHint @"
The official Microsoft bootstrapper failed. Its setup logs are normally written under:
  $env:TEMP\Windows Kits
"@
}

function Install-WingetPackage {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$WingetId,
    [string]$WingetVersion,
    [Parameter(Mandatory = $true)]
    [string]$DisplayName,
    [string]$DownloadDirectory
  )

  $winget = Get-WingetExe
  if ($null -eq $winget) {
    throw @"
winget.exe was not found, so $DisplayName cannot be installed automatically.

Remediation:
  - Install winget (App Installer) or install $DisplayName manually from Microsoft.
"@
  }

  $baseArgs = @(
    'install',
    '--id', $WingetId,
    '--exact',
    '--source', 'winget',
    '--accept-source-agreements',
    '--accept-package-agreements',
    '--silent'
  )

  $downloadDirFull = $null
  if (-not [string]::IsNullOrWhiteSpace($DownloadDirectory)) {
    $downloadDirFull = $DownloadDirectory
    if (-not [System.IO.Path]::IsPathRooted($downloadDirFull)) {
      $downloadDirFull = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $downloadDirFull))
    }

    if (-not (Test-Path -LiteralPath $downloadDirFull)) {
      New-Item -ItemType Directory -Force -Path $downloadDirFull | Out-Null
    }

    $baseArgs += @('--download-directory', $downloadDirFull)
  }

  $flagSets = @(
    @('--disable-interactivity', '--force'),
    @('--disable-interactivity'),
    @('--force'),
    @()
  )

  if (-not [string]::IsNullOrWhiteSpace($WingetVersion)) {
    $baseArgs += @('--version', $WingetVersion)
  }

  foreach ($flags in $flagSets) {
    $args = @($baseArgs + $flags)
    try {
      Invoke-ExternalCommand -FilePath $winget -Arguments $args -FailureHint @"
If this keeps failing on a CI runner, check whether the winget package ID/version has changed.
You can inspect available versions with:
  winget show --id $WingetId --versions
"@
      return
    } catch {
      $message = $_.Exception.Message

      $unknownForce = ($flags -contains '--force') -and ($message -match '(?i)(unknown|unrecognized).*(--force)')
      $unknownDisable = ($flags -contains '--disable-interactivity') -and ($message -match '(?i)(unknown|unrecognized).*(--disable-interactivity)')
      $unknownDownloadDir = ($baseArgs -contains '--download-directory') -and ($message -match '(?i)(unknown|unrecognized).*(--download-directory)')

      # If the failure is clearly due to an unsupported flag, try the next reduced flag set.
      if ($unknownForce -or $unknownDisable -or $unknownDownloadDir) {
        Write-ToolchainLog -Level WARN -Message "winget does not support one or more flags ($($flags -join ' ')); retrying with fewer flags."
        if ($unknownDownloadDir) {
          Write-ToolchainLog -Level WARN -Message 'winget does not support --download-directory; continuing without download caching.'
          $baseArgs = $baseArgs | Where-Object { $_ -ne '--download-directory' -and $_ -ne $downloadDirFull }
          $downloadDirFull = $null
        }
        continue
      }

      throw
    }
  }

  throw "winget install failed for $WingetId with all supported flag combinations."
}

function Ensure-WindowsKitToolchain {
  [CmdletBinding()]
  param(
    [Parameter()]
    [string]$PreferredWdkWingetId = 'Microsoft.WindowsWDK',
    [Parameter()]
    [string]$PreferredWdkKitVersion = '10.0.22621.0',
    [Parameter()]
    [string]$PreferredSdkBootstrapUri = 'https://go.microsoft.com/fwlink/?linkid=2311806',
    [Parameter()]
    [string]$PreferredWdkBootstrapUri = 'https://go.microsoft.com/fwlink/?linkid=2330411',
    [Parameter()]
    [string]$LegacyWin7KitVersion = '10.0.19041.0',
    [Parameter()]
    [string]$LegacyWin7SdkBootstrapUri = 'https://go.microsoft.com/fwlink/?linkid=2311805',
    [Parameter()]
    [string]$LegacyWin7WdkBootstrapUri = 'https://go.microsoft.com/fwlink/?linkid=2342425'
  )

  $toolchain = Resolve-WindowsKitToolchain -RequireWin7Inf2Cat
  $driverBuildSupport = Resolve-WindowsDriverKitBuildSupport -PreferredKitVersion $PreferredWdkKitVersion
  if ($null -ne $toolchain -and $null -ne $driverBuildSupport) {
    $toolchain.WindowsKits.DriverBuild = $driverBuildSupport
    return $toolchain
  }

  $kitFamilyPreference = @('10', '8.1')
  $inf2cat = Resolve-WindowsKitTool -ToolName 'Inf2Cat.exe' -Architectures @('x64', 'x86') -KitVersions $kitFamilyPreference -RequireWin7Inf2Cat
  $signtool = Resolve-WindowsKitTool -ToolName 'signtool.exe' -Architectures @('x64', 'x86') -KitVersions $kitFamilyPreference

  $needsDriverBuildSupport = ($null -eq $driverBuildSupport)
  $needsWdk = ($null -eq $inf2cat -or $needsDriverBuildSupport)
  $needsSdk = ($null -eq $signtool -or $needsDriverBuildSupport)

  $missing = @()
  if ($null -eq $inf2cat) { $missing += 'Inf2Cat.exe with Windows 7 catalog targets (WDK)' }
  if ($null -eq $signtool) { $missing += 'signtool.exe (Windows SDK)' }
  if ($needsDriverBuildSupport) { $missing += "complete WDK build support (headers + MSBuild integration for $PreferredWdkKitVersion)" }
  $missing = @($missing | Select-Object -Unique)

  if (-not (Test-IsAdministrator)) {
    throw @"
Windows driver tooling is missing ($($missing -join ', ')), but this process is not running with Administrator privileges.

Remediation:
  - Re-run this script in an elevated PowerShell (Run as Administrator), or
  - Install the Windows SDK/WDK manually.
"@
  }

  Write-ToolchainLog -Level WARN -Message "Required Windows Kits tooling not found ($($missing -join ', ')). Attempting to install the paired Windows SDK/WDK..."

  $winget = Get-WingetExe
  if ($null -eq $winget) {
    Write-ToolchainLog -Level WARN -Message 'winget.exe was not found. The official Microsoft standalone SDK/WDK bootstrappers will be used.'
  }

  # winget versions for SDK/WDK installers do not always match the installed Kit version exactly.
  $preferredVersion = ConvertTo-VersionSafe -VersionText $PreferredWdkKitVersion
  $preferredBuild = if ($null -ne $preferredVersion) { $preferredVersion.Build } else { $null }
  $discoveredWingetVersions = @()
  if ($null -ne $winget) {
    $discoveredWingetVersions += Get-WingetPackageVersionsForKit -WingetExe $winget -WingetId $PreferredWdkWingetId -PreferredKitVersion $PreferredWdkKitVersion
    $discoveredWingetVersions += Get-WingetPackageVersionsForKit -WingetExe $winget -WingetId 'Microsoft.WindowsSDK' -PreferredKitVersion $PreferredWdkKitVersion
  }
  $versionCandidates = @(
    $discoveredWingetVersions,
    ($preferredBuild | ForEach-Object { if ($_ -ne $null) { "10.0.$_.2428" } }),
    ($preferredBuild | ForEach-Object { if ($_ -ne $null) { "10.1.$_.2428" } }),
    ($preferredBuild | ForEach-Object { if ($_ -ne $null) { "10.1.$_.382" } }),
    $PreferredWdkKitVersion,
    ($PreferredWdkKitVersion -replace '^10\.0\.', '10.1.'),
    ($preferredBuild | ForEach-Object { if ($_ -ne $null) { "10.1.$_.1" } })
  ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique
  # Last resort: install whatever winget considers "latest" if we can't match the pinned version string.
  $versionCandidates += $null

  $wdkIdCandidates = @(
    $PreferredWdkWingetId,
    'Microsoft.WindowsDriverKit'
  ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique
  $sdkIdCandidates = @('Microsoft.WindowsSDK')

  if ($null -ne $winget) {
    foreach ($versionCandidate in $versionCandidates) {
      $versionLabel = if ([string]::IsNullOrWhiteSpace($versionCandidate)) { 'latest' } else { $versionCandidate }

      if ($needsSdk) {
        foreach ($sdkId in $sdkIdCandidates) {
          try {
            Write-ToolchainLog -Message "Installing Windows SDK via winget (id=$sdkId, version=$versionLabel)..."
            Install-WingetPackage -WingetId $sdkId -WingetVersion $versionCandidate -DisplayName 'Windows SDK' -DownloadDirectory $env:WDK_DOWNLOAD_CACHE
            break
          } catch {
            Write-ToolchainLog -Level WARN -Message "Windows SDK install attempt failed (id=$sdkId, version=$versionLabel): $($_.Exception.Message)"
          }
        }
      }

      if ($needsWdk) {
        foreach ($wdkId in $wdkIdCandidates) {
          try {
            Write-ToolchainLog -Message "Installing Windows Driver Kit via winget (id=$wdkId, version=$versionLabel)..."
            Install-WingetPackage -WingetId $wdkId -WingetVersion $versionCandidate -DisplayName 'Windows Driver Kit (WDK)' -DownloadDirectory $env:WDK_DOWNLOAD_CACHE
            break
          } catch {
            Write-ToolchainLog -Level WARN -Message "WDK install attempt failed (id=$wdkId, version=$versionLabel): $($_.Exception.Message)"
          }
        }
      }

      $toolchain = Resolve-WindowsKitToolchain -RequireWin7Inf2Cat
      $driverBuildSupport = Resolve-WindowsDriverKitBuildSupport -PreferredKitVersion $PreferredWdkKitVersion
      if ($null -ne $toolchain -and $null -ne $driverBuildSupport) {
        $toolchain.WindowsKits.DriverBuild = $driverBuildSupport
        return $toolchain
      }

      $needsDriverBuildSupport = ($null -eq $driverBuildSupport)
      $needsWdk = (
        $null -eq (Resolve-WindowsKitTool -ToolName 'Inf2Cat.exe' -Architectures @('x64', 'x86') -KitVersions $kitFamilyPreference -RequireWin7Inf2Cat) -or
        $needsDriverBuildSupport
      )
      $needsSdk = (
        $null -eq (Resolve-WindowsKitTool -ToolName 'signtool.exe' -Architectures @('x64', 'x86') -KitVersions $kitFamilyPreference) -or
        $needsDriverBuildSupport
      )
      if (-not $needsWdk -and -not $needsSdk) {
        break
      }
    }
  }

  try {
    if ($needsSdk) {
      Write-ToolchainLog -Message "Installing the Windows SDK for kit $PreferredWdkKitVersion via Microsoft's official standalone bootstrapper..."
      Install-MicrosoftKitBootstrapper `
        -Uri $PreferredSdkBootstrapUri `
        -FileName "winsdksetup-$PreferredWdkKitVersion.exe" `
        -DisplayName 'Windows SDK' `
        -DownloadDirectory $env:WDK_DOWNLOAD_CACHE
    }

    if ($needsWdk) {
      Write-ToolchainLog -Message "Installing the Windows Driver Kit for kit $PreferredWdkKitVersion via Microsoft's official standalone bootstrapper..."
      Install-MicrosoftKitBootstrapper `
        -Uri $PreferredWdkBootstrapUri `
        -FileName "wdksetup-$PreferredWdkKitVersion.exe" `
        -DisplayName 'Windows Driver Kit (WDK)' `
        -DownloadDirectory $env:WDK_DOWNLOAD_CACHE
    }
  } catch {
    Write-ToolchainLog -Level WARN -Message "Official Microsoft SDK/WDK bootstrapper install attempt failed: $($_.Exception.Message)"
  }

  if ($needsSdk -or $needsWdk) {
    Wait-WindowsKitPayload `
      -KitVersion $PreferredWdkKitVersion `
      -RequireDriverPayload:$needsWdk `
      -RequireSdkTools:$needsSdk
  }

  if ($null -eq (Resolve-VsDriverPlatformToolset)) {
    Write-ToolchainLog -Message 'Installing the Visual Studio Windows Driver Kit component required by VS 2022 17.11 and newer...'
    Install-VsDriverKitComponent
    Wait-VsDriverKitIntegration
  }

  $win7Inf2Cat = Resolve-WindowsKitTool `
    -ToolName 'Inf2Cat.exe' `
    -Architectures @('x64', 'x86') `
    -KitVersions @('10', '8.1') `
    -RequireWin7Inf2Cat
  if ($null -eq $win7Inf2Cat) {
    Write-ToolchainLog -Level WARN -Message "The installed modern WDK does not provide Windows 7 catalog targets. Installing Microsoft's supported legacy Windows 7 kit line ($LegacyWin7KitVersion) for Inf2Cat..."

    Install-MicrosoftKitBootstrapper `
      -Uri $LegacyWin7SdkBootstrapUri `
      -FileName "winsdksetup-$LegacyWin7KitVersion.exe" `
      -DisplayName "Windows SDK $LegacyWin7KitVersion" `
      -DownloadDirectory $env:WDK_DOWNLOAD_CACHE
    Install-MicrosoftKitBootstrapper `
      -Uri $LegacyWin7WdkBootstrapUri `
      -FileName "wdksetup-$LegacyWin7KitVersion.exe" `
      -DisplayName "Windows Driver Kit (WDK) $LegacyWin7KitVersion" `
      -DownloadDirectory $env:WDK_DOWNLOAD_CACHE

    Wait-Win7Inf2Cat
  }

  $toolchain = Resolve-WindowsKitToolchain -RequireWin7Inf2Cat
  $driverBuildSupport = Resolve-WindowsDriverKitBuildSupport -PreferredKitVersion $PreferredWdkKitVersion
  if ($null -ne $toolchain -and $null -ne $driverBuildSupport) {
    $toolchain.WindowsKits.DriverBuild = $driverBuildSupport
    return $toolchain
  }

  throw @"
Windows driver toolchain tooling is still missing after installation attempts.

Expected tools:
  - Inf2Cat.exe from the $LegacyWin7KitVersion kit line (must support /os:7_X86,7_X64)
  - signtool.exe (Windows SDK)
  - WDK kernel/DDI headers and Visual Studio driver build integration for $PreferredWdkKitVersion

Remediation:
  1. Inspect the Microsoft installer logs under:
       $env:TEMP\Windows Kits
  2. Install the Windows SDK and WDK manually and ensure they install under:
       ${env:ProgramFiles(x86)}\Windows Kits\10
  3. Re-run: pwsh -File ci/install-wdk.ps1

If you have multiple Windows Kits installed, this script selects the newest versioned bin directory for each tool (Inf2Cat.exe, signtool.exe, stampinf.exe).
"@
}

function Add-PathEntry {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Directory
  )

  if ([string]::IsNullOrWhiteSpace($Directory) -or -not (Test-Path -LiteralPath $Directory)) {
    return
  }

  $current = $env:PATH
  $segments = @()
  if (-not [string]::IsNullOrWhiteSpace($current)) {
    $segments = $current -split ';'
  }

  if ($segments -contains $Directory) {
    return
  }

  $env:PATH = "$Directory;$current"
}

function Publish-ToolchainToGitHubActions {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [pscustomobject]$Toolchain
  )

  $outputs = @{
    'toolchain_json' = $Toolchain.ToolchainJson
    'msbuild_exe'  = $Toolchain.MSBuildExe
    'inf2cat_exe'  = $Toolchain.Inf2CatExe
    'signtool_exe' = $Toolchain.SignToolExe
    'stampinf_exe' = $Toolchain.StampInfExe
  }

  if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_OUTPUT)) {
    foreach ($key in $outputs.Keys) {
      $val = $outputs[$key]
      if ($null -ne $val -and -not [string]::IsNullOrWhiteSpace($val)) {
        Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "$key=$val"
      }
    }
  }

  if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_ENV)) {
    foreach ($key in $outputs.Keys) {
      $envKey = $key.ToUpperInvariant()
      $val = $outputs[$key]
      if ($null -ne $val -and -not [string]::IsNullOrWhiteSpace($val)) {
        Add-Content -LiteralPath $env:GITHUB_ENV -Value "$envKey=$val"
      }
    }
  }

  if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_PATH)) {
    $dirs = @(
      (Split-Path -Path $Toolchain.MSBuildExe -Parent),
      (Split-Path -Path $Toolchain.Inf2CatExe -Parent),
      (Split-Path -Path $Toolchain.SignToolExe -Parent)
    )

    if (-not [string]::IsNullOrWhiteSpace($Toolchain.StampInfExe)) {
      $dirs += (Split-Path -Path $Toolchain.StampInfExe -Parent)
    }

    foreach ($dir in ($dirs | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)) {
      Add-Content -LiteralPath $env:GITHUB_PATH -Value $dir
    }
  }
}

Export-ModuleMember -Function `
  Write-ToolchainLog, `
  Get-VsDevCmdBat, `
  Get-VcVarsAllBat, `
  Get-MSBuildExe, `
  Ensure-WindowsKitToolchain, `
  Add-PathEntry, `
  Publish-ToolchainToGitHubActions

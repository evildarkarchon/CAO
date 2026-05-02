<#
.SYNOPSIS
Loads the Visual Studio x64 developer environment for repository builds.

.DESCRIPTION
Finds vcvars64.bat from an explicit Visual Studio path, the current VSINSTALLDIR,
or vswhere.exe, then imports the environment variables it sets into the current
PowerShell process. Any command passed after PowerShell's "--%" stop-parsing
token runs after the environment is loaded, which is useful for one-shot CMake
configure/build invocations.

.PARAMETER VisualStudioPath
Optional path to a Visual Studio installation root containing
VC\Auxiliary\Build\vcvars64.bat.

.PARAMETER VsWherePath
Optional path to vswhere.exe. Defaults to the Visual Studio Installer location.

.PARAMETER Force
Reload vcvars64.bat even when an x64 Visual Studio developer environment already
appears to be active.

.PARAMETER Quiet
Suppress status output.

.EXAMPLE
.\scripts\Invoke-VcVars64.ps1

.EXAMPLE
.\scripts\Invoke-VcVars64.ps1 --% cmake --preset ninja-windows
#>
[CmdletBinding(PositionalBinding = $false)]
param(
  [string] $VisualStudioPath,
  [string] $VsWherePath,
  [switch] $Force,
  [switch] $Quiet,
  [Parameter(Position = 0, ValueFromRemainingArguments = $true)]
  [string[]] $CommandLine
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Status {
  <#
  .SYNOPSIS
  Writes user-facing status unless quiet mode is enabled.
  #>
  param(
    [Parameter(Mandatory)]
    [string] $Message
  )

  if (-not $Quiet) {
    Write-Host $Message
  }
}

function Get-DefaultVsWherePath {
  <#
  .SYNOPSIS
  Returns the default Visual Studio Installer vswhere.exe path.
  #>
  $programFilesX86 = ${env:ProgramFiles(x86)}
  if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
    return $null
  }

  return Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
}

function Test-X64MsvcEnvironment {
  <#
  .SYNOPSIS
  Checks whether the current process already has the x64 MSVC build environment.
  #>
  if ([string]::IsNullOrWhiteSpace($env:VCToolsInstallDir)) {
    return $false
  }

  if ([string]::IsNullOrWhiteSpace($env:WindowsSdkDir)) {
    return $false
  }

  if (-not [string]::IsNullOrWhiteSpace($env:VSCMD_ARG_TGT_ARCH)) {
    return $env:VSCMD_ARG_TGT_ARCH -eq 'x64'
  }

  return $true
}

function Get-VcVars64PathFromVisualStudio {
  <#
  .SYNOPSIS
  Resolves vcvars64.bat below a Visual Studio installation root.
  #>
  param(
    [Parameter(Mandatory)]
    [string] $InstallationPath
  )

  $resolvedInstallationPath = Resolve-Path -LiteralPath $InstallationPath -ErrorAction Stop
  $vcvars64Path = Join-Path $resolvedInstallationPath 'VC\Auxiliary\Build\vcvars64.bat'

  if (-not (Test-Path -LiteralPath $vcvars64Path -PathType Leaf)) {
    throw "Visual Studio was found at '$resolvedInstallationPath', but '$vcvars64Path' does not exist. Install the Desktop development with C++ workload or pass -VisualStudioPath to a complete installation."
  }

  return $vcvars64Path
}

function Find-VcVars64Path {
  <#
  .SYNOPSIS
  Finds vcvars64.bat using an explicit path, VSINSTALLDIR, or vswhere.exe.
  #>
  param(
    [string] $RequestedVisualStudioPath,
    [string] $RequestedVsWherePath
  )

  if (-not [string]::IsNullOrWhiteSpace($RequestedVisualStudioPath)) {
    return Get-VcVars64PathFromVisualStudio -InstallationPath $RequestedVisualStudioPath
  }

  if (-not [string]::IsNullOrWhiteSpace($env:VSINSTALLDIR)) {
    $candidateFromEnvironment = Join-Path $env:VSINSTALLDIR 'VC\Auxiliary\Build\vcvars64.bat'
    if (Test-Path -LiteralPath $candidateFromEnvironment -PathType Leaf) {
      return $candidateFromEnvironment
    }
  }

  $effectiveVsWherePath = $RequestedVsWherePath
  if ([string]::IsNullOrWhiteSpace($effectiveVsWherePath)) {
    $effectiveVsWherePath = Get-DefaultVsWherePath
  }

  if ([string]::IsNullOrWhiteSpace($effectiveVsWherePath) -or -not (Test-Path -LiteralPath $effectiveVsWherePath -PathType Leaf)) {
    throw "Could not find vswhere.exe. Pass -VisualStudioPath or install Visual Studio Installer so the helper can locate vcvars64.bat."
  }

  $global:LASTEXITCODE = 0
  $installationPath = & $effectiveVsWherePath -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
    Select-Object -First 1

  $vsWhereExitCode = $global:LASTEXITCODE
  if ($vsWhereExitCode -ne 0) {
    throw "vswhere.exe exited with code $vsWhereExitCode while searching for Visual Studio C++ tools."
  }

  if ([string]::IsNullOrWhiteSpace($installationPath)) {
    throw "Could not find a Visual Studio installation with the x64 C++ tools. Install the Desktop development with C++ workload or pass -VisualStudioPath."
  }

  return Get-VcVars64PathFromVisualStudio -InstallationPath $installationPath.Trim()
}

function Import-VcVars64Environment {
  <#
  .SYNOPSIS
  Imports the environment produced by vcvars64.bat into this PowerShell process.
  #>
  param(
    [Parameter(Mandatory)]
    [string] $VcVars64Path
  )

  $command = "set VSCMD_SKIP_SENDTELEMETRY=1 && call `"$VcVars64Path`" >nul && set"
  $commandProcessor = $env:ComSpec
  if ([string]::IsNullOrWhiteSpace($commandProcessor)) {
    $commandProcessor = 'cmd.exe'
  }

  $global:LASTEXITCODE = 0
  $environmentLines = & $commandProcessor /d /s /c $command

  $vcvarsExitCode = $global:LASTEXITCODE
  if ($vcvarsExitCode -ne 0) {
    throw "vcvars64.bat exited with code $vcvarsExitCode."
  }

  $importedCount = 0
  foreach ($line in $environmentLines) {
    $separatorIndex = $line.IndexOf('=')

    # cmd.exe emits pseudo-variables such as "=C:=C:\..."; PowerShell's
    # environment provider cannot set those, and they are not build inputs.
    if ($separatorIndex -le 0) {
      continue
    }

    $name = $line.Substring(0, $separatorIndex)
    $value = $line.Substring($separatorIndex + 1)
    [System.Environment]::SetEnvironmentVariable($name, $value, 'Process')
    $importedCount++
  }

  return $importedCount
}

function Invoke-LoadedCommand {
  <#
  .SYNOPSIS
  Runs an optional command after the Visual Studio environment is loaded.
  #>
  param(
    [string[]] $Arguments
  )

  $effectiveArguments = @()
  if ($null -ne $Arguments) {
    $effectiveArguments = @($Arguments)
  }
  if ($effectiveArguments.Count -gt 0 -and $effectiveArguments[0] -eq '--') {
    $effectiveArguments = @($effectiveArguments | Select-Object -Skip 1)
  }

  if ($effectiveArguments.Count -eq 0) {
    $script:CommandExitCode = 0
    return
  }

  Write-Verbose "Running command after vcvars64.bat: $($effectiveArguments -join ' ')"

  $executable = $effectiveArguments[0]
  $executableArguments = @()
  if ($effectiveArguments.Count -gt 1) {
    $executableArguments = @($effectiveArguments | Select-Object -Skip 1)
  }

  $global:LASTEXITCODE = 0
  & $executable @executableArguments
  $script:CommandExitCode = $global:LASTEXITCODE
}

if (-not $Force -and [string]::IsNullOrWhiteSpace($VisualStudioPath) -and (Test-X64MsvcEnvironment)) {
  Write-Status "Visual Studio x64 developer environment is already active."
}
else {
  $vcvars64Path = Find-VcVars64Path -RequestedVisualStudioPath $VisualStudioPath -RequestedVsWherePath $VsWherePath
  $importedCount = Import-VcVars64Environment -VcVars64Path $vcvars64Path
  Write-Status "Imported Visual Studio x64 developer environment from $vcvars64Path ($importedCount variables)."
}

$script:CommandExitCode = 0
Invoke-LoadedCommand -Arguments $CommandLine
if ($script:CommandExitCode -ne 0) {
  exit $script:CommandExitCode
}

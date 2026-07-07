Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')
$scriptPath = Join-Path $repoRoot 'scripts\Invoke-VcVars64.ps1'
$ninjaWrapperPath = Join-Path $repoRoot 'scripts\Invoke-VsNinja.cmd'

if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
  throw "Expected Visual Studio environment helper at $scriptPath."
}

if (-not (Test-Path -LiteralPath $ninjaWrapperPath -PathType Leaf)) {
  throw "Expected Visual Studio Ninja wrapper at $ninjaWrapperPath."
}

$originalEnvironment = @{
  Path = $env:Path
  CAO_TEST_SEMICOLON = $env:CAO_TEST_SEMICOLON
  CAO_COMMAND_CAPTURE_PATH = $env:CAO_COMMAND_CAPTURE_PATH
  VCToolsInstallDir = $env:VCToolsInstallDir
  VSINSTALLDIR = $env:VSINSTALLDIR
  VisualStudioVersion = $env:VisualStudioVersion
  WindowsSdkDir = $env:WindowsSdkDir
  CAO_VCPKG_ROOT = $env:CAO_VCPKG_ROOT
  VCPKG_INSTALLATION_ROOT = $env:VCPKG_INSTALLATION_ROOT
  VCPKG_ROOT = $env:VCPKG_ROOT
  CAO_VISUAL_STUDIO_PATH = $env:CAO_VISUAL_STUDIO_PATH
  CAO_VISUAL_STUDIO_MAJOR_VERSION = $env:CAO_VISUAL_STUDIO_MAJOR_VERSION
  CAO_VSWHERE_PATH = $env:CAO_VSWHERE_PATH
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) "cao-vcvars64-test-$([System.Guid]::NewGuid())"
$fakeVisualStudioPath = Join-Path $tempRoot 'Microsoft Visual Studio\2022\Community'
$fakeVisualStudio2026Path = Join-Path $tempRoot 'Microsoft Visual Studio\2026\Preview'
$vcvarsDirectory = Join-Path $fakeVisualStudioPath 'VC\Auxiliary\Build'
$vcvars2026Directory = Join-Path $fakeVisualStudio2026Path 'VC\Auxiliary\Build'
$markerPath = Join-Path $tempRoot 'vcvars-called.txt'
$fakeToolsPath = Join-Path $fakeVisualStudioPath 'VC\Tools\MSVC\14.99.99999'
$fakeSdkPath = Join-Path $tempRoot 'Windows Kits\10'
$fakeVisualStudioVcpkgRoot = Join-Path $fakeVisualStudioPath 'Common7\IDE\CommonExtensions\Microsoft\CMake\Vcpkg'
$fakeUserVcpkgRoot = Join-Path $tempRoot 'UserVcpkg'
$fakeAmbientVcpkgRoot = Join-Path $tempRoot 'AmbientVcpkg'
$fakeCaoVcpkgRoot = Join-Path $tempRoot 'CaoVcpkg'
$fakeUserVcpkgExe = Join-Path $fakeUserVcpkgRoot 'vcpkg.exe'
$vsWhereArgumentsPath = Join-Path $tempRoot 'vswhere-arguments.txt'

try {
  New-Item -ItemType Directory -Path $vcvarsDirectory -Force | Out-Null
  New-Item -ItemType Directory -Path $vcvars2026Directory -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeToolsPath 'bin\Hostx64\x64') -Force | Out-Null
  New-Item -ItemType Directory -Path $fakeSdkPath -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeVisualStudioVcpkgRoot 'scripts\buildsystems') -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeUserVcpkgRoot 'scripts\buildsystems') -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeAmbientVcpkgRoot 'scripts\buildsystems') -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeCaoVcpkgRoot 'scripts\buildsystems') -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $fakeVisualStudioVcpkgRoot 'scripts\buildsystems\vcpkg.cmake') -Value 'set(CAO_FAKE_VS_VCPKG_TOOLCHAIN_INCLUDED TRUE)' -Encoding ASCII
  Set-Content -LiteralPath (Join-Path $fakeUserVcpkgRoot 'scripts\buildsystems\vcpkg.cmake') -Value 'set(CAO_FAKE_USER_VCPKG_TOOLCHAIN_INCLUDED TRUE)' -Encoding ASCII
  Set-Content -LiteralPath (Join-Path $fakeAmbientVcpkgRoot 'scripts\buildsystems\vcpkg.cmake') -Value 'set(CAO_FAKE_AMBIENT_VCPKG_TOOLCHAIN_INCLUDED TRUE)' -Encoding ASCII
  Set-Content -LiteralPath (Join-Path $fakeCaoVcpkgRoot 'scripts\buildsystems\vcpkg.cmake') -Value 'set(CAO_FAKE_CAO_VCPKG_TOOLCHAIN_INCLUDED TRUE)' -Encoding ASCII
  Set-Content -LiteralPath $fakeUserVcpkgExe -Value '@echo off' -Encoding ASCII
  Copy-Item -LiteralPath $env:ComSpec -Destination (Join-Path $fakeToolsPath 'bin\Hostx64\x64\ninja.exe')

  $vcvars64Path = Join-Path $vcvarsDirectory 'vcvars64.bat'
  @(
    '@echo off'
    "echo called> `"$markerPath`""
    "set VSINSTALLDIR=$fakeVisualStudioPath\"
    'set VisualStudioVersion=17.0'
    "set VCToolsInstallDir=$fakeToolsPath\"
    "set WindowsSdkDir=$fakeSdkPath\"
    "set VCPKG_INSTALLATION_ROOT=$fakeVisualStudioVcpkgRoot"
    "set VCPKG_ROOT=$fakeVisualStudioVcpkgRoot"
    "set PATH=$fakeToolsPath\bin\Hostx64\x64;%PATH%"
  ) | Set-Content -LiteralPath $vcvars64Path -Encoding ASCII

  $vcvars2026Path = Join-Path $vcvars2026Directory 'vcvars64.bat'
  @(
    '@echo off'
    "set VSINSTALLDIR=$fakeVisualStudio2026Path\"
    'set VisualStudioVersion=18.0'
    "set VCToolsInstallDir=$fakeToolsPath\"
    "set WindowsSdkDir=$fakeSdkPath\"
    "set PATH=$fakeToolsPath\bin\Hostx64\x64;%PATH%"
  ) | Set-Content -LiteralPath $vcvars2026Path -Encoding ASCII

  & $scriptPath -VisualStudioPath $fakeVisualStudioPath -Quiet

  if ($LASTEXITCODE -ne 0) {
    throw "Invoke-VcVars64.ps1 exited with code $LASTEXITCODE."
  }

  if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    throw 'Expected the fake vcvars64.bat to be invoked.'
  }

  if ($env:VSINSTALLDIR -ne "$fakeVisualStudioPath\") {
    throw "Expected VSINSTALLDIR to come from vcvars64.bat, got '$env:VSINSTALLDIR'."
  }

  if ($env:VCToolsInstallDir -ne "$fakeToolsPath\") {
    throw "Expected VCToolsInstallDir to come from vcvars64.bat, got '$env:VCToolsInstallDir'."
  }

  if ($env:WindowsSdkDir -ne "$fakeSdkPath\") {
    throw "Expected WindowsSdkDir to come from vcvars64.bat, got '$env:WindowsSdkDir'."
  }

  if (-not $env:Path.StartsWith((Join-Path $fakeToolsPath 'bin\Hostx64\x64'), [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Expected vcvars64.bat to prepend the fake MSVC tools path to PATH."
  }

  $env:CAO_TEST_SEMICOLON = 'left;right'
  $exportPath = Join-Path $tempRoot 'environment.cmake'
  & $scriptPath -VisualStudioPath $fakeVisualStudioPath -Quiet -Force -ExportCMakeEnvironment $exportPath

  if ($LASTEXITCODE -ne 0) {
    throw "Expected CMake environment export to exit successfully, got $LASTEXITCODE."
  }

  if (-not (Test-Path -LiteralPath $exportPath -PathType Leaf)) {
    throw "Expected CMake environment export at $exportPath."
  }

  $exportedEnvironment = Get-Content -LiteralPath $exportPath -Raw
  if (-not $exportedEnvironment.Contains("set(ENV{VCToolsInstallDir} [[$fakeToolsPath\]])")) {
    throw 'Expected exported CMake environment to include VCToolsInstallDir.'
  }

  if (-not $exportedEnvironment.Contains('set(ENV{CAO_TEST_SEMICOLON} [[left;right]])')) {
    throw 'Expected exported CMake environment to preserve semicolons.'
  }

  $fakeVsWherePath = Join-Path $tempRoot 'vswhere.cmd'
  @(
    '@echo off'
    "echo %*> `"$vsWhereArgumentsPath`""
    "echo $fakeVisualStudioPath"
  ) | Set-Content -LiteralPath $fakeVsWherePath -Encoding ASCII

  Remove-Item -LiteralPath 'Env:VSINSTALLDIR' -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:VisualStudioVersion' -ErrorAction SilentlyContinue

  & $scriptPath -VsWherePath $fakeVsWherePath -VisualStudioMajorVersion 17 -Quiet -Force

  if ($LASTEXITCODE -ne 0) {
    throw "Expected version-filtered vswhere lookup to exit successfully, got $LASTEXITCODE."
  }

  $vsWhereArguments = Get-Content -LiteralPath $vsWhereArgumentsPath -Raw
  if (-not $vsWhereArguments.Contains('-version [17.0,18.0)')) {
    throw "Expected vswhere arguments to request the VS 17 version range, got '$vsWhereArguments'."
  }

  Remove-Item -LiteralPath $vsWhereArgumentsPath -ErrorAction SilentlyContinue
  $env:CAO_VSWHERE_PATH = $fakeVsWherePath
  $env:CAO_VISUAL_STUDIO_MAJOR_VERSION = '17'
  $env:VSINSTALLDIR = "$fakeVisualStudio2026Path\"
  $env:VisualStudioVersion = '18.0'

  & $ninjaWrapperPath /d /s /c 'exit 0'

  if ($LASTEXITCODE -ne 0) {
    throw "Expected Visual Studio Ninja wrapper to exit successfully, got $LASTEXITCODE."
  }

  if (-not (Test-Path -LiteralPath $vsWhereArgumentsPath -PathType Leaf)) {
    throw 'Expected Visual Studio Ninja wrapper to query vswhere when the active Visual Studio major version does not match the request.'
  }

  $vsWhereArguments = Get-Content -LiteralPath $vsWhereArgumentsPath -Raw
  if (-not ($vsWhereArguments.Contains('-version') -and $vsWhereArguments.Contains('[17.0,18.0)'))) {
    throw "Expected Visual Studio Ninja wrapper to request the VS 17 version range, got '$vsWhereArguments'."
  }

  Remove-Item -LiteralPath $vsWhereArgumentsPath -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:CAO_VISUAL_STUDIO_MAJOR_VERSION' -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:VSINSTALLDIR' -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:VisualStudioVersion' -ErrorAction SilentlyContinue
  $env:CAO_VSWHERE_PATH = $fakeVsWherePath

  & $ninjaWrapperPath /d /s /c 'exit 0'

  if ($LASTEXITCODE -ne 0) {
    throw "Expected Visual Studio Ninja wrapper latest lookup to exit successfully, got $LASTEXITCODE."
  }

  $vsWhereArguments = Get-Content -LiteralPath $vsWhereArgumentsPath -Raw
  if ($vsWhereArguments.Contains('-version')) {
    throw "Expected Visual Studio Ninja wrapper latest lookup to omit -version, got '$vsWhereArguments'."
  }

  $commandCapturePath = Join-Path $tempRoot 'command-environment.txt'
  $env:CAO_COMMAND_CAPTURE_PATH = $commandCapturePath
  $currentPowerShell = (Get-Process -Id $PID).Path
  & $currentPowerShell -NoProfile -ExecutionPolicy Bypass -File $scriptPath -VisualStudioPath $fakeVisualStudioPath -Quiet --% cmd /d /s /c echo %VCToolsInstallDir%> "%CAO_COMMAND_CAPTURE_PATH%"

  if ($LASTEXITCODE -ne 0) {
    throw "Expected command forwarding to exit successfully, got $LASTEXITCODE."
  }

  $commandEnvironment = (Get-Content -LiteralPath $commandCapturePath -Raw).Trim()
  if ($commandEnvironment -ne "$fakeToolsPath\") {
    throw "Expected forwarded command to see VCToolsInstallDir '$fakeToolsPath\', got '$commandEnvironment'."
  }

  $cmakeImportSource = Join-Path $tempRoot 'cmake-import-source'
  $cmakeImportBuild = Join-Path $tempRoot 'cmake-import-build'
  New-Item -ItemType Directory -Path (Join-Path $cmakeImportSource 'scripts') -Force | Out-Null
  Copy-Item -LiteralPath $scriptPath -Destination (Join-Path $cmakeImportSource 'scripts\Invoke-VcVars64.ps1')

  $importScriptPath = Join-Path $repoRoot 'cmake\ImportVisualStudioEnvironment.cmake'
  $vcpkgToolchainPath = Join-Path $repoRoot 'cmake\VcpkgToolchain.cmake'
  $importScriptPathForCMake = $importScriptPath -replace '\\', '/'
  $vcpkgToolchainPathForCMake = $vcpkgToolchainPath -replace '\\', '/'
  @'
cmake_minimum_required(VERSION 3.24)
project(ImportVisualStudioEnvironmentRestore NONE)
include([==[__IMPORT_SCRIPT_PATH__]==])
if(DEFINED EXPECTED_VCPKG_ROOT)
  if(NOT "$ENV{VCPKG_ROOT}" STREQUAL "${EXPECTED_VCPKG_ROOT}")
    message(FATAL_ERROR "Expected VCPKG_ROOT to be restored to '${EXPECTED_VCPKG_ROOT}', got '$ENV{VCPKG_ROOT}'.")
  endif()
else()
  if(NOT "$ENV{VCPKG_ROOT}" STREQUAL "")
    message(FATAL_ERROR "Expected VCPKG_ROOT to be removed, got '$ENV{VCPKG_ROOT}'.")
  endif()
endif()
if(DEFINED EXPECTED_VCPKG_INSTALLATION_ROOT)
  if(NOT "$ENV{VCPKG_INSTALLATION_ROOT}" STREQUAL "${EXPECTED_VCPKG_INSTALLATION_ROOT}")
    message(FATAL_ERROR "Expected VCPKG_INSTALLATION_ROOT to be restored to '${EXPECTED_VCPKG_INSTALLATION_ROOT}', got '$ENV{VCPKG_INSTALLATION_ROOT}'.")
  endif()
else()
  if(NOT "$ENV{VCPKG_INSTALLATION_ROOT}" STREQUAL "")
    message(FATAL_ERROR "Expected VCPKG_INSTALLATION_ROOT to be removed, got '$ENV{VCPKG_INSTALLATION_ROOT}'.")
  endif()
endif()
if(DEFINED EXPECTED_RESOLVED_VCPKG_ROOT)
  include([==[__VCPKG_TOOLCHAIN_PATH__]==])
  file(TO_CMAKE_PATH "${EXPECTED_RESOLVED_VCPKG_ROOT}" expected_resolved_vcpkg_root)
  get_filename_component(expected_resolved_vcpkg_root "${expected_resolved_vcpkg_root}" ABSOLUTE)
  get_filename_component(actual_resolved_vcpkg_root "${CAO_RESOLVED_VCPKG_ROOT}" ABSOLUTE)
  if(NOT actual_resolved_vcpkg_root STREQUAL expected_resolved_vcpkg_root)
    message(FATAL_ERROR "Expected resolved vcpkg root '${expected_resolved_vcpkg_root}', got '${actual_resolved_vcpkg_root}'.")
  endif()
endif()
'@.
    Replace('__IMPORT_SCRIPT_PATH__', $importScriptPathForCMake).
    Replace('__VCPKG_TOOLCHAIN_PATH__', $vcpkgToolchainPathForCMake) |
    Set-Content -LiteralPath (Join-Path $cmakeImportSource 'CMakeLists.txt') -Encoding UTF8

  $env:CAO_VISUAL_STUDIO_PATH = $fakeVisualStudioPath
  $env:VCPKG_ROOT = $fakeUserVcpkgRoot
  Remove-Item -LiteralPath 'Env:VCPKG_INSTALLATION_ROOT' -ErrorAction SilentlyContinue

  cmake -S $cmakeImportSource -B $cmakeImportBuild -D "EXPECTED_VCPKG_ROOT=$fakeUserVcpkgRoot"
  if ($LASTEXITCODE -ne 0) {
    throw "Expected CMake Visual Studio environment import to restore caller vcpkg roots, got $LASTEXITCODE."
  }

  $cmakePathOnlyBuild = Join-Path $tempRoot 'cmake-path-only-build'
  if ([string]::IsNullOrEmpty($originalEnvironment.Path)) {
    $env:Path = $fakeUserVcpkgRoot
  }
  else {
    $env:Path = "$fakeUserVcpkgRoot;$($originalEnvironment.Path)"
  }
  Remove-Item -LiteralPath 'Env:CAO_VCPKG_ROOT' -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:VCPKG_ROOT' -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath 'Env:VCPKG_INSTALLATION_ROOT' -ErrorAction SilentlyContinue

  cmake -S $cmakeImportSource -B $cmakePathOnlyBuild -D "EXPECTED_RESOLVED_VCPKG_ROOT=$fakeUserVcpkgRoot"
  if ($LASTEXITCODE -ne 0) {
    throw "Expected CMake vcpkg resolution to prefer the caller's original PATH after import, got $LASTEXITCODE."
  }

  $cmakeExplicitCaoBuild = Join-Path $tempRoot 'cmake-explicit-cao-build'
  $env:VCPKG_ROOT = $fakeAmbientVcpkgRoot
  $env:VCPKG_INSTALLATION_ROOT = $fakeAmbientVcpkgRoot

  cmake -S $cmakeImportSource -B $cmakeExplicitCaoBuild -D "CAO_VCPKG_ROOT=$fakeCaoVcpkgRoot" -D "EXPECTED_VCPKG_ROOT=$fakeAmbientVcpkgRoot" -D "EXPECTED_VCPKG_INSTALLATION_ROOT=$fakeAmbientVcpkgRoot" -D "EXPECTED_RESOLVED_VCPKG_ROOT=$fakeCaoVcpkgRoot"
  if ($LASTEXITCODE -ne 0) {
    throw "Expected explicit CAO_VCPKG_ROOT to take priority over ambient vcpkg roots, got $LASTEXITCODE."
  }
}
finally {
  foreach ($name in $originalEnvironment.Keys) {
    $value = $originalEnvironment[$name]
    if ($null -eq $value) {
      Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    else {
      Set-Item -LiteralPath "Env:$name" -Value $value
    }
  }

  if (Test-Path -LiteralPath $tempRoot) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force
  }
}

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')
$scriptPath = Join-Path $repoRoot 'scripts\Invoke-VcVars64.ps1'

if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
  throw "Expected Visual Studio environment helper at $scriptPath."
}

$originalEnvironment = @{
  Path = $env:Path
  CAO_TEST_SEMICOLON = $env:CAO_TEST_SEMICOLON
  CAO_COMMAND_CAPTURE_PATH = $env:CAO_COMMAND_CAPTURE_PATH
  VCToolsInstallDir = $env:VCToolsInstallDir
  VSINSTALLDIR = $env:VSINSTALLDIR
  VisualStudioVersion = $env:VisualStudioVersion
  WindowsSdkDir = $env:WindowsSdkDir
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) "cao-vcvars64-test-$([System.Guid]::NewGuid())"
$fakeVisualStudioPath = Join-Path $tempRoot 'Microsoft Visual Studio\2022\Community'
$vcvarsDirectory = Join-Path $fakeVisualStudioPath 'VC\Auxiliary\Build'
$markerPath = Join-Path $tempRoot 'vcvars-called.txt'
$fakeToolsPath = Join-Path $fakeVisualStudioPath 'VC\Tools\MSVC\14.99.99999'
$fakeSdkPath = Join-Path $tempRoot 'Windows Kits\10'
$vsWhereArgumentsPath = Join-Path $tempRoot 'vswhere-arguments.txt'

try {
  New-Item -ItemType Directory -Path $vcvarsDirectory -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $fakeToolsPath 'bin\Hostx64\x64') -Force | Out-Null
  New-Item -ItemType Directory -Path $fakeSdkPath -Force | Out-Null

  $vcvars64Path = Join-Path $vcvarsDirectory 'vcvars64.bat'
  @(
    '@echo off'
    "echo called> `"$markerPath`""
    "set VSINSTALLDIR=$fakeVisualStudioPath\"
    'set VisualStudioVersion=17.0'
    "set VCToolsInstallDir=$fakeToolsPath\"
    "set WindowsSdkDir=$fakeSdkPath\"
    "set PATH=$fakeToolsPath\bin\Hostx64\x64;%PATH%"
  ) | Set-Content -LiteralPath $vcvars64Path -Encoding ASCII

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

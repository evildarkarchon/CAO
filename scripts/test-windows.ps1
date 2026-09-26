<#
.SYNOPSIS
Builds and tests an already configured Windows profile.
.DESCRIPTION
The default run is sandbox compatible and excludes the native console test. Run with
-NativeConsoleOnly outside the sandbox to verify real Windows Ctrl+C delivery.
.PARAMETER BuildDir
The configured profile build directory, relative to the repository or absolute.
.PARAMETER NativeConsoleOnly
Builds and runs only the native console integration test.
#>
[CmdletBinding()]
param(
    [string]$BuildDir = 'build/vs2026',
    [switch]$NativeConsoleOnly
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$requestedBuildDir = if ([IO.Path]::IsPathRooted($BuildDir)) {
    $BuildDir
} else {
    Join-Path $repoRoot $BuildDir
}
$resolvedBuildDir = (Resolve-Path -LiteralPath $requestedBuildDir).Path
if (-not (Test-Path -LiteralPath (Join-Path $resolvedBuildDir 'CMakeCache.txt'))) {
    throw "Configure the Windows profile at $resolvedBuildDir before running tests."
}

$originalTemp = $env:TEMP
$originalTmp = $env:TMP
$originalPath = $env:PATH
try {
    # Qt child processes need temporary fixtures beneath the sandbox's writable workspace.
    $testTemp = Join-Path $resolvedBuildDir 'sandbox-test-temp'
    [IO.Directory]::CreateDirectory($testTemp) | Out-Null
    $env:TEMP = $testTemp
    $env:TMP = $testTemp

    # The Windows sandbox can supply both Path and PATH; MSBuild rejects those duplicate keys.
    Remove-Item Env:PATH
    $env:PATH = $originalPath

    $buildArgs = @('--build', $resolvedBuildDir, '--config', 'Debug')
    if ($NativeConsoleOnly) {
        $buildArgs += @('--target', 'cao_cli_console_interrupt_tests')
    }
    $buildArgs += '--parallel'
    & cmake @buildArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Windows profile build failed with exit code $LASTEXITCODE."
    }

    $labelSelector = if ($NativeConsoleOnly) { '-L' } else { '-LE' }
    & ctest --test-dir $resolvedBuildDir -C Debug $labelSelector '^native-console$' --output-on-failure --no-tests=error
    if ($LASTEXITCODE -ne 0) {
        throw "Windows profile tests failed with exit code $LASTEXITCODE."
    }
} finally {
    if ($null -eq $originalTemp) {
        Remove-Item Env:TEMP -ErrorAction SilentlyContinue
    } else {
        $env:TEMP = $originalTemp
    }
    if ($null -eq $originalTmp) {
        Remove-Item Env:TMP -ErrorAction SilentlyContinue
    } else {
        $env:TMP = $originalTmp
    }
    $env:PATH = $originalPath
}

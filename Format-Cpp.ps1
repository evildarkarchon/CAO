[CmdletBinding()]
param(
    [string] $Root = $PSScriptRoot
)

$ErrorActionPreference = 'Stop'

$rootPath = (Resolve-Path -LiteralPath $Root).ProviderPath
$directorySeparators = [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
$normalizedRootPath = [System.IO.Path]::GetFullPath($rootPath).TrimEnd($directorySeparators)
$tes5EditPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'TES5Edit')).TrimEnd($directorySeparators)

# Child-name pruning below cannot protect traversal that starts inside the
# reference submodule, so reject that root before discovering any files.
if ([System.String]::Equals($normalizedRootPath, $tes5EditPath, [System.StringComparison]::OrdinalIgnoreCase) -or
    $normalizedRootPath.StartsWith($tes5EditPath + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to format '$Root': it is inside the read-only TES5Edit reference tree '$tes5EditPath'."
}

$formatExtensions = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($extension in @('.cc', '.cpp', '.h', '.hpp')) {
    [void] $formatExtensions.Add($extension)
}

$excludedDirectoryNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($directoryName in @('TES5Edit', 'tests', 'build', 'CMakeFiles', 'Testing', 'vcpkg_installed')) {
    [void] $excludedDirectoryNames.Add($directoryName)
}

function Get-FormatCppFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Directory
    )

    foreach ($entry in Get-ChildItem -LiteralPath $Directory -Force) {
        if ($entry.PSIsContainer) {
            if ($excludedDirectoryNames.Contains($entry.Name)) {
                continue
            }

            Get-FormatCppFiles -Directory $entry.FullName
            continue
        }

        if ($formatExtensions.Contains($entry.Extension)) {
            $entry
        }
    }
}

$formattedCount = 0

foreach ($file in Get-FormatCppFiles -Directory $rootPath) {
    $relativePath = [System.IO.Path]::GetRelativePath($rootPath, $file.FullName)
    Write-Host "Formatting $relativePath"

    & clang-format -i $file.FullName
    if ($LASTEXITCODE -ne 0) {
        throw "clang-format failed for '$relativePath' with exit code $LASTEXITCODE."
    }

    $formattedCount++
}

Write-Host "Formatted $formattedCount file(s)."

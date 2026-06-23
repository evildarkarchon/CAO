@echo off
setlocal EnableExtensions DisableDelayedExpansion

if defined CAO_VISUAL_STUDIO_PATH (
  set "CAO_VSINSTALLDIR=%CAO_VISUAL_STUDIO_PATH%"
  goto :FoundVisualStudio
)

if defined VSINSTALLDIR (
  if exist "%VSINSTALLDIR%\VC\Auxiliary\Build\vcvars64.bat" (
    set "CAO_VSINSTALLDIR=%VSINSTALLDIR%"
    goto :FoundVisualStudio
  )
)

set "CAO_EFFECTIVE_VSWHERE=%CAO_VSWHERE_PATH%"
if not defined CAO_EFFECTIVE_VSWHERE set "CAO_EFFECTIVE_VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"

if not exist "%CAO_EFFECTIVE_VSWHERE%" (
  echo Could not find vswhere.exe. Set CAO_VSWHERE_PATH or install Visual Studio Installer. 1>&2
  exit /b 1
)

for /f "usebackq delims=" %%I in (`"%CAO_EFFECTIVE_VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
  if not defined CAO_VSINSTALLDIR set "CAO_VSINSTALLDIR=%%I"
)

:FoundVisualStudio
if not defined CAO_VSINSTALLDIR (
  echo Could not find a Visual Studio installation with the x64 C++ tools. 1>&2
  exit /b 1
)

set "CAO_VCVARS64=%CAO_VSINSTALLDIR%\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%CAO_VCVARS64%" (
  echo Visual Studio was found at "%CAO_VSINSTALLDIR%", but "%CAO_VCVARS64%" does not exist. 1>&2
  exit /b 1
)

set VSCMD_SKIP_SENDTELEMETRY=1
call "%CAO_VCVARS64%" >nul
if errorlevel 1 exit /b %ERRORLEVEL%

where ninja.exe >nul 2>nul
if errorlevel 1 (
  echo Could not find ninja.exe after loading the Visual Studio developer environment. 1>&2
  exit /b 1
)

ninja.exe %*
exit /b %ERRORLEVEL%

@echo off
setlocal EnableExtensions DisableDelayedExpansion

if defined CAO_VISUAL_STUDIO_PATH (
  set "CAO_VSINSTALLDIR=%CAO_VISUAL_STUDIO_PATH%"
  goto :FoundVisualStudio
)

if defined VSINSTALLDIR (
  call :TryUseActiveVisualStudioEnvironment
  if not errorlevel 1 (
    goto :FoundVisualStudio
  )
)

set "CAO_EFFECTIVE_VSWHERE=%CAO_VSWHERE_PATH%"
if not defined CAO_EFFECTIVE_VSWHERE set "CAO_EFFECTIVE_VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
rem Use the short path so Program Files (x86) does not break parsed FOR /F commands below.
for %%I in ("%CAO_EFFECTIVE_VSWHERE%") do set "CAO_EFFECTIVE_VSWHERE=%%~sI"

if not exist "%CAO_EFFECTIVE_VSWHERE%" (
  echo Could not find vswhere.exe. Set CAO_VSWHERE_PATH or install Visual Studio Installer. 1>&2
  exit /b 1
)

if defined CAO_VISUAL_STUDIO_MAJOR_VERSION (
  call :SetVsWhereVersionRange
  if errorlevel 1 exit /b 1
  call :FindVisualStudioWithVersion
) else (
  call :FindLatestVisualStudio
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

:TryUseActiveVisualStudioEnvironment
rem Reuse VSINSTALLDIR only when it matches the configure-time major-version request.
if defined CAO_VISUAL_STUDIO_MAJOR_VERSION (
  if not defined VisualStudioVersion exit /b 1

  for /f "tokens=1 delims=." %%I in ("%VisualStudioVersion%") do set "CAO_ACTIVE_VISUAL_STUDIO_MAJOR_VERSION=%%I"
  if not "%CAO_ACTIVE_VISUAL_STUDIO_MAJOR_VERSION%"=="%CAO_VISUAL_STUDIO_MAJOR_VERSION%" exit /b 1
)

if exist "%VSINSTALLDIR%\VC\Auxiliary\Build\vcvars64.bat" (
  set "CAO_VSINSTALLDIR=%VSINSTALLDIR%"
  exit /b 0
)

exit /b 1

:SetVsWhereVersionRange
rem vswhere accepts half-open ranges such as [17.0,18.0) to select one VS major line.
echo(%CAO_VISUAL_STUDIO_MAJOR_VERSION%| findstr /r "^[0-9][0-9]*$" >nul
if errorlevel 1 (
  echo CAO_VISUAL_STUDIO_MAJOR_VERSION must contain only digits. 1>&2
  exit /b 1
)

set /a "CAO_NEXT_VISUAL_STUDIO_MAJOR_VERSION=%CAO_VISUAL_STUDIO_MAJOR_VERSION% + 1" >nul
exit /b 0

:FindVisualStudioWithVersion
rem Run vswhere with the requested Visual Studio major-version range.
for /f "delims=" %%I in ('""%CAO_EFFECTIVE_VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -version "[%CAO_VISUAL_STUDIO_MAJOR_VERSION%.0^,%CAO_NEXT_VISUAL_STUDIO_MAJOR_VERSION%.0^)" -property installationPath"') do (
  if not defined CAO_VSINSTALLDIR set "CAO_VSINSTALLDIR=%%I"
)
exit /b 0

:FindLatestVisualStudio
rem Preserve the previous latest-instance behavior when no major version is requested.
for /f "delims=" %%I in ('""%CAO_EFFECTIVE_VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath"') do (
  if not defined CAO_VSINSTALLDIR set "CAO_VSINSTALLDIR=%%I"
)
exit /b 0

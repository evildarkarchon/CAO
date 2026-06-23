get_property(_cao_visual_studio_environment_imported GLOBAL PROPERTY CAO_VISUAL_STUDIO_ENVIRONMENT_IMPORTED)
if(_cao_visual_studio_environment_imported)
  return()
endif()
set_property(GLOBAL PROPERTY CAO_VISUAL_STUDIO_ENVIRONMENT_IMPORTED TRUE)

if(DEFINED CMAKE_HOST_WIN32 AND NOT CMAKE_HOST_WIN32)
  return()
endif()

if("$ENV{CAO_SKIP_VISUAL_STUDIO_ENVIRONMENT}" MATCHES "^(1|ON|TRUE|YES)$")
  message(STATUS "Skipping Visual Studio developer environment import because CAO_SKIP_VISUAL_STUDIO_ENVIRONMENT is set")
  return()
endif()

set(_cao_vcvars_script "${CMAKE_SOURCE_DIR}/scripts/Invoke-VcVars64.ps1")
if(NOT EXISTS "${_cao_vcvars_script}")
  message(FATAL_ERROR "Visual Studio developer environment helper was not found at '${_cao_vcvars_script}'.")
endif()

find_program(CAO_POWERSHELL_EXECUTABLE NAMES pwsh pwsh.exe powershell powershell.exe)
if(NOT CAO_POWERSHELL_EXECUTABLE)
  message(FATAL_ERROR "Could not find PowerShell. Install PowerShell or set CAO_SKIP_VISUAL_STUDIO_ENVIRONMENT=ON to skip the Visual Studio environment import.")
endif()

set(_cao_environment_script "${CMAKE_BINARY_DIR}/CMakeFiles/ImportedVisualStudioEnvironment.cmake")
file(REMOVE "${_cao_environment_script}")

set(_cao_original_path "$ENV{PATH}")
set(_cao_original_vcpkg_installation_root "$ENV{VCPKG_INSTALLATION_ROOT}")
set(_cao_original_vcpkg_root "$ENV{VCPKG_ROOT}")

set(_cao_vcvars_arguments
  -NoProfile
  -ExecutionPolicy
  Bypass
  -File
  "${_cao_vcvars_script}"
  -Quiet
  -ExportCMakeEnvironment
  "${_cao_environment_script}"
)

if(NOT "$ENV{CAO_VISUAL_STUDIO_PATH}" STREQUAL "")
  list(APPEND _cao_vcvars_arguments -VisualStudioPath "$ENV{CAO_VISUAL_STUDIO_PATH}")
endif()

if(NOT "$ENV{CAO_VSWHERE_PATH}" STREQUAL "")
  list(APPEND _cao_vcvars_arguments -VsWherePath "$ENV{CAO_VSWHERE_PATH}")
endif()

if(NOT "$ENV{CAO_VISUAL_STUDIO_MAJOR_VERSION}" STREQUAL "")
  list(APPEND _cao_vcvars_arguments -VisualStudioMajorVersion "$ENV{CAO_VISUAL_STUDIO_MAJOR_VERSION}")
endif()

execute_process(
  COMMAND "${CAO_POWERSHELL_EXECUTABLE}" ${_cao_vcvars_arguments}
  RESULT_VARIABLE _cao_vcvars_result
  OUTPUT_VARIABLE _cao_vcvars_output
  ERROR_VARIABLE _cao_vcvars_error
)

if(NOT _cao_vcvars_result EQUAL 0)
  message(FATAL_ERROR "Failed to import the Visual Studio developer environment.\n${_cao_vcvars_output}${_cao_vcvars_error}")
endif()

if(NOT EXISTS "${_cao_environment_script}")
  message(FATAL_ERROR "Visual Studio developer environment helper did not create '${_cao_environment_script}'.")
endif()

include("${_cao_environment_script}")

set(ENV{CAO_ORIGINAL_PATH} "${_cao_original_path}")
if(NOT "${_cao_original_vcpkg_installation_root}" STREQUAL "")
  set(ENV{VCPKG_INSTALLATION_ROOT} "${_cao_original_vcpkg_installation_root}")
endif()
if(NOT "${_cao_original_vcpkg_root}" STREQUAL "")
  set(ENV{VCPKG_ROOT} "${_cao_original_vcpkg_root}")
endif()

message(STATUS "Imported Visual Studio x64 developer environment")

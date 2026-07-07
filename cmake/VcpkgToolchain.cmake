get_property(_cao_vcpkg_in_try_compile GLOBAL PROPERTY IN_TRY_COMPILE)
set(_cao_resolved_vcpkg_root "")
if(DEFINED CAO_RESOLVED_VCPKG_ROOT AND NOT "${CAO_RESOLVED_VCPKG_ROOT}" STREQUAL "")
  set(_cao_resolved_vcpkg_root "${CAO_RESOLVED_VCPKG_ROOT}")
elseif(NOT "$ENV{CAO_RESOLVED_VCPKG_ROOT}" STREQUAL "")
  set(_cao_resolved_vcpkg_root "$ENV{CAO_RESOLVED_VCPKG_ROOT}")
endif()

if(_cao_vcpkg_in_try_compile AND NOT "${_cao_resolved_vcpkg_root}" STREQUAL "")
  file(TO_CMAKE_PATH "${_cao_resolved_vcpkg_root}" _cao_vcpkg_root)
  set(_cao_vcpkg_toolchain "${_cao_vcpkg_root}/scripts/buildsystems/vcpkg.cmake")
  if(EXISTS "${_cao_vcpkg_toolchain}")
    include("${_cao_vcpkg_toolchain}")
    return()
  endif()
endif()

set(_cao_vcpkg_candidate_roots)
set(_cao_vcpkg_environment_candidate_roots)
set(_cao_vcpkg_variable_candidate_roots)

foreach(_cao_vcpkg_root_name IN ITEMS CAO_VCPKG_ROOT VCPKG_INSTALLATION_ROOT VCPKG_ROOT)
  if(NOT "$ENV{${_cao_vcpkg_root_name}}" STREQUAL "")
    file(TO_CMAKE_PATH "$ENV{${_cao_vcpkg_root_name}}" _cao_vcpkg_env_root)
    list(APPEND _cao_vcpkg_environment_candidate_roots "${_cao_vcpkg_env_root}")
  endif()

  if(DEFINED ${_cao_vcpkg_root_name} AND NOT "${${_cao_vcpkg_root_name}}" STREQUAL "")
    file(TO_CMAKE_PATH "${${_cao_vcpkg_root_name}}" _cao_vcpkg_var_root)
    list(APPEND _cao_vcpkg_variable_candidate_roots "${_cao_vcpkg_var_root}")
  endif()
endforeach()

set(_cao_vcpkg_candidate_roots
  ${_cao_vcpkg_environment_candidate_roots}
  ${_cao_vcpkg_variable_candidate_roots}
)

if(NOT "$ENV{CAO_ORIGINAL_PATH}" STREQUAL "")
  file(TO_CMAKE_PATH "$ENV{CAO_ORIGINAL_PATH}" _cao_original_path)
  find_program(_cao_vcpkg_executable_from_original_path NAMES vcpkg vcpkg.exe PATHS ${_cao_original_path} NO_DEFAULT_PATH NO_CACHE)
  if(_cao_vcpkg_executable_from_original_path)
    get_filename_component(_cao_vcpkg_original_executable_dir "${_cao_vcpkg_executable_from_original_path}" DIRECTORY)
    list(APPEND _cao_vcpkg_candidate_roots "${_cao_vcpkg_original_executable_dir}")
  endif()
endif()

find_program(_cao_vcpkg_executable NAMES vcpkg vcpkg.exe NO_CACHE)
if(_cao_vcpkg_executable)
  get_filename_component(_cao_vcpkg_executable_dir "${_cao_vcpkg_executable}" DIRECTORY)
  list(APPEND _cao_vcpkg_candidate_roots "${_cao_vcpkg_executable_dir}")
endif()

if(NOT "$ENV{VSINSTALLDIR}" STREQUAL "")
  file(TO_CMAKE_PATH "$ENV{VSINSTALLDIR}" _cao_vs_install_dir)
  list(APPEND _cao_vcpkg_candidate_roots
    "${_cao_vs_install_dir}/Common7/IDE/CommonExtensions/Microsoft/CMake/Vcpkg"
    "${_cao_vs_install_dir}/VC/vcpkg"
  )
endif()

set(_cao_vcpkg_root "")
set(_cao_vcpkg_toolchain "")

foreach(_cao_vcpkg_candidate_root IN LISTS _cao_vcpkg_candidate_roots)
  if("${_cao_vcpkg_candidate_root}" STREQUAL "")
    continue()
  endif()

  get_filename_component(_cao_vcpkg_candidate_root "${_cao_vcpkg_candidate_root}" ABSOLUTE)
  set(_cao_vcpkg_candidate_toolchain "${_cao_vcpkg_candidate_root}/scripts/buildsystems/vcpkg.cmake")

  if(EXISTS "${_cao_vcpkg_candidate_toolchain}")
    set(_cao_vcpkg_root "${_cao_vcpkg_candidate_root}")
    set(_cao_vcpkg_toolchain "${_cao_vcpkg_candidate_toolchain}")
    break()
  endif()
endforeach()

if("${_cao_vcpkg_toolchain}" STREQUAL "")
  message(FATAL_ERROR "Could not find vcpkg's scripts/buildsystems/vcpkg.cmake. Set CAO_VCPKG_ROOT, VCPKG_ROOT, or VCPKG_INSTALLATION_ROOT to a vcpkg checkout, or put vcpkg.exe on PATH.")
endif()

set(CAO_RESOLVED_VCPKG_ROOT "${_cao_vcpkg_root}" CACHE PATH "Resolved vcpkg root used by the repository toolchain wrapper" FORCE)
set(VCPKG_ROOT "${_cao_vcpkg_root}" CACHE PATH "Resolved vcpkg root" FORCE)
set(ENV{CAO_RESOLVED_VCPKG_ROOT} "${_cao_vcpkg_root}")
set(ENV{VCPKG_ROOT} "${_cao_vcpkg_root}")
set(ENV{VCPKG_INSTALLATION_ROOT} "${_cao_vcpkg_root}")

message(STATUS "Using vcpkg from ${_cao_vcpkg_root}")
include("${_cao_vcpkg_toolchain}")

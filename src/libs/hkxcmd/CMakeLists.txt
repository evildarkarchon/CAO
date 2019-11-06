#hkxcmd
Project(hkxcmd)

set (HAVOK_LIBS_DIR_RELEASE
    ${PROJECT_SOURCE_DIR}/havok_2010_2_0/Lib/win32_net_9-0/release_multithreaded)

set (HAVOK_LIBS_DIR_DEBUG
    ${PROJECT_SOURCE_DIR}/havok_2010_2_0/Lib/win32_net_9-0/debug_multithreaded)

set (HAVOK_LIBS
    hkaAnimation
    hkaInternal
    hkaRagdoll
    hkBase
    hkCompat
    hkgBridge
    hkgCommon
    hkgDx10
    hkgDx9
    hkgDx9s
    hkGeometryUtilities
    hkgOgl
    hkgOglES
    hkgOglES2
    hkgOgls
    hkInternal
    hkpCollide
    hkpConstraintSolver
    hkpDynamics
    hkpInternal
    hkpUtilities
    hkpVehicle
    hkSceneData
    hksCommon
    hkSerialize
    hksXAudio2
    hkVisualize
    )

foreach(havok_lib IN LISTS HAVOK_LIBS)
    add_library(
        ${havok_lib}
        STATIC
        IMPORTED GLOBAL
        )

    set_target_properties(
        ${havok_lib}
        PROPERTIES IMPORTED_LOCATION
        "${HAVOK_LIBS_DIR_RELEASE}/${havok_lib}.lib"
        )

    set_target_properties(
        ${havok_lib}
        PROPERTIES IMPORTED_LOCATION_DEBUG
        "${HAVOK_LIBS_DIR_DEBUG}/${havok_lib}.lib"
        )

endforeach()

file(GLOB_RECURSE HAVOK_HEADERS
    "${PROJECT_SOURCE_DIR}/havok_2010_2_0/Source/*.h"
    "${PROJECT_SOURCE_DIR}/havok_2010_2_0/Source/*.inl"
    )

#2010 Behavior compat

file(GLOB_RECURSE HAVOK_COMPAT_HEADERS
    "${PROJECT_SOURCE_DIR}/havok_2010_2_0/compat/*.h"
    "${PROJECT_SOURCE_DIR}/havok_2010_2_0/compat/*.inl"
    "${PROJECT_SOURCE_DIR}/havok_2010_2_0/compat/*.cpp"
    )

add_library(hkxcmd STATIC
    hkfutils.cpp
    hkfutils.h
    hkxcore.cpp
    hkxcore.h
    hkxcore.hxx
    hkxutils.cpp
    hkxutils.h
    stdafx.h
    ${HAVOK_COMPAT_HEADERS} ${HAVOK_HEADERS})

#MSVC
IF (MSVC)
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} /bigobj /D_SCL_SECURE_NO_WARNINGS=1 /D_CRT_NONSTDC_NO_DEPRECATE=1 /EHsc")
add_definitions(-D_CRT_SECURE_NO_WARNINGS=1)
set (CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} /NODEFAULTLIB:MSVCRT /NODEFAULTLIB:MSVCRTD")
set (CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} /NODEFAULTLIB:MSVCRT /NODEFAULTLIB:MSVCRTD")
ENDIF ()


target_link_libraries(hkxcmd PUBLIC ${Havok_libs} Shlwapi.lib legacy_stdio_definitions.lib)
target_include_directories(hkxcmd PUBLIC AFTER ${PROJECT_SOURCE_DIR}/havok_2010_2_0/Source)
target_include_directories(hkxcmd PUBLIC AFTER ${PROJECT_SOURCE_DIR}/havok_2010_2_0/compat)
target_compile_features(hkxcmd PUBLIC cxx_std_17)


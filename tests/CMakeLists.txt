find_package(Catch2 CONFIG REQUIRED)
find_package(Qt5 COMPONENTS Core Widgets REQUIRED)
find_package(plog CONFIG REQUIRED)
find_package(bethutil CONFIG REQUIRED)
find_package(nifly CONFIG REQUIRED)
find_package(directxtex CONFIG REQUIRED)

add_executable(Cathedral_Assets_Optimizer_Tests
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlan.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/FilesystemOperations.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ManagerPlanning.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/OptionsCAO.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormats.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/AssetWorkPlan.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/AssetWorkPlan.h
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/FilesystemOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/FilesystemOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/ManagerPlanning.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/ManagerPlanning.h
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/OptionsCAO.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/OptionsCAO.h
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/Profiles.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/Profiles.h
)

target_include_directories(Cathedral_Assets_Optimizer_Tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../src
)

target_link_libraries(Cathedral_Assets_Optimizer_Tests PRIVATE
    Catch2::Catch2WithMain
    Qt5::Core
    Qt5::Widgets
    plog::plog
    nifly
    Microsoft::DirectXTex
    btu::bethutil
)

target_compile_features(Cathedral_Assets_Optimizer_Tests PRIVATE cxx_std_20)

include(Catch)
catch_discover_tests(Cathedral_Assets_Optimizer_Tests)

find_program(POWERSHELL_EXECUTABLE NAMES pwsh pwsh.exe powershell powershell.exe REQUIRED)
set(POWERSHELL_TEST_ARGS -NoProfile)
if(WIN32)
    list(APPEND POWERSHELL_TEST_ARGS -ExecutionPolicy Bypass)
endif()

# Visual Studio generators guard add_test() entries by configuration, but the
# project's canonical CTest command intentionally omits -C. Register this
# script through a CTest include so it remains available to that command.
set(POWERSHELL_CTEST_FILE "${CMAKE_CURRENT_BINARY_DIR}/Invoke-VcVars64.Tests.cmake")
set(POWERSHELL_CTEST_CONTENT "add_test([=[Invoke-VcVars64.Tests]=] [=[${POWERSHELL_EXECUTABLE}]=]")
foreach(POWERSHELL_TEST_ARG IN LISTS POWERSHELL_TEST_ARGS)
    string(APPEND POWERSHELL_CTEST_CONTENT " [=[${POWERSHELL_TEST_ARG}]=]")
endforeach()
string(APPEND POWERSHELL_CTEST_CONTENT " [=[-File]=] [=[${CMAKE_CURRENT_SOURCE_DIR}/Invoke-VcVars64.Tests.ps1]=])\n")
file(GENERATE OUTPUT "${POWERSHELL_CTEST_FILE}" CONTENT "${POWERSHELL_CTEST_CONTENT}")
set_property(DIRECTORY APPEND PROPERTY TEST_INCLUDE_FILES "${POWERSHELL_CTEST_FILE}")

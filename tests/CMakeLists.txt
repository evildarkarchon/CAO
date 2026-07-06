find_package(Catch2 CONFIG REQUIRED)
find_package(Qt5 COMPONENTS Core REQUIRED)

add_executable(Cathedral_Assets_Optimizer_Tests
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlan.Tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/AssetWorkPlan.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/AssetWorkPlan.h
)

target_include_directories(Cathedral_Assets_Optimizer_Tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../src
)

target_link_libraries(Cathedral_Assets_Optimizer_Tests PRIVATE
    Catch2::Catch2WithMain
    Qt5::Core
)

target_compile_features(Cathedral_Assets_Optimizer_Tests PRIVATE cxx_std_20)

include(Catch)
catch_discover_tests(Cathedral_Assets_Optimizer_Tests)

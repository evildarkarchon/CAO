add_library(cao_run_discovery
    ArchiveFirstAssetDiscovery.cpp
    ArchiveFirstAssetDiscovery.h
)

find_package(bethutil CONFIG REQUIRED)

# Bethutil's exported target omits its public header root; bsa exports the shared include root.
target_link_libraries(cao_run_discovery PUBLIC
    cao_asset_routing
    btu::bethutil
    bsa::bsa
)
target_include_directories(cao_run_discovery PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_discovery PUBLIC cxx_std_20)

add_library(cao_run_setup
    RunSetup.cpp
    RunSetup.h
)

target_link_libraries(cao_run_setup PUBLIC cao_asset_routing)
target_include_directories(cao_run_setup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_setup PUBLIC cxx_std_20)

add_library(cao_run_discovery
    ArchiveFirstAssetDiscovery.cpp
    ArchiveFirstAssetDiscovery.h
)

find_package(bethutil CONFIG REQUIRED)
find_package(utf8proc CONFIG REQUIRED)
target_link_libraries(cao_run_discovery PRIVATE utf8proc::utf8proc)

# Bethutil's exported target omits its public header root; bsa exports the shared include root.
target_link_libraries(cao_run_discovery PUBLIC
    cao_asset_routing
    btu::bethutil
    bsa::bsa
)
target_include_directories(cao_run_discovery PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_discovery PUBLIC cxx_std_20)

add_library(cao_asset_run
    AssetRun.cpp
    AssetRun.h
)

target_link_libraries(cao_asset_run PUBLIC
    cao_asset_routing
    cao_run_discovery
)
target_include_directories(cao_asset_run PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_asset_run PUBLIC cxx_std_20)

add_library(cao_run_lifecycle
    RunLifecycle.cpp
    RunLifecycle.h
    RunPreparation.h
)

target_link_libraries(cao_run_lifecycle PUBLIC cao_asset_routing)
target_include_directories(cao_run_lifecycle PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_lifecycle PUBLIC cxx_std_20)

add_library(cao_run_executor
    RunExecutor.cpp
    RunExecutor.h
    TemporaryArtifactRegistry.cpp
    TemporaryArtifactRegistry.h
    StagingRecovery.cpp
    StagingRecovery.h
    StagingPaths.h
)

target_link_libraries(cao_run_executor PUBLIC cao_run_lifecycle cao_run_setup)
target_link_libraries(cao_run_executor PRIVATE utf8proc::utf8proc)
target_include_directories(cao_run_executor PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_executor PUBLIC cxx_std_20)

add_library(cao_run_setup
    RunSetup.cpp
    RunSetup.h
)

target_link_libraries(cao_run_setup PUBLIC cao_asset_routing)
target_include_directories(cao_run_setup PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_run_setup PUBLIC cxx_std_20)

find_package(Threads REQUIRED)

add_library(cao_optimization_run
    OptimizationRunService.cpp
    OptimizationRunService.h
    RunScheduler.cpp
    RunScheduler.h
)

target_link_libraries(cao_optimization_run PUBLIC
    cao_run_executor
    Threads::Threads
)
target_include_directories(cao_optimization_run PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_optimization_run PUBLIC cxx_std_20)

find_package(Qt5 COMPONENTS Test Widgets REQUIRED)
find_package(bethutil CONFIG REQUIRED)
find_package(directxtex CONFIG REQUIRED)
find_package(nifly CONFIG REQUIRED)

add_executable(cao_asset_routing_tests
    AssetRoutingTests.cpp
)

target_link_libraries(cao_asset_routing_tests PRIVATE
    Qt5::Test
    cao_asset_routing
)

set_target_properties(cao_asset_routing_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_asset_routing_tests COMMAND cao_asset_routing_tests)

add_executable(cao_asset_execution_tests
    AssetExecutionTests.cpp
)

target_link_libraries(cao_asset_execution_tests PRIVATE
    Qt5::Test
    cao_asset_execution
    cao_asset_routing
)

set_target_properties(cao_asset_execution_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_asset_execution_tests COMMAND cao_asset_execution_tests)

add_executable(cao_mesh_reference_maintenance_tests
    MeshReferenceMaintenanceTests.cpp
)

target_link_libraries(cao_mesh_reference_maintenance_tests PRIVATE
    Qt5::Test
    cao_mesh_reference_maintenance
)

set_target_properties(cao_mesh_reference_maintenance_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_mesh_reference_maintenance_tests COMMAND cao_mesh_reference_maintenance_tests)

add_executable(cao_archive_first_asset_discovery_tests
    ArchiveFirstAssetDiscoveryTests.cpp
)

target_link_libraries(cao_archive_first_asset_discovery_tests PRIVATE
    Qt5::Test
    cao_run_discovery
)

set_target_properties(cao_archive_first_asset_discovery_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_archive_first_asset_discovery_tests COMMAND cao_archive_first_asset_discovery_tests)

add_executable(cao_asset_run_tests
    AssetRunTests.cpp
)

target_link_libraries(cao_asset_run_tests PRIVATE
    Qt5::Test
    cao_asset_run
)

set_target_properties(cao_asset_run_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_asset_run_tests COMMAND cao_asset_run_tests)

add_executable(cao_run_executor_tests
    RunExecutorTests.cpp
)

target_link_libraries(cao_run_executor_tests PRIVATE
    Qt5::Test
    cao_run_executor
)

set_target_properties(cao_run_executor_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_run_executor_tests COMMAND cao_run_executor_tests)

add_executable(cao_run_setup_tests
    RunSetupTests.cpp
)

target_link_libraries(cao_run_setup_tests PRIVATE
    Qt5::Test
    cao_run_setup
)

set_target_properties(cao_run_setup_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_run_setup_tests COMMAND cao_run_setup_tests)

add_executable(cao_application_logging_tests
    ApplicationLoggingTests.cpp
)

target_link_libraries(cao_application_logging_tests PRIVATE
    Qt5::Test
    cao_application_logging
    plog::plog
)

set_target_properties(cao_application_logging_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_application_logging_tests COMMAND cao_application_logging_tests)

add_executable(cao_main_optimizer_tests
    MainOptimizerTests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/AnimationsOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/FilesystemOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/MainOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/MeshesOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/PluginsOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/Profiles.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/TexturesOptimizer.cpp
)

target_link_libraries(cao_main_optimizer_tests PRIVATE
    Qt5::Test
    Qt5::Widgets
    cao_application_logging
    cao_asset_execution
    cao_asset_routing
    cao_mesh_reference_maintenance
    nifly
    Microsoft::DirectXTex
    btu::bethutil
    plog::plog
)

set_target_properties(cao_main_optimizer_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_main_optimizer_tests COMMAND cao_main_optimizer_tests)

add_executable(cao_application_run_setup_tests
    ApplicationRunSetupTests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/Profiles.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/Run/ApplicationRunSetup.cpp
)

target_link_libraries(cao_application_run_setup_tests PRIVATE
    Qt5::Test
    Qt5::Widgets
    cao_run_setup
    nifly
    Microsoft::DirectXTex
    btu::bethutil
    bsa::bsa
    plog::plog
)

target_compile_definitions(cao_application_run_setup_tests PRIVATE
    CAO_SOURCE_DIR="${PROJECT_SOURCE_DIR}"
)

target_include_directories(cao_application_run_setup_tests PRIVATE
    ${PROJECT_SOURCE_DIR}/src
)

set_target_properties(cao_application_run_setup_tests PROPERTIES AUTOMOC ON)

add_test(NAME cao_application_run_setup_tests COMMAND cao_application_run_setup_tests)

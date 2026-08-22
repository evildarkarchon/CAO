find_package(Qt5 COMPONENTS Test REQUIRED)

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

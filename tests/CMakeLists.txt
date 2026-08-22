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

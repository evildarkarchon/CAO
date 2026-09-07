add_library(cao_asset_execution
    AssetExecutor.cpp
    AssetExecutor.h
)

target_link_libraries(cao_asset_execution PUBLIC cao_asset_routing)
target_include_directories(cao_asset_execution PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_asset_execution PUBLIC cxx_std_20)

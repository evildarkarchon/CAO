# The mutation vocabulary is header-only and depends on nothing, so the Temporary Ownership
# registry in cao_run_executor can name it without linking cao_asset_execution, which itself
# links cao_run_executor and would otherwise form a cycle.
add_library(cao_mutation_state INTERFACE
    MutationState.h
)

target_include_directories(cao_mutation_state INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/..)

add_library(cao_asset_execution
    AssetExecutor.cpp
    AssetExecutor.h
)

target_link_libraries(cao_asset_execution PUBLIC cao_mutation_state)
target_link_libraries(cao_asset_execution PUBLIC cao_asset_routing)
target_link_libraries(cao_asset_execution PUBLIC cao_run_executor)
target_include_directories(cao_asset_execution PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_compile_features(cao_asset_execution PUBLIC cxx_std_20)

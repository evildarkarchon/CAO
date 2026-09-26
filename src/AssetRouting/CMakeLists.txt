add_library(cao_asset_routing
    AssetRouter.cpp
    AssetRouter.h
)

target_include_directories(cao_asset_routing PUBLIC
    "${PROJECT_SOURCE_DIR}/src"
)

target_compile_features(cao_asset_routing PUBLIC cxx_std_20)

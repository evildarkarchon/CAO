#Setting version for C++ files
configure_file (
    "${CMAKE_CURRENT_SOURCE_DIR}/Version.h.in"
    "${CMAKE_CURRENT_SOURCE_DIR}/Version.h"
    )

add_subdirectory(AssetRouting)
add_subdirectory(AssetExecution)
add_subdirectory(Run)

# Find includes in corresponding build directories
set(CMAKE_INCLUDE_CURRENT_DIR ON)

#Qt
find_package(Qt5 COMPONENTS Core Widgets REQUIRED)
#Qt Linguist
find_package(Qt5LinguistTools REQUIRED)

# Build either the desktop GUI or the command-line application from the same run-setup path.
option(CAO_BUILD_GUI "Build the Qt desktop GUI instead of the command-line application" ON)
set(GUI ${CAO_BUILD_GUI})

#SOURCES

set(SOURCES
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationsOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationsOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/BSAOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/BSAOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/FilesystemOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/FilesystemOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Logger.h
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Manager.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/Manager.h
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshesOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshesOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/OptionsCAO.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/OptionsCAO.h
    ${CMAKE_CURRENT_SOURCE_DIR}/PluginsOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/PluginsOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Profiles.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/Profiles.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Run/ApplicationRunSetup.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/Run/ApplicationRunSetup.h
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormats.h
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/main.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/pch.h
    )

#Ui
if(GUI)
    list (APPEND SOURCES
        ${CMAKE_CURRENT_SOURCE_DIR}/MainWindow.cpp
        ${CMAKE_CURRENT_SOURCE_DIR}/MainWindow.h
        ${CMAKE_CURRENT_SOURCE_DIR}/MainWindow.ui
        ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormatSelectDialog.cpp
        ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormatSelectDialog.h
        ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormatSelectDialog.ui)
endif()

#Icon and qrc
set(RESOURCES
    "${CMAKE_CURRENT_SOURCE_DIR}/Cathedral_Assets_Optimizer.rc"
    "${CMAKE_CURRENT_SOURCE_DIR}/Cathedral_Assets_Optimizer.manifest"
    "${CMAKE_CURRENT_SOURCE_DIR}/styles/qdarkstyle/style.qrc"
    )

#Generate translations
set(TS_FILES
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_de.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_en.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_es.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_fr_FR.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_ja.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_ko_KR.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_nl.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_tr.ts
    ${CMAKE_CURRENT_SOURCE_DIR}/translations/AssetsOpt_zh_CN.ts
    )

if(GUI)
    message("Updating translations files")
    qt5_create_translation(QM_FILES ${TS_FILES} ${SOURCES})
endif()

#plog
message("Adding Plog library")
find_path(PLOG_INCLUDE_DIRS "plog/Appenders/AndroidAppender.h")
add_compile_definitions(PLOG_OMIT_PLOG_DEFINES)
#bsa
find_package(bethutil CONFIG REQUIRED)
#Nif Library
find_package(nifly CONFIG REQUIRED)

add_library(cao_mesh_reference_maintenance
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshReferenceMaintenance.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshReferenceMaintenance.h
)
target_link_libraries(cao_mesh_reference_maintenance PUBLIC nifly)
target_include_directories(cao_mesh_reference_maintenance PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_features(cao_mesh_reference_maintenance PUBLIC cxx_std_20)
# DirectXTex
find_package(directxtex CONFIG REQUIRED)

# Instruct CMake to run moc automatically when needed
set(CMAKE_AUTOMOC ON)
# Create code from a list of Qt designer ui files
set(CMAKE_AUTOUIC ON)
#Ressources
set(CMAKE_AUTORCC ON)


#Build
message("Adding CAO.exe")
if(GUI)
    add_executable(Cathedral_Assets_Optimizer WIN32 ${SOURCES} ${RESOURCES} ${QM_FILES})
    target_compile_definitions(Cathedral_Assets_Optimizer PRIVATE GUI)
else()
    add_executable(Cathedral_Assets_Optimizer ${SOURCES} ${RESOURCES} ${QM_FILES})
endif()
#Windows 7 compatibility
target_link_libraries(Cathedral_Assets_Optimizer PRIVATE
    cao_asset_execution
    cao_asset_routing
    cao_asset_run
    cao_run_discovery
    cao_run_setup
    cao_mesh_reference_maintenance
    Qt5::Core Qt5::Widgets
    nifly Microsoft::DirectXTex btu::bethutil)
target_include_directories(Cathedral_Assets_Optimizer PRIVATE ${PLOG_INCLUDE_DIRS})

# PCH
target_precompile_headers(Cathedral_Assets_Optimizer PRIVATE "pch.h")

target_compile_features(Cathedral_Assets_Optimizer PUBLIC cxx_std_20)


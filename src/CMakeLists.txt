#Setting version for C++ files
configure_file (
    "${CMAKE_CURRENT_SOURCE_DIR}/Version.h.in"
    "${CMAKE_CURRENT_SOURCE_DIR}/Version.h"
    )

# Find includes in corresponding build directories
set(CMAKE_INCLUDE_CURRENT_DIR ON)

#Qt
find_package(Qt5 COMPONENTS Core Widgets REQUIRED)
#Qt Linguist
find_package(Qt5LinguistTools REQUIRED)

#Setting GUI
#Comment out the next line to create a CLI version instead (untested)
set(GUI true)

# Instruct CMake to run moc automatically when needed
set(CMAKE_AUTOMOC ON)
# Create code from a list of Qt designer ui files
set(CMAKE_AUTOUIC ON)
#Ressources
set(CMAKE_AUTORCC ON)

#SOURCES

set(CORE_SOURCES
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetTransaction.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkMode.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkExecutionPolicy.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetPathConflicts.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetPathConflicts.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptions.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptions.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPolicyResolver.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPolicyResolver.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkProfileSnapshot.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkProfileSnapshot.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptionsUiState.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptionsUiState.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPolicy.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPolicy.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlan.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlan.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlanExecutor.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkPlanExecutor.h
    ${CMAKE_CURRENT_SOURCE_DIR}/LooseAssetScheduler.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/LooseAssetScheduler.h
    ${CMAKE_CURRENT_SOURCE_DIR}/FilesystemOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/FilesystemOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Logger.h
    ${CMAKE_CURRENT_SOURCE_DIR}/ManagerPlanning.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ManagerPlanning.h
    ${CMAKE_CURRENT_SOURCE_DIR}/ModAssetMetadata.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ModAssetMetadata.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptionsDraft.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptionsDraft.h
    ${CMAKE_CURRENT_SOURCE_DIR}/PluginsOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/PluginsOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/PluginsOperationsInternal.h
    ${CMAKE_CURRENT_SOURCE_DIR}/Profiles.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/Profiles.h
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesFormats.h
    ${CMAKE_CURRENT_SOURCE_DIR}/pch.h
    )

set(EXECUTION_SOURCES
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationAssetTransaction.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationAssetTransaction.h
    ${CMAKE_CURRENT_SOURCE_DIR}/ArchiveEngine.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ArchiveEngine.h
    ${CMAKE_CURRENT_SOURCE_DIR}/ArchiveExecutionError.h
    ${CMAKE_CURRENT_SOURCE_DIR}/ArchiveFileOperations.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ArchiveFileOperations.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationsOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/AnimationsOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/BsaOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/BsaOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizerInternal.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MainOptimizerInternal.h
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshesOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/MeshesOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesOptimizer.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/TexturesOptimizer.h
    ${CMAKE_CURRENT_SOURCE_DIR}/TextureAssetTransaction.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/TextureAssetTransaction.h
    )

set(APP_SOURCES
    ${CMAKE_CURRENT_SOURCE_DIR}/Manager.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/Manager.h
    ${CMAKE_CURRENT_SOURCE_DIR}/AssetWorkOptionsDraftUi.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/ProfilesUi.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/main.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/pch.h
    )

#Ui
if(GUI)
    list (APPEND APP_SOURCES
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

message("Updating translations files")
qt5_create_translation(QM_FILES ${TS_FILES} ${CORE_SOURCES} ${APP_SOURCES})

#plog
message("Adding Plog library")
find_path(PLOG_INCLUDE_DIRS "plog/Appenders/AndroidAppender.h")
add_compile_definitions(PLOG_OMIT_PLOG_DEFINES)
#bsa
find_package(bethutil CONFIG REQUIRED)
#Nif Library
find_package(nifly CONFIG REQUIRED)
# DirectXTex
find_package(directxtex CONFIG REQUIRED)

#Build
message("Adding CAO core library")
add_library(Cathedral_Assets_Optimizer_Core STATIC ${CORE_SOURCES})
target_link_libraries(Cathedral_Assets_Optimizer_Core PUBLIC
    Qt5::Core Qt5::Widgets
    nifly Microsoft::DirectXTex btu::bethutil)
target_include_directories(Cathedral_Assets_Optimizer_Core PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${PLOG_INCLUDE_DIRS})
target_precompile_headers(Cathedral_Assets_Optimizer_Core PRIVATE "pch.h")
target_compile_features(Cathedral_Assets_Optimizer_Core PUBLIC cxx_std_20)

message("Adding CAO execution library")
add_library(Cathedral_Assets_Optimizer_Execution STATIC ${EXECUTION_SOURCES})
target_link_libraries(Cathedral_Assets_Optimizer_Execution PUBLIC
    Cathedral_Assets_Optimizer_Core)
target_precompile_headers(Cathedral_Assets_Optimizer_Execution PRIVATE "pch.h")
target_compile_features(Cathedral_Assets_Optimizer_Execution PUBLIC cxx_std_20)

message("Adding CAO.exe")
add_executable(Cathedral_Assets_Optimizer WIN32 ${APP_SOURCES} ${RESOURCES} ${QM_FILES})
#Windows 7 compatibility
target_compile_definitions(Cathedral_Assets_Optimizer PRIVATE GUI)
target_link_libraries(Cathedral_Assets_Optimizer PRIVATE
    Cathedral_Assets_Optimizer_Execution)

# PCH
target_precompile_headers(Cathedral_Assets_Optimizer PRIVATE "pch.h")

target_compile_features(Cathedral_Assets_Optimizer PUBLIC cxx_std_20)


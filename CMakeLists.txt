cmake_minimum_required(VERSION 3.12...3.22)

project(Cathedral_Assets_Optimizer VERSION 5.3.14 LANGUAGES CXX)

include(CTest)

find_package(plog CONFIG REQUIRED)

add_subdirectory(src)

if(BUILD_TESTING)
    add_subdirectory(tests)
endif()

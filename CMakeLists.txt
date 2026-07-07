cmake_minimum_required(VERSION 3.24...4.3)

project(Cathedral_Assets_Optimizer VERSION 5.3.14 LANGUAGES CXX)

option(BUILD_TESTING "Build tests" OFF)
include(CTest)

add_subdirectory(src)

if(BUILD_TESTING)
  add_subdirectory(tests)
endif()

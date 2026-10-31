{{#if cxx_tests == GoogleTest}}
include(FetchContent)
FetchContent_Declare(googletest
    URL https://github.com/google/googletest/archive/refs/tags/v1.17.0.zip
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
FetchContent_MakeAvailable(googletest)
include(GoogleTest)

add_executable({{name_snake}}_tests test_{{name_snake}}.cpp)
target_link_libraries({{name_snake}}_tests PRIVATE {{name_snake}} GTest::gtest_main)
gtest_discover_tests({{name_snake}}_tests)
{{/if}}
{{#if cxx_tests == Catch2}}
include(FetchContent)
FetchContent_Declare(Catch2
    URL https://github.com/catchorg/Catch2/archive/refs/tags/v3.8.1.zip
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_MakeAvailable(Catch2)
list(APPEND CMAKE_MODULE_PATH ${catch2_SOURCE_DIR}/extras)
include(Catch)

add_executable({{name_snake}}_tests test_{{name_snake}}.cpp)
target_link_libraries({{name_snake}}_tests PRIVATE {{name_snake}} Catch2::Catch2WithMain)
catch_discover_tests({{name_snake}}_tests)
{{/if}}
{{#if cxx_tests == CTest}}
add_executable({{name_snake}}_tests test_{{name_snake}}.cpp)
target_link_libraries({{name_snake}}_tests PRIVATE {{name_snake}})
add_test(NAME {{name_snake}}_tests COMMAND {{name_snake}}_tests)
{{/if}}

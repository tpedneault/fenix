cmake_minimum_required(VERSION 3.25)
{{#if language == C++}}
project({{name_snake}} VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD {{cxx_standard}})
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
{{else}}
project({{name_snake}} VERSION 0.1.0 LANGUAGES C)

set(CMAKE_C_STANDARD {{c_standard}})
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
{{/if}}
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
# Programs, tests and shared libraries side by side, so a test finds the
# DLL it links on Windows.
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin)

include(GenerateExportHeader)

{{#if artifact == shared library}}
add_library({{name_snake}} SHARED
{{else}}
add_library({{name_snake}} STATIC
{{/if}}
{{#if language == C++}}
    src/{{name_snake}}.cpp
{{else}}
    src/{{name_snake}}.c
{{/if}}
)
target_include_directories({{name_snake}} PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
    $<BUILD_INTERFACE:${CMAKE_CURRENT_BINARY_DIR}/include>
)
generate_export_header({{name_snake}} EXPORT_FILE_NAME include/{{name_snake}}/export.h)
{{#if artifact != shared library}}
target_compile_definitions({{name_snake}} PUBLIC {{name_upper}}_STATIC_DEFINE)
{{/if}}
if(MSVC)
    target_compile_options({{name_snake}} PRIVATE /W4)
else()
    target_compile_options({{name_snake}} PRIVATE -Wall -Wextra -Wpedantic)
endif()
{{#if artifact == executable}}

add_executable({{name_snake}}_app
{{#if language == C++}}
    src/main.cpp
{{else}}
    src/main.c
{{/if}}
)
target_link_libraries({{name_snake}}_app PRIVATE {{name_snake}})
set_target_properties({{name_snake}}_app PROPERTIES OUTPUT_NAME {{name_kebab}})
add_custom_target(run COMMAND {{name_snake}}_app DEPENDS {{name_snake}}_app USES_TERMINAL)
{{/if}}
{{#if language == C++}}
{{#if cxx_tests != none}}

enable_testing()
add_subdirectory(tests)
{{/if}}
{{else}}
{{#if c_tests != none}}

enable_testing()
add_subdirectory(tests)
{{/if}}
{{/if}}

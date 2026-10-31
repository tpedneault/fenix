add_executable({{name_snake}}_tests test_{{name_snake}}.c)
target_link_libraries({{name_snake}}_tests PRIVATE {{name_snake}})
add_test(NAME {{name_snake}}_tests COMMAND {{name_snake}}_tests)

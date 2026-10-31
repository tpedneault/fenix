{{#if cxx_tests == GoogleTest}}
#include <gtest/gtest.h>

#include "{{name_snake}}/{{name_snake}}.hpp"

TEST({{name_pascal}}, AddsTwoNumbers) {
    EXPECT_EQ({{name_snake}}::add(2, 3), 5);
}
{{/if}}
{{#if cxx_tests == Catch2}}
#include <catch2/catch_test_macros.hpp>

#include "{{name_snake}}/{{name_snake}}.hpp"

TEST_CASE("add adds two numbers") {
    REQUIRE({{name_snake}}::add(2, 3) == 5);
}
{{/if}}
{{#if cxx_tests == CTest}}
#include <cstdlib>
#include <iostream>

#include "{{name_snake}}/{{name_snake}}.hpp"

int main() {
    if ({{name_snake}}::add(2, 3) != 5) {
        std::cerr << "add(2, 3) isn't 5\n";
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
{{/if}}

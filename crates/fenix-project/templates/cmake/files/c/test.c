#include <stdio.h>

#include "{{name_snake}}/{{name_snake}}.h"

int main(void) {
    if ({{name_snake}}_add(2, 3) != 5) {
        fprintf(stderr, "{{name_snake}}_add(2, 3) isn't 5\n");
        return 1;
    }
    return 0;
}

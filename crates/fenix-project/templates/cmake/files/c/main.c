#include <stdio.h>

#include "{{name_snake}}/{{name_snake}}.h"

int main(void) {
    printf("{{name}}: 2 + 3 = %d\n", {{name_snake}}_add(2, 3));
    return 0;
}

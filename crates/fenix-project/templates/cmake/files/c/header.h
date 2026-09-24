#ifndef {{name_upper}}_H
#define {{name_upper}}_H

#include "{{name_snake}}/export.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Adds two numbers -- a stand-in for the library's first real function. */
{{name_upper}}_EXPORT int {{name_snake}}_add(int a, int b);

#ifdef __cplusplus
}
#endif

#endif

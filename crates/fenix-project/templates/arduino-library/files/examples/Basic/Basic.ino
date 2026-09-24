#include <{{name}}.h>

{{name_pascal}} led(LED_BUILTIN);

void setup() {
    led.begin();
}

void loop() {
    led.blink(500);
}

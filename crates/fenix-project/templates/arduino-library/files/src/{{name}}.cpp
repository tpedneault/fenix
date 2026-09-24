#include "{{name}}.h"

{{name_pascal}}::{{name_pascal}}(uint8_t pin) : pin_(pin) {}

void {{name_pascal}}::begin() {
    pinMode(pin_, OUTPUT);
}

void {{name_pascal}}::blink(unsigned long ms) {
    digitalWrite(pin_, HIGH);
    delay(ms);
    digitalWrite(pin_, LOW);
    delay(ms);
}

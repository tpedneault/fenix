#pragma once

#include <Arduino.h>

// Blinks an LED -- a stand-in for the library's first real class.
class {{name_pascal}} {
public:
    explicit {{name_pascal}}(uint8_t pin);
    void begin();
    void blink(unsigned long ms);

private:
    uint8_t pin_;
};

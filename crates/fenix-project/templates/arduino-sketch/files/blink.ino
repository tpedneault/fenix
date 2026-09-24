// {{name}}: blinks the built-in LED once a second.

void setup() {
  pinMode(LED_BUILTIN, OUTPUT);
  Serial.begin({{baud}});
}

void loop() {
  digitalWrite(LED_BUILTIN, HIGH);
  delay(500);
  digitalWrite(LED_BUILTIN, LOW);
  delay(500);
}

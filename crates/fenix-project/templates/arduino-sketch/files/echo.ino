// {{name}}: echoes every line received on the serial port.

void setup() {
  Serial.begin({{baud}});
  Serial.println("{{name}} ready");
}

void loop() {
  if (Serial.available()) {
    String line = Serial.readStringUntil('\n');
    Serial.print("echo: ");
    Serial.println(line);
  }
}

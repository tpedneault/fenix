# heater line 3 to thermostat control
proc heaters_auto {} {
    tc::send ZTC08101 -PTH00101 3, -PTH00102 AUTO
    
}

[![License BSD-2-Clause](https://img.shields.io/badge/License-BSD--2--Clause-blue.svg)](https://opensource.org/licenses/BSD-2-Clause)
[![License MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)


# `moisturesensor-rp2040-picow`
Welcome to `moisturesensor-rp2040-picow` 🎉

This firmware is a Raspberry Pi Pico W application that can read data from a capacitive moisture sensor connected via
the analogue pin, and publishes them via MQTT.


## Config
The sensor needs a network and MQTT configuration, which has to be deployed independently of the firmware image:

1. Create an INI-like configuration file `my-moisture-sensor.cfg`:
   ```ini
   # WIFI SSID
   WIFI_SSID=My WiFi Name
   WIFI_PASS=My WiFi Password lol
   
   # MQTT configuration
   MQTT_ADDR=192.0.2.1:1883
   MQTT_USER=my optional mqtt username
   MQTT_PASS=my optional mqtt password
   MQTT_PRFX=my-optional-mqtt-prefix/
   
   # Sleep intervals
   SENSOR_SLEEP_SECS=600
   SENSOR_ALERT_SECS=15
   ```

2. Then, copy the file into the userdata section on your device via
   [`picotool`](https://github.com/raspberrypi/picotool):
   ```sh
   picotool load ./my-moisture-sensor.cfg -t bin -o 0x101FF000
   ```

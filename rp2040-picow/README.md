[![License BSD-2-Clause](https://img.shields.io/badge/License-BSD--2--Clause-blue.svg)](https://opensource.org/licenses/BSD-2-Clause)
[![License MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)


# `moisturesensor-rp2040-picow`
Welcome to `moisturesensor-rp2040-picow` 🎉

This firmware is a Raspberry Pi Pico W application that can read data from a capacitive moisture sensor connected via
the analogue pin, and publishes them via MQTT.


## Hardware and Wiring
The firmware is designed to run on an original Raspberry Pi Pico W with an analogue capacitive moisture sensor with the
signal pin connected to [`GP28`/`ADC2`](./RPi%20Pico%20Pinout.png). The firmware will read the analogue voltage on that
pin and transmit the values via MQTT. 


## Setup
You will need the following prerequisites:
- A suitable **release** image. You can fetch the latest release from
  <https://github.com/KizzyCode/MoistureSensor/releases>, or build the release yourself.

  **Important**: Make sure to fetch or build a _release_ image, not a debug image – the debug images will not work as
  they will crash if there is no debugger attached.
- The [Raspberry Pi `picotool`](https://github.com/raspberrypi/picotool), to flash the image and configuration.
- The targeted Raspberry Pi Pico in `BOOTSEL`-mode connected via USB.


### Flash the Firmware
To flash the firmware, simple execute the `picotool` with your firmware image of choice:
```sh
picotool load -v ./firmware.elf
```


### Flash the Config
The sensor needs a network and MQTT configuration, which has to be deployed independently of the firmware image:

1. Create an INI-like configuration file `moisturesensor.cfg`:
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

2. Copy the file into the userdata section on your device via [`picotool`](https://github.com/raspberrypi/picotool):
   ```sh
   picotool load -v ./moisturesensor.cfg -t bin -o 0x101FF000
   ```

   **Important**: `0x101FF000` is the address of the userdata-config section; do not change that number unless you
   really know what you're doing.

3. Now you can powercycle the device, and the firmware should start with the correct configuration.

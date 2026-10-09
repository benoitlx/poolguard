# poolguard

Embedded system for pool water quality monitoring. An ESP32 (WT32-ETH01, wired Ethernet) reads sensors through a shared ADC and exposes data + control over MQTT, with OTA updates so the enclosure never needs to be opened for a firmware update.

## Status

**Implemented**: Ethernet + authenticated MQTT + OTA base; pH, pressure, and ORP sensing via a single ADS1115 ADC with per-channel adjustable voltage dividers; two-point calibration for pH and ORP, persisted to NVS; firmware version counter incremented on each OTA update; structured event log over MQTT; acid/base/chlorine dosing pump triggering with fail-safe defaults, mutual exclusion, and a duration clamp; MQTT Discovery so all of the above shows up in Home Assistant automatically.

**Not yet implemented**: temperature probes, the full dosing safety design (sensor validation, fractional dosing, daily limits, fault codes), pressure in physical units (bar). See [doc/roadmap.md](doc/roadmap.md).

## Quickstart

```bash
source ~/export-esp.sh                              # activate Xtensa toolchain
cp .env.example .env                                 # fill in MQTT_USERNAME / MQTT_PASSWORD
just build                                            # compile
just flash                                            # flash via UART + open serial monitor
```

First flash needs UART (see [doc/development.md](doc/development.md)); every update after that can go over OTA (see [doc/mqtt.md](doc/mqtt.md#commands-subscribed-by-device)).

## Documentation

| Doc | Covers |
|-----|--------|
| [doc/hardware.md](doc/hardware.md) | Components, wiring, GPIO usage |
| [doc/mqtt.md](doc/mqtt.md) | Full MQTT topic reference (telemetry + commands) |
| [doc/calibration.md](doc/calibration.md) | pH / ORP two-point calibration procedure and pitfalls |
| [doc/home-assistant.md](doc/home-assistant.md) | Exposing sensors as Home Assistant entities |
| [doc/development.md](doc/development.md) | Toolchain setup, build, flash, OTA workflow |
| [doc/ota-troubleshooting.md](doc/ota-troubleshooting.md) | Toolchain/partition-table/MQTT pitfalls hit while bringing this up — reference, not a bug list |
| [doc/roadmap.md](doc/roadmap.md) | Planned features not yet implemented (temperature, dosing, safety logic) |

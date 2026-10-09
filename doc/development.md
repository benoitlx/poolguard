# Development Guide

## Toolchain setup (one-time)

The ESP32 uses an Xtensa architecture not supported by the standard Rust compiler, so Espressif maintains a fork:

```bash
# Install the Xtensa Rust toolchain + LLVM
cargo install espup
espup install
# Creates ~/export-esp.sh — add this to your shell profile
source ~/export-esp.sh

# Install the flashing tool and linker proxy
cargo install espflash
cargo install ldproxy
```

`rust-toolchain.toml` (`channel = "esp"`) pins the project to this toolchain automatically once it's installed.

## Build

```bash
cargo build --release
# or: just build
```

The `build-std` setting in `.cargo/config.toml` downloads and compiles `std` from source for the Xtensa target — first build takes several minutes, subsequent builds are incremental.

MQTT credentials are baked in at compile time from `MQTT_USERNAME`/`MQTT_PASSWORD` env vars (see `.env.example` — copy to `.env`, `just` loads it automatically via `dotenv-load`).

## Hardware wiring to flash

The WT32-ETH01 has no onboard USB chip. You need a USB-to-UART adapter (CP2102/CP2104, CH340, FTDI) — **check it's 3.3V logic**, the board is not 5V-tolerant:

```
Adapter          WT32-ETH01
-------          ----------
TX       →       RX  (GPIO3)
RX       ←       TX  (GPIO1)
GND      —       GND
3.3V     →       3.3V (do NOT use 5V)
```

To enter bootloader/download mode before flashing:
1. Hold **IO0** (GPIO0) LOW
2. Pulse **EN** (reset pin) LOW then HIGH
3. Release IO0

Exit download mode by pressing EN after flashing completes (with IO0 released, not held low).

If flashing fails with `Permission denied` on the serial port: `sudo usermod -aG dialout $USER`, then log out/in completely.

## Flash

```bash
# Flash + open serial monitor immediately after
cargo run --release
# or: just flash

# Or manually, targeting a specific port
espflash flash --port /dev/ttyUSB0 --monitor \
  --partition-table=partitions.csv \
  target/xtensa-esp32-espidf/release/poolguard
```

`espflash` auto-detects the port in most cases. The `runner` entry in `.cargo/config.toml` means `cargo run`/`just flash` call espflash directly with the correct partition table.

To monitor serial output without flashing:
```bash
espflash monitor --port /dev/ttyUSB0
# or: just monitor
```

## OTA

OTA is implemented (`src/ota.rs`) — the device fetches a firmware image over HTTP on receiving a URL on `pool/cmd/ota`, flashes it via `esp_idf_svc::ota::EspOta`, bumps the version counter (`pool/status/version`), and reboots. Since the board typically lives inside a sealed enclosure, this is the normal path for every update after the first UART flash.

```bash
just ota-image                    # build + package the flashable app image
just ota-serve                    # serve it over HTTP (separate terminal, blocking)
just ota-push                     # publish the URL to pool/cmd/ota
```

See [ota-troubleshooting.md](ota-troubleshooting.md) for the partition table / image format / MQTT deadlock pitfalls this is built on, and [mqtt.md](mqtt.md) for the full topic reference.

## Typical iteration loop

```
edit code → just build → just flash --monitor   (first deploy, or after a partition table change)
edit code → just build → OTA (ota-image / ota-serve / ota-push)   (thereafter)
```

The UART adapter is only needed again if the firmware crashes before the ethernet/MQTT stack comes up (an OTA can't fix that, since it needs MQTT to even receive the command), or after a partition table change (OTA can't rewrite the partition table itself — see [ota-troubleshooting.md](ota-troubleshooting.md)).

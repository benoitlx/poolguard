# Run `just` with no arguments to list all recipes.
#
# MQTT credentials are read from the environment (MQTT_USERNAME/MQTT_PASSWORD).
# Copy .env.example to .env and fill it in — `just` loads it automatically.
#
# See OTA.md for the full story behind these commands (partition table,
# why the OTA image isn't the raw ELF, the MQTT deadlock gotcha, etc).

set dotenv-load := true

broker := "192.168.1.142"
serial_port := "/dev/ttyUSB0"
default_bin := "poolguard"
http_port := "8080"
ota_image := "ota.bin"
lan_ip := `hostname -I | awk '{print $1}'`

default:
    @just --list

# --- UART ---

# Build + flash + open serial monitor over UART
flash bin=default_bin:
    cargo run --release --bin {{bin}}

# Build only, no flash
build bin=default_bin:
    cargo build --release --bin {{bin}}

# Open the serial monitor without reflashing
monitor port=serial_port:
    espflash monitor --port {{port}}

# Wipe the whole flash chip — needed once after changing the partition table
# (OTA can't do this transition itself, see OTA.md)
erase port=serial_port:
    espflash erase-flash --port {{port}}

# --- OTA ---

# Build + package the flashable app image (NOT the raw ELF from `build`)
ota-image bin=default_bin: (build bin)
    espflash save-image --chip esp32 target/xtensa-esp32-espidf/release/{{bin}} {{ota_image}}

# Serve the OTA image over HTTP — blocking, run in its own terminal
# If unreachable from other devices on the LAN: sudo firewall-cmd --add-port={{http_port}}/tcp
ota-serve port=http_port:
    python3 -m http.server {{port}} --directory .

# Trigger the OTA update: publishes the image URL to pool/cmd/ota.
# Run ota-image then ota-serve (separate terminal) first.
ota-push ip=lan_ip port=http_port:
    mosquitto_pub -h {{broker}} -u "$MQTT_USERNAME" -P "$MQTT_PASSWORD" \
        -t pool/cmd/ota -m "http://{{ip}}:{{port}}/{{ota_image}}"

# --- Debug helpers ---

# Watch the heartbeat topic
watch-heartbeat:
    mosquitto_sub -h {{broker}} -u "$MQTT_USERNAME" -P "$MQTT_PASSWORD" -t pool/test/heartbeat -v

# Print this machine's best-guess LAN IP (used as ota-push's default ip=)
ip:
    @echo {{lan_ip}}

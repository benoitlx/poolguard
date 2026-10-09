# Home Assistant Integration

## Automatic (MQTT Discovery) — recommended

The firmware publishes [MQTT Discovery](https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery) config payloads for every sensor and command topic, right after connecting (`discovery::publish_all`, called from `mqtt::heartbeat_loop`). If Home Assistant's MQTT integration is set up and discovery is enabled (the default), a **"Pool Monitor"** device appears automatically under **Settings → Devices & Services → MQTT** with all entities grouped under it — no YAML needed.

Published entities:
- **Sensors**: pH, ORP, Pressure, pH/ORP/Pressure Raw, Firmware Version, Log, Heartbeat (the last four as diagnostic entities)
- **Numbers** (write-only, optimistic — see the note below): pH/Pressure/ORP Divider Ratio, pH/ORP Calibration Point, Acid/Base/Chlorine Dose Duration

Discovery payloads are retained, so they persist on the broker and don't need to be re-sent for HA to pick them up on its next restart — but the firmware republishes them on every boot/reconnect anyway (harmless: identical retained content is a no-op for HA).

> **Number entities here are optimistic**: there's no `state_topic` for divider ratios or calibration points (nothing publishes them back yet), and dose durations are one-shot commands with nothing to reflect. The number you see in the HA UI right after setting it is just an echo of what you typed, not a confirmation the device applied it — check `pool/status/log` for that (see [calibration.md](calibration.md) and [mqtt.md](mqtt.md#dosing-behavior)).

If discovery ever needs to be redone from scratch (e.g. after renaming a topic), delete the stale retained config from the broker (`mosquitto_pub -t homeassistant/sensor/poolguard/<object_id>/config -n -r` to clear a retained message) — HA removes the entity once the empty retained payload arrives.

## Manual YAML (fallback / reference)

If discovery is disabled, or you want more control over icons/grouping than the firmware's defaults, entities can still be set up by hand against the topics in [mqtt.md](mqtt.md), either through **Settings → Devices & Services → MQTT → Add entity**, or in YAML:

```yaml
mqtt:
  sensor:
    - name: "Pool pH"
      state_topic: "pool/sensors/ph"
      device_class: ph
      icon: mdi:flask

    - name: "Pool ORP"
      state_topic: "pool/sensors/orp"
      unit_of_measurement: "mV"
      icon: mdi:flash

    - name: "Pool Pressure"
      state_topic: "pool/sensors/pressure"
      unit_of_measurement: "V"
      icon: mdi:gauge

    - name: "Pool pH Raw"
      state_topic: "pool/sensors/ph/raw"
      unit_of_measurement: "mV"
      entity_category: diagnostic

    - name: "Pool ORP Raw"
      state_topic: "pool/sensors/orp/raw"
      unit_of_measurement: "mV"
      entity_category: diagnostic

    - name: "Pool Pressure Raw"
      state_topic: "pool/sensors/pressure/raw"
      unit_of_measurement: "mV"
      entity_category: diagnostic

    - name: "Pool Firmware Version"
      state_topic: "pool/status/version"
      entity_category: diagnostic

    - name: "Pool Log"
      state_topic: "pool/status/log"
      entity_category: diagnostic
```

Reload with **Developer tools → YAML → Reload MQTT** (or restart Home Assistant).

> **Note**: `pool/sensors/ph` and `pool/sensors/orp` publish a raw divider-corrected voltage (e.g. `1.9125`) instead of a real pH/mV value until both calibration points are set — see [calibration.md](calibration.md). Home Assistant will happily display that number under the `pH`/`mV` unit with no error, which is misleading until calibration is done.

## Sending commands from Home Assistant

Calibration, divider ratio, and dosing commands already get a `number` entity each via MQTT Discovery (see above) — no manual setup needed for those. **OTA is the one command left to wire up manually**, since the firmware doesn't publish a discovery entity for it (an MQTT `text` entity needs a relatively recent HA version, and the risk of a typo triggering an unwanted flash felt worth keeping deliberate/manual for now).

**Reminder for any manual entity you add**: the payload must be the bare value, e.g. `4.00`, not `calibrate pH 4.00` — see [calibration.md](calibration.md#pitfall-payload-format).

Example — a helper to trigger OTA from a dashboard:

```yaml
mqtt:
  text:
    - name: "Pool OTA URL"
      command_topic: "pool/cmd/ota"
      command_template: "{{ value }}"
```

## Config file editing over SSH (vim)

If you're editing `configuration.yaml` over SSH with vim and pasting multi-line YAML:

1. Locate the file if unsure of the path: `find / -iname "configuration.yaml" 2>/dev/null` (typically `/config/configuration.yaml` on Home Assistant OS/Supervised via the SSH & Terminal add-on).
2. Check whether a top-level `mqtt:` key already exists (`grep -n "^mqtt:" configuration.yaml`) — YAML doesn't allow duplicate top-level keys, so new entries need to be merged into the existing `sensor:` list, not pasted as a second `mqtt:` block.
3. In vim, enable paste mode before pasting to avoid autoindent mangling the YAML: `:set paste`, then `i` to insert, paste, `Esc`, `:set nopaste`, `:wq`.
4. Validate before restarting: **Developer tools → YAML → Check configuration**.

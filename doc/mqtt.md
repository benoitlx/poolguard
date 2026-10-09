# MQTT Interface

All topics are prefixed with `pool/`. Broker: `192.168.1.142:1883`, username/password auth (see `.env.example`).

> **Payload format**: every command topic below takes the **bare value only** — e.g. publish `4.00` to `pool/cmd/calibrate/ph`, not the text `calibrate pH 4.00`. There is currently **no** single command topic that parses full text commands, even though that style is what earlier project notes described. Sending a full sentence as the payload produces `ERROR: Invalid ... payload` on `pool/status/log` — this bit a real Home Assistant setup once already, see [calibration.md](calibration.md).

## Telemetry (published by device)

### Status

| Topic | Payload | Retained | Description |
|-------|---------|----------|-------------|
| `pool/status` | `pool-monitor #42` | No | Heartbeat, published roughly every 5s (delayed if a log message is waiting — see below) |
| `pool/status/version` | `3` | **Yes** | OTA update counter — starts at 0, incremented once per successful OTA flash (see [development.md](development.md)). Published once right after MQTT connects, retained so late subscribers get it immediately |
| `pool/status/log` | `INFO: ...` / `ERROR: ...` | No | Structured event log — calibration confirmations, divider ratio changes, OTA command received, invalid command payloads, I2C/ADS1115 init failures. See [calibration.md](calibration.md) for example messages |

### Sensors

Polled every 5s. Each sensor publishes two topics: the corrected/calibrated value, and a `/raw` topic showing the ADS1115 pin voltage *before* the divider-ratio correction — useful for debugging wiring/dividers independent of calibration.

| Topic | Payload | Description |
|-------|---------|-------------|
| `pool/sensors/ph` | `7.42` or `1.9125` or `ERR: ...` | Calibrated pH (2 decimals) once both calibration points are set; otherwise the divider-corrected probe voltage in V (4 decimals) as a placeholder |
| `pool/sensors/ph/raw` | `1912.50` or `ERR: ...` | Raw ADS1115 pin voltage in mV, before the divider ratio is applied |
| `pool/sensors/orp` | `562.3` or `1.9125` or `ERR: ...` | Calibrated ORP in mV (1 decimal) once both calibration points are set; otherwise the divider-corrected probe voltage in V (4 decimals) |
| `pool/sensors/orp/raw` | `1275.25` or `ERR: ...` | Raw ADS1115 pin voltage in mV, before the divider ratio is applied |
| `pool/sensors/pressure` | `1.9125` or `ERR: ...` | Divider-corrected probe voltage in V — **not yet converted to a physical pressure unit** (bar), see [roadmap.md](roadmap.md) |
| `pool/sensors/pressure/raw` | `1275.25` or `ERR: ...` | Raw ADS1115 pin voltage in mV |

## Commands (subscribed by device)

Subscribed once, right after the MQTT `Connected` event.

| Topic | Payload | Description |
|-------|---------|-------------|
| `pool/cmd/ota` | `http://192.168.1.50:8080/ota.bin` | Fetches and flashes the firmware at this URL, bumps the version counter, reboots. No quotes in the payload |
| `pool/cmd/divider/ph` | `1.5` | Sets the pH channel's voltage-divider ratio (Vsensor/Vadc), applied to all subsequent readings |
| `pool/cmd/divider/pressure` | `1.5` | Same, for the pressure channel |
| `pool/cmd/divider/orp` | `1.5` | Same, for the ORP channel |
| `pool/cmd/calibrate/ph` | `4.00` then `7.00` | Two-point pH calibration — see [calibration.md](calibration.md) |
| `pool/cmd/calibrate/orp` | `0.0` then `470` | Two-point ORP calibration — see [calibration.md](calibration.md) |
| `pool/cmd/dose/acid` | `5` | Runs the acid pump (GPIO14) for 5 seconds |
| `pool/cmd/dose/base` | `5` | Runs the base pump (GPIO1) for 5 seconds |
| `pool/cmd/dose/chlorine` | `5` | Runs the chlorine pump (GPIO3) for 5 seconds |

> **Changing a divider ratio invalidates any existing calibration** on that channel — the calibration points were recorded in the old voltage scale. Recalibrate after adjusting a divider ratio.

### Dosing behavior

- **Duration is clamped** to a hard maximum (10s as shipped, see `dosing.rs`) regardless of what's requested — a mistyped duration can't run a pump indefinitely. A clamped dose logs `ERROR: ... dose clamped from Xs to Ys (max Ys)` on `pool/status/log`.
- **Only one pump runs at a time.** A dose command received while another pump is active is **rejected outright**, not queued — `ERROR: Dose command for <pump> rejected: another pump is active` on `pool/status/log`. Wait for the current dose to finish before sending the next.
- This is a baseline safety net only (fail-safe-off GPIO default, mutual exclusion, duration clamp) — there's no sensor validation, fractional/re-measure dosing, daily limits, or fault-code state yet. See [roadmap.md](roadmap.md).

## Not yet implemented

Referenced in earlier planning but with no firmware code yet:

- `pool/cmd/fault/clear`, `pool/cmd/counter/reset` — dosing safety state management (daily limits, fault codes)
- `pool/sensors/temperature/N` — temperature probes
- `pool/dosing/acid/daily`, `pool/dosing/base/daily` — dosing counters

See [roadmap.md](roadmap.md).

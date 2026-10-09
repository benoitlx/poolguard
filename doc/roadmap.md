# Roadmap

Features described in earlier project planning that have **no firmware code yet**. Kept here so the design work isn't lost, but clearly separated from [hardware.md](hardware.md) / [mqtt.md](mqtt.md), which describe only what's actually running today.

## Temperature

DS18B20 waterproof probes (1-Wire, multiple probes on one GPIO). Needs a free GPIO — see the reserved-pin table in [hardware.md](hardware.md) before picking one (GPIO32 was the original candidate but is now used by I2C).

Planned topics: `pool/sensors/temperature/0`, `pool/sensors/temperature/1`, ... (one per probe, index assignment TBD — by 1-Wire discovery order, or a fixed ROM-address mapping for stability across reboots).

## Pressure in physical units

`pool/sensors/pressure` currently publishes the divider-corrected probe voltage in volts, not a pressure reading. Converting to bar needs the transducer's actual voltage-to-pressure transfer function (linear, but slope/offset depend on the specific sensor's rated range — e.g. a 4-20mA transducer through a shunt resistor has a different curve than a direct 0.5-4.5V output sensor).

## Dosing pumps — safety design beyond the baseline

Basic GPIO triggering for all three pumps (acid GPIO14, base GPIO17, chlorine GPIO5) **is implemented** — `pool/cmd/dose/{acid,base,chlorine}` ← duration in seconds, see [mqtt.md](mqtt.md). What's implemented is a baseline safety net only: fail-safe-off default, active-high, mutual exclusion (one pump at a time, extras rejected not queued), and a hard per-dose duration clamp (`dosing.rs`). Pin selection took a few iterations to land on GPIOs that actually work — see [hardware.md](hardware.md).

Everything below this point — sensor-gated dosing, fractional/re-measure dosing, daily limits, fault-code state, power-loss recovery — was designed but **not built**. Preserved here as the intended behavior for whoever implements it next:

**Hardware watchdog**
A hardware watchdog resets the MCU if firmware hangs; pump GPIOs return to LOW on reset (this part of the fail-safe story is still open — the current implementation's LOW-on-entry default covers a normal reboot, not a hang-while-a-pump-is-on scenario).

**Sensor validation before dosing**
A dose only triggers if all of:
- Reading is within physical validity range (pH: 0–14, ORP: −2000 to +2000 mV).
- At least 3 consecutive readings agree within ±0.1 pH / ±10 mV (rejects single-sample spikes).
- Sensor hasn't timed out or returned an error.
- pH isn't already in the safe range (7.2–7.8) — no dose needed.

**Fractional dosing, never all at once**
pH is logarithmic — a dose sized to correct 8.0→7.5 will overshoot to ~4 if doubled. A large single burst also creates a local extreme-pH zone near the return jet before the water mixes. Use small repeated doses instead:
1. Inject a short dose.
2. Wait for at least one full pool circulation (minimum interval, below).
3. Re-read; dose again only if still needed.

This avoids computing a target dose upfront, since buffering capacity (total alkalinity) isn't measured directly and makes the pH response non-linear.

**Per-dose and daily limits** (worked example for a ~50,000 L pool, ~100 mL/min pump — recompute for your actual pool volume and pump flow rate)

| Parameter | Acid | Base | Rationale |
|-----------|------|------|-----------|
| Max single dose | 10 s | 10 s | ≈17 mL at 100 mL/min — enough to shift pH ~0.1–0.2 units; larger single doses risk overshoot |
| Max daily total | 300 s | 300 s | ≈500 mL/day; hitting this daily signals a chemistry problem (e.g. high total alkalinity) needing manual intervention, not more acid |
| Min interval between doses | 15 min | 15 min | One full circulation cycle, so the sensor reads homogenized water, not the local concentration near the probe |

Daily counters persist in NVS, survive power cycles, reset at midnight or via `pool/cmd/counter/reset`.

**Power failure recovery**
On boot, read the last dosing state from NVS. If a dose was in progress when power was lost, don't resume it — enter fault state `power_loss_mid_dose`, wait for `pool/cmd/fault/clear`.

**Fault codes** — published to a status topic on entry, halts all dosing until `pool/cmd/fault/clear`:

| Code | Cause |
|------|-------|
| `sensor_timeout` | Sensor didn't respond within the polling window |
| `sensor_out_of_range` | Reading outside physical validity bounds |
| `daily_limit_acid` / `daily_limit_base` | Daily dosing limit reached |
| `concurrent_dose` | A pump commanded while another was already running — currently just rejected with a log message (`dosing.rs`), not tracked as a persistent fault requiring `clear` |
| `power_loss_mid_dose` | Power failed mid-dose |
| `ph_emergency_low` / `ph_emergency_high` | pH outside 6.5–8.5 — stop all dosing of the corresponding type |

Note: `pool/status/log` (see [mqtt.md](mqtt.md)) already exists and could carry these as `ERROR:`-prefixed messages, but a dedicated machine-parseable fault-code topic (separate from the free-text log) is probably still worth keeping distinct for automation to key off of.

## Chlorine dosing via ORP setpoint

Manual chlorine dosing (`pool/cmd/dose/chlorine` ← duration) is implemented; **automatic** dosing tied to the ORP reading is not. Not a direct formula (mV → mL) — see the closed-loop control approach (on/off with hysteresis, or proportional dosing against an ORP setpoint) discussed in project notes. Would reuse the same daily-limit / fractional-dosing machinery described above for acid/base, on top of the mutual exclusion that already covers all three pumps today.

## MQTT Discovery for Home Assistant — mostly done

Implemented (`src/discovery.rs`) for all sensor and command topics except OTA — see [home-assistant.md](home-assistant.md). Left open:

- **OTA entity**: deliberately not auto-discovered (see home-assistant.md's rationale — wanted to keep triggering a flash a manual, deliberate step). Could add as a `text` entity if that's reconsidered.
- **No availability/LWT topic**: entities don't show as "unavailable" in HA when the device is offline/crashed — they just stop updating. Adding a `pool/status/availability` topic with an MQTT Last Will and testament (published `offline` on disconnect, `online` on connect) and wiring `availability_topic` into every discovery config would fix this.
- **Numbers are optimistic**: divider ratios and calibration points have no `state_topic` to read back the device's actual current value — the HA UI just echoes what was last typed. Would need the firmware to publish current divider ratios (and maybe calibration points) somewhere for numbers to reflect real device state.

## Open design question: command payload format

Early project notes (and `CLAUDE.md`, before this doc pass) described commands as full text sentences, e.g. `calibrate ORP 225` — implying a single command topic that parses a leading keyword. What's actually implemented is a **separate topic per command with a bare-value payload** (`pool/cmd/calibrate/orp` ← `225`), see [mqtt.md](mqtt.md). This mismatch caused a real `ERROR: Invalid ... payload` when a Home Assistant setup was configured against the text-command assumption.

This hasn't been decided either way. The dosing commands (`pool/cmd/dose/{acid,base,chlorine}`) were added following the existing per-topic bare-payload convention for consistency, reinforcing that precedent by default rather than resolving the question — worth deciding deliberately before adding `fault/clear` and `counter/reset`, rather than continuing to accrete topics ad hoc.

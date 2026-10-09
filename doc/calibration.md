# Sensor Calibration

Both pH and ORP use the same scheme: **two-point empirical linear calibration**, persisted to NVS so it survives reboots and OTA updates. There's no dependency on probe/board datasheet formulas or assumed supply voltages — the firmware just linearly interpolates between two `(probe voltage, known reference value)` points you provide, the same way for both sensors.

## How the two-point scheme works

- Sending a calibration command records **one point**: whatever the probe voltage was at that instant, paired with the reference value you sent.
- The two most recent points are kept in a ring buffer of size 2. A third calibration call overwrites the **older** of the two — so recalibrating never requires a reboot, and you don't need to "reset" anything between calibration sessions.
- Once both points are set, the sensor topic (`pool/sensors/ph`, `pool/sensors/orp`) publishes the interpolated value. Before that, it falls back to the raw divider-corrected probe voltage in volts, so you can tell at a glance whether calibration is active.
- **Consequence**: after the very first full calibration, the point count shown in `pool/status/log` is *always* `2/2` from then on — that's expected, not a bug. To actually see what changed on a recalibration, look at the full point list also included in that log line (see below), not just the count.

## pH

1. Immerse the probe in the first buffer solution (e.g. pH 4.00). Watch `pool/sensors/ph/raw` and wait for it to **stabilize** — give it at least one full 5s poll cycle after the reading looks steady, not just after it *looks* stable on the first sample (see [pitfall #1](#pitfall-stale-voltage-on-quick-recalibration) below).
2. Publish the buffer's value to `pool/cmd/calibrate/ph`:
   ```
   topic: pool/cmd/calibrate/ph
   payload: 4.00
   ```
3. Rinse the probe, immerse in the second buffer (e.g. pH 7.00), wait for `pool/sensors/ph/raw` to settle on a **clearly different** value.
4. Publish:
   ```
   topic: pool/cmd/calibrate/ph
   payload: 7.00
   ```
5. Confirm on `pool/status/log`:
   ```
   INFO: pH calibration point recorded: 1.6500V = pH4.00 (1/2 points set) — points: [1.6500V=pH4.00, unset]
   INFO: pH calibration point recorded: 2.1000V = pH7.00 (2/2 points set) — points: [1.6500V=pH4.00, 2.1000V=pH7.00]
   ```
   The two voltages should differ meaningfully — a probe swinging from pH 4 to pH 7 (3 units) should show a clear voltage shift, not a difference in the noise floor (a fraction of a mV).

## ORP

Same procedure, using a known ORP reference instead of a pH buffer. A convenient pair:
- **0 mV** via the conditioning board's short-to-ground button. This does **not** read as 0V on `pool/sensors/orp/raw` — the button shorts the *probe input*, not the board's output, and the conditioning circuit's own bias point still produces a nonzero board output voltage for a 0mV probe signal. That's expected; it's why calibration is needed in the first place.
- **A buffer solution** at a known ORP (e.g. 470mV).

```
topic: pool/cmd/calibrate/orp
payload: 0.0
```
(with the probe shorted, after waiting for `pool/sensors/orp/raw` to settle)

```
topic: pool/cmd/calibrate/orp
payload: 470
```
(in the buffer solution, after waiting for `pool/sensors/orp/raw` to settle on a different value)

**Don't use demineralized/deionized water as a sanity-check reference.** It has no defined ORP — with no buffering ionic species present, the reading is dominated by dissolved oxygen and drifts slowly and unpredictably (order of a few hundred mV, but not reproducible). To verify a calibration is holding, re-dip in the known buffer solution and check you get back close to its labeled value, or re-press the short-circuit button and check you get back close to 0.

## Pitfalls

### Pitfall: stale voltage on quick recalibration

`last_ph_voltage` / `last_orp_voltage` are refreshed only once per 5s polling cycle, not on demand. If you send a calibration command right after changing the physical setup (swapping buffer solutions, pressing the short-circuit button) without waiting for the next poll tick, the firmware may capture a voltage that still reflects the *previous* state.

**Symptom**: both calibration points end up with nearly identical voltages (e.g. 0.2mV apart) despite being taken at very different references. Since the calibration slope is `(ref2 - ref1) / (v2 - v1)`, a tiny voltage denominator makes the slope huge — tiny probe noise then translates into large swings in the reported value (e.g. re-shorting the ORP probe reads 81mV instead of ~0).

**Fix**: always confirm the `/raw` topic has settled on a new, stable value — and let at least one full poll cycle pass after it looks stable — before publishing the calibration command.

### Pitfall: payload format

The calibration topics take the **bare numeric value only** (`4.00`, not `calibrate pH 4.00`). See [mqtt.md](mqtt.md) for the full payload reference — this is the actual cause of `ERROR: Invalid ... calibration payload on ...` messages on `pool/status/log`, most commonly hit when publishing from a UI (Home Assistant, MQTT Explorer) that makes it easy to type the whole sentence out of habit.

### Pitfall: divider ratio changed after calibration

`pool/cmd/divider/ph` and `pool/cmd/divider/orp` change the scale of the voltage fed into calibration. If you adjust a divider ratio *after* calibrating, the stored calibration points are now in a different (stale) voltage scale than live readings — recalibrate any time you touch a divider ratio.

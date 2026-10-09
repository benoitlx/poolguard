use esp_idf_hal::gpio::{Output, PinDriver};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::mqtt::LogLevel;

/// Hard ceiling on any single dose regardless of what was requested — a
/// mistyped/runaway duration should never be able to run a pump
/// indefinitely. See doc/roadmap.md's per-dose limit rationale (this value
/// is a conservative placeholder; size it to your actual pump flow rate and
/// pool volume).
pub const MAX_DOSE_SECONDS: f32 = 10.0;

#[derive(Clone, Copy)]
pub enum Pump {
    Acid,
    Base,
    Chlorine,
}

impl Pump {
    pub fn name(self) -> &'static str {
        match self {
            Pump::Acid => "acid",
            Pump::Base => "base",
            Pump::Chlorine => "chlorine",
        }
    }
}

pub struct DoseCommand {
    pub pump: Pump,
    pub duration_secs: f32,
}

/// Turns one pump on for the requested duration, then off. Commands are
/// consumed one at a time from `dose_rx` — this single-threaded loop is
/// what makes the three pumps mutually exclusive in practice: only one pump
/// is ever on at once, and `mqtt::event_loop` additionally rejects a new
/// command outright (rather than queuing it) whenever `dosing_busy` is
/// already set — see its handling of the dose topics.
///
/// All three pumps are driven off on entry, before anything else runs, so a
/// pump can never be left on by a stale/undefined GPIO state at boot.
///
/// This implements only the baseline safety net (fail-safe-off default,
/// mutual exclusion, max-duration clamp). Sensor validation, fractional
/// dosing, daily limits, and fault-code state are *not* implemented — see
/// doc/roadmap.md.
pub fn dosing_loop(
    mut acid: PinDriver<'static, Output>,
    mut base: PinDriver<'static, Output>,
    mut chlorine: PinDriver<'static, Output>,
    dose_rx: mpsc::Receiver<DoseCommand>,
    dosing_busy: Arc<Mutex<bool>>,
    log_tx: mpsc::SyncSender<(LogLevel, String)>,
    gpio_reset_diag: String,
) {
    let acid_ok = acid.set_low().is_ok();
    let base_ok = base.set_low().is_ok();
    let chlorine_ok = chlorine.set_low().is_ok();

    // Published over MQTT, not just `log::info!`, because the dosing GPIOs
    // aren't necessarily safe to assume serial is available (see
    // doc/hardware.md's pin-selection history) — this is the only signal
    // that the thread reached this point at all and didn't die/panic on
    // startup before ever driving the pumps off. `gpio_reset_diag` reports
    // gpio_reset_pin()'s results (0 = ESP_OK) — see main.rs; remove this
    // field once GPIO14 either works or is abandoned for acid.
    let startup_msg = format!(
        "Dosing thread started — pumps set off: acid={acid_ok} base={base_ok} chlorine={chlorine_ok} — {gpio_reset_diag}"
    );
    log::info!("{startup_msg}");
    let _ = log_tx.try_send((LogLevel::Info, startup_msg));

    for cmd in dose_rx {
        let name = cmd.pump.name();
        let requested = cmd.duration_secs;
        let duration = requested.clamp(0.0, MAX_DOSE_SECONDS);

        if requested > MAX_DOSE_SECONDS {
            log::warn!("{name} dose clamped from {requested:.1}s to {duration:.1}s (max)");
            let _ = log_tx.try_send((
                LogLevel::Error,
                format!(
                    "{name} dose clamped from {requested:.1}s to {duration:.1}s (max {MAX_DOSE_SECONDS}s)"
                ),
            ));
        }

        if duration > 0.0 {
            log::info!("Dosing {name} for {duration:.1}s");
            let _ =
                log_tx.try_send((LogLevel::Info, format!("Dosing {name} for {duration:.1}s")));

            let pin = match cmd.pump {
                Pump::Acid => &mut acid,
                Pump::Base => &mut base,
                Pump::Chlorine => &mut chlorine,
            };
            let _ = pin.set_high();
            std::thread::sleep(Duration::from_secs_f32(duration));
            let _ = pin.set_low();
        }

        *dosing_busy.lock().unwrap() = false;
    }
}

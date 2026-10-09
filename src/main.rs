//! Pool monitor firmware — ethernet + MQTT + OTA, pH/pressure/ORP sensing,
//! and acid/base/chlorine dosing pump control.
//!
//! See doc/ for the full picture: doc/mqtt.md (topic reference),
//! doc/calibration.md (pH/ORP calibration), doc/hardware.md (wiring, GPIO
//! usage), doc/ota-troubleshooting.md (the ethernet/MQTT/partition-table
//! pitfalls this is built on).
//!
//! Build with credentials injected via env vars (never hardcoded/committed):
//!
//! ```bash
//! MQTT_USERNAME=pool-monitor MQTT_PASSWORD=... cargo run --release
//! ```
//!
//! Trigger OTA once it's up (see `justfile`'s `ota-*` recipes):
//!
//! ```bash
//! mosquitto_pub -h 192.168.1.142 -u <user> -P <pass> \
//!   -t pool/cmd/ota -m "http://<dev-machine-ip>:8080/ota.bin"
//! ```

mod discovery;
mod dosing;
mod ethernet;
mod mqtt;
mod ota;
mod sensors;
mod version;

use anyhow::Result;
use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use std::sync::{mpsc, Arc, Mutex};

use sensors::{DividerRatios, OrpCalibration, PhCalibration};

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let p = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;

    // I2C bits, extracted separately from the RMII pins below — `p` can't be
    // moved as a whole once individual fields have been moved out of it.
    let i2c0 = p.i2c0;
    let i2c_sda = p.pins.gpio33;
    let i2c_scl = p.pins.gpio32;

    // Dosing pump MOSFET gates. GPIO1/GPIO3 (UART0 TXD/RXD) and GPIO14
    // (JTAG MTMS) were tried here first and didn't work reliably — see
    // doc/hardware.md for the pin-selection history before changing these
    // assignments again.
    //
    // esp-idf-hal's PinDriver::output() only calls gpio_set_direction(),
    // which flips input/output-enable bits but does NOT reclaim the pin's
    // IO_MUX function from whatever it defaults to at reset (JTAG for
    // GPIO12-15, UART0 for GPIO1/GPIO3) — only the higher-level
    // gpio_config()/gpio_reset_pin() do that
    // (gpio_hal_iomux_func_sel(..., PIN_FUNC_GPIO) in ESP-IDF's gpio.c).
    // Without it, digital writes silently do nothing to the physical pin
    // even though the ESP-IDF call reports success. gpio_reset_pin() also
    // calls rtc_gpio_deinit() internally for RTC-capable pins, so this
    // supersedes an earlier standalone rtc_gpio_deinit(14) attempt. Applied
    // to all three defensively — harmless on pins that don't need it (17,
    // 5), since gpio_config() unconditionally selects PIN_FUNC_GPIO
    // regardless of whether the pin needed reclaiming.
    let acid_reset = unsafe { esp_idf_hal::sys::gpio_reset_pin(14) };
    let base_reset = unsafe { esp_idf_hal::sys::gpio_reset_pin(17) };
    let chlorine_reset = unsafe { esp_idf_hal::sys::gpio_reset_pin(5) };
    // Forwarded into dosing::dosing_loop's startup message (visible on
    // pool/status/log, since this runs before MQTT connects and serial
    // isn't reliably available — see doc/hardware.md). 0 = ESP_OK.
    let gpio_reset_diag =
        format!("gpio_reset_pin: acid(14)={acid_reset} base(17)={base_reset} chlorine(5)={chlorine_reset}");

    // Driven low immediately, right here, rather than waiting for
    // dosing::dosing_loop to do it — that thread only starts after
    // ethernet+MQTT setup below, which can take seconds. This closes that
    // gap; it doesn't cover the boot time *before* this line runs at all
    // (ROM bootloader, esp-idf startup) — a hardware pull-down on each gate
    // is the only thing that can guarantee that, see doc/hardware.md.
    let mut acid_out = PinDriver::output(p.pins.gpio14)?;
    let mut base_out = PinDriver::output(p.pins.gpio17)?;
    let mut chlorine_out = PinDriver::output(p.pins.gpio5)?;
    let _ = acid_out.set_low();
    let _ = base_out.set_low();
    let _ = chlorine_out.set_low();

    log::info!("Starting pool monitor…");
    let _eth = ethernet::start(
        p.mac,
        p.pins.gpio25,
        p.pins.gpio26,
        p.pins.gpio27,
        p.pins.gpio23,
        p.pins.gpio22,
        p.pins.gpio21,
        p.pins.gpio19,
        p.pins.gpio18,
        p.pins.gpio0,
        p.pins.gpio16,
        &sysloop,
    )?;

    log::info!("Connecting to {}…", mqtt::BROKER_URL);
    let (client, conn) = mqtt::connect()?;
    let divider_ratios = Arc::new(Mutex::new(DividerRatios::default()));
    let ph_calibration = Arc::new(Mutex::new(PhCalibration::load(nvs_partition.clone())));
    let last_ph_voltage = Arc::new(Mutex::new(0.0f32));
    let orp_calibration = Arc::new(Mutex::new(OrpCalibration::load(nvs_partition.clone())));
    let last_orp_voltage = Arc::new(Mutex::new(0.0f32));

    let version = version::load(nvs_partition.clone());
    log::info!("Firmware version (OTA count): {version}");

    let (ota_tx, ota_rx) = mpsc::sync_channel::<String>(1);
    let (connected_tx, connected_rx) = mpsc::sync_channel::<()>(1);
    let (log_tx, log_rx) = mpsc::sync_channel::<(mqtt::LogLevel, String)>(16);
    let (dose_tx, dose_rx) = mpsc::sync_channel::<dosing::DoseCommand>(4);
    let dosing_busy = Arc::new(Mutex::new(false));
    let dosing_busy_for_thread = dosing_busy.clone();
    let log_tx_for_dosing = log_tx.clone();

    {
        let dosing_busy = dosing_busy.clone();
        let divider_ratios = divider_ratios.clone();
        let ph_calibration = ph_calibration.clone();
        let last_ph_voltage = last_ph_voltage.clone();
        let orp_calibration = orp_calibration.clone();
        let last_orp_voltage = last_orp_voltage.clone();
        std::thread::Builder::new().stack_size(8192).spawn(move || {
            mqtt::event_loop(
                conn,
                ota_tx,
                connected_tx,
                log_tx,
                dose_tx,
                dosing_busy,
                divider_ratios,
                ph_calibration,
                last_ph_voltage,
                orp_calibration,
                last_orp_voltage,
            )
        })?;
    }

    {
        std::thread::Builder::new().stack_size(8192).spawn(move || {
            dosing::dosing_loop(
                acid_out,
                base_out,
                chlorine_out,
                dose_rx,
                dosing_busy_for_thread,
                log_tx_for_dosing,
                gpio_reset_diag,
            )
        })?;
    }

    {
        let client = client.clone();
        std::thread::Builder::new()
            .stack_size(8192)
            .spawn(move || mqtt::heartbeat_loop(client, connected_rx, log_rx, version))?;
    }

    {
        let client = client.clone();
        let divider_ratios = divider_ratios.clone();
        let ph_calibration = ph_calibration.clone();
        let last_ph_voltage = last_ph_voltage.clone();
        let orp_calibration = orp_calibration.clone();
        let last_orp_voltage = last_orp_voltage.clone();
        std::thread::Builder::new().stack_size(8192).spawn(move || {
            sensors::polling_loop(
                i2c0,
                i2c_sda,
                i2c_scl,
                client,
                divider_ratios,
                ph_calibration,
                last_ph_voltage,
                orp_calibration,
                last_orp_voltage,
            )
        })?;
    }

    // Main thread just waits for an OTA URL and flashes it when one arrives.
    for url in ota_rx {
        if let Err(e) = ota::perform(&url, nvs_partition.clone()) {
            log::error!("OTA failed: {e:?}");
        }
    }

    unreachable!()
}

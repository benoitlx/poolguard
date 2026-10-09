use ads1x1x::{channel, Ads1x1x, FullScaleRange, TargetAddr};
use esp_idf_hal::i2c::{I2C0, I2cConfig, I2cDriver};
use esp_idf_hal::units::FromValueType;
use esp_idf_svc::mqtt::client::QoS;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::mqtt::{MqttClient, TOPIC_LOG};

const PH_CAL_NVS_NAMESPACE: &str = "ph_cal";
const PH_CAL_NVS_KEY: &str = "points";
const ORP_CAL_NVS_NAMESPACE: &str = "orp_cal";
const ORP_CAL_NVS_KEY: &str = "points";

pub const TOPIC_PH: &str = "pool/sensors/ph";
pub const TOPIC_PRESSURE: &str = "pool/sensors/pressure";
pub const TOPIC_ORP: &str = "pool/sensors/orp";

// Raw ADC voltage (mV) at the ADS1115 pin, *before* the divider-ratio
// correction — i.e. what the ADC itself sees, independent of calibration or
// divider tuning. Useful to debug the divider/wiring itself: e.g. pressing
// the ORP board's short-to-ground button should read here as ~0mV exactly,
// since it's a hard short, not something calibration can be trusted to
// confirm (calibration only corrects the *calibrated* topics below).
pub const TOPIC_PH_RAW: &str = "pool/sensors/ph/raw";
pub const TOPIC_PRESSURE_RAW: &str = "pool/sensors/pressure/raw";
pub const TOPIC_ORP_RAW: &str = "pool/sensors/orp/raw";

// ADS1115 LSB size in volts for FullScaleRange::Within4_096V (see Table 7-1
// in the ADS1115 datasheet: LSB = FSR / 2^16 = 4.096V / 65536... but single-
// ended readings only use the positive half of the code range, so effectively
// FSR / 2^15).
const ADS1115_LSB_VOLTS: f32 = 4.096 / 32768.0;

/// Voltage-divider ratios (Vsensor / Vadc) for each analog channel, applied
/// when publishing so the reported value reflects the sensor-side voltage
/// rather than the raw post-divider ADC reading. Defaults assume a divider
/// that halves-ish the input (ratio 1.5); adjustable at runtime via
/// `pool/cmd/divider/{ph,pressure,orp}` since the actual resistor values are
/// still being tuned per channel.
#[derive(Clone, Copy)]
pub struct DividerRatios {
    pub ph: f32,
    pub pressure: f32,
    pub orp: f32,
}

impl Default for DividerRatios {
    fn default() -> Self {
        Self {
            ph: 1.5,
            pressure: 1.5,
            orp: 1.5,
        }
    }
}

/// Two-point pH calibration: `calibrate pH <value>` (payload = buffer pH,
/// e.g. "4.00" then "7.00") records a point using whatever probe voltage was
/// last measured — see `last_ph_voltage` below. Once both points are set,
/// `convert()` linearly maps probe voltage to pH; a third `add_point` call
/// overwrites the older of the two points (ring buffer of 2), so
/// recalibrating never requires a restart.
///
/// Persisted to NVS (namespace "ph_cal") on every `add_point`, so calibration
/// survives reboots/OTA updates — no need to recalibrate after every flash.
pub struct PhCalibration {
    points: [Option<(f32, f32)>; 2], // (voltage, ph)
    next: usize,
    nvs: Option<EspNvs<NvsDefault>>,
}

impl PhCalibration {
    /// Opens (or creates) the "ph_cal" NVS namespace and loads any
    /// previously saved calibration. Best-effort: if NVS can't be opened or
    /// nothing was saved yet, starts uncalibrated rather than failing boot.
    pub fn load(nvs_partition: EspDefaultNvsPartition) -> Self {
        let nvs = match EspNvs::new(nvs_partition, PH_CAL_NVS_NAMESPACE, true) {
            Ok(nvs) => Some(nvs),
            Err(e) => {
                log::warn!("pH calibration NVS open failed: {e:?}");
                None
            }
        };

        let mut cal = Self {
            points: [None, None],
            next: 0,
            nvs,
        };

        if let Some(nvs) = &cal.nvs {
            let mut buf = [0u8; 16];
            if let Ok(Some(bytes)) = nvs.get_blob(PH_CAL_NVS_KEY, &mut buf) {
                if let Ok(bytes) = <[u8; 16]>::try_from(bytes) {
                    let v1 = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
                    let ph1 = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    let v2 = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
                    let ph2 = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
                    cal.points = [Some((v1, ph1)), Some((v2, ph2))];
                    log::info!(
                        "Loaded pH calibration from NVS: {v1:.4}V=pH{ph1:.2}, {v2:.4}V=pH{ph2:.2}"
                    );
                }
            }
        }

        cal
    }

    /// Returns both calibration slots as they stand after this call — once
    /// both are set, every further call overwrites the older slot (ring
    /// buffer), so the *count* alone stops being informative after the first
    /// full calibration. Returning the actual points lets the caller show
    /// what's really stored (e.g. over MQTT) instead of just a stuck "2/2".
    pub fn add_point(&mut self, ph: f32, voltage: f32) -> [Option<(f32, f32)>; 2] {
        self.points[self.next] = Some((voltage, ph));
        self.next = (self.next + 1) % self.points.len();
        self.save();
        self.points
    }

    fn save(&self) {
        let (Some(nvs), Some((v1, ph1)), Some((v2, ph2))) =
            (&self.nvs, self.points[0], self.points[1])
        else {
            return; // no NVS handle, or not enough points yet to persist
        };

        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&v1.to_le_bytes());
        buf[4..8].copy_from_slice(&ph1.to_le_bytes());
        buf[8..12].copy_from_slice(&v2.to_le_bytes());
        buf[12..16].copy_from_slice(&ph2.to_le_bytes());
        if let Err(e) = nvs.set_blob(PH_CAL_NVS_KEY, &buf) {
            log::warn!("pH calibration NVS save failed: {e:?}");
        } else {
            log::info!("pH calibration saved to NVS");
        }
    }

    /// `None` until both calibration points have been recorded.
    pub fn convert(&self, voltage: f32) -> Option<f32> {
        let (Some((v1, ph1)), Some((v2, ph2))) = (self.points[0], self.points[1]) else {
            return None;
        };
        if (v2 - v1).abs() < f32::EPSILON {
            return None; // identical points — can't derive a slope
        }
        let slope = (ph2 - ph1) / (v2 - v1);
        Some(slope * (voltage - v1) + ph1)
    }
}

/// Two-point ORP calibration: `calibrate ORP <reference_mv>` (payload =
/// known ORP standard in mV) records a point using whatever probe voltage
/// was last measured — see `last_orp_voltage` in `polling_loop`. Sent twice
/// with two different references, e.g. `calibrate ORP 0` while the
/// conditioning board's short-to-ground button is held (true 0mV), then
/// `calibrate ORP 470` in a 470mV buffer solution. Once both points are set,
/// `convert()` linearly maps probe voltage to ORP; a third `add_point` call
/// overwrites the older of the two points (ring buffer of 2), same scheme as
/// `PhCalibration`.
///
/// This is a purely empirical fit between the board's own output voltage
/// (`voltage`, already divider-corrected in `polling_loop`) and the known
/// ORP of each reference — unlike a formula derived from the manufacturer's
/// datasheet, it doesn't need to assume the board's supply voltage or bias
/// point, since both are implicitly captured by the two calibration points.
///
/// Persisted to NVS (namespace "orp_cal") on every `add_point`, so
/// calibration survives reboots/OTA updates.
pub struct OrpCalibration {
    points: [Option<(f32, f32)>; 2], // (voltage, orp_mv)
    next: usize,
    nvs: Option<EspNvs<NvsDefault>>,
}

impl OrpCalibration {
    /// Opens (or creates) the "orp_cal" NVS namespace and loads any
    /// previously saved calibration. Best-effort: if NVS can't be opened or
    /// nothing was saved yet, starts uncalibrated rather than failing boot.
    pub fn load(nvs_partition: EspDefaultNvsPartition) -> Self {
        let nvs = match EspNvs::new(nvs_partition, ORP_CAL_NVS_NAMESPACE, true) {
            Ok(nvs) => Some(nvs),
            Err(e) => {
                log::warn!("ORP calibration NVS open failed: {e:?}");
                None
            }
        };

        let mut cal = Self {
            points: [None, None],
            next: 0,
            nvs,
        };

        if let Some(nvs) = &cal.nvs {
            let mut buf = [0u8; 16];
            if let Ok(Some(bytes)) = nvs.get_blob(ORP_CAL_NVS_KEY, &mut buf) {
                if let Ok(bytes) = <[u8; 16]>::try_from(bytes) {
                    let v1 = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
                    let orp1 = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    let v2 = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
                    let orp2 = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
                    cal.points = [Some((v1, orp1)), Some((v2, orp2))];
                    log::info!(
                        "Loaded ORP calibration from NVS: {v1:.4}V={orp1:.1}mV, {v2:.4}V={orp2:.1}mV"
                    );
                }
            }
        }

        cal
    }

    /// Returns both calibration slots as they stand after this call — once
    /// both are set, every further call overwrites the older slot (ring
    /// buffer), so the *count* alone stops being informative after the first
    /// full calibration. Returning the actual points lets the caller show
    /// what's really stored (e.g. over MQTT) instead of just a stuck "2/2".
    pub fn add_point(&mut self, orp_mv: f32, voltage: f32) -> [Option<(f32, f32)>; 2] {
        self.points[self.next] = Some((voltage, orp_mv));
        self.next = (self.next + 1) % self.points.len();
        self.save();
        self.points
    }

    fn save(&self) {
        let (Some(nvs), Some((v1, orp1)), Some((v2, orp2))) =
            (&self.nvs, self.points[0], self.points[1])
        else {
            return; // no NVS handle, or not enough points yet to persist
        };

        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&v1.to_le_bytes());
        buf[4..8].copy_from_slice(&orp1.to_le_bytes());
        buf[8..12].copy_from_slice(&v2.to_le_bytes());
        buf[12..16].copy_from_slice(&orp2.to_le_bytes());
        if let Err(e) = nvs.set_blob(ORP_CAL_NVS_KEY, &buf) {
            log::warn!("ORP calibration NVS save failed: {e:?}");
        } else {
            log::info!("ORP calibration saved to NVS");
        }
    }

    /// `None` until both calibration points have been recorded.
    pub fn convert(&self, voltage: f32) -> Option<f32> {
        let (Some((v1, orp1)), Some((v2, orp2))) = (self.points[0], self.points[1]) else {
            return None;
        };
        if (v2 - v1).abs() < f32::EPSILON {
            return None; // identical points — can't derive a slope
        }
        let slope = (orp2 - orp1) / (v2 - v1);
        Some(slope * (voltage - v1) + orp1)
    }
}

fn publish(client: &MqttClient, topic: &str, msg: &str) {
    match client
        .lock()
        .unwrap()
        .publish(topic, QoS::AtMostOnce, false, msg.as_bytes())
    {
        Ok(_) => log::info!("{topic}: {msg}"),
        Err(e) => log::warn!("{topic} publish failed: {e:?}"),
    }
}

/// pH + pressure + ORP sensors — single ADS1115 I2C ADC at 3.3V (AIN0 = pH
/// probe, AIN1 = pressure sensor, AIN2 = ORP probe). Each channel sits
/// behind its own voltage divider sized to keep the input under 3.3V — no
/// level shifter needed since the ADS1115 itself runs on the same 3.3V
/// rail as the ESP32's I2C bus. Publishes the divider-corrected voltage
/// (raw code × LSB × ratio) — still not full sensor calibration, just
/// backs out the divider so the published value is the probe-side volts.
pub fn polling_loop(
    i2c0: I2C0<'static>,
    sda: esp_idf_hal::gpio::Gpio33<'static>,
    scl: esp_idf_hal::gpio::Gpio32<'static>,
    client: MqttClient,
    divider_ratios: Arc<Mutex<DividerRatios>>,
    ph_calibration: Arc<Mutex<PhCalibration>>,
    last_ph_voltage: Arc<Mutex<f32>>,
    orp_calibration: Arc<Mutex<OrpCalibration>>,
    last_orp_voltage: Arc<Mutex<f32>>,
) {
    let i2c_config = I2cConfig::new().baudrate(100.kHz().into());
    let i2c = match I2cDriver::new(i2c0, sda, scl, &i2c_config) {
        Ok(i2c) => i2c,
        Err(e) => {
            let msg = format!("ERROR: I2C init failed: {e:?}");
            log::error!("{msg}");
            let _ = client
                .lock()
                .unwrap()
                .publish(TOPIC_LOG, QoS::AtMostOnce, false, msg.as_bytes());
            return;
        }
    };

    let mut adc = Ads1x1x::new_ads1115(i2c, TargetAddr::default());
    if let Err(e) = adc.set_full_scale_range(FullScaleRange::Within4_096V) {
        let msg = format!("ERROR: ADS1115 config failed: {e:?}");
        log::error!("{msg}");
        let _ = client
            .lock()
            .unwrap()
            .publish(TOPIC_LOG, QoS::AtMostOnce, false, msg.as_bytes());
        return;
    }

    loop {
        let ratios = *divider_ratios.lock().unwrap();

        match nb::block!(adc.read(channel::SingleA1)) {
            Ok(raw) => {
                let raw_mv = raw as f32 * ADS1115_LSB_VOLTS * 1000.0;
                publish(&client, TOPIC_PH_RAW, &format!("{raw_mv:.2}"));

                let voltage = raw as f32 * ADS1115_LSB_VOLTS * ratios.ph;
                *last_ph_voltage.lock().unwrap() = voltage;
                let ph_msg = match ph_calibration.lock().unwrap().convert(voltage) {
                    Some(ph) => format!("{ph:.2}"),
                    None => format!("{voltage:.4}"), // not calibrated yet — raw volts
                };
                publish(&client, TOPIC_PH, &ph_msg);
            }
            Err(e) => {
                log::warn!("pH ADC read failed: {e:?}");
                let msg = format!("ERR: {e:?}");
                publish(&client, TOPIC_PH_RAW, &msg);
                publish(&client, TOPIC_PH, &msg);
            }
        }

        match nb::block!(adc.read(channel::SingleA2)) {
            Ok(raw) => {
                let raw_mv = raw as f32 * ADS1115_LSB_VOLTS * 1000.0;
                publish(&client, TOPIC_PRESSURE_RAW, &format!("{raw_mv:.2}"));

                let voltage = raw as f32 * ADS1115_LSB_VOLTS * ratios.pressure;
                publish(&client, TOPIC_PRESSURE, &format!("{voltage:.4}"));
            }
            Err(e) => {
                log::warn!("Pressure ADC read failed: {e:?}");
                let msg = format!("ERR: {e:?}");
                publish(&client, TOPIC_PRESSURE_RAW, &msg);
                publish(&client, TOPIC_PRESSURE, &msg);
            }
        }

        match nb::block!(adc.read(channel::SingleA0)) {
            Ok(raw) => {
                let raw_mv = raw as f32 * ADS1115_LSB_VOLTS * 1000.0;
                publish(&client, TOPIC_ORP_RAW, &format!("{raw_mv:.2}"));

                let voltage = raw as f32 * ADS1115_LSB_VOLTS * ratios.orp;
                *last_orp_voltage.lock().unwrap() = voltage;
                let orp_msg = match orp_calibration.lock().unwrap().convert(voltage) {
                    Some(orp) => format!("{orp:.1}"),
                    None => format!("{voltage:.4}"), // not calibrated yet — raw volts
                };
                publish(&client, TOPIC_ORP, &orp_msg);
            }
            Err(e) => {
                log::warn!("ORP ADC read failed: {e:?}");
                let msg = format!("ERR: {e:?}");
                publish(&client, TOPIC_ORP_RAW, &msg);
                publish(&client, TOPIC_ORP, &msg);
            }
        }

        std::thread::sleep(Duration::from_secs(5));
    }
}

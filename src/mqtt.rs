use anyhow::Result;
use esp_idf_svc::mqtt::client::{
    EspMqttClient, EspMqttConnection, EventPayload, MqttClientConfiguration, QoS,
};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::dosing::{DoseCommand, Pump};
use crate::sensors::{DividerRatios, OrpCalibration, PhCalibration};

pub const BROKER_URL: &str = "mqtt://192.168.1.142:1883";
pub const TOPIC_HEARTBEAT: &str = "pool/status";
pub const TOPIC_VERSION: &str = "pool/status/version";
pub const TOPIC_OTA: &str = "pool/cmd/ota";
pub const TOPIC_LOG: &str = "pool/status/log";
pub const TOPIC_CMD_DIVIDER_PH: &str = "pool/cmd/divider/ph";
pub const TOPIC_CMD_DIVIDER_PRESSURE: &str = "pool/cmd/divider/pressure";
pub const TOPIC_CMD_DIVIDER_ORP: &str = "pool/cmd/divider/orp";
pub const TOPIC_CMD_CALIBRATE_PH: &str = "pool/cmd/calibrate/ph";
pub const TOPIC_CMD_CALIBRATE_ORP: &str = "pool/cmd/calibrate/orp";
pub const TOPIC_CMD_DOSE_ACID: &str = "pool/cmd/dose/acid";
pub const TOPIC_CMD_DOSE_BASE: &str = "pool/cmd/dose/base";
pub const TOPIC_CMD_DOSE_CHLORINE: &str = "pool/cmd/dose/chlorine";

const MQTT_USERNAME: &str = env!("MQTT_USERNAME", "set MQTT_USERNAME before building");
const MQTT_PASSWORD: &str = env!("MQTT_PASSWORD", "set MQTT_PASSWORD before building");

pub type MqttClient = Arc<Mutex<EspMqttClient<'static>>>;

/// Severity tag prefixed onto every message published to `TOPIC_LOG`, so
/// subscribers (dashboards, alerting) can filter without parsing text.
#[derive(Clone, Copy)]
pub enum LogLevel {
    Info,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Error => "ERROR",
        }
    }
}

/// Renders both calibration slots as "<voltage>V=<reference>" — once both
/// slots are filled, `add_point`'s point count is always 2, so this is what
/// actually shows the caller whether a recalibration landed on the right
/// slot instead of the stuck "2/2" count.
fn format_cal_points(points: [Option<(f32, f32)>; 2], fmt_ref: fn(f32) -> String) -> String {
    points
        .iter()
        .map(|p| match p {
            Some((v, r)) => format!("{v:.4}V={}", fmt_ref(*r)),
            None => "unset".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn connect() -> Result<(MqttClient, EspMqttConnection)> {
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("pool-monitor"),
        username: Some(MQTT_USERNAME),
        password: Some(MQTT_PASSWORD),
        keep_alive_interval: Some(Duration::from_secs(60)),
        task_stack: 8192, // default (0 -> Kconfig default, 6144) sends the keepalive PINGREQ; give it headroom
        ..Default::default()
    };
    let (client, conn) = EspMqttClient::new(BROKER_URL, &mqtt_config)?;
    Ok((Arc::new(Mutex::new(client)), conn))
}

/// esp-idf-svc's event channel is a blocking rendezvous: the underlying
/// ESP-IDF MQTT task stays blocked inside our callback for as long as we
/// hold the current event, until we call `conn.next()` again. So this
/// function must ONLY read `conn` and forward plain data out — calling any
/// `MqttClient` method from here (even from a different topic's handler) can
/// deadlock against that same blocked MQTT task. Updating `divider_ratios`
/// directly (just a `Mutex`, no MQTT call) is fine from here though.
pub fn event_loop(
    mut conn: EspMqttConnection,
    ota_tx: mpsc::SyncSender<String>,
    connected_tx: mpsc::SyncSender<()>,
    log_tx: mpsc::SyncSender<(LogLevel, String)>,
    dose_tx: mpsc::SyncSender<DoseCommand>,
    dosing_busy: Arc<Mutex<bool>>,
    divider_ratios: Arc<Mutex<DividerRatios>>,
    ph_calibration: Arc<Mutex<PhCalibration>>,
    last_ph_voltage: Arc<Mutex<f32>>,
    orp_calibration: Arc<Mutex<OrpCalibration>>,
    last_orp_voltage: Arc<Mutex<f32>>,
) {
    while let Ok(event) = conn.next() {
        match event.payload() {
            EventPayload::Connected(_) => {
                log::info!("MQTT connected");
                let _ = connected_tx.try_send(());
            }
            EventPayload::Received { topic, data, .. } => {
                let topic = topic.unwrap_or("");
                if topic == TOPIC_OTA {
                    if let Ok(url) = std::str::from_utf8(data) {
                        let url = url.trim().to_string();
                        log::info!("OTA command received: {url}");
                        let _ = log_tx
                            .try_send((LogLevel::Info, format!("OTA command received: {url}")));
                        let _ = ota_tx.try_send(url);
                    }
                } else if topic == TOPIC_CMD_CALIBRATE_PH {
                    match std::str::from_utf8(data)
                        .ok()
                        .and_then(|s| s.trim().parse::<f32>().ok())
                    {
                        Some(buffer_ph) => {
                            let voltage = *last_ph_voltage.lock().unwrap();
                            let points =
                                ph_calibration.lock().unwrap().add_point(buffer_ph, voltage);
                            let n = points.iter().flatten().count();
                            let points_str =
                                format_cal_points(points, |ph| format!("pH{ph:.2}"));
                            log::info!("pH calibration point added: {voltage:.4}V = pH {buffer_ph:.2}");
                            let _ = log_tx.try_send((
                                LogLevel::Info,
                                format!(
                                    "pH calibration point recorded: {voltage:.4}V = pH{buffer_ph:.2} ({n}/2 points set) — points: [{points_str}]"
                                ),
                            ));
                        }
                        None => {
                            log::warn!("Invalid calibration payload on {topic}");
                            let _ = log_tx.try_send((
                                LogLevel::Error,
                                format!("Invalid pH calibration payload on {topic}"),
                            ));
                        }
                    }
                } else if topic == TOPIC_CMD_CALIBRATE_ORP {
                    match std::str::from_utf8(data)
                        .ok()
                        .and_then(|s| s.trim().parse::<f32>().ok())
                    {
                        Some(reference_mv) => {
                            let voltage = *last_orp_voltage.lock().unwrap();
                            let points = orp_calibration
                                .lock()
                                .unwrap()
                                .add_point(reference_mv, voltage);
                            let n = points.iter().flatten().count();
                            let points_str =
                                format_cal_points(points, |mv| format!("{mv:.1}mV"));
                            log::info!(
                                "ORP calibration point added: {voltage:.4}V = {reference_mv:.1}mV"
                            );
                            let _ = log_tx.try_send((
                                LogLevel::Info,
                                format!(
                                    "ORP calibration point recorded: {voltage:.4}V = {reference_mv:.1}mV ({n}/2 points set) — points: [{points_str}]"
                                ),
                            ));
                        }
                        None => {
                            log::warn!("Invalid calibration payload on {topic}");
                            let _ = log_tx.try_send((
                                LogLevel::Error,
                                format!("Invalid ORP calibration payload on {topic}"),
                            ));
                        }
                    }
                } else if let Some(field) = match topic {
                    TOPIC_CMD_DIVIDER_PH => Some("ph"),
                    TOPIC_CMD_DIVIDER_PRESSURE => Some("pressure"),
                    TOPIC_CMD_DIVIDER_ORP => Some("orp"),
                    _ => None,
                } {
                    match std::str::from_utf8(data)
                        .ok()
                        .and_then(|s| s.trim().parse::<f32>().ok())
                    {
                        Some(ratio) => {
                            let mut ratios = divider_ratios.lock().unwrap();
                            match field {
                                "ph" => ratios.ph = ratio,
                                "pressure" => ratios.pressure = ratio,
                                "orp" => ratios.orp = ratio,
                                _ => unreachable!(),
                            }
                            log::info!("{field} divider ratio set to {ratio}");
                            let _ = log_tx.try_send((
                                LogLevel::Info,
                                format!("{field} divider ratio set to {ratio}"),
                            ));
                        }
                        None => {
                            log::warn!("Invalid divider ratio on {topic}");
                            let _ = log_tx.try_send((
                                LogLevel::Error,
                                format!("Invalid divider ratio on {topic}"),
                            ));
                        }
                    }
                } else if let Some(pump) = match topic {
                    TOPIC_CMD_DOSE_ACID => Some(Pump::Acid),
                    TOPIC_CMD_DOSE_BASE => Some(Pump::Base),
                    TOPIC_CMD_DOSE_CHLORINE => Some(Pump::Chlorine),
                    _ => None,
                } {
                    match std::str::from_utf8(data)
                        .ok()
                        .and_then(|s| s.trim().parse::<f32>().ok())
                    {
                        Some(duration_secs) => {
                            let name = pump.name();
                            let mut busy = dosing_busy.lock().unwrap();
                            if *busy {
                                log::warn!("Dose command for {name} rejected: another pump is active");
                                let _ = log_tx.try_send((
                                    LogLevel::Error,
                                    format!(
                                        "Dose command for {name} rejected: another pump is active"
                                    ),
                                ));
                            } else {
                                *busy = true;
                                drop(busy);
                                let _ = dose_tx.try_send(DoseCommand { pump, duration_secs });
                            }
                        }
                        None => {
                            log::warn!("Invalid dose payload on {topic}");
                            let _ = log_tx
                                .try_send((LogLevel::Error, format!("Invalid dose payload on {topic}")));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Subscribes (once connected), then alternates between forwarding log
/// messages from `log_rx` (fed by `event_loop`, which can't publish itself —
/// see its doc comment) onto `TOPIC_LOG` as they arrive, and publishing a
/// heartbeat every ~5s of otherwise-idle time. Never touches `conn`, so it's
/// safe for it to call blocking `client` methods.
pub fn heartbeat_loop(
    client: MqttClient,
    connected_rx: mpsc::Receiver<()>,
    log_rx: mpsc::Receiver<(LogLevel, String)>,
    version: u32,
) {
    if connected_rx.recv().is_ok() {
        for topic in [
            TOPIC_OTA,
            TOPIC_CMD_DIVIDER_PH,
            TOPIC_CMD_DIVIDER_PRESSURE,
            TOPIC_CMD_DIVIDER_ORP,
            TOPIC_CMD_CALIBRATE_PH,
            TOPIC_CMD_CALIBRATE_ORP,
            TOPIC_CMD_DOSE_ACID,
            TOPIC_CMD_DOSE_BASE,
            TOPIC_CMD_DOSE_CHLORINE,
        ] {
            match client.lock().unwrap().subscribe(topic, QoS::AtLeastOnce) {
                Ok(_) => log::info!("Subscribed to {topic}"),
                Err(e) => log::warn!("Subscribe to {topic} failed: {e:?}"),
            }
        }

        // Retained so any client subscribing later (dashboard, HA, …) gets
        // the current version immediately without waiting on the heartbeat.
        let msg = version.to_string();
        match client
            .lock()
            .unwrap()
            .publish(TOPIC_VERSION, QoS::AtLeastOnce, true, msg.as_bytes())
        {
            Ok(_) => log::info!("Published version: {msg}"),
            Err(e) => log::warn!("Version publish failed: {e:?}"),
        }

        crate::discovery::publish_all(&client, version);
    }

    let mut counter: u32 = 0;
    loop {
        match log_rx.recv_timeout(Duration::from_secs(5)) {
            Ok((level, msg)) => {
                let msg = format!("{}: {msg}", level.as_str());
                match client
                    .lock()
                    .unwrap()
                    .publish(TOPIC_LOG, QoS::AtLeastOnce, false, msg.as_bytes())
                {
                    Ok(_) => log::info!("{TOPIC_LOG}: {msg}"),
                    Err(e) => log::warn!("Log publish failed: {e:?}"),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let msg = format!("pool-monitor #{counter}");
                match client
                    .lock()
                    .unwrap()
                    .publish(TOPIC_HEARTBEAT, QoS::AtMostOnce, false, msg.as_bytes())
                {
                    Ok(_) => log::info!("Published: {msg}"),
                    Err(e) => log::warn!("Publish failed: {e:?}"),
                }
                counter = counter.wrapping_add(1);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

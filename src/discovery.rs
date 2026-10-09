use esp_idf_svc::mqtt::client::QoS;

use crate::dosing::MAX_DOSE_SECONDS;
use crate::mqtt::{
    MqttClient, TOPIC_CMD_CALIBRATE_ORP, TOPIC_CMD_CALIBRATE_PH, TOPIC_CMD_DIVIDER_ORP,
    TOPIC_CMD_DIVIDER_PH, TOPIC_CMD_DIVIDER_PRESSURE, TOPIC_CMD_DOSE_ACID, TOPIC_CMD_DOSE_BASE,
    TOPIC_CMD_DOSE_CHLORINE, TOPIC_HEARTBEAT, TOPIC_LOG, TOPIC_VERSION,
};
use crate::sensors::{
    TOPIC_ORP, TOPIC_ORP_RAW, TOPIC_PH, TOPIC_PH_RAW, TOPIC_PRESSURE, TOPIC_PRESSURE_RAW,
};

/// Home Assistant's default discovery prefix — it listens on `<prefix>/#`
/// and auto-creates entities from any retained config payload found there.
const DISCOVERY_PREFIX: &str = "homeassistant";
/// Used as both the HA "node_id" path segment and the `device.identifiers`
/// value, so every entity below groups under one device ("Pool Monitor") in
/// the HA UI instead of showing up as loose, ungrouped entities.
const DEVICE_ID: &str = "poolguard";

struct SensorConfig {
    object_id: &'static str,
    name: &'static str,
    state_topic: &'static str,
    unit: Option<&'static str>,
    device_class: Option<&'static str>,
    diagnostic: bool,
}

const SENSORS: &[SensorConfig] = &[
    SensorConfig {
        object_id: "ph",
        name: "pH",
        state_topic: TOPIC_PH,
        unit: None,
        device_class: Some("ph"),
        diagnostic: false,
    },
    SensorConfig {
        object_id: "ph_raw",
        name: "pH Raw",
        state_topic: TOPIC_PH_RAW,
        unit: Some("mV"),
        device_class: None,
        diagnostic: true,
    },
    SensorConfig {
        object_id: "orp",
        name: "ORP",
        state_topic: TOPIC_ORP,
        unit: Some("mV"),
        device_class: None,
        diagnostic: false,
    },
    SensorConfig {
        object_id: "orp_raw",
        name: "ORP Raw",
        state_topic: TOPIC_ORP_RAW,
        unit: Some("mV"),
        device_class: None,
        diagnostic: true,
    },
    SensorConfig {
        object_id: "pressure",
        name: "Pressure",
        state_topic: TOPIC_PRESSURE,
        unit: Some("V"),
        device_class: None,
        diagnostic: false,
    },
    SensorConfig {
        object_id: "pressure_raw",
        name: "Pressure Raw",
        state_topic: TOPIC_PRESSURE_RAW,
        unit: Some("mV"),
        device_class: None,
        diagnostic: true,
    },
    SensorConfig {
        object_id: "version",
        name: "Firmware Version",
        state_topic: TOPIC_VERSION,
        unit: None,
        device_class: None,
        diagnostic: true,
    },
    SensorConfig {
        object_id: "log",
        name: "Log",
        state_topic: TOPIC_LOG,
        unit: None,
        device_class: None,
        diagnostic: true,
    },
    SensorConfig {
        object_id: "heartbeat",
        name: "Heartbeat",
        state_topic: TOPIC_HEARTBEAT,
        unit: None,
        device_class: None,
        diagnostic: true,
    },
];

struct NumberConfig {
    object_id: &'static str,
    name: &'static str,
    command_topic: &'static str,
    min: f32,
    max: f32,
    step: f32,
    unit: Option<&'static str>,
}

const NUMBERS: &[NumberConfig] = &[
    NumberConfig {
        object_id: "divider_ph",
        name: "pH Divider Ratio",
        command_topic: TOPIC_CMD_DIVIDER_PH,
        min: 0.1,
        max: 10.0,
        step: 0.01,
        unit: None,
    },
    NumberConfig {
        object_id: "divider_pressure",
        name: "Pressure Divider Ratio",
        command_topic: TOPIC_CMD_DIVIDER_PRESSURE,
        min: 0.1,
        max: 10.0,
        step: 0.01,
        unit: None,
    },
    NumberConfig {
        object_id: "divider_orp",
        name: "ORP Divider Ratio",
        command_topic: TOPIC_CMD_DIVIDER_ORP,
        min: 0.1,
        max: 10.0,
        step: 0.01,
        unit: None,
    },
    NumberConfig {
        object_id: "calibrate_ph",
        name: "pH Calibration Point",
        command_topic: TOPIC_CMD_CALIBRATE_PH,
        min: 0.0,
        max: 14.0,
        step: 0.01,
        unit: None,
    },
    NumberConfig {
        object_id: "calibrate_orp",
        name: "ORP Calibration Point",
        command_topic: TOPIC_CMD_CALIBRATE_ORP,
        min: -2000.0,
        max: 2000.0,
        step: 1.0,
        unit: Some("mV"),
    },
    NumberConfig {
        object_id: "dose_acid",
        name: "Acid Dose Duration",
        command_topic: TOPIC_CMD_DOSE_ACID,
        min: 0.0,
        max: MAX_DOSE_SECONDS,
        step: 0.5,
        unit: Some("s"),
    },
    NumberConfig {
        object_id: "dose_base",
        name: "Base Dose Duration",
        command_topic: TOPIC_CMD_DOSE_BASE,
        min: 0.0,
        max: MAX_DOSE_SECONDS,
        step: 0.5,
        unit: Some("s"),
    },
    NumberConfig {
        object_id: "dose_chlorine",
        name: "Chlorine Dose Duration",
        command_topic: TOPIC_CMD_DOSE_CHLORINE,
        min: 0.0,
        max: MAX_DOSE_SECONDS,
        step: 0.5,
        unit: Some("s"),
    },
];

fn device_block(version: u32) -> String {
    format!(
        r#""device":{{"identifiers":["{DEVICE_ID}"],"name":"Pool Monitor","manufacturer":"DIY","model":"WT32-ETH01","sw_version":"{version}"}}"#
    )
}

fn publish_retained(client: &MqttClient, topic: &str, payload: &str) {
    match client
        .lock()
        .unwrap()
        .publish(topic, QoS::AtLeastOnce, true, payload.as_bytes())
    {
        Ok(_) => log::info!("Discovery published: {topic}"),
        Err(e) => log::warn!("Discovery publish failed for {topic}: {e:?}"),
    }
}

fn publish_sensor(client: &MqttClient, cfg: &SensorConfig, device: &str) {
    let mut fields = vec![
        format!(r#""name":"{}""#, cfg.name),
        format!(r#""unique_id":"{DEVICE_ID}_{}""#, cfg.object_id),
        format!(r#""state_topic":"{}""#, cfg.state_topic),
    ];
    if let Some(unit) = cfg.unit {
        fields.push(format!(r#""unit_of_measurement":"{unit}""#));
    }
    if let Some(dc) = cfg.device_class {
        fields.push(format!(r#""device_class":"{dc}""#));
    }
    if cfg.diagnostic {
        fields.push(r#""entity_category":"diagnostic""#.to_string());
    }
    fields.push(device.to_string());
    let json = format!("{{{}}}", fields.join(","));

    let topic = format!("{DISCOVERY_PREFIX}/sensor/{DEVICE_ID}/{}/config", cfg.object_id);
    publish_retained(client, &topic, &json);
}

fn publish_number(client: &MqttClient, cfg: &NumberConfig, device: &str) {
    let mut fields = vec![
        format!(r#""name":"{}""#, cfg.name),
        format!(r#""unique_id":"{DEVICE_ID}_{}""#, cfg.object_id),
        format!(r#""command_topic":"{}""#, cfg.command_topic),
        format!(r#""min":{}"#, cfg.min),
        format!(r#""max":{}"#, cfg.max),
        format!(r#""step":{}"#, cfg.step),
        r#""mode":"box""#.to_string(),
    ];
    if let Some(unit) = cfg.unit {
        fields.push(format!(r#""unit_of_measurement":"{unit}""#));
    }
    fields.push(device.to_string());
    let json = format!("{{{}}}", fields.join(","));

    let topic = format!("{DISCOVERY_PREFIX}/number/{DEVICE_ID}/{}/config", cfg.object_id);
    publish_retained(client, &topic, &json);
}

/// Publishes retained MQTT Discovery config payloads for every sensor and
/// command topic this firmware exposes, so Home Assistant creates all
/// entities automatically instead of requiring manual YAML (see
/// doc/home-assistant.md). Safe to call on every boot/reconnect — retained
/// messages with identical content are a no-op from HA's perspective.
///
/// `number` entities here are optimistic (no `state_topic`): divider ratios
/// and calibration points aren't echoed back anywhere yet, and dose
/// durations are one-shot commands with nothing to reflect back either.
pub fn publish_all(client: &MqttClient, version: u32) {
    let device = device_block(version);
    for s in SENSORS {
        publish_sensor(client, s, &device);
    }
    for n in NUMBERS {
        publish_number(client, n, &device);
    }
}

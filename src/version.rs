use anyhow::Result;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

const NVS_NAMESPACE: &str = "sys";
const NVS_KEY: &str = "ota_version";

/// Firmware version, expressed as an OTA update counter: starts at 0 on
/// first flash (nothing in NVS yet) and is incremented once per successful
/// OTA update (see `bump`, called from `ota.rs` right before reboot) — not a
/// semver/build id, just "how many OTA updates has this device received".
pub fn load(nvs_partition: EspDefaultNvsPartition) -> u32 {
    match EspNvs::<NvsDefault>::new(nvs_partition, NVS_NAMESPACE, true) {
        Ok(nvs) => nvs.get_u32(NVS_KEY).ok().flatten().unwrap_or(0),
        Err(e) => {
            log::warn!("Version NVS open failed: {e:?}");
            0
        }
    }
}

/// Increments and persists the OTA counter, returning the new value. Called
/// right before rebooting into freshly-flashed firmware, so it's the *next*
/// boot that reports the bumped version.
pub fn bump(nvs_partition: EspDefaultNvsPartition) -> Result<u32> {
    let nvs = EspNvs::<NvsDefault>::new(nvs_partition, NVS_NAMESPACE, true)?;
    let next = nvs.get_u32(NVS_KEY)?.unwrap_or(0) + 1;
    nvs.set_u32(NVS_KEY, next)?;
    Ok(next)
}

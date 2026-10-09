use anyhow::{bail, Result};
use embedded_svc::http::client::Client as HttpClient;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::ota::EspOta;

pub fn perform(url: &str, nvs_partition: EspDefaultNvsPartition) -> Result<()> {
    log::info!("Starting OTA from {url}");

    let connection = EspHttpConnection::new(&HttpConfiguration::default())?;
    let mut http = HttpClient::wrap(connection);
    let request = http.get(url)?;
    let mut response = request.submit()?;

    let status = response.status();
    if status != 200 {
        bail!("OTA fetch failed: HTTP {status}");
    }

    let mut ota = EspOta::new()?;
    let mut update = ota.initiate_update()?;

    let mut buf = [0u8; 1024];
    let mut total = 0usize;
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        update.write(&buf[..n])?;
        total += n;
    }
    log::info!("OTA download complete: {total} bytes, activating…");

    update.complete()?;

    match crate::version::bump(nvs_partition) {
        Ok(v) => log::info!("OTA applied — now v{v} — rebooting"),
        Err(e) => {
            log::warn!("Failed to bump version counter: {e:?} — rebooting anyway");
        }
    }
    esp_idf_hal::reset::restart();
}

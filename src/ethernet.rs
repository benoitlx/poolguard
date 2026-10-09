use anyhow::Result;
use esp_idf_svc::eth::{BlockingEth, EspEth, EthDriver, RmiiClockConfig, RmiiEth, RmiiEthChipset};
use esp_idf_svc::eventloop::EspSystemEventLoop;

/// WT32-ETH01 internal EMAC: RXD0/RXD1/CRS_DV/TXD0/TXD1/TX_EN are fixed
/// silicon pins (GPIO25/26/27/19/22/21). MDC/MDIO (GPIO23/18) and the PHY
/// reset (GPIO16) are board wiring — see sdkconfig.defaults.
#[allow(clippy::too_many_arguments)]
pub fn start(
    mac: esp_idf_hal::mac::MAC<'static>,
    rmii_rdx0: esp_idf_hal::gpio::Gpio25<'static>,
    rmii_rdx1: esp_idf_hal::gpio::Gpio26<'static>,
    rmii_crs_dv: esp_idf_hal::gpio::Gpio27<'static>,
    rmii_mdc: esp_idf_hal::gpio::Gpio23<'static>,
    rmii_txd1: esp_idf_hal::gpio::Gpio22<'static>,
    rmii_tx_en: esp_idf_hal::gpio::Gpio21<'static>,
    rmii_txd0: esp_idf_hal::gpio::Gpio19<'static>,
    rmii_mdio: esp_idf_hal::gpio::Gpio18<'static>,
    rmii_ref_clk: esp_idf_hal::gpio::Gpio0<'static>,
    phy_rst: esp_idf_hal::gpio::Gpio16<'static>,
    sysloop: &EspSystemEventLoop,
) -> Result<BlockingEth<EspEth<'static, RmiiEth>>> {
    let eth_driver = EthDriver::new_rmii(
        mac,
        rmii_rdx0,
        rmii_rdx1,
        rmii_crs_dv,
        rmii_mdc,
        rmii_txd1,
        rmii_tx_en,
        rmii_txd0,
        rmii_mdio,
        RmiiClockConfig::Input(rmii_ref_clk),
        Some(phy_rst),
        RmiiEthChipset::LAN87XX,
        Some(1), // PHY address
        sysloop.clone(),
    )?;
    let eth = EspEth::wrap(eth_driver)?;
    let mut eth = BlockingEth::wrap(eth, sysloop.clone())?;
    eth.start()?;
    log::info!("Waiting for ethernet link…");
    eth.wait_netif_up()?;
    log::info!("Ethernet up: {:?}", eth.eth().netif().get_ip_info()?);
    Ok(eth)
}

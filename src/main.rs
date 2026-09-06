#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources};
use embassy_time::{Duration, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    ram,
    rng::Rng,
    timer::timg::TimerGroup,
};
use esp_println as _;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    sta::StationConfig,
};

esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty, $value:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit().write($value)
    }};
}

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");
const HOSTNAME_PREFIX: &str = env!("HOSTNAME_PREFIX");

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let station_config = Config::Station(
        StationConfig::default()
            .with_ssid(SSID.try_into().unwrap())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                PASSWORD.try_into().unwrap(),
            )),
    );

    let wifi_interface = Interface::station();
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .unwrap();

    assert!(
        HOSTNAME_PREFIX.len() <= 25,
        "HOSTNAME_PREFIX must be at most 25 bytes"
    );
    let station_mac = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let station_mac_bytes = station_mac.as_bytes();
    let hostname = format!(
        "{HOSTNAME_PREFIX}-{:02x}{:02x}{:02x}",
        station_mac_bytes[3], station_mac_bytes[4], station_mac_bytes[5]
    );
    let mut dhcp_config = embassy_net::DhcpConfig::default();
    dhcp_config.hostname = Some(hostname.as_str().try_into().unwrap());
    let network_config = embassy_net::Config::dhcpv4(dhcp_config);
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        network_config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());

    stack.wait_config_up().await;
    if let Some(config) = stack.config_v4() {
        let address = config.address.address().octets();
        defmt::info!(
            "Acquired IPv4 configuration: {}.{}.{}.{}/{}",
            address[0],
            address[1],
            address[2],
            address[3],
            config.address.prefix_len()
        );
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(info) => {
                defmt::info!("Wi-Fi connected: {:?}", info);
                let reason = controller.wait_for_disconnect_async().await.ok();
                defmt::warn!("Wi-Fi disconnected: {:?}", reason);
            }
            Err(error) => defmt::warn!("Wi-Fi connection failed: {:?}", error),
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

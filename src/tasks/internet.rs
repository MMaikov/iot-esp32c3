use defmt::{info,error};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi;
use esp_radio::wifi::{ConnectionError::{self, Failed}, Interface, WifiController, sta::StationConfig};
use embassy_net::{Runner, Stack, StackResources};

pub struct Network {
    controller: WifiController<'static>,
    pub stack: Stack<'static>,
    runner: Runner<'static, Interface>
}

pub fn init_wifi(wifi_peripheral: WIFI<'static>) -> Network {
    let controller = WifiController::new(wifi_peripheral, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    let interface = esp_radio::wifi::Interface::station();

    let config = embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default());

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    const NETWORK_SOCKETS: usize = 5;

    let (stack, runner) = embassy_net::new(interface, config, 
        crate::mk_static!(
            StackResources<NETWORK_SOCKETS>, 
            StackResources::<NETWORK_SOCKETS>::new()
        ), seed);

    Network { controller, stack, runner }
}

#[embassy_executor::task]
pub async fn network_task(spawner: Spawner, mut network: Network) -> ! {
    info!("Connecting to WiFi network");
    const SSID: &str = env!("SSID");
    const PASSWORD: &str = env!("PASSWORD");

    let station_config = wifi::Config::Station(
        StationConfig::default()
            .with_ssid(SSID.try_into().unwrap())
            .with_authentication(
                esp_radio::wifi::AuthenticationMethodConfig::Wpa2Personal(
                    PASSWORD.try_into().unwrap()
                ))
    );
    
    spawner.spawn(network_runner_task(network.runner).unwrap());

    if let Err(wifi_error) = network.controller.set_config(&station_config) {
        error!("Error when trying to set configuration for the WiFi controller: {}", &wifi_error);
    }
    
    loop {
        info!("Attempting to connect to WiFi");

        match network.controller.connect_async().await {
            Ok(connected_info) => 
                info!("Connected to \"{}\" Wifi network on channel {} with {} security!", 
                    &connected_info.ssid, &connected_info.channel, &connected_info.authmode),
            Err(e) => match e {
                Failed(disconnected_info) => 
                    error!("{}", &disconnected_info.reason),
                ConnectionError::WifiError(wifi_error) => 
                    error!("Got WiFi error when connecting to the network: {}", &wifi_error),
                _ => error!("Unknown error occured when connecting to WiFi network")
            }
        }

        network.stack.wait_link_up().await;
        network.stack.wait_config_up().await;

        let ipv4_config = network.stack.config_v4().unwrap();
        info!("IP address: {}", &ipv4_config.address);
        info!("Gateway: {}", ipv4_config.gateway.unwrap());
        info!("DNS: {}", ipv4_config.dns_servers.first().unwrap());

        loop {
            Timer::after(Duration::from_secs(1)).await;
            if !network.stack.is_link_up() {
                info!("WiFi link lost!");
                break;
            }
        }
    }
}

#[embassy_executor::task]
async fn network_runner_task(mut runner: embassy_net::Runner<'static, Interface>) -> ! {
    runner.run().await;
}
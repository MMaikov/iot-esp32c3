use core::net::SocketAddr;

use defmt::{error, info, warn};
use embassy_net::{Stack, udp::{PacketMetadata, UdpSocket}};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::{Receiver, Watch}};
use embassy_time::{Duration, TimeoutError, Timer, with_timeout};
use sntpc::NtpContext;
use sntpc_time_embassy::EmbassyTimestampGenerator;
use sntpc_net_embassy::UdpSocketWrapper;

use crate::{network::dns_query, rtc};

pub const SNTP_MAX_TASKS: usize = 4;
pub static SNTP_SYNCED: Watch<CriticalSectionRawMutex, bool, SNTP_MAX_TASKS> = Watch::new_with(false);

pub type SntpReceiver<'a> = Receiver<'a, CriticalSectionRawMutex, bool, SNTP_MAX_TASKS>;

#[embassy_executor::task]
pub async fn sntp_sync_task(stack: Stack<'static>) -> ! {

    let sntp_sender = SNTP_SYNCED.sender();

    stack.wait_link_up().await;
    stack.wait_config_up().await;

    info!("Internet up, syncing time");

    let ntp_address = dns_query(stack, "pool.ntp.org").await;

    const NTP_BUFFER_SIZE: usize = 4096;
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut rx_buffer = [0; NTP_BUFFER_SIZE];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_buffer = [0; NTP_BUFFER_SIZE];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buffer, &mut tx_meta, &mut tx_buffer);

    socket.bind(123).expect("Failed to bind to NTP UDP 123");

    let socket = UdpSocketWrapper::new(socket);

    loop {
        stack.wait_link_up().await;
        stack.wait_config_up().await;
        let result = match with_timeout(Duration::from_secs(5), sntpc::get_time(
            SocketAddr::from((ntp_address, 123)), 
            &socket, 
            NtpContext::new(EmbassyTimestampGenerator::default())
        )).await {
            Ok(ntp_data) => ntp_data,
            Err(TimeoutError) => {
                warn!("Timeout when acquiring time from NTP!");
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }
        };

        match result {
            Ok(time) => {
                info!("Time synced!");
                rtc::set_epoch_secs(time.sec()).await;
                sntp_sender.send(true);
            },
            Err(e) => {
                error!("Failed to do sntp sync: {:?}", defmt::Debug2Format(&e));
            }
        }

        Timer::after(Duration::from_secs(3600)).await;
    }
}
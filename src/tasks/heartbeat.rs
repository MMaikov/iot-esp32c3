use defmt::info;
use embassy_time::{Duration, Timer};

#[embassy_executor::task]
pub async fn heartbeat_task() -> ! {
    loop {
        info!("Heartbeat");
        Timer::after(Duration::from_secs(10)).await;
    }
}
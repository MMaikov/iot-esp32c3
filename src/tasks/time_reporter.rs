use defmt::{info, warn};
use embassy_time::{Duration, Timer};
use jiff::Timestamp;

use crate::tasks::sntp::SNTP_SYNCED;


#[embassy_executor::task]
pub async fn time_reporter_task() -> ! {

    info!("Waiting for time to sync!");
    SNTP_SYNCED.receiver().expect("Exceeded receiver limits").get_and(|val| *val == true).await;

    loop {
        match crate::rtc::now_epoch_secs().await {
            Some(seconds) => {
                let ts = Timestamp::from_second(seconds as i64).expect("Failed to convert unix timestamp");
                let datetime = ts.to_zoned(jiff::tz::TimeZone::UTC);
                info!("Time is {}", datetime);
            },
            None => warn!("Couldn't get RTC time!")
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}
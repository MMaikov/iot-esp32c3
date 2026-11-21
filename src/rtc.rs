use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embedded_tls::TlsClock;
use esp_hal::rtc_cntl::Rtc;
use static_cell::StaticCell;

static RTC: StaticCell<Mutex<CriticalSectionRawMutex, Option<Rtc<'static>>>> = StaticCell::new();
static RTC_HANDLE: Mutex<CriticalSectionRawMutex, Option<&'static Mutex<CriticalSectionRawMutex, Option<Rtc<'static>>>>> =
    Mutex::new(None);

pub async fn init(rtc: Rtc<'static>) {
    let m = RTC.init(Mutex::new(Some(rtc)));
    *RTC_HANDLE.lock().await = Some(m);
}

pub async fn set_epoch_secs(epoch_secs: u64) {
    let Some(m) = *RTC_HANDLE.lock().await else {
        return;
    };
    let mut guard = m.lock().await;
    if let Some(rtc) = guard.as_mut() {
        rtc.set_current_time_us(epoch_secs.saturating_mul(1_000_000));
    }
}

pub async fn now_epoch_secs() -> Option<u64> {
    let Some(m) = *RTC_HANDLE.lock().await else {
        return None;
    };
    let guard = m.lock().await;
    guard
        .as_ref()
        .map(|rtc: &Rtc<'_>| rtc.current_time_us() / 1_000_000)
}

pub fn try_now_epoch_secs() -> Option<u64> {
    let m = {
        let guard = RTC_HANDLE.try_lock().ok()?;
        *guard.as_ref()?
    };

    let rtc_guard = m.try_lock().ok()?;
    match rtc_guard.as_ref() {
        Some(rtc) => Some(rtc.current_time_us() / 1_000_000),
        None => None,
    }
}

pub struct RtcClock;

impl TlsClock for RtcClock {
    fn now() -> Option<u64> {
        crate::rtc::try_now_epoch_secs()
    }
}
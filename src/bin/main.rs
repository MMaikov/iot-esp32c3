#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::TrngSource;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::{i2c, time};
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use iot_esp32c3::tasks;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 2.0.0
    // generator parameters: -o esp32c3 -o esp32c3-wroom-02 -o unstable-hal -o alloc -o embassy -o wifi -o defmt -o esp-backtrace -o vscode -o stable-x86_64-pc-windows-msvc

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO2
    // - GPIO8
    // - GPIO9
    // These GPIO pins are in use by some feature of the module and should not be used.
    let _gpio11 = peripherals.GPIO11;
    let _gpio12 = peripherals.GPIO12;
    let _gpio13 = peripherals.GPIO13;
    let _gpio14 = peripherals.GPIO14;
    let _gpio15 = peripherals.GPIO15;
    let _gpio16 = peripherals.GPIO16;
    let _gpio17 = peripherals.GPIO17;

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 66320);
    esp_alloc::heap_allocator!(size: 32 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let rtc = Rtc::new(peripherals.RTC_TIMER);
    iot_esp32c3::rtc::init(rtc).await;

    info!("Embassy initialized!");

    let network = tasks::internet::init_wifi(peripherals.WIFI);
    let stack = network.stack;

    let i2c = i2c::master::I2c::new(
        peripherals.I2C0, 
        i2c::master::Config::default().with_frequency(time::Rate::from_khz(100))
    ).unwrap().with_sda(peripherals.GPIO3).with_scl(peripherals.GPIO4).into_async();

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    spawner.spawn(tasks::heartbeat::heartbeat_task().unwrap());
    spawner.spawn(tasks::internet::network_task(spawner, network).unwrap());
    spawner.spawn(tasks::sntp::sntp_sync_task(stack).unwrap());
    spawner.spawn(tasks::mqtt::mqtt_task(stack).unwrap());
    spawner.spawn(tasks::scd41::scd41_task(i2c).unwrap());
    spawner.spawn(tasks::time_reporter::time_reporter_task().unwrap());

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}

use defmt::{error, info};
use embassy_time::{Delay, Duration, Timer};
use esp_hal::{Async, i2c};
use scd4x::{Scd4xAsync, types::SensorData};
use core::fmt::Write;
use crate::tasks::mqtt::{MQTT_CHANNEL, MQTT_CONNECTED, MqttMessage, MqttReceiver};

#[embassy_executor::task]
pub async fn scd41_task(i2c: i2c::master::I2c<'static, Async>) -> ! {

    info!("Waiting for MQTT");
    let mut mqtt_receiver: MqttReceiver<'_> = MQTT_CONNECTED.receiver().expect("MQTT receiver limit exceeded");
    mqtt_receiver.get_and(|val| *val == true).await;
    info!("MQTT available. Starting up SCD41");

    let mut scd41 = Scd4xAsync::new(i2c, Delay);

    scd41.stop_periodic_measurement().await.unwrap();
    Timer::after(Duration::from_millis(500)).await;
    scd41.reinit().await.unwrap();
    Timer::after(Duration::from_millis(20)).await;

    let serial = scd41.serial_number().await.unwrap();
    info!("SCD41 serial: {}", serial);

    scd41.start_periodic_measurement().await.unwrap();
    info!("Started SCD41 periodic measurement");

    loop {
        Timer::after(Duration::from_secs(5)).await;

        match scd41.data_ready_status().await {
            Ok(true) => match scd41.measurement().await {
                Ok(data) => {
                    info!("CO2: {} ppm, Temperature: {}C, Humidty: {} %RH", 
                        data.co2, data.temperature, data.humidity);
                    send_scd41_mqtt(&mut mqtt_receiver, data).await;
                }
                Err(e) => error!("SCD41 measurement error: {:?}", e)
            },
            Ok(false) => error!("Data not ready yet"),
            Err(e) => error!("SCD41 I2C error: {:?}", e)
        }
    }
}

fn string_from_f32<const N: usize>(value: f32) -> heapless::String<N> {
    let mut res = heapless::String::<N>::new();
    write!(res, "{:.2}", value).unwrap();
    res
}

async fn send_scd41_mqtt<'a>(receiver: &mut MqttReceiver<'a>, scd41: SensorData) {
    receiver.get_and(|val| *val == true).await;
    MQTT_CHANNEL.send(MqttMessage {
        topic: "scd41/co2",
        payload: heapless::String::try_from(scd41.co2).unwrap()
    }).await;
    MQTT_CHANNEL.send(MqttMessage {
        topic: "scd41/temperature",
        payload: string_from_f32(scd41.temperature)
    }).await;
    MQTT_CHANNEL.send(MqttMessage {
        topic: "scd41/humidity",
        payload: string_from_f32(scd41.humidity)
    }).await;
}
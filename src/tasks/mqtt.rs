
use defmt::{error, info};
use embassy_futures::select::{Either, select};
use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, watch::{Watch, Receiver}};
use embassy_time::{Duration, Timer};
use embedded_tls::TlsError;
use minimq::{Buffers, ConfigBuilder, PubError, Publication, Session};

use crate::{network::{WebSocket, WebSocketBuffers, WsConfig}, ws_stream::{WsError}};

pub const MQTT_PAYLOAD_SIZE: usize = 32;
pub const MQTT_MAX_TASKS: usize = 4;

pub struct MqttMessage {
    pub topic: &'static str,
    pub payload: heapless::String<MQTT_PAYLOAD_SIZE>,
}

pub static MQTT_CHANNEL: Channel<CriticalSectionRawMutex, MqttMessage, 16> = Channel::new();

pub static MQTT_CONNECTED: Watch<CriticalSectionRawMutex, bool, MQTT_MAX_TASKS> = Watch::new_with(false);

pub type MqttReceiver<'a> = Receiver<'a, CriticalSectionRawMutex, bool, MQTT_MAX_TASKS>;

#[embassy_executor::task]
pub async fn mqtt_task(stack: Stack<'static>) -> ! {

    let mqtt_sender = MQTT_CONNECTED.sender();

    loop {
        stack.wait_link_up().await;
        stack.wait_config_up().await;

        let mut ws_buffers = WebSocketBuffers::new();
        let ws_config = WsConfig::new(env!("MQTT_BROKER_HOST"));

        let ws_conn = match WebSocket::new(stack, &mut ws_buffers, ws_config).await {
            Ok(ws) => ws,
            Err(e) => {
                error!("Websocket connection failed: {}", e);
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }
        };

        let (user_name, password) = (env!("MQTT_USER"), env!("MQTT_PASSWORD").as_bytes());

        const MQTT_BUFFER_SIZE: usize = 1024;
        let rx = &mut [0u8; MQTT_BUFFER_SIZE];
        let tx = &mut [0u8; MQTT_BUFFER_SIZE];
        let mut session = Session::new(
            ConfigBuilder::new(Buffers::new(rx, tx))
                        .client_id("esp32c3-co2")
                        .unwrap()
                        .auth(user_name, password)
                        .unwrap()
                        .keepalive_interval(30)
        );

        let mut connection = match session.connect(ws_conn).await {
            Ok(conn) => conn,
            Err(e) => {
                error!("MQTT connection failed: {:?}", e);
                mqtt_sender.send(false);
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }
        };

        info!("Connected to MQTT broker.");
        mqtt_sender.send(true);

        let connection_result: Result<(), PubError<(), WsError<TlsError>>> = loop {

            match select(MQTT_CHANNEL.receiver().receive(), connection.poll())
            .await {
                Either::First(msg) => {
                    let publication = Publication::text(msg.topic, &msg.payload)
                        .qos(minimq::QoS::AtLeastOnce);
                    match connection.publish(publication).await {
                        Ok(Some(op)) => {
                            let mut publish_err: Option<PubError<(), WsError<TlsError>>> = None;
                            while connection.is_pending(&op) {
                                if let Err(e) = connection.poll().await {
                                    error!("MQTT poll error: {:?}", e);
                                    publish_err = Some(PubError::Session(e));
                                    break;
                                }

                                if connection.is_invalidated(&op) {
                                    error!("MQTT operation was invalidated!");
                                    break;
                                }

                                Timer::after(Duration::from_millis(10)).await;
                            }

                            if let Some(err) = publish_err {
                                break Err(err);
                            }
                        },
                        Ok(None) => (),
                        Err(e) => {
                            error!("Something went wrong..., {:?}", defmt::Debug2Format(&e));
                            break Err(e);
                        }
                    }
                },
                Either::Second(poll_res) => {
                    if let Err(e) = poll_res {
                        error!("Background MQTT poll error: {}", e);
                        break Err(PubError::Session(e));
                    }
                },  
            }
        };

        if let Err(_) = connection_result {
            mqtt_sender.send(false);
        }

        info!("Disconnected from MQTT broker");
        Timer::after(Duration::from_secs(5)).await;
    }
}
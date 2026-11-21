use base64::{EncodeSliceError, Engine, engine::general_purpose::STANDARD};
use defmt::{info, warn};
use embassy_net::{IpAddress, Stack, dns, tcp::{ConnectError, TcpSocket}};
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read, Write};
use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, TlsError, };
use esp_hal::{rng::{Trng, TrngError}};
use rand_core::RngCore;
use sha1::{Digest, Sha1};

use crate::{esp_cryptoprovider::EspCryptoProvider, tasks::sntp::SNTP_SYNCED, ws_stream::WsStream};

const TCP_BUFFER_SIZE: usize = 1024;
const TLS_RX_BUFFER_SIZE: usize = 16640;
const TLS_TX_BUFFER_SIZE: usize = 4096;
pub struct WebSocketBuffers {
    tcp_rx_buffer: [u8; TCP_BUFFER_SIZE],
    tcp_tx_buffer: [u8; TCP_BUFFER_SIZE],
    tls_read_buffer: [u8; TLS_RX_BUFFER_SIZE],
    tls_write_buffer: [u8; TLS_TX_BUFFER_SIZE],
}

pub struct WebSocket;

#[derive(Debug)]
#[derive(defmt::Format)]
pub enum WebSocketError {
    Tcp(ConnectError),
    Tls(TlsError),
    Trng(TrngError),
    DnsResolution(dns::Error),
    DnsNoIp,
    UpgradeRejected,
    Base64Error,
    WsUpgradeBufferExceeded,
    UnexpectedEof
}

impl From<TlsError> for WebSocketError {
    fn from(value: TlsError) -> Self {
        WebSocketError::Tls(value)
    }
}

impl From<ConnectError> for WebSocketError {
    fn from(value: ConnectError) -> Self {
        WebSocketError::Tcp(value)
    }
}

impl From<TrngError> for WebSocketError {
    fn from(value: TrngError) -> Self {
        WebSocketError::Trng(value)
    }
}

impl From<EncodeSliceError> for WebSocketError {
    fn from(_value: EncodeSliceError) -> Self {
        WebSocketError::Base64Error
    }
}

impl From<embassy_net::dns::Error> for WebSocketError {
    fn from(value: embassy_net::dns::Error) -> Self {
        WebSocketError::DnsResolution(value)
    }
}

#[derive(Clone)]
pub struct WsConfig<'a> {
    pub host: &'a str,
    pub path: &'a str,
    pub tcp_timeout: Option<Duration>,
    pub keep_alive: Option<Duration>,
    pub alpn: &'a [&'a [u8]],
    pub port: u16
}

impl<'a> WsConfig<'a> {
    pub const fn new(host: &'a str) -> Self {
        Self {
            host,
            path: "/",
            tcp_timeout: Some(Duration::from_secs(60)),
            keep_alive: Some(Duration::from_secs(15)),
            alpn: &[b"http/1.1"],
            port: 443
        }
    }

    pub const fn with_path(mut self, path: &'a str) -> Self {
        self.path = path;
        self
    }

    pub const fn with_tcp_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.tcp_timeout = timeout;
        self
    }

    pub const fn with_keep_alive(mut self, interval: Option<Duration>) -> Self {
        self.keep_alive = interval;
        self
    }

    pub const fn with_alpn(mut self, alpn: &'a [&'a [u8]]) -> Self {
        self.alpn = alpn;
        self
    }

    pub const fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

impl WebSocketBuffers {
    pub fn new() -> Self {
        WebSocketBuffers { 
            tcp_rx_buffer: [0u8; TCP_BUFFER_SIZE], 
            tcp_tx_buffer: [0u8; TCP_BUFFER_SIZE],
            tls_read_buffer: [0u8; TLS_RX_BUFFER_SIZE],
            tls_write_buffer: [0u8; TLS_TX_BUFFER_SIZE],
        }
    }
}

impl WebSocket {
    pub async fn new<'a>(stack: Stack<'static>, bufs: &'a mut WebSocketBuffers, config: WsConfig<'a>) 
        -> Result<WsStream<TlsConnection<'a, TcpSocket<'a>, Aes128GcmSha256>>, WebSocketError> 
    {
        let remote_addrs = stack.dns_query(config.host, dns::DnsQueryType::A)
            .await?;

        let Some(remote) = remote_addrs.first() else {
            return Err(WebSocketError::DnsNoIp);
        };

        info!("Resolved {} to {}", config.host, remote);

        info!("Waiting for time sync");
        SNTP_SYNCED.receiver().expect("Receiver capacity reached").get_and(|val| *val == true).await;

        let mut tcp_socket = TcpSocket::new(stack, &mut bufs.tcp_rx_buffer, &mut bufs.tcp_tx_buffer);
        tcp_socket.set_timeout(config.tcp_timeout);
        tcp_socket.set_keep_alive(config.keep_alive);

        tcp_socket.connect((*remote, config.port)).await?;

        info!("TCP connection succeeded!");

        let trng = Trng::try_new()?;
        let mut tls_conn = TlsConnection::new(
            tcp_socket, 
            &mut bufs.tls_read_buffer, 
            &mut bufs.tls_write_buffer
        );

        const ROOT_CA_DER: &[u8] = include_bytes!("../certs/isrgrootx1.der");
        const MAX_CERT_SIZE: usize = 8192;

        let tls_config = TlsConfig::new()
            .with_server_name(config.host)
            .with_alpn(config.alpn);
        let provider = EspCryptoProvider::<MAX_CERT_SIZE>::new(trng, ROOT_CA_DER);
        let context = TlsContext::new(&tls_config, provider);
        tls_conn.open(context).await?;

        info!("TLS handshake completed!");

        websocket_upgrade(config.host, config.path, &mut tls_conn).await?;

        info!("Upgraded to Websockets!");

        Ok(WsStream::new(tls_conn))
    }
}

async fn websocket_upgrade<S>(host: &str, path: &str, conn: &mut S) -> Result<(), WebSocketError>
where S: Read + Write, S::Error: Into<WebSocketError> {

    let mut nonce = [0u8; 16];
    Trng::try_new()?.fill_bytes(&mut nonce);

    let mut key_b64 = [0u8; 24];
    STANDARD
        .encode_slice(nonce, &mut key_b64)?;

    const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(&key_b64);
    hasher.update(WS_GUID);
    let sha1_digest = hasher.finalize();

    let mut expected_accept = [0u8; 28];
    STANDARD.encode_slice(sha1_digest, &mut expected_accept)?;

    conn.write_all(b"GET ").await.map_err(Into::into)?;
    conn.write_all(path.as_bytes()).await.map_err(Into::into)?;
    conn.write_all(b" HTTP/1.1\r\nHost: ").await.map_err(Into::into)?;
    conn.write_all(host.as_bytes()).await.map_err(Into::into)?;
    conn.write_all(b"\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ")
        .await.map_err(Into::into)?;
    conn.write_all(&key_b64).await.map_err(Into::into)?;
    conn.write_all(b"\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: mqtt\r\nOrigin: https://")
        .await.map_err(Into::into)?;
    conn.write_all(host.as_bytes()).await.map_err(Into::into)?;
    conn.write_all(b"\r\n\r\n").await.map_err(Into::into)?;
    conn.flush().await.map_err(Into::into)?;

    let mut resp_buf = [0u8; 1024];
    let mut pos = 0;
    loop {
        if pos >= resp_buf.len() {
            return Err(WebSocketError::WsUpgradeBufferExceeded);
        }

        let n = conn.read(&mut resp_buf[pos..pos+1]).await.map_err(Into::into)?;
        if n == 0 {
            return Err(WebSocketError::UnexpectedEof);
        }

        pos += n;

        if pos >= 4 && &resp_buf[pos - 4..pos] == b"\r\n\r\n" {
            break;
        }
    }

    let headers = &resp_buf[..pos];
    if !headers.starts_with(b"HTTP/1.1 101") {
        return Err(WebSocketError::UpgradeRejected);
    }

    const HEADER_NAME: &[u8] = b"\r\nsec-websocket-accept:";
    let valid_accept = headers
        .windows(HEADER_NAME.len())
        .enumerate()
        .any(|(i, w)| {
            if !w.eq_ignore_ascii_case(HEADER_NAME) {
                return false;
            }
            let rest = &headers[i + HEADER_NAME.len()..];
            let end = rest.iter().position(|&b| b == b'\r').unwrap_or(rest.len());
            rest[..end].trim_ascii() == &expected_accept
        });

    if !valid_accept {
        return Err(WebSocketError::UpgradeRejected);
    }

    Ok(())
}

pub async fn dns_query(stack: Stack<'static>, name: &str) -> IpAddress {
    loop {
        match stack.dns_query(name, dns::DnsQueryType::A).await {
            Ok(addrs) if !addrs.is_empty() => break *addrs.first().expect("Unreachable"),
            Ok(_) => warn!("DNS returned no IP addresses"),
            Err(e) => warn!("DNS error: {}", e),
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

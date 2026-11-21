use embedded_io_async::{Error as IoError, ErrorKind, ErrorType, Read, ReadExactError, Write};
use esp_hal::rng::Trng;

#[derive(Debug)]
#[derive(defmt::Format)]
pub enum WsError<E> {
    Io(E),
    UnexpectedEof,
}

impl<E> From<E> for WsError<E> {
    fn from(err: E) -> Self {
        WsError::Io(err)
    }
}

impl<E: IoError> IoError for WsError<E> {
    fn kind(&self) -> ErrorKind {
        match self {
            WsError::Io(e) => e.kind(),
            WsError::UnexpectedEof => ErrorKind::BrokenPipe,
        }
    }
}

impl<E> From<ReadExactError<E>> for WsError<E> {
    fn from(err: ReadExactError<E>) -> Self {
        match err {
            ReadExactError::Other(e) => WsError::Io(e),
            ReadExactError::UnexpectedEof => WsError::UnexpectedEof,
        }
    }
}

impl<E: core::fmt::Display> core::fmt::Display for WsError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WsError::Io(e) => write!(f, "IO error: {}", e),
            WsError::UnexpectedEof => write!(f, "Unexpected EOF"),
        }
    }
}

impl<E: IoError> core::error::Error for WsError<E> {}

enum ReadState {
    Header { buf: [u8; 2], pos: usize },
    ExtLen { buf: [u8; 8], len: usize, pos: usize, opcode: u8, masked: bool },
    Mask { buf: [u8; 4], pos: usize, opcode: u8, payload_len: usize },
    ControlPayload { buf: [u8; 125], len: usize, pos: usize, opcode: u8 },
    DataPayload { remaining: usize },
}

pub struct WsStream<S> {
    inner: S,
    state: ReadState,
}

impl<S> WsStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            state: ReadState::Header { buf: [0; 2], pos: 0 },
        }
    }
}

impl<S: ErrorType> ErrorType for WsStream<S> {
    type Error = WsError<S::Error>;
}

impl<S: Read + Write> Read for WsStream<S> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            match &mut self.state {
                ReadState::Header { buf: hdr, pos } => {
                    while *pos < 2 {
                        let n = self.inner.read(&mut hdr[*pos..2]).await?;
                        if n == 0 {
                            return Err(WsError::UnexpectedEof);
                        }
                        *pos += n;
                    }

                    let opcode = hdr[0] & 0x0F;
                    let masked = (hdr[1] & 0x80) != 0;
                    let len_code = hdr[1] & 0x7F;

                    self.state = match len_code {
                        126 => ReadState::ExtLen { buf: [0; 8], len: 2, pos: 0, opcode, masked },
                        127 => ReadState::ExtLen { buf: [0; 8], len: 8, pos: 0, opcode, masked },
                        n => {
                            let payload_len = n as usize;
                            if masked {
                                ReadState::Mask { buf: [0; 4], pos: 0, opcode, payload_len }
                            } else {
                                Self::next_payload_state(opcode, payload_len)
                            }
                        }
                    }
                },
                ReadState::ExtLen { buf: ext, len, pos, opcode, masked } => {
                    while *pos < *len {
                        let n = self.inner.read(&mut ext[*pos..*len]).await?;
                        if n == 0 {
                            return Err(WsError::UnexpectedEof);
                        }
                        *pos += n;
                    }

                    let payload_len = if *len == 2 {
                        u16::from_be_bytes([ext[0], ext[1]]) as usize
                    } else {
                        u64::from_be_bytes(*ext) as usize
                    };

                    let (op, is_masked) = (*opcode, *masked);
                    self.state = if is_masked {
                        ReadState::Mask { buf: [0; 4], pos: 0, opcode: op, payload_len }
                    } else {
                        Self::next_payload_state(op, payload_len)
                    }
                },
                ReadState::Mask { buf: mask, pos, opcode, payload_len } => {
                    while *pos < 4 {
                        let n = self.inner.read(&mut mask[*pos..4]).await?;
                        if n == 0 {
                            return Err(WsError::UnexpectedEof);
                        }
                        *pos += n;
                    }
                    self.state = Self::next_payload_state(*opcode, *payload_len);
                },
                ReadState::ControlPayload { buf: ctrl, len, pos, opcode } => {
                    while *pos < *len {
                        let n = self.inner.read(&mut ctrl[*pos..*len]).await?;
                        if n == 0 {
                            return Err(WsError::UnexpectedEof);
                        }
                        *pos += n;
                    }
                    let (op, payload_len) = (*opcode, *len);
                    if op == 0x08 {
                        self.state = ReadState::Header { buf: [0; 2], pos: 0 };
                        return Ok(0);
                    }
                    if op == 0x09 {
                        let mask = Trng::try_new().expect("Trng failed").random().to_ne_bytes();
                        let pong_hdr = [0x8A, 0x80 | (payload_len as u8), mask[0], mask[1], mask[2], mask[3]];
                        for i in 0..payload_len {
                            ctrl[i] ^= mask[i & 3];
                        }
                        self.inner.write_all(&pong_hdr).await?;
                        self.inner.write_all(&ctrl[..payload_len]).await?;
                        self.inner.flush().await?;
                    }
                    self.state = ReadState::Header { buf: [0; 2], pos: 0 };
                },
                ReadState::DataPayload { remaining } => {
                    if *remaining == 0 {
                        self.state = ReadState::Header { buf: [0; 2], pos: 0 };
                        continue;
                    }
                    let to_read = buf.len().min(*remaining);
                    let n = self.inner.read(&mut buf[..to_read]).await?;
                    if n == 0 {
                        return Err(WsError::UnexpectedEof);
                    }
                    *remaining -= n;
                    if *remaining == 0 {
                        self.state = ReadState::Header { buf: [0; 2], pos: 0 };
                    }
                    return Ok(n);
                }
            }
        }
    }
}

impl<S> WsStream<S> {
    fn next_payload_state(opcode: u8, payload_len: usize) -> ReadState {
        match opcode {
            0x00 | 0x01 | 0x02 => {
                if payload_len == 0 {
                    ReadState::Header { buf: [0; 2], pos: 0 }
                } else {
                    ReadState::DataPayload { remaining: payload_len }
                }
            },
            _ => ReadState::ControlPayload { 
                buf: [0; 125], 
                len: payload_len.min(125), 
                pos: 0, 
                opcode 
            },
        }
    }
}

impl<S: Write> Write for WsStream<S> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mask = Trng::try_new().expect("Trng failed").random().to_ne_bytes();
        let len = buf.len();

        if len <= 125 {
            let header = [0x82, 0x80 | (len as u8), mask[0], mask[1], mask[2], mask[3]];
            self.inner.write_all(&header).await?;
        } else if len <= u16::MAX as usize {
            let ext = (len as u16).to_be_bytes();
            let header = [0x82, 0x80 | 126, ext[0], ext[1], mask[0], mask[1], mask[2], mask[3]];
            self.inner.write_all(&header).await?;
        } else {
            let ext = (len as u64).to_be_bytes();
            let header = [
                0x82, 0x80 | 127,
                ext[0], ext[1], ext[2], ext[3], ext[4], ext[5], ext[6], ext[7],
                mask[0], mask[1], mask[2], mask[3],
            ];
            self.inner.write_all(&header).await?;
        }

        let mut masked_buf = [0u8; 128];
        let mut offset = 0;
        while offset < len {
            let chunk = (len - offset).min(masked_buf.len());
            for i in 0..chunk {
                masked_buf[i] = buf[offset + i] ^ mask[(offset + i) & 3];
            }
            self.inner.write_all(&masked_buf[..chunk]).await?;
            offset += chunk;
        }

        Ok(len)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush().await.map_err(WsError::Io)
    }
}
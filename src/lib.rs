#![no_std]

extern crate alloc;

pub mod tasks;
pub mod ws_stream;
pub mod network;
pub mod rtc;
pub mod esp_cryptoprovider;

#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}
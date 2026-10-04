//! Safe value operations shared by the Rust backend and the native runtime ABI.

use crate::bytes::Bytes;
use crate::number::DecimalValue;
use crate::render::Render;

/// The finite Decimal128 interchange representation is little-endian bits,
/// independent of the private Rust DecimalValue layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecimalBits {
    pub low: u64,
    pub high: u64,
}

impl DecimalBits {
    pub fn decode(self) -> Result<DecimalValue, &'static str> {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.low.to_le_bytes());
        bytes[8..].copy_from_slice(&self.high.to_le_bytes());
        DecimalValue::from_bytes(bytes)
    }

    pub fn encode(value: DecimalValue) -> Self {
        let bytes = value.to_bytes();
        Self {
            low: u64::from_le_bytes(bytes[..8].try_into().expect("Decimal low word")),
            high: u64::from_le_bytes(bytes[8..].try_into().expect("Decimal high word")),
        }
    }
}

pub fn text_from_utf8(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Bytes::new(bytes.to_vec())
        .into_text()
        .map(String::into_bytes)
}

pub fn int_to_text(value: i64) -> String {
    value.render()
}

pub fn float_to_text(value: f64) -> String {
    value.render()
}

pub fn decimal_to_text(value: DecimalBits) -> Result<String, &'static str> {
    value.decode().map(|value| value.render())
}

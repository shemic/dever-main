use std::fmt;
use zeroize::Zeroizing;

/// Opaque source value. Only approved runtime sinks may inspect its bytes.
#[derive(Clone)]
// The payload is consumed only when an approved crypto sink is linked.
#[cfg_attr(not(feature = "crypto"), allow(dead_code))]
pub struct Secret(pub(crate) Zeroizing<Vec<u8>>);

impl Secret {
    /// Compiler-owned input decoding and test construction; no source-level reveal.
    pub fn from_input(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("Secret([REDACTED])")
    }
}

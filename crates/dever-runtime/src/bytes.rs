#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bytes(bytes::Bytes);

impl Bytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes.into())
    }
    pub fn values(&self) -> &[u8] {
        &self.0
    }
    pub fn into_values(self) -> BytesIntoValues {
        self.0.into_iter()
    }
    pub(crate) fn from_http(bytes: bytes::Bytes) -> Self {
        Self(bytes)
    }
    pub(crate) fn into_http(self) -> bytes::Bytes {
        self.0
    }
    pub fn from_text(text: &str) -> Self {
        Self::new(text.as_bytes().to_vec())
    }
    pub fn from_string(text: String) -> Self {
        Self::new(text.into_bytes())
    }
    pub fn to_text(&self) -> Result<String, String> {
        decode_text(self.0.to_vec())
    }
    pub fn into_text(self) -> Result<String, String> {
        decode_text(self.0.into())
    }
    pub fn length(&self) -> i64 {
        self.0.len() as i64
    }
    pub fn at(&self, index: i64) -> Option<i64> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.0.get(index))
            .map(|value| i64::from(*value))
    }
    pub fn slice(&self, start: i64, end: i64) -> Result<Self, &'static str> {
        let start = usize::try_from(start).map_err(|_| "byte range is out of bounds")?;
        let end = usize::try_from(end).map_err(|_| "byte range is out of bounds")?;
        if start > end || end > self.0.len() {
            return Err("byte range is out of bounds");
        }
        Ok(Self(self.0.slice(start..end)))
    }
    pub fn concat(self, other: &Self) -> Self {
        if other.values().is_empty() {
            return self;
        }
        if self.values().is_empty() {
            return other.clone();
        }
        let mut bytes = match self.0.try_into_mut() {
            Ok(bytes) => bytes,
            Err(bytes) => {
                let mut result = bytes::BytesMut::with_capacity(bytes.len() + other.0.len());
                result.extend_from_slice(&bytes);
                result
            }
        };
        bytes.extend_from_slice(other.values());
        Self(bytes.freeze())
    }
    pub fn from_ints(values: &[i64]) -> Result<Self, &'static str> {
        values
            .iter()
            .map(|value| u8::try_from(*value).map_err(|_| "byte value must be between 0 and 255"))
            .collect::<Result<Vec<_>, _>>()
            .map(Self::new)
    }
}

pub type BytesIntoValues = bytes::buf::IntoIter<bytes::Bytes>;

fn decode_text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

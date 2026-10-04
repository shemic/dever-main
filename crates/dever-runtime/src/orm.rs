use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::number::DecimalValue;
use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Uuid(Uuid),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Related<T> {
    Unloaded,
    Loaded(Box<T>),
}

impl<T: crate::render::Render> crate::render::Render for Related<T> {
    fn render_to(&self, output: &mut String) {
        match self {
            Self::Unloaded => output.push_str("Related.Unloaded"),
            Self::Loaded(value) => {
                output.push_str("Related.Loaded(");
                value.render_to(output);
                output.push(')');
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Uuid([u8; 16]);

impl Uuid {
    pub const fn from_bytes(value: [u8; 16]) -> Self {
        Self(value)
    }

    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        if text.len() != 36
            || ![8, 13, 18, 23]
                .into_iter()
                .all(|index| text.as_bytes()[index] == b'-')
        {
            return Err(Error::invalid_data(
                "Uuid must use the canonical 8-4-4-4-12 form",
            ));
        }
        let mut output = [0; 16];
        let mut source = text.bytes().filter(|byte| *byte != b'-');
        for byte in &mut output {
            let high = hex(source.next().expect("validated Uuid length"))?;
            let low = hex(source.next().expect("validated Uuid length"))?;
            *byte = (high << 4) | low;
        }
        Ok(Self(output))
    }

    #[cfg(feature = "database")]
    pub fn new_v7() -> Result<Self, Error> {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Error::new(ErrorKind::Database, error.to_string()))?
            .as_millis();
        if millis > 0xffff_ffff_ffff {
            return Err(Error::new(
                ErrorKind::Database,
                "system time exceeds the UUIDv7 timestamp range",
            ));
        }
        let mut value = [0; 16];
        getrandom::getrandom(&mut value[6..])
            .map_err(|error| Error::new(ErrorKind::Database, error.to_string()))?;
        let timestamp = (millis as u64).to_be_bytes();
        value[..6].copy_from_slice(&timestamp[2..]);
        value[6] = (value[6] & 0x0f) | 0x70;
        value[8] = (value[8] & 0x3f) | 0x80;
        Ok(Self(value))
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                formatter.write_str("-")?;
            }
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    values: Vec<Value>,
}

type RowProducer<T> = Pin<Box<dyn Stream<Item = Result<T, Error>> + Send>>;

struct RowStreamState<T> {
    producer: Mutex<Option<RowProducer<T>>>,
    closed: Semaphore,
    worker: StdMutex<Option<crate::task::ScopedOwner>>,
    worker_fault: Option<crate::task::ScopeFault>,
    _database_lease: Option<Arc<dyn Send + Sync>>,
}

/// Internal database stream used by generated Model code. Dever exposes it as AsyncStream<T>.
pub struct RowStream<T>(Arc<RowStreamState<T>>);

impl<T> Clone for RowStream<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> fmt::Debug for RowStream<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RowStream(<resource>)")
    }
}

impl<T> RowStream<T> {
    pub(crate) fn new(producer: impl Stream<Item = Result<T, Error>> + Send + 'static) -> Self {
        Self(Arc::new(RowStreamState {
            producer: Mutex::new(Some(Box::pin(producer))),
            closed: Semaphore::new(0),
            worker: StdMutex::new(None),
            worker_fault: None,
            _database_lease: None,
        }))
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn hold_database(mut self, lease: Arc<dyn Send + Sync>) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("new database stream has one owner")
            ._database_lease = Some(lease);
        self
    }

    pub async fn pull(&self) -> Result<Option<T>, Error> {
        let mut producer = self.0.producer.lock().await;
        let next = match producer.as_mut() {
            Some(producer) => match &self.0.worker_fault {
                Some(fault) => tokio::select! {
                    biased;
                    _ = self.0.closed.acquire() => None,
                    message = fault.wait() => Some(Err(Error::new(ErrorKind::Database, message))),
                    value = producer.next() => value,
                },
                None => tokio::select! {
                    biased;
                    _ = self.0.closed.acquire() => None,
                    value = producer.next() => value,
                },
            },
            None => None,
        };
        let result = match next {
            Some(Ok(value)) => Ok(Some(value)),
            Some(Err(error)) => {
                producer.take();
                Err(error)
            }
            None => {
                producer.take();
                Ok(None)
            }
        };
        if result.as_ref().is_err() || matches!(&result, Ok(None)) {
            drop(producer);
            self.0.finish_worker();
        }
        result
    }

    pub fn close(&self) {
        self.0.closed.close();
        if let Ok(mut producer) = self.0.producer.try_lock() {
            producer.take();
        }
        self.0.stop_worker();
    }

    pub fn close_on_drop(&self) -> CloseRowStreamOnDrop<T> {
        CloseRowStreamOnDrop(self.clone())
    }
}

impl<T: Send + 'static> RowStream<T> {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn from_receiver(
        receiver: tokio::sync::mpsc::Receiver<Result<T, Error>>,
        worker: crate::task::ScopedOwner,
        worker_fault: crate::task::ScopeFault,
    ) -> Self {
        Self(Arc::new(RowStreamState {
            producer: Mutex::new(Some(Box::pin(stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|value| (value, receiver)) },
            )))),
            closed: Semaphore::new(0),
            worker: StdMutex::new(Some(worker)),
            worker_fault: Some(worker_fault),
            _database_lease: None,
        }))
    }

    pub fn map<U: Send + 'static>(
        self,
        transform: impl FnMut(T) -> Result<U, Error> + Send + 'static,
    ) -> RowStream<U> {
        let owner = self.close_on_drop();
        RowStream::new(stream::unfold(
            (owner, transform),
            |(owner, mut transform)| async move {
                owner.0.pull().await.transpose().map(|value| {
                    let value = value.and_then(&mut transform);
                    (value, (owner, transform))
                })
            },
        ))
    }
}

impl<T> RowStreamState<T> {
    fn finish_worker(&self) {
        let owner = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(owner) = owner {
            owner.finish_in_background();
        }
    }

    fn stop_worker(&self) {
        let owner = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(owner) = owner {
            owner.stop_in_background();
        }
    }
}

impl<T> Drop for RowStreamState<T> {
    fn drop(&mut self) {
        self.closed.close();
        self.producer.get_mut().take();
        let owner = self
            .worker
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(owner) = owner {
            owner.stop_in_background();
        }
    }
}

pub struct CloseRowStreamOnDrop<T>(RowStream<T>);

impl<T> Drop for CloseRowStreamOnDrop<T> {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    pub fn get(&self, index: usize) -> Result<&Value, Error> {
        self.values.get(index).ok_or_else(|| {
            Error::invalid_data(format!("database row has no column at index {index}"))
        })
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn into_values(self) -> std::vec::IntoIter<Value> {
        self.values.into_iter()
    }
}

pub fn required_row(rows: Vec<Row>, operation: &str) -> Result<Row, Error> {
    let mut rows = rows.into_iter();
    let row = rows.next().ok_or_else(|| {
        Error::invalid_data(format!("SQL operation '{operation}' returned no rows"))
    })?;
    if rows.next().is_some() {
        return Err(Error::invalid_data(format!(
            "SQL operation '{operation}' returned more than one row"
        )));
    }
    Ok(row)
}

pub fn optional_row(rows: Vec<Row>, operation: &str) -> Result<Option<Row>, Error> {
    let mut rows = rows.into_iter();
    let row = rows.next();
    if rows.next().is_some() {
        return Err(Error::invalid_data(format!(
            "SQL operation '{operation}' returned more than one row"
        )));
    }
    Ok(row)
}

pub fn bounded_rows(rows: Vec<Row>, maximum: usize) -> Result<Vec<Row>, Error> {
    if rows.len() > maximum {
        let message = if maximum == 1 {
            "database query returned more than one row".into()
        } else {
            format!("database query returned more than {maximum} rows")
        };
        Err(Error::invalid_data(message))
    } else {
        Ok(rows)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Pool,
    PoolExhausted,
    Connection,
    Timeout,
    Cancelled,
    Database,
    Constraint,
    NotFound,
    InvalidData,
    Migration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid_data(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidData, message)
    }

    pub fn not_found(model: &str) -> Self {
        Self::new(ErrorKind::NotFound, format!("{model} was not found"))
    }

    pub fn migration(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Migration, message)
    }

    pub fn cause(mut self, error: impl fmt::Display) -> Self {
        self.message.push_str("; caused by: ");
        self.message.push_str(&error.to_string());
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn keeps_connection(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Constraint
                | ErrorKind::NotFound
                | ErrorKind::InvalidData
                | ErrorKind::Migration
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub fn boolean(value: Value) -> Result<bool, Error> {
    match value {
        Value::Bool(value) => Ok(value),
        Value::Int(0) => Ok(false),
        Value::Int(1) => Ok(true),
        _ => Err(Error::invalid_data("expected a database Bool")),
    }
}

pub fn int(value: Value) -> Result<i64, Error> {
    match value {
        Value::Int(value) => Ok(value),
        _ => Err(Error::invalid_data("expected a database Int")),
    }
}

pub fn float(value: Value) -> Result<f64, Error> {
    match value {
        Value::Float(value) => Ok(value),
        Value::Int(value) => Ok(value as f64),
        _ => Err(Error::invalid_data("expected a database Float")),
    }
}

pub fn decimal(value: Value) -> Result<DecimalValue, Error> {
    match value {
        Value::Decimal(value) | Value::Text(value) => decimal_text(&value)
            .and_then(|value| DecimalValue::parse(&value).map_err(Error::invalid_data)),
        _ => Err(Error::invalid_data("expected a database Decimal")),
    }
}

pub fn decimal_text(value: &str) -> Result<String, Error> {
    if value.starts_with('D') {
        decode_decimal_storage(value).map(|value| value.to_string())
    } else {
        DecimalValue::parse(value)
            .map(|value| value.to_string())
            .map_err(Error::invalid_data)
    }
}

pub fn decimal_value(value: DecimalValue, precision: u8, scale: u8) -> Result<Value, Error> {
    decimal_storage(&value.to_string(), precision, scale).map(Value::Decimal)
}

pub fn decimal_storage(text: &str, precision: u8, scale: u8) -> Result<String, Error> {
    let (negative, mut digits) = scaled_decimal(text, precision, scale)?;
    if negative {
        for digit in &mut digits {
            *digit = b'9' - (*digit - b'0');
        }
    }
    Ok(format!(
        "D{}{}:{scale:02}",
        if negative { '0' } else { '1' },
        String::from_utf8(digits).expect("ASCII decimal digits")
    ))
}

pub fn uuid(value: Value) -> Result<Uuid, Error> {
    match value {
        Value::Uuid(value) => Ok(value),
        Value::Bytes(value) => <[u8; 16]>::try_from(value)
            .map(Uuid::from_bytes)
            .map_err(|_| Error::invalid_data("database Uuid must contain 16 bytes")),
        _ => Err(Error::invalid_data("expected a database Uuid")),
    }
}

fn scaled_decimal(text: &str, precision: u8, scale: u8) -> Result<(bool, Vec<u8>), Error> {
    let (negative, text) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exponent) = text
        .find(['e', 'E'])
        .map(|index| {
            text[index + 1..]
                .parse::<i32>()
                .map(|exponent| (&text[..index], exponent))
                .map_err(|_| Error::invalid_data("invalid Decimal exponent"))
        })
        .transpose()?
        .unwrap_or((text, 0));
    let mut point = None;
    let mut digits = Vec::with_capacity(mantissa.len() + usize::from(scale));
    for byte in mantissa.bytes() {
        if byte == b'.' && point.is_none() {
            point = Some(digits.len());
        } else if byte.is_ascii_digit() {
            digits.push(byte);
        } else {
            return Err(Error::invalid_data("invalid Decimal value"));
        }
    }
    if digits.is_empty() {
        return Err(Error::invalid_data("invalid Decimal value"));
    }
    let integer_digits = i64::try_from(point.unwrap_or(digits.len()))
        .map_err(|_| Error::invalid_data("Decimal value is too long"))?
        .checked_add(i64::from(exponent))
        .ok_or_else(|| Error::invalid_data("Decimal exponent is out of range"))?;
    let scaled_length = integer_digits
        .checked_add(i64::from(scale))
        .ok_or_else(|| Error::invalid_data("Decimal scale is out of range"))?;
    let first_nonzero = digits.iter().position(|digit| *digit != b'0');
    let Some(first_nonzero) = first_nonzero else {
        return Ok((false, vec![b'0'; usize::from(precision)]));
    };
    if scaled_length <= i64::try_from(first_nonzero).expect("index fits i64") {
        return Err(Error::invalid_data(format!(
            "Decimal has more than {scale} fractional digits"
        )));
    }
    let significant_length = scaled_length - i64::try_from(first_nonzero).expect("index fits i64");
    if significant_length > i64::from(precision) {
        return Err(Error::invalid_data(format!(
            "Decimal exceeds precision {precision} with scale {scale}"
        )));
    }
    let scaled_length = usize::try_from(scaled_length)
        .map_err(|_| Error::invalid_data("Decimal scale is out of range"))?;
    if scaled_length < digits.len() {
        if digits[scaled_length..].iter().any(|digit| *digit != b'0') {
            return Err(Error::invalid_data(format!(
                "Decimal has more than {scale} fractional digits"
            )));
        }
        digits.truncate(scaled_length);
    } else {
        digits.resize(scaled_length, b'0');
    }
    let mut magnitude = digits.split_off(first_nonzero);
    let mut padded = vec![b'0'; usize::from(precision) - magnitude.len()];
    padded.append(&mut magnitude);
    Ok((negative, padded))
}

fn decode_decimal_storage(text: &str) -> Result<DecimalValue, Error> {
    let Some(encoded) = text.strip_prefix('D') else {
        return Err(Error::invalid_data("invalid stored Decimal"));
    };
    let (sign, encoded) = encoded
        .split_at_checked(1)
        .ok_or_else(|| Error::invalid_data("invalid stored Decimal"))?;
    let (digits, scale) = encoded
        .rsplit_once(':')
        .ok_or_else(|| Error::invalid_data("invalid stored Decimal"))?;
    let scale = scale
        .parse::<usize>()
        .map_err(|_| Error::invalid_data("invalid stored Decimal scale"))?;
    if !matches!(sign, "0" | "1")
        || digits.len() < scale
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::invalid_data("invalid stored Decimal"));
    }
    let digits = if sign == "0" {
        digits
            .bytes()
            .map(|digit| char::from(b'9' - (digit - b'0')))
            .collect::<String>()
    } else {
        digits.to_owned()
    };
    let point = digits.len() - scale;
    let mut decimal = if scale == 0 {
        digits
    } else {
        format!("{}.{}", &digits[..point], &digits[point..])
    };
    if sign == "0" {
        decimal.insert(0, '-');
    }
    DecimalValue::parse(&decimal).map_err(Error::invalid_data)
}

fn hex(value: u8) -> Result<u8, Error> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(Error::invalid_data("Uuid contains a non-hexadecimal digit")),
    }
}

pub fn text(value: Value) -> Result<String, Error> {
    match value {
        Value::Text(value) => Ok(value),
        _ => Err(Error::invalid_data("expected database Text")),
    }
}

pub fn bytes(value: Value) -> Result<crate::bytes::Bytes, Error> {
    match value {
        Value::Bytes(value) => Ok(crate::bytes::Bytes::new(value)),
        _ => Err(Error::invalid_data("expected database Bytes")),
    }
}

pub fn nullable<T>(
    value: Value,
    decode: impl FnOnce(Value) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    match value {
        Value::Null => Ok(None),
        value => decode(value).map(Some),
    }
}

pub fn column(values: &mut std::vec::IntoIter<Value>, name: &str) -> Result<Value, Error> {
    values
        .next()
        .ok_or_else(|| Error::invalid_data(format!("database row is missing column '{name}'")))
}

pub fn scalar(row: Row) -> Result<Value, Error> {
    let mut values = row.into_values();
    let value = column(&mut values, "scalar")?;
    if values.next().is_some() {
        return Err(Error::invalid_data(
            "database scalar query returned more than one column",
        ));
    }
    Ok(value)
}

pub fn pagination(page: i64, size: i64, maximum: usize) -> Result<(i64, i64, i64), Error> {
    let maximum = i64::try_from(maximum)
        .map_err(|_| Error::invalid_data("database page limit is too large"))?;
    if page < 1 {
        return Err(Error::invalid_data("page must be at least 1"));
    }
    if size < 1 || size > maximum {
        return Err(Error::invalid_data(format!(
            "page size must be between 1 and {maximum}"
        )));
    }
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(size))
        .ok_or_else(|| Error::invalid_data("page offset is too large"))?;
    Ok((page, size, offset))
}

pub fn cursor_size(size: i64, maximum: usize) -> Result<i64, Error> {
    let maximum = i64::try_from(maximum)
        .map_err(|_| Error::invalid_data("database cursor limit is too large"))?;
    if size < 1 || size > maximum || size == i64::MAX {
        return Err(Error::invalid_data(format!(
            "cursor size must be between 1 and {maximum}"
        )));
    }
    Ok(size)
}

pub fn relation_limit(maximum: usize) -> Result<i64, Error> {
    i64::try_from(maximum).map_err(|_| Error::invalid_data("database relation limit is too large"))
}

pub fn stream_capacity(size: i64, maximum: usize) -> Result<usize, Error> {
    let maximum = i64::try_from(maximum)
        .map_err(|_| Error::invalid_data("database stream limit is too large"))?;
    if size < 1 || size > maximum {
        return Err(Error::invalid_data(format!(
            "stream buffer size must be between 1 and {maximum}"
        )));
    }
    usize::try_from(size).map_err(|_| Error::invalid_data("stream buffer size is too large"))
}

pub fn affected(value: usize) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::invalid_data("affected row count is too large"))
}

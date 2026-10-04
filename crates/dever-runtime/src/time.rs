use chrono::{Datelike, Timelike};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MILLIS_PER_DAY: i64 = 86_400_000;

/// A worker and its application test share this clock; advancing it never sleeps.
#[derive(Clone, Default)]
pub struct Clock(Option<std::sync::Arc<std::sync::atomic::AtomicI64>>);

impl Clock {
    pub fn for_test() -> Self {
        Self(Some(std::sync::Arc::new(
            std::sync::atomic::AtomicI64::new(0),
        )))
    }

    pub fn now(&self) -> Result<i64, crate::orm::Error> {
        match &self.0 {
            Some(clock) => Ok(clock.load(std::sync::atomic::Ordering::SeqCst)),
            None => now().map_err(crate::orm::Error::invalid_data),
        }
    }

    pub fn advance(&self, millis: i64) -> Result<(), String> {
        if millis < 0 {
            return Err("test clock cannot move backwards".into());
        }
        let clock = self
            .0
            .as_ref()
            .ok_or("production clock cannot be advanced")?;
        clock
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |current| current.checked_add(millis),
            )
            .map(|_| ())
            .map_err(|_| "test clock overflow".into())
    }
}

pub fn now() -> Result<i64, String> {
    let value = unix_millis()?;
    datetime(value)?;
    Ok(value)
}

fn datetime(value: i64) -> Result<chrono::DateTime<chrono::Utc>, String> {
    chrono::DateTime::from_timestamp_millis(value)
        .filter(|value| (0..=9999).contains(&value.year()))
        .ok_or_else(|| "DateTime is outside the four-digit UTC calendar range".into())
}

pub fn parse_datetime(text: &str) -> Result<i64, String> {
    if let Some(fraction) = text.get(19..).and_then(|tail| tail.strip_prefix('.'))
        && fraction.bytes().take_while(u8::is_ascii_digit).count() > 3
    {
        return Err("DateTime requires at most three fractional digits".into());
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|_| "invalid RFC3339 DateTime".to_owned())?;
    if parsed.nanosecond() >= 1_000_000_000 || parsed.nanosecond() % 1_000_000 != 0 {
        return Err("DateTime requires millisecond precision and no leap seconds".into());
    }
    let value = parsed.timestamp_millis();
    datetime(value)?;
    Ok(value)
}

pub fn format_datetime(value: i64) -> Result<String, String> {
    let value = datetime(value)?;
    let precision = if value.timestamp_subsec_millis() == 0 {
        chrono::SecondsFormat::Secs
    } else {
        chrono::SecondsFormat::Millis
    };
    Ok(value.to_rfc3339_opts(precision, true))
}

pub fn parse_date(text: &str) -> Result<i64, String> {
    if text.len() != 10
        || text.as_bytes()[4] != b'-'
        || text.as_bytes()[7] != b'-'
        || !text
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return Err("Date must use YYYY-MM-DD".into());
    }
    let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| "invalid calendar Date".to_owned())?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp_millis())
}

pub fn format_date(value: i64) -> Result<String, String> {
    if value.rem_euclid(MILLIS_PER_DAY) != 0 {
        return Err("Date must be UTC midnight milliseconds".into());
    }
    Ok(datetime(value)?.format("%Y-%m-%d").to_string())
}

pub fn parse_time(text: &str) -> Result<i64, String> {
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 8 | 12)
        || bytes[2] != b':'
        || bytes[5] != b':'
        || (bytes.len() == 12 && bytes[8] != b'.')
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 2 | 5 | 8) || byte.is_ascii_digit())
    {
        return Err("Time must use HH:MM:SS[.sss]".into());
    }
    let time = chrono::NaiveTime::parse_from_str(text, "%H:%M:%S%.f")
        .map_err(|_| "invalid clock Time".to_owned())?;
    if time.nanosecond() >= 1_000_000_000 {
        return Err("Time does not accept leap seconds".into());
    }
    Ok(i64::from(time.num_seconds_from_midnight()) * 1000
        + i64::from(time.nanosecond() / 1_000_000))
}

pub fn format_time(value: i64) -> Result<String, String> {
    if !(0..MILLIS_PER_DAY).contains(&value) {
        return Err("Time must be milliseconds within one day".into());
    }
    let time = chrono::NaiveTime::from_num_seconds_from_midnight_opt(
        (value / 1000) as u32,
        (value % 1000) as u32 * 1_000_000,
    )
    .expect("validated time of day");
    Ok(time
        .format(if value % 1000 == 0 {
            "%H:%M:%S"
        } else {
            "%H:%M:%S%.3f"
        })
        .to_string())
}

pub fn duration(millis: i64) -> i64 {
    millis
}

pub fn add(value: i64, duration: i64) -> Result<i64, String> {
    value
        .checked_add(duration)
        .ok_or_else(|| "DateTime addition overflow".into())
}

pub fn subtract(value: i64, duration: i64) -> Result<i64, String> {
    value
        .checked_sub(duration)
        .ok_or_else(|| "DateTime subtraction overflow".into())
}

pub fn difference(later: i64, earlier: i64) -> Result<i64, String> {
    later
        .checked_sub(earlier)
        .ok_or_else(|| "Duration difference overflow".into())
}

pub fn unix_millis() -> Result<i64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| "clock value is out of Int range".into())
}

pub fn monotonic_nanos() -> Result<i64, String> {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    i64::try_from(ORIGIN.get_or_init(Instant::now).elapsed().as_nanos())
        .map_err(|_| "clock value is out of Int range".into())
}

pub fn sleep(millis: i64) -> Result<(), String> {
    let millis = u64::try_from(millis).map_err(|_| "sleep duration must not be negative")?;
    std::thread::sleep(Duration::from_millis(millis));
    Ok(())
}

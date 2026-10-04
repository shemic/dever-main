//! Bounded, timezone-independent five-field UTC schedules shared with the compiler.
use chrono::{Datelike, Timelike};

#[derive(Clone, Debug)]
pub struct Cron {
    fields: [u64; 5],
    day_any: bool,
    weekday_any: bool,
}

impl Cron {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 512 {
            return Err("cron exceeds 512 bytes".into());
        }
        let parts = text.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err("cron requires five UTC fields".into());
        }
        let mut fields = [0; 5];
        for (index, (min, max)) in [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)]
            .into_iter()
            .enumerate()
        {
            fields[index] = field(parts[index], min, max)?;
        }
        let day_any = fields[2] == ((1u64 << 32) - 2);
        let weekday_any = fields[4] == 127;
        if weekday_any && !day_any {
            let possible = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                .into_iter()
                .enumerate()
                .any(|(month, days)| {
                    fields[3] & (1 << (month + 1)) != 0
                        && fields[2] & ((1u64 << (days + 1)) - 2) != 0
                });
            if !possible {
                return Err("cron has no possible UTC calendar date".into());
            }
        }
        Ok(Self {
            fields,
            day_any,
            weekday_any,
        })
    }

    pub fn fingerprint(&self) -> String {
        format!(
            "cron-v1:{:?}:{}:{}",
            self.fields, self.day_any, self.weekday_any
        )
    }

    pub fn matches_minute(&self, minute: i64) -> bool {
        let Some(seconds) = minute.checked_mul(60) else {
            return false;
        };
        let Some(date) = chrono::DateTime::from_timestamp(seconds, 0) else {
            return false;
        };
        let matches = |index: usize, value: u32| self.fields[index] & (1u64 << value) != 0;
        let day = matches(2, date.day());
        let weekday = matches(4, date.weekday().num_days_from_sunday());
        let calendar = match (self.day_any, self.weekday_any) {
            (false, false) => day || weekday,
            (true, false) => weekday,
            (false, true) => day,
            (true, true) => true,
        };
        matches(0, date.minute()) && matches(1, date.hour()) && matches(3, date.month()) && calendar
    }
}

fn field(text: &str, minimum: u32, maximum: u32) -> Result<u64, String> {
    let mut mask = 0;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, number(step, 1, maximum + 1)?),
            None => (part, 1),
        };
        let (first, last) = if range == "*" {
            (minimum, maximum)
        } else if let Some((first, last)) = range.split_once('-') {
            (
                number(first, minimum, maximum)?,
                number(last, minimum, maximum)?,
            )
        } else {
            let value = number(range, minimum, maximum)?;
            (value, if part.contains('/') { maximum } else { value })
        };
        if first > last {
            return Err("cron range is reversed".into());
        }
        for value in (first..=last).step_by(step as usize) {
            mask |= 1u64 << value;
        }
    }
    if mask == 0 {
        return Err("empty cron field".into());
    }
    Ok(mask)
}

fn number(text: &str, minimum: u32, maximum: u32) -> Result<u32, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("cron fields require unsigned decimal numbers".into());
    }
    text.parse::<u32>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| "cron field is outside its allowed range".into())
}

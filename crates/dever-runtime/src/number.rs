use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;

use dec::{Context, Decimal128, Rounding};

thread_local! {
    static CONTEXT: RefCell<Context<Decimal128>> = RefCell::new(Context::default());
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecimalValue(Decimal128);

impl Eq for DecimalValue {}

impl PartialOrd for DecimalValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DecimalValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .partial_cmp(&other.0)
            .expect("Decimal values are finite")
    }
}

impl fmt::Display for DecimalValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reduced = Context::<Decimal128>::default().reduce(self.0);
        if reduced.is_zero() {
            formatter.write_str("0")
        } else {
            write!(formatter, "{reduced}")
        }
    }
}

impl DecimalValue {
    pub const ZERO: Self = Self(Decimal128::ZERO);

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        CONTEXT.with_borrow_mut(|context| {
            context.clear_status();
            let value = context.parse(text).map_err(|_| "invalid Decimal literal")?;
            let value = finite_result(context, value)?;
            if context.status().inexact() {
                return Err("Decimal literal is not exactly representable in 34 digits");
            }
            Ok(value)
        })
    }

    pub fn from_int(value: i64) -> Self {
        Self(Decimal128::from(value))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Result<Self, &'static str> {
        let value = Decimal128::from_le_bytes(bytes);
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err("Decimal must be finite")
        }
    }

    pub fn to_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    pub fn checked_add(self, other: Self) -> Result<Self, &'static str> {
        operation(|context| context.add(self.0, other.0))
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, &'static str> {
        operation(|context| context.sub(self.0, other.0))
    }

    pub fn checked_mul(self, other: Self) -> Result<Self, &'static str> {
        operation(|context| context.mul(self.0, other.0))
    }

    pub fn checked_div(self, other: Self) -> Result<Self, &'static str> {
        operation(|context| context.div(self.0, other.0))
    }

    pub fn checked_neg(self) -> Result<Self, &'static str> {
        operation(|context| context.minus(self.0))
    }

    pub fn round(self, places: i64) -> Result<Self, &'static str> {
        let exponent = places
            .checked_neg()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or("Decimal rounding precision is out of range")?;
        if !(-6176..=6111).contains(&exponent) {
            return Err("Decimal rounding precision is out of range");
        }
        operation(|context| {
            let mut quantum = Decimal128::ONE;
            context.set_exponent(&mut quantum, exponent);
            context.quantize(self.0, quantum)
        })
    }

    pub fn next_up(self) -> Option<Self> {
        let value = Context::<Decimal128>::default().next_plus(self.0);
        value.is_finite().then_some(Self(value))
    }

    pub fn next_down(self) -> Option<Self> {
        let value = Context::<Decimal128>::default().next_minus(self.0);
        value.is_finite().then_some(Self(value))
    }

    pub fn minimum() -> Self {
        Self::parse("-9.999999999999999999999999999999999E+6144").expect("Decimal128 minimum")
    }

    pub fn maximum() -> Self {
        Self::parse("9.999999999999999999999999999999999E+6144").expect("Decimal128 maximum")
    }
}

fn operation(
    apply: impl FnOnce(&mut Context<Decimal128>) -> Decimal128,
) -> Result<DecimalValue, &'static str> {
    CONTEXT.with_borrow_mut(|context| {
        context.clear_status();
        context.set_rounding(Rounding::HalfEven);
        let value = apply(context);
        finite_result(context, value)
    })
}

fn finite_result(
    context: &Context<Decimal128>,
    value: Decimal128,
) -> Result<DecimalValue, &'static str> {
    let status = context.status();
    if status.division_by_zero() || status.division_undefined() {
        Err("Decimal division by zero")
    } else if status.overflow() {
        Err("Decimal overflow")
    } else if status.underflow() {
        Err("Decimal underflow")
    } else if status.invalid_operation() || status.invalid_context() || !value.is_finite() {
        Err("invalid Decimal operation")
    } else {
        Ok(DecimalValue(value))
    }
}

pub fn int_add(left: i64, right: i64) -> Result<i64, &'static str> {
    left.checked_add(right).ok_or("Int overflow")
}

pub fn parse_int(text: &str) -> Option<i64> {
    text.parse().ok()
}

pub fn parse_decimal(text: &str) -> Option<DecimalValue> {
    decimal_text(text)
        .then(|| DecimalValue::parse(text).ok())
        .flatten()
}

pub fn parse_float(text: &str) -> Option<f64> {
    if !decimal_text(text) && !matches!(text, "NaN" | "inf" | "-inf") {
        return None;
    }
    text.parse().ok()
}

fn decimal_text(text: &str) -> bool {
    text.bytes().any(|byte| byte.is_ascii_digit())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.' | b'e' | b'E'))
}

pub fn int_sub(left: i64, right: i64) -> Result<i64, &'static str> {
    left.checked_sub(right).ok_or("Int overflow")
}

pub fn int_mul(left: i64, right: i64) -> Result<i64, &'static str> {
    left.checked_mul(right).ok_or("Int overflow")
}

pub fn int_neg(value: i64) -> Result<i64, &'static str> {
    value.checked_neg().ok_or("Int overflow")
}

pub fn int_div(left: i64, right: i64) -> Result<i64, &'static str> {
    if right == 0 {
        return Err("Int division by zero");
    }
    left.checked_div(right).ok_or("Int overflow")
}

pub fn int_rem(left: i64, right: i64) -> Result<i64, &'static str> {
    if right == 0 {
        return Err("Int division by zero");
    }
    left.checked_rem(right).ok_or("Int overflow")
}

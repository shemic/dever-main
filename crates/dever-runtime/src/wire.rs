//! Strict wire boundary. Parsed nodes never escape into generated business signatures.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::Engine as _;
use serde::Deserializer;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
pub const MAX_ELEMENTS: usize = 65_536;

pub struct Node<'a> {
    raw: &'a str,
    kind: Kind<'a>,
}

enum Kind<'a> {
    Null,
    Bool(bool),
    Number,
    Text(String),
    List(Vec<Node<'a>>),
    Object(BTreeMap<String, Node<'a>>),
}

pub fn parse(text: &str) -> Result<Node<'_>, String> {
    if text.len() > MAX_BYTES {
        return Err("wire JSON exceeds byte limit".into());
    }
    let raw: &RawValue = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let mut remaining = MAX_ELEMENTS;
    parse_node(raw, 0, &mut remaining)
}

fn parse_node<'a>(
    raw: &'a RawValue,
    depth: usize,
    remaining: &mut usize,
) -> Result<Node<'a>, String> {
    if depth > MAX_DEPTH {
        return Err("wire JSON exceeds depth limit".into());
    }
    *remaining = remaining
        .checked_sub(1)
        .ok_or("wire JSON exceeds element limit")?;
    let text = raw.get();
    let kind = match text.as_bytes()[0] {
        b'n' => Kind::Null,
        b't' => Kind::Bool(true),
        b'f' => Kind::Bool(false),
        b'"' => Kind::Text(serde_json::from_str(text).map_err(|error| error.to_string())?),
        b'[' | b'{' => {
            if depth == MAX_DEPTH {
                return Err("wire JSON exceeds depth limit".into());
            }
            let mut decoder = serde_json::Deserializer::from_str(text);
            let kind = decoder
                .deserialize_any(Container { depth, remaining })
                .map_err(|error| error.to_string())?;
            decoder.end().map_err(|error| error.to_string())?;
            kind
        }
        _ => Kind::Number,
    };
    Ok(Node { raw: text, kind })
}

struct Container<'b> {
    depth: usize,
    remaining: &'b mut usize,
}

impl<'de> Visitor<'de> for Container<'_> {
    type Value = Kind<'de>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded JSON container")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(raw) = sequence.next_element::<&'de RawValue>()? {
            values
                .push(parse_node(raw, self.depth + 1, self.remaining).map_err(de::Error::custom)?);
        }
        Ok(Kind::List(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON field"));
            }
            let raw = object.next_value::<&'de RawValue>()?;
            values.insert(
                key,
                parse_node(raw, self.depth + 1, self.remaining).map_err(de::Error::custom)?,
            );
        }
        Ok(Kind::Object(values))
    }
}

impl<'a> Node<'a> {
    pub fn bytes(&self) -> Result<crate::bytes::Bytes, String> {
        base64::engine::general_purpose::STANDARD
            .decode(self.text()?)
            .map(crate::bytes::Bytes::new)
            .map_err(|_| "invalid wire Bytes base64".into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self.kind, Kind::Null)
    }
    pub fn raw(&self) -> &'a str {
        self.raw
    }
    pub fn text(&self) -> Result<&str, String> {
        match &self.kind {
            Kind::Text(value) => Ok(value),
            _ => Err("expected wire string".into()),
        }
    }
    pub fn boolean(&self) -> Result<bool, String> {
        match self.kind {
            Kind::Bool(value) => Ok(value),
            _ => Err("expected wire Bool".into()),
        }
    }
    pub fn int(&self) -> Result<i64, String> {
        if !matches!(self.kind, Kind::Number) {
            return Err("expected wire integer".into());
        }
        self.raw
            .parse()
            .map_err(|_| "wire integer is not an exact i64".into())
    }
    pub fn float(&self) -> Result<f64, String> {
        if !matches!(self.kind, Kind::Number) {
            return Err("expected wire Float".into());
        }
        let value: f64 = self.raw.parse().map_err(|_| "invalid wire Float")?;
        if !value.is_finite() {
            return Err("non-finite wire Float".into());
        }
        Ok(value)
    }
    pub fn list(&self) -> Result<&[Node<'a>], String> {
        match &self.kind {
            Kind::List(values) => Ok(values),
            _ => Err("expected wire List".into()),
        }
    }
    pub fn fields(&self, names: &[&str]) -> Result<&BTreeMap<String, Node<'a>>, String> {
        let fields = self.object()?;
        if fields.keys().any(|name| !names.contains(&name.as_str())) {
            return Err("unknown wire record field".into());
        }
        Ok(fields)
    }
    pub fn object(&self) -> Result<&BTreeMap<String, Node<'a>>, String> {
        match &self.kind {
            Kind::Object(fields) => Ok(fields),
            _ => Err("expected wire record".into()),
        }
    }
}

/// Only validated JSON can be placed in an API envelope without re-encoding its numbers.
pub struct Encoded(String);
impl Encoded {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn null() -> Self {
        Self("null".into())
    }
}

enum Frame {
    Array(usize),
    Object {
        names: BTreeSet<String>,
        awaiting_value: bool,
    },
}

#[derive(Default)]
pub struct Encoder {
    output: String,
    stack: Vec<Frame>,
    elements: usize,
    failed: bool,
}

impl Encoder {
    pub fn bytes(&mut self, value: &crate::bytes::Bytes) -> Result<(), String> {
        let length = base64::encoded_len(value.values().len(), true)
            .and_then(|length| length.checked_add(2))
            .ok_or("wire JSON exceeds byte limit")?;
        if length > MAX_BYTES.saturating_sub(self.output.len()) {
            return Err("wire JSON exceeds byte limit".into());
        }
        self.text(&base64::engine::general_purpose::STANDARD.encode(value.values()))
    }

    fn append(&mut self, text: &str) -> Result<(), String> {
        if text.len() > MAX_BYTES.saturating_sub(self.output.len()) {
            // Frame bookkeeping may already have advanced before an append fails.
            // Such a partial write must never become a validated Encoded value.
            self.failed = true;
            return Err("wire JSON exceeds byte limit".into());
        }
        self.output.push_str(text);
        Ok(())
    }
    fn value(&mut self) -> Result<(), String> {
        if self.elements == MAX_ELEMENTS {
            return Err("wire JSON exceeds element limit".into());
        }
        self.elements += 1;
        let comma = match self.stack.last_mut() {
            Some(Frame::Array(count)) => {
                *count += 1;
                *count > 1
            }
            Some(Frame::Object { awaiting_value, .. }) if *awaiting_value => {
                *awaiting_value = false;
                false
            }
            Some(Frame::Object { .. }) => return Err("wire object requires a field name".into()),
            None if self.elements != 1 => return Err("wire JSON has multiple roots".into()),
            None => false,
        };
        if comma {
            self.append(",")?;
        }
        Ok(())
    }
    pub fn null(&mut self) -> Result<(), String> {
        self.scalar("null")
    }
    pub fn boolean(&mut self, value: bool) -> Result<(), String> {
        self.scalar(if value { "true" } else { "false" })
    }
    pub fn int(&mut self, value: i64) -> Result<(), String> {
        self.scalar(&value.to_string())
    }
    pub fn float(&mut self, value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err("non-finite wire Float".into());
        }
        self.scalar(&serde_json::to_string(&value).map_err(|error| error.to_string())?)
    }
    pub fn text(&mut self, value: &str) -> Result<(), String> {
        if value.len() > MAX_BYTES.saturating_sub(self.output.len()) {
            return Err("wire JSON exceeds byte limit".into());
        }
        self.scalar(&serde_json::to_string(value).map_err(|error| error.to_string())?)
    }
    fn scalar(&mut self, text: &str) -> Result<(), String> {
        self.value()?;
        self.append(text)
    }
    pub fn json(&mut self, text: &str) -> Result<(), String> {
        if text.len() > MAX_BYTES.saturating_sub(self.output.len()) {
            return Err("wire JSON exceeds byte limit".into());
        }
        let raw: &RawValue = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut remaining = MAX_ELEMENTS - self.elements;
        parse_node(raw, self.stack.len(), &mut remaining)?;
        self.value()?;
        self.elements = MAX_ELEMENTS - remaining;
        self.append(raw.get())
    }
    pub fn begin_array(&mut self) -> Result<(), String> {
        self.begin(false)
    }
    pub fn begin_object(&mut self) -> Result<(), String> {
        self.begin(true)
    }
    fn begin(&mut self, object: bool) -> Result<(), String> {
        if self.stack.len() >= MAX_DEPTH {
            return Err("wire JSON exceeds depth limit".into());
        }
        self.value()?;
        self.append(if object { "{" } else { "[" })?;
        self.stack.push(if object {
            Frame::Object {
                names: BTreeSet::new(),
                awaiting_value: false,
            }
        } else {
            Frame::Array(0)
        });
        Ok(())
    }
    pub fn key(&mut self, name: &str) -> Result<(), String> {
        if name.len() > MAX_BYTES.saturating_sub(self.output.len()) {
            return Err("wire JSON exceeds byte limit".into());
        }
        let Some(Frame::Object {
            names,
            awaiting_value,
        }) = self.stack.last_mut()
        else {
            return Err("wire field outside record".into());
        };
        if *awaiting_value {
            return Err("wire field has no value".into());
        }
        if !names.insert(name.to_owned()) {
            return Err("duplicate JSON field".into());
        }
        *awaiting_value = true;
        let comma = names.len() > 1;
        if comma {
            self.append(",")?;
        }
        self.append(&serde_json::to_string(name).map_err(|error| error.to_string())?)?;
        self.append(":")
    }
    pub fn end(&mut self) -> Result<(), String> {
        if matches!(
            self.stack.last(),
            Some(Frame::Object {
                awaiting_value: true,
                ..
            })
        ) {
            return Err("incomplete wire container".into());
        }
        match self.stack.pop() {
            Some(Frame::Array(_)) => self.append("]"),
            Some(Frame::Object {
                awaiting_value: false,
                ..
            }) => self.append("}"),
            _ => Err("incomplete wire container".into()),
        }
    }
    pub fn finish(self) -> Result<Encoded, String> {
        if self.failed || !self.stack.is_empty() || self.elements == 0 {
            return Err("incomplete wire JSON".into());
        }
        Ok(Encoded(self.output))
    }
}

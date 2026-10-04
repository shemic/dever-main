use std::collections::BTreeMap;
use std::fmt;

use crate::http::Request;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputError(pub String);

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InputError {}

pub struct Inputs {
    values: BTreeMap<String, String>,
    query: bool,
}

#[derive(Clone, Copy)]
pub enum QueryValue {
    Text,
    Int,
    Bool,
    Float,
}

impl Inputs {
    pub(crate) fn from_fields(values: BTreeMap<String, String>) -> Self {
        Self {
            values,
            query: true,
        }
    }
    pub fn from_request(request: &Request) -> Result<Self, InputError> {
        let query = request
            .target
            .split_once('?')
            .map_or("", |(_, query)| query);
        match request.method.as_str() {
            "GET" | "DELETE" => {
                if !request.body.values().is_empty() {
                    return Err(InputError(
                        "GET and DELETE inputs must use the query string".into(),
                    ));
                }
                let mut values = BTreeMap::new();
                for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                    let name = decode_query(name)?;
                    let value = serde_json::to_string(&decode_query(value)?)
                        .map_err(|error| InputError(error.to_string()))?;
                    if values.insert(name.clone(), value).is_some() {
                        return Err(InputError(format!("duplicate input '{name}'")));
                    }
                }
                Ok(Self {
                    values,
                    query: true,
                })
            }
            "POST" | "PUT" => {
                if !query.is_empty() {
                    return Err(InputError(
                        "POST and PUT inputs must use a JSON object body".into(),
                    ));
                }
                let mut content_types = request
                    .headers
                    .iter()
                    .filter(|header| header.name.eq_ignore_ascii_case("content-type"));
                let content_type = content_types
                    .next()
                    .and_then(|header| std::str::from_utf8(header.value.values()).ok());
                if content_types.next().is_some()
                    || !content_type.is_some_and(|value| {
                        value
                            .split(';')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case("application/json")
                    })
                {
                    return Err(InputError(
                        "POST and PUT require Content-Type: application/json".into(),
                    ));
                }
                let body = std::str::from_utf8(request.body.values())
                    .map_err(|_| InputError("JSON body must be UTF-8".into()))?;
                Self::from_json_object(body)
            }
            _ => Err(InputError("unsupported API request method".into())),
        }
    }

    pub fn from_json_object(body: &str) -> Result<Self, InputError> {
        let node = crate::wire::parse(body)
            .map_err(|error| InputError(format!("invalid JSON object body: {error}")))?;
        let values = node
            .object()
            .map_err(|error| InputError(format!("invalid JSON object body: {error}")))?
            .iter()
            .map(|(name, value)| (name.clone(), value.raw().to_owned()))
            .collect();
        Ok(Self {
            values,
            query: false,
        })
    }

    pub fn raw_json(&mut self, name: &str) -> Result<String, InputError> {
        self.values.remove(name).ok_or_else(|| missing(name))
    }

    pub fn optional_raw_json(&mut self, name: &str) -> Result<Option<String>, InputError> {
        Ok(self.values.remove(name))
    }

    pub fn json(&mut self, name: &str, kind: QueryValue) -> Result<String, InputError> {
        let value = self.raw_json(name)?;
        self.convert_query(name, kind, value)
    }

    pub fn optional_json(&mut self, name: &str, kind: QueryValue) -> Result<String, InputError> {
        let value = self
            .optional_raw_json(name)?
            .unwrap_or_else(|| "null".into());
        self.convert_query(name, kind, value)
    }

    fn convert_query(
        &self,
        name: &str,
        kind: QueryValue,
        value: String,
    ) -> Result<String, InputError> {
        if !self.query || value == "null" {
            return Ok(value);
        }
        let text: String =
            serde_json::from_str(&value).map_err(|error| InputError(error.to_string()))?;
        match kind {
            QueryValue::Text => Ok(value),
            QueryValue::Int => text
                .parse::<i64>()
                .map(|value| value.to_string())
                .map_err(|_| InputError(format!("input '{name}' must be Int"))),
            QueryValue::Bool => text
                .parse::<bool>()
                .map(|value| value.to_string())
                .map_err(|_| InputError(format!("input '{name}' must be Bool"))),
            QueryValue::Float => text
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| value.to_string())
                .ok_or_else(|| InputError(format!("input '{name}' must be Float"))),
        }
    }

    pub fn text(&mut self, name: &str) -> Result<String, InputError> {
        self.optional_text(name)?.ok_or_else(|| missing(name))
    }

    pub fn optional_text(&mut self, name: &str) -> Result<Option<String>, InputError> {
        self.take(name, "Text", |node, _| node.text().ok().map(str::to_owned))
    }

    pub fn int(&mut self, name: &str) -> Result<i64, InputError> {
        self.optional_int(name)?.ok_or_else(|| missing(name))
    }

    pub fn optional_int(&mut self, name: &str) -> Result<Option<i64>, InputError> {
        self.take(name, "Int", |node, query| {
            if query {
                node.text().ok()?.parse().ok()
            } else {
                node.int().ok()
            }
        })
    }

    pub fn boolean(&mut self, name: &str) -> Result<bool, InputError> {
        self.optional_boolean(name)?.ok_or_else(|| missing(name))
    }

    pub fn optional_boolean(&mut self, name: &str) -> Result<Option<bool>, InputError> {
        self.take(name, "Bool", |node, query| {
            if query {
                node.text().ok()?.parse().ok()
            } else {
                node.boolean().ok()
            }
        })
    }

    fn take<T>(
        &mut self,
        name: &str,
        expected: &str,
        decode: impl FnOnce(&crate::wire::Node<'_>, bool) -> Option<T>,
    ) -> Result<Option<T>, InputError> {
        match self.values.remove(name) {
            None => Ok(None),
            Some(value) if value == "null" => Ok(None),
            Some(value) => {
                let node = crate::wire::parse(&value).map_err(InputError)?;
                decode(&node, self.query)
                    .map(Some)
                    .ok_or_else(|| InputError(format!("input '{name}' must be {expected}")))
            }
        }
    }

    pub fn finish(self) -> Result<(), InputError> {
        match self.values.keys().next() {
            None => Ok(()),
            Some(name) => Err(InputError(format!("unknown input '{name}'"))),
        }
    }
}

pub fn text_bounds(
    name: &str,
    value: Option<&str>,
    minimum: u32,
    maximum: Option<u32>,
) -> Result<(), InputError> {
    let Some(value) = value else { return Ok(()) };
    let length = value.chars().count();
    if length < minimum as usize || maximum.is_some_and(|limit| length > limit as usize) {
        return Err(InputError(format!(
            "input '{name}' is outside Model Text length bounds"
        )));
    }
    Ok(())
}

fn missing(name: &str) -> InputError {
    InputError(format!("input '{name}' is required"))
}

fn decode_query(value: &str) -> Result<String, InputError> {
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes.next().and_then(|byte| (byte as char).to_digit(16));
                let low = bytes.next().and_then(|byte| (byte as char).to_digit(16));
                match (high, low) {
                    (Some(high), Some(low)) => ((high << 4) | low) as u8,
                    _ => return Err(InputError("query contains invalid percent encoding".into())),
                }
            }
            byte => byte,
        });
    }
    String::from_utf8(decoded).map_err(|_| InputError("query must contain UTF-8 text".into()))
}

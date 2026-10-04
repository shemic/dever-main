use std::time::Duration;

use hyper::Uri;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    Tcp,
    WebSocket,
    Sse,
}

impl Protocol {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "tcp" => Ok(Self::Tcp),
            "ws" => Ok(Self::WebSocket),
            "sse" => Ok(Self::Sse),
            _ => Err("PROTOCOL must be tcp, ws, or sse".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::WebSocket => "ws",
            Self::Sse => "sse",
        }
    }

    fn scheme(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::WebSocket => "ws",
            Self::Sse => "http",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Tcp => "/",
            Self::WebSocket => "/ws",
            Self::Sse => "/events",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub port: u16,
    pub path: String,
    pub url: String,
}

impl Target {
    fn parse(protocol: Protocol, value: &str) -> Result<Self, String> {
        let uri = value
            .parse::<Uri>()
            .map_err(|error| format!("URL is invalid: {error}"))?;
        if uri.scheme_str() != Some(protocol.scheme()) {
            return Err(format!(
                "{} URL must use the {} scheme",
                protocol.name(),
                protocol.scheme()
            ));
        }
        if uri.host() != Some("127.0.0.1") {
            return Err("live benchmark URL must use 127.0.0.1".into());
        }
        let port = uri.port_u16().ok_or("URL must include an explicit port")?;
        if uri.path() != protocol.path() || uri.query().is_some() {
            return Err(format!(
                "{} URL must use the {} path without a query",
                protocol.name(),
                protocol.path()
            ));
        }
        Ok(Self {
            port,
            path: uri.path().to_owned(),
            url: value.to_owned(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Arguments {
    pub protocol: Protocol,
    pub target: Target,
    pub connections: usize,
    pub cycles: usize,
    pub hold: Duration,
    pub timeout: Duration,
}

impl Arguments {
    pub fn parse(values: Vec<String>) -> Result<Self, String> {
        let [protocol, url, connections, cycles, hold_ms, timeout_ms] = values.as_slice() else {
            return Err(crate::usage());
        };
        let protocol = Protocol::parse(protocol)?;
        let target = Target::parse(protocol, url)?;
        let connections = number(connections, "CONNECTIONS", 1, 4_096)? as usize;
        let cycles = number(cycles, "CYCLES", 1, 100)? as usize;
        let hold_ms = number(hold_ms, "HOLD_MS", 300, 60_000)?;
        let timeout_ms = number(timeout_ms, "TIMEOUT_MS", 1, 300_000)?;
        Ok(Self {
            protocol,
            target,
            connections,
            cycles,
            hold: Duration::from_millis(hold_ms),
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

fn number(value: &str, name: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be an integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{name} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

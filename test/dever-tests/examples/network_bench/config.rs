use std::time::Duration;

use crate::settings::Settings;

pub const HEADER_BYTES: usize = 8_192;
pub const BODY_BYTES: usize = 65_536;
pub const HTTP2_FRAME_BYTES: u32 = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpVersion {
    H1,
    H2,
}

impl HttpVersion {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "h1" => Ok(Self::H1),
            "h2" => Ok(Self::H2),
            _ => Err("HTTP_VERSION must be 'h1' or 'h2'".into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::H1 => "h1",
            Self::H2 => "h2",
        }
    }

    pub fn alpn(self) -> &'static [u8] {
        match self {
            Self::H1 => b"http/1.1",
            Self::H2 => b"h2",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConnectionShape {
    pub physical_connections: usize,
    pub streams_per_connection: usize,
    pub request_slots: usize,
}

impl ConnectionShape {
    pub fn new(
        http_version: HttpVersion,
        physical_connections: usize,
        streams_per_connection: usize,
    ) -> Result<Self, String> {
        if !(1..=65_536).contains(&physical_connections) {
            return Err("PHYSICAL_CONNECTIONS must be between 1 and 65536".into());
        }
        match http_version {
            HttpVersion::H1 if streams_per_connection != 1 => {
                return Err("HTTP/1 requires STREAMS_PER_CONNECTION to be 1".into());
            }
            HttpVersion::H2 if !(1..=65_536).contains(&streams_per_connection) => {
                return Err(
                    "HTTP/2 requires STREAMS_PER_CONNECTION to be between 1 and 65536".into(),
                );
            }
            _ => {}
        }
        let request_slots = physical_connections
            .checked_mul(streams_per_connection)
            .ok_or("connection shape is too large")?;
        if request_slots > 65_536 {
            return Err(
                "PHYSICAL_CONNECTIONS * STREAMS_PER_CONNECTION must not exceed 65536".into(),
            );
        }
        Ok(Self {
            physical_connections,
            streams_per_connection,
            request_slots,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Http2Config {
    pub streams: usize,
    pub stream_window_bytes: u32,
    pub connection_window_bytes: u32,
}

impl Http2Config {
    pub fn from_settings(settings: &Settings, streams: usize) -> Result<Self, String> {
        Ok(Self {
            streams,
            stream_window_bytes: settings.integer(
                "http2_stream_window_bytes",
                65_535,
                1,
                i32::MAX as usize,
            )? as u32,
            connection_window_bytes: settings.integer(
                "http2_connection_window_bytes",
                262_144,
                65_535,
                i32::MAX as usize,
            )? as u32,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceKind {
    RuntimeHttp,
    RuntimeHttps,
    RuntimeHttp2,
    RuntimeHttps2,
    HyperHttp,
    HyperHttps,
    HyperHttp2,
    HyperHttps2,
}

impl ServiceKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "runtime-http" => Ok(Self::RuntimeHttp),
            "runtime-https" => Ok(Self::RuntimeHttps),
            "runtime-http2" => Ok(Self::RuntimeHttp2),
            "runtime-https2" => Ok(Self::RuntimeHttps2),
            "hyper-http" => Ok(Self::HyperHttp),
            "hyper-https" => Ok(Self::HyperHttps),
            "hyper-http2" => Ok(Self::HyperHttp2),
            "hyper-https2" => Ok(Self::HyperHttps2),
            _ => Err(format!("unknown service mode '{value}'")),
        }
    }

    pub fn uses_runtime(self) -> bool {
        matches!(
            self,
            Self::RuntimeHttp | Self::RuntimeHttps | Self::RuntimeHttp2 | Self::RuntimeHttps2
        )
    }

    pub fn uses_tls(self) -> bool {
        matches!(
            self,
            Self::RuntimeHttps | Self::RuntimeHttps2 | Self::HyperHttps | Self::HyperHttps2
        )
    }

    pub fn http_version(self) -> HttpVersion {
        match self {
            Self::RuntimeHttp | Self::RuntimeHttps | Self::HyperHttp | Self::HyperHttps => {
                HttpVersion::H1
            }
            Self::RuntimeHttp2 | Self::RuntimeHttps2 | Self::HyperHttp2 | Self::HyperHttps2 => {
                HttpVersion::H2
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub workers: usize,
    pub connections: usize,
    pub task_capacity: usize,
    pub timeout: Duration,
    pub lifetime: Duration,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub http2: Option<Http2Config>,
}

impl ServiceConfig {
    pub fn from_settings(settings: &Settings, kind: ServiceKind) -> Result<Self, String> {
        let workers = settings.workers()?;
        let connections = settings.integer("connections", 32, 1, 65_536)?;
        let default_tasks = connections
            .saturating_mul(2)
            .saturating_add(16)
            .clamp(272, 65_536);
        let task_capacity = settings.integer("task_capacity", default_tasks, 1, 65_536)?;
        let timeout_ms = settings.integer("timeout_ms", 2_000, 1, 300_000)?;
        let lifetime_ms = settings.integer("lifetime_ms", 10_000, 1, 86_400_000)?;
        let (cert_path, key_path) = if kind.uses_tls() {
            (
                Some(required_text(settings, "cert_path")?),
                Some(required_text(settings, "key_path")?),
            )
        } else {
            (None, None)
        };
        let http2 = if kind.http_version() == HttpVersion::H2 {
            let streams = settings.integer("http2_streams", 16, 1, 65_536)?;
            let request_slots = connections
                .checked_mul(streams)
                .ok_or("benchmark.connections * benchmark.http2_streams is too large")?;
            if request_slots > 65_536 {
                return Err(
                    "benchmark.connections * benchmark.http2_streams must not exceed 65536".into(),
                );
            }
            Some(Http2Config::from_settings(settings, streams)?)
        } else {
            None
        };
        Ok(Self {
            workers,
            connections,
            task_capacity,
            timeout: Duration::from_millis(timeout_ms as u64),
            lifetime: Duration::from_millis(lifetime_ms as u64),
            cert_path,
            key_path,
            http2,
        })
    }
}

fn required_text(settings: &Settings, name: &str) -> Result<String, String> {
    settings
        .value(name)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("benchmark.{name} must be nonempty Text"))
}

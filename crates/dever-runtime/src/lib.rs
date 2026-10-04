use std::io::{self, Write};

pub mod abi;
#[cfg(feature = "wire")]
pub mod api;
#[cfg(feature = "api")]
pub mod application;
pub mod async_stream;
#[cfg(feature = "api")]
pub mod auth;
pub mod bytes;
pub mod channel;
pub mod collections;
#[cfg(feature = "wire")]
pub mod component;
pub mod concurrent;
#[cfg(any(feature = "database", feature = "wire"))]
pub mod config;
pub mod cron;
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod database;
mod endpoint;
#[cfg(feature = "external")]
pub mod external;
pub mod http;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod job;
#[cfg(any(feature = "database", feature = "wire"))]
pub mod lifecycle;
pub mod log;
pub mod net;
pub mod number;
pub mod orm;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod pool;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod process;
pub mod render;
pub mod resource;
pub mod secret;
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod sse;
pub mod task;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod tenant;
pub mod text;
pub mod time;
pub mod tls;
mod transport;
pub mod websocket;
#[cfg(feature = "wire")]
pub mod wire;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Id(pub String);

/// 将故障格式化移出正常计算路径，避免每个数值操作携带重复格式化代码。
#[cold]
#[inline(never)]
pub fn located_error(location: &str, error: impl std::fmt::Display) -> String {
    format!("{location}: {error}")
}

pub fn println(text: &str) -> io::Result<()> {
    write_line(&mut io::stdout().lock(), text)
}

pub fn write_line(output: &mut impl Write, text: &str) -> io::Result<()> {
    writeln!(output, "{text}")?;
    output.flush()
}

use crate::{
    tls::{ClientTls, ServerTls},
    transport::Transport,
};

#[derive(Clone, Copy, Debug)]
pub struct Http2Limits {
    pub streams: i64,
    pub stream_window_bytes: i64,
    pub connection_window_bytes: i64,
}

impl Http2Limits {
    pub(super) fn check(self, connections: i64, headers: usize) -> Result<(), String> {
        if !(1..=65536).contains(&self.streams) || connections * self.streams > 65536 {
            return Err("HTTP/2 connections * streams must be between 1 and 65536".into());
        }
        if !(1..=i32::MAX as i64).contains(&self.stream_window_bytes) {
            return Err("HTTP/2 stream_window_bytes must be between 1 and 2147483647".into());
        }
        if !(65535..=i32::MAX as i64).contains(&self.connection_window_bytes) {
            return Err(
                "HTTP/2 connection_window_bytes must be between 65535 and 2147483647".into(),
            );
        }
        if u32::try_from(headers).is_err() {
            return Err("HTTP/2 header_bytes must fit an unsigned 32-bit limit".into());
        }
        Ok(())
    }

    pub(super) fn client_builder<E: Clone>(
        self,
        executor: E,
        headers: usize,
    ) -> hyper::client::conn::http2::Builder<E> {
        let mut builder = hyper::client::conn::http2::Builder::new(executor);
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .adaptive_window(false)
            .initial_stream_window_size(self.stream_window_bytes as u32)
            .initial_connection_window_size(self.connection_window_bytes as u32)
            .initial_max_send_streams(self.streams as usize)
            .max_concurrent_streams(0)
            .max_header_list_size(headers as u32)
            .max_frame_size(FRAME_BYTES)
            .max_send_buf_size(FRAME_BYTES as usize);
        builder
    }

    pub(super) fn server_builder<E>(
        self,
        executor: E,
        headers: usize,
    ) -> hyper::server::conn::http2::Builder<E> {
        let mut builder = hyper::server::conn::http2::Builder::new(executor);
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .adaptive_window(false)
            .initial_stream_window_size(self.stream_window_bytes as u32)
            .initial_connection_window_size(self.connection_window_bytes as u32)
            .max_concurrent_streams(self.streams as u32)
            .max_header_list_size(headers as u32)
            .max_frame_size(FRAME_BYTES)
            .max_send_buf_size(FRAME_BYTES as usize);
        builder
    }
}

const FRAME_BYTES: u32 = 16384;

// HTTP 单独派生 ALPN；不能修改 WSS 和其他 TLS 使用者共享的配置。
pub(super) fn client_tls(tls: ClientTls, http2: Option<Http2Limits>) -> ClientTls {
    let mut config = (*tls.0).clone();
    config.alpn_protocols = vec![alpn(http2).to_vec()];
    ClientTls(std::sync::Arc::new(config))
}

pub(super) fn server_tls(tls: ServerTls, http2: Option<Http2Limits>) -> ServerTls {
    let mut config = (*tls.0).clone();
    config.alpn_protocols = vec![alpn(http2).to_vec()];
    ServerTls(std::sync::Arc::new(config))
}

fn alpn(http2: Option<Http2Limits>) -> &'static [u8] {
    if http2.is_some() { b"h2" } else { b"http/1.1" }
}

pub(super) fn check_alpn(stream: &Transport, http2: Option<Http2Limits>) -> Result<(), String> {
    if let Transport::Tls(stream) = stream
        && http2.is_some()
        && stream.get_ref().1.alpn_protocol() != Some(b"h2")
    {
        return Err("HTTP/2 TLS peer did not negotiate h2".into());
    }
    Ok(())
}

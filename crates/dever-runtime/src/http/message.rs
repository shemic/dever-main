use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Body, Incoming};
use hyper::header::{CONTENT_LENGTH, HOST, TRANSFER_ENCODING, UPGRADE};
use hyper::{HeaderMap, Method, StatusCode, Uri};

use super::{Budget, Header, Request, Response};
use crate::bytes::Bytes;

#[derive(Clone, Copy, Eq, PartialEq)]
enum MessageKind {
    Request,
    Response,
}

pub(super) fn header_size(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
        .sum::<usize>()
        + 2
}

pub(super) fn decode_headers(headers: HeaderMap) -> Vec<Header> {
    headers
        .iter()
        .map(|(name, value)| Header {
            name: name.to_string(),
            value: Bytes::new(value.as_bytes().to_vec()),
        })
        .collect()
}

fn encode_headers(
    headers: Vec<Header>,
    budget: Budget,
    kind: MessageKind,
) -> Result<HeaderMap, String> {
    let mut result = HeaderMap::new();
    let mut length = 2;
    for header in headers {
        length += header.name.len() + header.value.values().len() + 4;
        if length > budget.headers {
            return Err("HTTP headers exceed header_bytes".into());
        }
        let name = hyper::header::HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|error| error.to_string())?;
        if name == CONTENT_LENGTH || name == TRANSFER_ENCODING {
            return Err("HTTP framing headers are owned by the engine".into());
        }
        if name == UPGRADE {
            return Err("HTTP upgrades are not supported yet".into());
        }
        let value = hyper::header::HeaderValue::from_maybe_shared(header.value.into_http())
            .map_err(|error| error.to_string())?;
        if budget.http2.is_some() {
            check_http2_header(name.as_str(), value.as_bytes(), kind)?;
        }
        result
            .try_append(name, value)
            .map_err(|error| error.to_string())?;
    }
    Ok(result)
}

fn check_http2_header(name: &str, value: &[u8], kind: MessageKind) -> Result<(), String> {
    if matches!(
        name,
        "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade"
    ) || (name == "te"
        && (kind == MessageKind::Response || !value.eq_ignore_ascii_case(b"trailers")))
    {
        return Err(format!("HTTP/2 forbids connection-specific header {name}"));
    }
    Ok(())
}

// Source 继续使用 origin-form；scheme/authority 只在 HTTP/2 线协议边界补齐。
pub(super) fn http2_request<B>(
    request: &mut hyper::Request<B>,
    scheme: &str,
) -> Result<(), String> {
    let host = request
        .headers()
        .get(HOST)
        .ok_or("HTTP request requires Host")?
        .to_str()
        .map_err(|error| error.to_string())?;
    let authority = host
        .parse::<hyper::http::uri::Authority>()
        .map_err(|error| error.to_string())?;
    if authority.as_str().contains('@') {
        return Err("HTTP/2 authority cannot contain credentials".into());
    }
    let mut uri = request.uri().clone().into_parts();
    uri.scheme = Some(
        scheme
            .parse::<hyper::http::uri::Scheme>()
            .map_err(|error| error.to_string())?,
    );
    uri.authority = Some(authority);
    *request.uri_mut() = Uri::from_parts(uri).map_err(|error| error.to_string())?;
    *request.version_mut() = hyper::Version::HTTP_2;
    Ok(())
}

fn check_message(
    headers: &HeaderMap,
    body: &Bytes,
    start_bytes: usize,
    budget: Budget,
) -> Result<(), String> {
    // 预留引擎生成的 Content-Length 和 Date；不截断用户内容。
    if header_size(headers) + start_bytes + 80 > budget.headers {
        return Err("HTTP headers exceed header_bytes".into());
    }
    if body.values().len() > budget.body {
        return Err("HTTP body exceeds body_bytes".into());
    }
    Ok(())
}

pub(super) fn encode_request(
    request: Request,
    budget: Budget,
) -> Result<hyper::Request<Full<bytes::Bytes>>, String> {
    let method =
        Method::from_bytes(request.method.as_bytes()).map_err(|error| error.to_string())?;
    let uri = request
        .target
        .parse::<Uri>()
        .map_err(|error| error.to_string())?;
    if method == Method::CONNECT
        || uri.scheme().is_some()
        || uri.authority().is_some()
        || !(request.target.starts_with('/')
            || (method == Method::OPTIONS && request.target == "*"))
    {
        return Err(
            "HTTP send requires an origin-form target or OPTIONS *; CONNECT is not supported"
                .into(),
        );
    }
    let headers = encode_headers(request.headers, budget, MessageKind::Request)?;
    if headers.get_all(HOST).iter().count() != 1 {
        return Err("HTTP request requires exactly one Host header".into());
    }
    let start_bytes = request.method.len() + request.target.len() + 14;
    check_message(&headers, &request.body, start_bytes, budget)?;
    let mut result = hyper::Request::new(Full::new(request.body.into_http()));
    *result.method_mut() = method;
    *result.uri_mut() = uri;
    *result.headers_mut() = headers;
    Ok(result)
}

pub(super) fn encode_response(
    response: Response,
    budget: Budget,
) -> Result<hyper::Response<Full<bytes::Bytes>>, String> {
    let status = u16::try_from(response.status)
        .ok()
        .filter(|status| (200..=599).contains(status))
        .ok_or("HTTP response status must be between 200 and 599")?;
    let status = StatusCode::from_u16(status).map_err(|error| error.to_string())?;
    let no_body = matches!(
        status,
        StatusCode::NO_CONTENT | StatusCode::RESET_CONTENT | StatusCode::NOT_MODIFIED
    );
    if no_body && !response.body.values().is_empty() {
        return Err("HTTP 204, 205 and 304 responses cannot contain a body".into());
    }
    let headers = encode_headers(response.headers, budget, MessageKind::Response)?;
    check_message(&headers, &response.body, 32, budget)?;
    let mut result = hyper::Response::new(Full::new(response.body.into_http()));
    *result.status_mut() = status;
    *result.headers_mut() = headers;
    Ok(result)
}

pub(super) async fn collect_body(body: Incoming, limit: usize) -> Result<Bytes, StatusCode> {
    if body.size_hint().lower() > limit as u64 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut incoming = Limited::new(body, limit);
    let mut body = Bytes::new(Vec::new());
    // 单帧直接共享，多帧逐步扩容；不为大量微小帧保留一份元数据队列。
    while let Some(frame) = incoming.frame().await {
        let frame = frame.map_err(|error| {
            if error.is::<LengthLimitError>() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            }
        })?;
        match frame.into_data() {
            Ok(bytes) => body = body.concat(&Bytes::from_http(bytes)),
            Err(frame)
                if frame
                    .trailers_ref()
                    .is_some_and(|trailers| !trailers.is_empty()) =>
            {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
            Err(_) => {}
        }
    }
    Ok(body)
}

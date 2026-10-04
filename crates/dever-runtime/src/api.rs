use base64::Engine as _;
use std::cell::RefCell;
use std::future::Future;

use std::collections::BTreeMap;

use crate::bytes::Bytes;
use crate::http::{self, Header, Request, Response};

pub use serde_json::{Map as Object, Value};
mod input;
pub mod upload;
pub use input::{InputError, Inputs, QueryValue, text_bounds};

tokio::task_local! { static REQUEST: RefCell<RequestState>; }

struct RequestState {
    id: u64,
    method: String,
    path: String,
    client_address: Option<String>,
    headers: Vec<Header>,
    cookies: BTreeMap<String, crate::secret::Secret>,
    pending_response_headers: Vec<Header>,
    committed_response_headers: Vec<Header>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

pub struct CookieOptions {
    pub path: String,
    pub domain: Option<String>,
    pub same_site: SameSite,
    pub secure: bool,
    pub http_only: bool,
    pub max_age: Option<i64>,
}

/// 路由由编译器生成；运行时只负责既有 HTTP 引擎的配置与启动。
pub async fn serve<F, Fut>(route: F) -> Result<(), String>
where
    F: Fn(Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    if !crate::lifecycle::api_enabled() {
        return Ok(());
    }
    let settings = crate::config::http_settings()?;
    let listener = crate::net::listen(&settings.host, settings.port).await?;
    let limits = settings.limits;
    let components = crate::component::current();
    #[cfg(feature = "api")]
    let session = crate::application::current();
    let server = http::serve_incoming(
        move |incoming| {
            let route = route.clone();
            let settings = settings.clone();
            let request = async move {
                upload::scope(
                    incoming.body,
                    settings,
                    scoped_request(incoming.head, route),
                )
                .await
            };
            #[cfg(feature = "api")]
            let request = crate::application::scope_resources(session.clone(), request);
            crate::component::scope_resources(components.clone(), request)
        },
        listener.clone(),
        limits,
    );
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        _ = crate::lifecycle::shutdown_requested() => {
            listener.close()?;
            server.await
        }
    }
}

/// The scope owns response metadata; no source-level Context value exists.
pub async fn scoped_request<F, Fut>(request: Request, route: F) -> Result<Response, String>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Response, String>>,
{
    let cookies = match read_cookies(&request.headers) {
        Ok(cookies) => cookies,
        Err(error) => return Ok(invalid_input(error)),
    };
    let state = RequestState {
        id: http::request_id(),
        method: request.method.clone(),
        path: request.target.split('?').next().unwrap_or("").to_owned(),
        client_address: http::client_address(),
        headers: request.headers.clone(),
        cookies,
        pending_response_headers: Vec::new(),
        committed_response_headers: Vec::new(),
    };
    REQUEST
        .scope(RefCell::new(state), async move {
            let mut response = route(request).await?;
            REQUEST.with(|state| {
                response
                    .headers
                    .append(&mut state.borrow_mut().committed_response_headers)
            });
            Ok(response)
        })
        .await
}

/// Publish staged metadata only after the generated handler has serialized its
/// response and committed its database transaction.
pub fn commit_response_metadata() -> Result<(), String> {
    with_request_mut(|request| {
        request
            .committed_response_headers
            .append(&mut request.pending_response_headers);
        Ok(())
    })
}

fn with_request<T>(read: impl FnOnce(&RequestState) -> Result<T, String>) -> Result<T, String> {
    REQUEST
        .try_with(|state| read(&state.borrow()))
        .map_err(|_| "API request capability is available only within an HTTP request".to_owned())?
}

fn with_request_mut<T>(
    write: impl FnOnce(&mut RequestState) -> Result<T, String>,
) -> Result<T, String> {
    REQUEST
        .try_with(|state| write(&mut state.borrow_mut()))
        .map_err(|_| "API request capability is available only within an HTTP request".to_owned())?
}

pub fn request_id() -> Result<String, String> {
    with_request(|request| Ok(request.id.to_string()))
}
pub fn method() -> Result<String, String> {
    with_request(|request| Ok(request.method.clone()))
}
pub fn path() -> Result<String, String> {
    with_request(|request| Ok(request.path.clone()))
}
pub fn client_address() -> Result<Option<String>, String> {
    with_request(|request| Ok(request.client_address.clone()))
}

pub fn header(name: &str) -> Result<Option<String>, String> {
    validate_header_name(name)?;
    if [
        "cookie",
        "authorization",
        "proxy-authorization",
        "set-cookie",
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ]
    .iter()
    .any(|blocked| name.eq_ignore_ascii_case(blocked))
    {
        return Err("sensitive request headers require a typed Secret reader".into());
    }
    with_request(|request| {
        let mut values = request
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(name));
        let Some(value) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            return Err("duplicate request header".into());
        }
        let value = std::str::from_utf8(value.value.values())
            .map_err(|_| "request header is not UTF-8".to_owned())?;
        Ok(Some(value.to_owned()))
    })
}

pub fn cookie(name: &str) -> Result<Option<String>, String> {
    validate_cookie_name(name)?;
    with_request(|request| {
        let Some(raw) = request.cookies.get(name) else {
            return Ok(None);
        };
        if raw.0.starts_with(b"v1.") {
            return Err("Secret Cookie values require the typed Secret reader".into());
        }
        let value =
            std::str::from_utf8(&raw.0).map_err(|_| "Cookie value is not UTF-8".to_owned())?;
        Ok(Some(value.to_owned()))
    })
}

pub fn secret_cookie(name: &str) -> Result<Option<crate::secret::Secret>, String> {
    validate_cookie_name(name)?;
    with_request(|request| {
        let Some(raw) = request.cookies.get(name) else {
            return Ok(None);
        };
        let Some(encoded) = raw.0.strip_prefix(b"v1.") else {
            return Ok(None);
        };
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()
            .map(crate::secret::Secret::from_input))
    })
}

fn read_cookies(headers: &[Header]) -> Result<BTreeMap<String, crate::secret::Secret>, InputError> {
    let mut cookies = BTreeMap::new();
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("cookie"))
    {
        let raw = header.value.values();
        if !raw.is_ascii() {
            return Err(InputError("Cookie header must be ASCII".into()));
        }
        for pair in raw.split(|byte| *byte == b';') {
            let pair = pair.trim_ascii();
            if pair.is_empty() {
                continue;
            }
            let Some(separator) = pair.iter().position(|byte| *byte == b'=') else {
                return Err(InputError("invalid Cookie header".into()));
            };
            let (key, encoded) = pair.split_at(separator);
            let value = &encoded[1..];
            let value = if value.len() >= 2
                && value.first() == Some(&b'"')
                && value.last() == Some(&b'"')
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            let name = std::str::from_utf8(key).expect("ASCII Cookie header");
            validate_cookie_name(name).map_err(InputError)?;
            validate_cookie_value(value).map_err(InputError)?;
            if cookies
                .insert(
                    name.to_owned(),
                    crate::secret::Secret::from_input(value.to_vec()),
                )
                .is_some()
            {
                return Err(InputError("duplicate Cookie name".into()));
            }
        }
    }
    Ok(cookies)
}

pub fn response_header(name: &str, value: &str) -> Result<(), String> {
    validate_header_name(name)?;
    if [
        "content-length",
        "content-type",
        "content-encoding",
        "transfer-encoding",
        "connection",
        "set-cookie",
        "host",
        "upgrade",
        "keep-alive",
        "trailer",
        "te",
    ]
    .iter()
    .any(|blocked| name.eq_ignore_ascii_case(blocked))
    {
        return Err("response header is controlled by the HTTP runtime".into());
    }
    hyper::header::HeaderValue::from_str(value)
        .map_err(|_| "invalid response header value".to_owned())?;
    with_request_mut(|request| {
        request.pending_response_headers.push(Header {
            name: name.to_owned(),
            value: Bytes::from_text(value),
        });
        Ok(())
    })
}

pub fn response_cookie(name: &str, value: &str, options: CookieOptions) -> Result<(), String> {
    write_cookie(name, value.as_bytes(), options, false)
}

pub fn response_secret_cookie(
    name: &str,
    value: crate::secret::Secret,
    options: CookieOptions,
) -> Result<(), String> {
    let length = base64::encoded_len(value.0.len(), false)
        .ok_or_else(|| "Secret Cookie value is too large".to_owned())?;
    let mut encoded = vec![0; length + 3];
    encoded[..3].copy_from_slice(b"v1.");
    let written = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode_slice(&value.0, &mut encoded[3..])
        .map_err(|_| "Secret Cookie encoding failed".to_owned())?;
    encoded.truncate(written + 3);
    write_cookie(name, &encoded, options, true)
}

fn write_cookie(
    name: &str,
    value: &[u8],
    options: CookieOptions,
    secret: bool,
) -> Result<(), String> {
    require_same_origin_for_cookie_write()?;
    validate_cookie_name(name)?;
    validate_cookie_value(value)?;
    if secret && (!options.secure || !options.http_only) {
        return Err("Secret cookies require Secure and HttpOnly".into());
    }
    if options.same_site == SameSite::None && !options.secure {
        return Err("SameSite=None requires Secure".into());
    }
    if !options.path.starts_with('/')
        || !options
            .path
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b';')
    {
        return Err("invalid Cookie Path".into());
    }
    if let Some(domain) = &options.domain
        && (domain.len() > 253
            || domain.is_empty()
            || domain.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            }))
    {
        return Err("invalid Cookie Domain".into());
    }
    if options.max_age.is_some_and(|age| age < 0) {
        return Err("Cookie Max-Age must be nonnegative".into());
    }
    let mut encoded = Vec::with_capacity(name.len() + value.len() + 96);
    encoded.extend_from_slice(name.as_bytes());
    encoded.push(b'=');
    encoded.extend_from_slice(value);
    encoded.extend_from_slice(b"; Path=");
    encoded.extend_from_slice(options.path.as_bytes());
    if let Some(domain) = options.domain {
        encoded.extend_from_slice(b"; Domain=");
        encoded.extend_from_slice(domain.as_bytes());
    }
    encoded.extend_from_slice(match options.same_site {
        SameSite::Strict => b"; SameSite=Strict",
        SameSite::Lax => b"; SameSite=Lax",
        SameSite::None => b"; SameSite=None",
    });
    if options.secure {
        encoded.extend_from_slice(b"; Secure");
    }
    if options.http_only {
        encoded.extend_from_slice(b"; HttpOnly");
    }
    if let Some(age) = options.max_age {
        encoded.extend_from_slice(format!("; Max-Age={age}").as_bytes());
    }
    if encoded.len() > 4096 {
        return Err("Set-Cookie header exceeds 4096 bytes".into());
    }
    with_request_mut(|request| {
        request.pending_response_headers.push(Header {
            name: "set-cookie".into(),
            value: Bytes::new(encoded),
        });
        Ok(())
    })
}

fn require_same_origin_for_cookie_write() -> Result<(), String> {
    with_request(|request| {
        if matches!(request.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
            return Ok(());
        }
        #[cfg(feature = "api")]
        {
            let origin = crate::auth::cookie_origin()
                .map_err(|_| "site origin is required for unsafe Cookie writes".to_owned())?;
            validate_same_origin(&request.headers, &origin)
        }
        #[cfg(not(feature = "api"))]
        {
            Err("site authentication is required for unsafe Cookie writes".into())
        }
    })
}

#[cfg(feature = "api")]
pub(crate) fn validate_same_origin(
    headers: &[Header],
    expected: &crate::config::HttpOrigin,
) -> Result<(), String> {
    let host = unique_header(headers, "host")?
        .ok_or_else(|| "unsafe Cookie request requires Host".to_owned())?;
    let origin = unique_header(headers, "origin")?
        .ok_or_else(|| "unsafe Cookie request requires Origin".to_owned())?;
    let origin = origin
        .parse::<hyper::Uri>()
        .map_err(|_| "invalid Origin".to_owned())?;
    if origin.path() != "/" || origin.query().is_some() {
        return Err("Origin must not contain a path or query".into());
    }
    let origin_authority = origin
        .authority()
        .ok_or_else(|| "invalid Origin".to_owned())?;
    let request_authority = host
        .parse::<hyper::http::uri::Authority>()
        .map_err(|_| "invalid Host".to_owned())?;
    let default_port = if expected.scheme() == "https" {
        443
    } else {
        80
    };
    if origin.scheme_str() != Some(expected.scheme())
        || normalized_authority(origin_authority, default_port)
            != (expected.host().to_owned(), expected.port())
        || normalized_authority(&request_authority, default_port)
            != (expected.host().to_owned(), expected.port())
    {
        return Err("Origin and Host must exactly match the configured site origin".into());
    }
    Ok(())
}

#[cfg(feature = "api")]
fn unique_header(headers: &[Header], name: &str) -> Result<Option<String>, String> {
    let mut values = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(name));
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(format!("duplicate {name} header"));
    }
    let value = std::str::from_utf8(value.value.values())
        .map_err(|_| format!("{name} header is not UTF-8"))?;
    Ok(Some(value.to_owned()))
}

#[cfg(feature = "api")]
fn normalized_authority(
    authority: &hyper::http::uri::Authority,
    default_port: u16,
) -> (String, u16) {
    (
        authority.host().to_ascii_lowercase(),
        authority.port_u16().unwrap_or(default_port),
    )
}

fn validate_header_name(name: &str) -> Result<(), String> {
    hyper::header::HeaderName::from_bytes(name.as_bytes())
        .map(|_| ())
        .map_err(|_| "invalid HTTP header name".to_owned())
}

fn validate_cookie_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        ..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
    {
        return Err("invalid Cookie name".into());
    }
    Ok(())
}

fn validate_cookie_value(value: &[u8]) -> Result<(), String> {
    if value
        .iter()
        .all(|byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e))
    {
        Ok(())
    } else {
        Err("invalid Cookie value".into())
    }
}

pub fn success(data: crate::wire::Encoded) -> Response {
    envelope(200, 0, "ok", data)
}

pub fn invalid_input(error: InputError) -> Response {
    envelope(400, 400, &error.0, crate::wire::Encoded::null())
}

pub fn standard_error(status: i64) -> Response {
    let message = match status {
        400 => "invalid request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not found",
        409 => "conflict",
        429 => "too many requests",
        _ => return internal_error(),
    };
    envelope(status, status, message, crate::wire::Encoded::null())
}

#[cfg(feature = "api")]
pub fn auth_error(error: crate::auth::Error) -> Response {
    match error {
        crate::auth::Error::Unauthorized => standard_error(401),
        crate::auth::Error::Forbidden => standard_error(403),
        crate::auth::Error::Internal => {
            crate::log::error("authentication runtime failed", Vec::new());
            internal_error()
        }
    }
}

pub fn not_found() -> Response {
    envelope(404, 404, "not found", crate::wire::Encoded::null())
}

pub fn method_not_allowed(methods: &[&str]) -> Response {
    let mut response = envelope(405, 405, "method not allowed", crate::wire::Encoded::null());
    response.headers.push(Header {
        name: "allow".into(),
        value: Bytes::from_string(methods.join(", ")),
    });
    response
}

pub fn internal_error() -> Response {
    envelope(
        500,
        500,
        "internal server error",
        crate::wire::Encoded::null(),
    )
}

fn envelope(status: i64, code: i64, message: &str, data: crate::wire::Encoded) -> Response {
    Response {
        status,
        headers: vec![Header {
            name: "content-type".into(),
            value: Bytes::from_text("application/json; charset=utf-8"),
        }],
        body: Bytes::from_string(format!(
            "{{\"code\":{code},\"message\":{},\"data\":{}}}",
            serde_json::to_string(message).expect("Text JSON encoding"),
            data.as_str()
        )),
    }
}

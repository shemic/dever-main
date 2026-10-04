use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::{Response, StatusCode};

pub const PLAIN_PATH: &str = "/plain";
pub const JSON_PATH: &str = "/json";
pub const BYTES_PATH: &str = "/bytes";
pub const ORM_COUNT_PATH: &str = "/benchmark/item/count";

const PLAIN_BODY: &[u8] = b"hello";
const JSON_BODY: &[u8] = br#"{"ok":true}"#;

#[derive(Clone, Debug)]
pub struct ExpectedResponse {
    pub body: Bytes,
    pub content_type: &'static str,
}

impl ExpectedResponse {
    pub fn for_path(path: &str, orm_count: Option<usize>) -> Result<Self, String> {
        match path {
            PLAIN_PATH => Ok(Self {
                body: Bytes::from_static(PLAIN_BODY),
                content_type: "text/plain",
            }),
            JSON_PATH => Ok(Self {
                body: Bytes::from_static(JSON_BODY),
                content_type: "application/json",
            }),
            BYTES_PATH => Ok(Self {
                body: Bytes::from(vec![b'x'; 65_536]),
                content_type: "application/octet-stream",
            }),
            ORM_COUNT_PATH => Ok(Self {
                body: Bytes::from(format!(
                    "{{\"code\":0,\"message\":\"ok\",\"data\":{}}}",
                    orm_count.ok_or("benchmark.orm_count is required for ORM count requests")?
                )),
                content_type: "application/json; charset=utf-8",
            }),
            _ => Err(format!("unsupported benchmark path '{path}'")),
        }
    }
}

pub struct HyperResponses {
    bytes: Bytes,
}

impl HyperResponses {
    pub fn new() -> Self {
        Self {
            bytes: Bytes::from(vec![b'x'; 65_536]),
        }
    }

    pub fn response(&self, path: &str) -> Response<Full<Bytes>> {
        let (body, content_type) = match path {
            PLAIN_PATH => (Bytes::from_static(PLAIN_BODY), "text/plain"),
            JSON_PATH => (Bytes::from_static(JSON_BODY), "application/json"),
            BYTES_PATH => (self.bytes.clone(), "application/octet-stream"),
            _ => {
                let mut response = Response::new(Full::new(Bytes::new()));
                *response.status_mut() = StatusCode::NOT_FOUND;
                return response;
            }
        };
        let mut response = Response::new(Full::new(body));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        response
    }
}

#[derive(Clone)]
pub struct RuntimeResponses {
    bytes: dever_runtime::bytes::Bytes,
}

impl RuntimeResponses {
    pub fn new() -> Self {
        Self {
            bytes: dever_runtime::bytes::Bytes::new(vec![b'x'; 65_536]),
        }
    }

    pub fn response(&self, path: &str) -> dever_runtime::http::Response {
        let (body, content_type) = match path {
            PLAIN_PATH => (
                dever_runtime::bytes::Bytes::from_text("hello"),
                "text/plain",
            ),
            JSON_PATH => (
                dever_runtime::bytes::Bytes::from_text(r#"{"ok":true}"#),
                "application/json",
            ),
            BYTES_PATH => (self.bytes.clone(), "application/octet-stream"),
            _ => {
                return dever_runtime::http::Response {
                    status: 404,
                    headers: vec![],
                    body: dever_runtime::bytes::Bytes::new(vec![]),
                };
            }
        };
        dever_runtime::http::Response {
            status: 200,
            headers: vec![dever_runtime::http::Header {
                name: "content-type".into(),
                value: dever_runtime::bytes::Bytes::from_text(content_type),
            }],
            body,
        }
    }
}

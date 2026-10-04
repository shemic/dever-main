use dever_runtime::api::{self, CookieOptions, SameSite};
use dever_runtime::bytes::Bytes;
use dever_runtime::http::{Header, Request};
use dever_runtime::{http, net, task};
use std::time::Duration;

fn request(path: &str, cookie: &str) -> Request {
    Request {
        method: "GET".into(),
        target: path.into(),
        headers: vec![Header {
            name: "cookie".into(),
            value: Bytes::from_text(cookie),
        }],
        body: Bytes::from_text(""),
    }
}

fn cookie_options() -> CookieOptions {
    CookieOptions {
        path: "/".into(),
        domain: None,
        same_site: SameSite::Lax,
        secure: true,
        http_only: true,
        max_age: Some(3600),
    }
}

#[tokio::test]
async fn request_scope_is_isolated_and_merges_validated_response_metadata() {
    let first = api::scoped_request(
        request("/first?ignored=1", "session=v1.Zmlyc3Q; theme=dark"),
        |_| async {
            assert_eq!(api::method().unwrap(), "GET");
            assert_eq!(api::path().unwrap(), "/first");
            assert!(api::request_id().unwrap().parse::<u64>().is_ok());
            assert!(api::header("cookie").is_err());
            assert!(api::header("x-forwarded-for").is_err());
            assert_eq!(api::cookie("theme").unwrap().as_deref(), Some("dark"));
            assert!(api::cookie("session").is_err());
            assert_eq!(api::cookie("missing").unwrap(), None);
            let incoming = api::secret_cookie("session").unwrap().unwrap();
            assert_eq!(format!("{incoming:?}"), "Secret([REDACTED])");
            api::response_header("x-request-state", "first").unwrap();
            api::response_secret_cookie(
                "session",
                dever_runtime::secret::Secret::from_input(vec![0, 255, 0]),
                cookie_options(),
            )
            .unwrap();
            api::response_secret_cookie("copy", incoming, cookie_options()).unwrap();
            api::response_cookie("theme", "dark", cookie_options()).unwrap();
            let mut insecure = cookie_options();
            insecure.secure = false;
            assert!(
                api::response_secret_cookie(
                    "session",
                    dever_runtime::secret::Secret::from_input(b"unsafe".to_vec()),
                    insecure,
                )
                .is_err()
            );
            api::commit_response_metadata().unwrap();
            Ok(api::success(dever_runtime::wire::Encoded::null()))
        },
    );
    let second = api::scoped_request(request("/second", "session=v1.c2Vjb25k"), |_| async {
        assert_eq!(api::path().unwrap(), "/second");
        assert!(api::response_header("set-cookie", "bypass").is_err());
        assert!(api::response_header("content-type", "text/html").is_err());
        assert!(api::response_header("x-unsafe", "injected\r\nheader").is_err());
        api::response_header("x-request-state", "second").unwrap();
        api::commit_response_metadata().unwrap();
        Ok(api::success(dever_runtime::wire::Encoded::null()))
    });
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(
        first
            .headers
            .iter()
            .any(|header| header.name == "x-request-state" && header.value.values() == b"first")
    );
    assert!(
        second
            .headers
            .iter()
            .any(|header| header.name == "x-request-state" && header.value.values() == b"second")
    );
    assert!(first.headers.iter().any(|header| {
        header.name == "set-cookie"
            && header
                .value
                .values()
                .starts_with(b"session=v1.AP8A; Path=/")
    }));
    assert!(first.headers.iter().any(|header| {
        header.name == "set-cookie"
            && header
                .value
                .values()
                .starts_with(b"copy=v1.Zmlyc3Q; Path=/")
    }));
    assert!(
        first
            .headers
            .iter()
            .any(|header| header.name == "set-cookie"
                && header.value.values().starts_with(b"theme=dark; Path=/"))
    );
    assert!(
        !second
            .headers
            .iter()
            .any(|header| header.name == "set-cookie")
    );
    assert!(api::request_id().is_err());
}

#[tokio::test]
async fn malformed_or_duplicate_cookie_is_a_bad_request_before_handler_execution() {
    for cookie in ["session=one; session=two", "session=one; broken"] {
        let result = api::scoped_request(request("/profile", cookie), |_| async {
            Err("malformed Cookie reached the API handler".into())
        })
        .await
        .unwrap();
        assert_eq!(result.status, 400);
    }
}

#[tokio::test]
async fn quoted_cookie_value_uses_the_same_secret_codec() {
    let response = api::scoped_request(request("/profile", "session=\"v1.Zmlyc3Q\""), |_| async {
        let token = api::secret_cookie("session")?.ok_or("missing quoted session")?;
        api::response_secret_cookie("copy", token, cookie_options())?;
        api::commit_response_metadata()?;
        Ok(api::success(dever_runtime::wire::Encoded::null()))
    })
    .await
    .unwrap();
    assert!(response.headers.iter().any(|header| {
        header.name == "set-cookie"
            && header
                .value
                .values()
                .starts_with(b"copy=v1.Zmlyc3Q; Path=/")
    }));
}

#[tokio::test]
async fn uncommitted_response_metadata_is_discarded_on_failure() {
    let response = api::scoped_request(request("/profile", ""), |_| async {
        api::response_header("x-staged", "discarded")?;
        Ok(api::internal_error())
    })
    .await
    .unwrap();
    assert_eq!(response.status, 500);
    assert!(
        !response
            .headers
            .iter()
            .any(|header| header.name == "x-staged")
    );
}

#[test]
fn cookie_and_model_text_validation_reject_invalid_values() {
    assert!(api::text_bounds("name", Some(""), 1, Some(64)).is_err());
    assert!(api::text_bounds("name", Some("abc"), 1, Some(2)).is_err());
    assert!(api::text_bounds("name", None, 1, Some(2)).is_ok());
    assert!(api::response_cookie("session", "a;b", cookie_options()).is_err());
    assert!(api::response_cookie("session", "valid", cookie_options()).is_err());
}

#[test]
fn client_address_comes_from_the_tcp_peer_not_forwarded_headers() {
    let limits = http::Limits {
        header_bytes: 8192,
        body_bytes: 1024,
        timeout_ms: 1000,
        connections: 2,
        http2: None,
    };
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 16,
        },
        async {
            tokio::time::timeout(Duration::from_secs(8), async {
                let listener = net::listen("127.0.0.1", 0).await?;
                let port = net::port(&listener)?;
                let owned = listener.clone();
                let server = task::run(async move {
                    http::serve(
                        |request| {
                            api::scoped_request(request, |_| async {
                                let peer =
                                    api::client_address()?.ok_or("missing TCP peer address")?;
                                let mut body = dever_runtime::wire::Encoder::default();
                                body.text(&peer)?;
                                Ok(api::success(body.finish()?))
                            })
                        },
                        owned,
                        limits,
                    )
                    .await
                })
                .await?;
                let response = http::send(
                    "127.0.0.1",
                    port,
                    Request {
                        method: "GET".into(),
                        target: "/peer".into(),
                        headers: vec![
                            Header {
                                name: "host".into(),
                                value: Bytes::from_text("localhost"),
                            },
                            Header {
                                name: "x-forwarded-for".into(),
                                value: Bytes::from_text("203.0.113.99"),
                            },
                        ],
                        body: Bytes::new(vec![]),
                    },
                    limits,
                )
                .await?;
                assert_eq!(response.status, 200);
                let envelope: serde_json::Value = serde_json::from_slice(response.body.values())
                    .map_err(|error| error.to_string())?;
                assert_eq!(envelope["data"], "127.0.0.1");
                task::stop(server).await?;
                listener.close()?;
                Ok::<(), String>(())
            })
            .await
            .map_err(|_| "API peer test deadline exceeded".to_owned())?
        },
    )
    .unwrap();
}

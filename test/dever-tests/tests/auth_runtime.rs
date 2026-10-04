#[path = "support/temp.rs"]
mod temp;

use std::fs;

use dever_runtime::api;
use dever_runtime::auth::{self, Error, Identity};
use dever_runtime::bytes::Bytes;
use dever_runtime::config::Settings;
use dever_runtime::http::{Header, Request};

fn settings() -> (temp::TemporaryDirectory, Settings) {
    let root = temp::TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/setting.json"), r#"{
      "auth": {
        "providers": {
          "session": {
            "verify": "user.account.verify",
            "jwtSecret": "0123456789abcdef0123456789abcdef",
            "ttlSeconds": 3600,
            "cookie": "cms_session"
          }
        }
      },
      "sites": {
        "admin": {"path": "admin", "auth": "session", "hosts": ["cms.example.com"], "origin": "https://cms.example.com"},
        "front": {"path": "front", "auth": "session", "hosts": ["cms.example.com"], "origin": "https://cms.example.com"}
      }
    }"#).unwrap();
    let settings = Settings::load_project(root.path()).unwrap();
    (root, settings)
}

fn request(method: &str, extra_headers: Vec<Header>) -> Request {
    let mut headers = vec![Header {
        name: "host".into(),
        value: Bytes::from_text("cms.example.com"),
    }];
    headers.extend(extra_headers);
    Request {
        method: method.into(),
        target: "/news/article/profile".into(),
        headers,
        body: Bytes::from_text(""),
    }
}

#[tokio::test]
async fn site_scope_issues_and_verifies_a_bound_identity() {
    let (_root, settings) = settings();
    let login_request = request(
        "POST",
        vec![Header {
            name: "origin".into(),
            value: Bytes::from_text("https://cms.example.com"),
        }],
    );
    let login_settings = &settings;
    let response = api::scoped_request(login_request, move |request| async move {
        let prepared = auth::prepare_with_settings(login_settings, &["front"], &request, true)
            .map_err(|error| error.to_string())?;
        auth::scope(prepared, None, async {
            auth::issue_cookie("account:7", "session:19", Some("tenant:3"))?;
            api::commit_response_metadata()?;
            Ok(api::success(dever_runtime::wire::Encoded::null()))
        })
        .await
        .map_err(|error| error.to_string())?
    })
    .await
    .unwrap();

    let cookie = response
        .headers
        .iter()
        .find(|header| header.name == "set-cookie")
        .map(|header| {
            std::str::from_utf8(header.value.values())
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        })
        .expect("issued session cookie");
    let authenticated = request(
        "GET",
        vec![Header {
            name: "cookie".into(),
            value: Bytes::from_string(cookie.clone()),
        }],
    );
    let prepared =
        auth::prepare_with_settings(&settings, &["front", "profile"], &authenticated, false)
            .unwrap();
    let claims = prepared.claims().unwrap();
    assert_eq!(claims.subject, "account:7");
    assert_eq!(claims.session, "session:19");
    assert_eq!(claims.tenant.as_deref(), Some("tenant:3"));
    assert_eq!(claims.site, "front");

    let identity = Identity {
        id: "account:7".into(),
        user_id: Some(7),
        tenant_id: Some(3),
    };
    auth::scope(prepared, Some(identity), async {
        assert_eq!(auth::site_key().unwrap(), "front");
        assert_eq!(auth::id().unwrap(), "account:7");
        assert_eq!(auth::session().unwrap(), "session:19");
        assert_eq!(auth::user_id().unwrap(), Some(7));
        assert_eq!(auth::tenant_id().unwrap(), Some(3));
        #[cfg(feature = "sqlite")]
        dever_runtime::job::bind_permission("news.article.front.read").unwrap();
        #[cfg(feature = "sqlite")]
        assert!(matches!(
            dever_runtime::job::execution_identity().unwrap(),
            dever_runtime::job::ExecutionIdentity::User { permission_key, .. }
                if permission_key == "news.article.front.read"
        ));
    })
    .await
    .unwrap();

    let cross_site = request(
        "GET",
        vec![Header {
            name: "cookie".into(),
            value: Bytes::from_string(cookie),
        }],
    );
    assert_eq!(
        auth::prepare_with_settings(&settings, &["admin"], &cross_site, false)
            .err()
            .expect("site-bound token must fail"),
        Error::Unauthorized
    );
}

#[test]
fn public_routes_reject_bad_credentials_and_cookie_writes_require_origin() {
    let (_root, settings) = settings();
    let invalid = request(
        "GET",
        vec![Header {
            name: "authorization".into(),
            value: Bytes::from_text("Bearer invalid"),
        }],
    );
    assert_eq!(
        auth::prepare_with_settings(&settings, &["front"], &invalid, true)
            .err()
            .expect("invalid credential must not become anonymous"),
        Error::Unauthorized
    );

    let cookie_write = request(
        "DELETE",
        vec![Header {
            name: "cookie".into(),
            value: Bytes::from_text("cms_session=v1.aW52YWxpZA"),
        }],
    );
    assert_eq!(
        auth::prepare_with_settings(&settings, &["front"], &cookie_write, false)
            .err()
            .expect("unsafe cookie request requires Origin"),
        Error::Forbidden
    );

    let stale_cookie = request(
        "POST",
        vec![Header {
            name: "cookie".into(),
            value: Bytes::from_text("cms_session=v1.invalid"),
        }],
    );
    let prepared = auth::prepare_with_settings(&settings, &["front"], &stale_cookie, true)
        .expect("public login must ignore stale Cookie credentials");
    assert!(prepared.claims().is_none());
}

#[tokio::test]
async fn unsafe_cookie_writes_require_the_configured_external_origin() {
    let (_root, settings) = settings();
    for origin in [
        None,
        Some("http://cms.example.com"),
        Some("https://cms.example.com:444"),
        Some("https://other.example.com"),
    ] {
        let headers = origin
            .into_iter()
            .map(|value| Header {
                name: "origin".into(),
                value: Bytes::from_text(value),
            })
            .collect();
        let request = request("POST", headers);
        let test_settings = &settings;
        let response = api::scoped_request(request, |request| async move {
            let prepared = auth::prepare_with_settings(test_settings, &["front"], &request, true)
                .map_err(|error| error.to_string())?;
            auth::scope(prepared, None, async {
                assert!(auth::issue_cookie("account:7", "session:19", Some("tenant:3")).is_err());
                Ok(api::success(dever_runtime::wire::Encoded::null()))
            })
            .await
            .map_err(|error| error.to_string())?
        })
        .await
        .unwrap();
        assert!(
            !response
                .headers
                .iter()
                .any(|header| header.name == "set-cookie")
        );
    }

    let valid = request(
        "POST",
        vec![Header {
            name: "origin".into(),
            value: Bytes::from_text("https://cms.example.com"),
        }],
    );
    let test_settings = &settings;
    let response = api::scoped_request(valid, |request| async move {
        let prepared = auth::prepare_with_settings(test_settings, &["front"], &request, true)
            .map_err(|error| error.to_string())?;
        auth::scope(prepared, None, async {
            auth::issue_cookie("account:7", "session:19", Some("tenant:3"))?;
            api::commit_response_metadata()?;
            Ok(api::success(dever_runtime::wire::Encoded::null()))
        })
        .await
        .map_err(|error| error.to_string())?
    })
    .await
    .unwrap();
    assert!(
        response
            .headers
            .iter()
            .any(|header| header.name == "set-cookie")
    );
}

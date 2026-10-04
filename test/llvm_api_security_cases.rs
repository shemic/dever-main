//! Real checked routes exercise the existing identity, RBAC and tenant owners.
use super::native::{Server, TRANSPORT, application_ir, settings};
use dever_backend_bridge::{Target, emit_object};
use dever_runtime::{
    api, auth,
    bytes::Bytes,
    config::Settings,
    http::{Header, Request},
    task, wire,
};
use serde_json::{Value, json};

pub(super) fn files() -> Vec<(&'static str, &'static str)> {
    let mut files = TRANSPORT[3..].to_vec();
    files[1].0 = "sample/echo/api/test/probe.dever";
    files.extend([
        ("user/account/model.dever", r#"global type Account { email: Text(1, 254) unique }
seed { { email = "owner" }
  { email = "reader" } }
"#),
        ("platform/tenant/model.dever", r#"global type Tenant { name: Text(1, 64) unique }
seed { { name = "one" }
  { name = "two" } }
"#),
        ("platform/tenant/app.dever", r#"resolve(key: null) (id: model.id) { fail(dever.api.Error.Unauthorized) }
resolve(key: Text) (id: model.id) {
  found = model.first({ where = name == key })
  id = required(found)
}
required(value: null) (id: model.id) { fail(dever.api.Error.Unauthorized) }
required(value: model.Tenant) (id: model.id) { id = value.id }
"#),
        ("user/account/app.dever", r#"type Identity {
  id: Text
  user_id: model.id?
  tenant_id: platform.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  domain.active(claims.session == "active")
  found = model.first({ where = email == claims.subject })
  identity = Identity { id = claims.subject
    user_id = required(found)
    tenant_id = platform.tenant.resolve(claims.tenant) }
}
required(value: null) (id: model.id) { fail(dever.api.Error.Unauthorized) }
required(value: model.Account) (id: model.id) { id = value.id }
"#),
        ("user/account/domain.dever", "active(value: true) () {}\nactive(value: false) () { fail(dever.api.Error.Unauthorized) }"),
        ("news/article/model.dever", r#"type Article {
  private owner author_id: user.account.model.id = dever.auth.user_id()
  name: Text(1, 32) unique create = domain.normalize(input) replace = domain.normalize(input) search = domain.normalize(input)
  note: Text?
  private created_by: Text create = dever.auth.id()
  private updated_by: Text? replace = dever.auth.id()
}
"#),
        ("news/article/domain.dever", "normalize(value: Text) (normalized: Text) pure { normalized = text.trim(value) }"),
        ("news/article/app.dever", r#"type IdentityView {
  id: Text
  user_id: Int?
  tenant_id: Int?
  session: Text
  site: Text
  request_id: Text
}
identity() (view: IdentityView) {
  dever.task.sleep(1)
  view = IdentityView { id = dever.auth.id()
    user_id = dever.auth.user_id()
    tenant_id = dever.auth.tenant_id()
    session = dever.auth.session()
    site = dever.site.key()
    request_id = dever.api.request_id() }
}
permissions() (values: List<dever.auth.Permission>) { values = dever.auth.permissions() }
role(id: Text, keys: List<Text>) (saved: Bool) { dever.auth.save_role(id, id, false, keys)
  saved = true }
grant(user_id: Int, role_id: Text) (saved: Bool) { dever.auth.grant_role(user_id, role_id)
  saved = true }
revoke(user_id: Int, role_id: Text) (saved: Bool) { dever.auth.revoke_role(user_id, role_id)
  saved = true }
disable(role_id: Text) (saved: Bool) { dever.auth.disable_role(role_id)
  saved = true }
rollback(id: model.id) (value: Text) {
  changed = model.update(id, { name = "rolled-back" })
  dever.api.set_header("x-staged", "discarded")
  fail(dever.api.Error.Forbidden)
}
cookies() (value: Text) {
  dever.api.set_cookie("theme", "dark", dever.api.secure_cookie_options())
  dever.api.set_secret_cookie("copy", dever.auth.issue(dever.auth.id(), dever.auth.session(), null), dever.api.secure_cookie_options())
  value = "issued"
}
"#),
        ("news/article/api/admin/manage.dever", "rest model\nget identity = app.identity\nget permissions = app.permissions\npost role = app.role\npost grant = app.grant\npost revoke = app.revoke\npost disable = app.disable\npost rollback = app.rollback\nget cookies = app.cookies"),
        ("news/article/api/front/browse.dever", "get identity = app.identity"),
    ]);
    files
}

pub(super) fn configuration() -> Value {
    let mut value: Value = serde_json::from_str(&settings(8080)).unwrap();
    value["database"]["default"]["tenant_directory"] = "data/tenants".into();
    value["database"]["default"]["max_page_size"] = 4.into();
    value["tenant"] = json!({"database": "default", "max_pools": 2, "idle_timeout_ms": 1000});
    value["sites"]["test"]["path"] = "test".into();
    for site in ["admin", "front"] {
        value["sites"][site] = json!({"path": site, "auth": "session", "hosts": ["localhost"], "origin": "https://localhost"});
    }
    value
}

pub(super) fn tenant_bootstrap() -> Vec<Vec<String>> {
    [
        vec!["--dever-tenant-migrate", "1"],
        vec!["--dever-tenant-migrate", "2"],
        vec!["--dever-tenant-owner", "1", "admin", "1"],
        vec!["--dever-tenant-owner", "2", "admin", "1"],
    ]
    .map(|args| args.into_iter().map(str::to_owned).collect())
    .into()
}

/// Mint credentials through the existing approved Secret cookie sink, never reveal Secret.
pub(super) fn credential(
    settings: &Settings,
    site: &str,
    subject: &str,
    session: &str,
    tenant: &str,
) -> String {
    task::run_entry(async {
        let request = Request {
            method: "POST".into(),
            target: "/fixture/login".into(),
            headers: vec![
                Header {
                    name: "host".into(),
                    value: Bytes::from_text("localhost"),
                },
                Header {
                    name: "origin".into(),
                    value: Bytes::from_text("https://localhost"),
                },
            ],
            body: Bytes::new(vec![]),
        };
        let response = api::scoped_request(request, |request| async move {
            let prepared = auth::prepare_with_settings(settings, &[site], &request, true)
                .map_err(|error| error.to_string())?;
            auth::scope(prepared, None, async {
                auth::issue_cookie(subject, session, Some(tenant))?;
                api::commit_response_metadata()?;
                Ok(api::success(wire::Encoded::null()))
            })
            .await
            .map_err(|error| error.to_string())?
        })
        .await?;
        let cookie = response
            .headers
            .iter()
            .find(|header| header.name == "set-cookie")
            .ok_or("missing fixture credential")?;
        let cookie =
            std::str::from_utf8(cookie.value.values()).map_err(|error| error.to_string())?;
        assert!(cookie.contains("Secure") && cookie.contains("HttpOnly"));
        Ok(cookie.split(';').next().unwrap().to_owned())
    })
    .unwrap()
}

pub(super) fn request(
    server: &Server,
    method: &str,
    path: &str,
    cookie: &str,
    body: Value,
    expected: u16,
) -> Value {
    let headers = [
        ("cookie", cookie),
        ("origin", "https://localhost"),
        ("content-type", "application/json"),
    ];
    let body = if method == "GET" || method == "DELETE" {
        String::new()
    } else {
        body.to_string()
    };
    let response = server.request(method, path, &headers, &body).unwrap();
    assert_eq!(
        response.status,
        expected,
        "{method} {path}: {}\n{}",
        response.body,
        server.stderr()
    );
    response.body["data"].clone()
}

#[test]
fn trusted_identity_and_rest_objects_cover_all_six_targets() {
    let ir = application_ir(&files(), &configuration().to_string(), false, None);
    for target in Target::ALL {
        assert!(!emit_object(&ir, target).unwrap().is_empty());
    }
}

#[test]
#[ignore = "requires explicit runtime-api/sqlite archive; uses only owned loopback and two owned tenant files"]
fn private_rest_isolates_tenants_roles_identity_and_transactions() {
    exercise_private_rest(configuration(), 64);
}

fn exercise_private_rest(configuration: Value, iterations: usize) {
    let commands = tenant_bootstrap();
    let mut server = Server::start_with(&files(), iterations, configuration, &commands);
    let settings = Settings::load_project(server.directory.path()).unwrap();
    let owner_one = credential(&settings, "admin", "owner", "active", "one");
    let owner_two = credential(&settings, "admin", "owner", "active", "two");
    let reader = credential(&settings, "admin", "reader", "active", "one");
    let front = credential(&settings, "front", "owner", "active", "one");
    let unknown = credential(&settings, "admin", "unknown", "active", "one");
    let stale = credential(&settings, "admin", "owner", "stale", "one");
    let path = "/news/article/admin/manage";
    for cycle in 1..=iterations + 1 {
        server.ready_at("/sample/echo/test/probe/increment?q=1");
        assert_eq!(server.request("GET", path, &[], "").unwrap().status, 401);
        for cookie in [&unknown, &stale, &front] {
            request(&server, "GET", path, cookie, Value::Null, 401);
        }
        request(&server, "GET", path, &reader, Value::Null, 403);
        for invalid in ["?unknown=1", "?page=0", "?size=5", "?name="] {
            request(
                &server,
                "GET",
                &format!("{path}{invalid}"),
                &owner_one,
                Value::Null,
                400,
            );
        }
        request(
            &server,
            "POST",
            path,
            &owner_one,
            json!({"name": null}),
            400,
        );
        let keys = request(
            &server,
            "GET",
            &format!("{path}/permissions"),
            &owner_one,
            Value::Null,
            200,
        );
        assert!(
            keys.as_array()
                .unwrap()
                .iter()
                .all(|permission| permission["site"] != "test")
        );
        request(
            &server,
            "POST",
            &format!("{path}/role"),
            &owner_one,
            json!({"id": "reader", "keys": ["news.article.admin.read", "news.article.admin.identity"]}),
            200,
        );
        request(
            &server,
            "POST",
            &format!("{path}/grant"),
            &owner_one,
            json!({"user_id": 2, "role_id": "reader"}),
            200,
        );
        request(
            &server,
            "GET",
            &format!("{path}?size=4"),
            &reader,
            Value::Null,
            200,
        );
        request(
            &server,
            "POST",
            path,
            &reader,
            json!({"name": "blocked"}),
            403,
        );
        request(
            &server,
            "POST",
            &format!("{path}/role"),
            &owner_one,
            json!({"id": "writer", "keys": ["news.article.admin.create"]}),
            200,
        );
        request(
            &server,
            "POST",
            &format!("{path}/grant"),
            &owner_one,
            json!({"user_id": 2, "role_id": "writer"}),
            200,
        );
        let own = request(
            &server,
            "POST",
            path,
            &reader,
            json!({"name": " reader ", "note": null}),
            200,
        );
        assert_eq!(own["name"], "reader");
        assert!(own.get("author_id").is_none() && own.get("created_by").is_none());
        let owner = request(
            &server,
            "POST",
            path,
            &owner_one,
            json!({"name": " shared ", "note": "one"}),
            200,
        );
        let second = request(
            &server,
            "POST",
            path,
            &owner_two,
            json!({"name": "shared", "note": "two"}),
            200,
        );
        assert_eq!(owner["name"], "shared");
        request(
            &server,
            "GET",
            &format!("{path}?name=%20shared%20&size=4"),
            &owner_one,
            Value::Null,
            200,
        );
        let selected = request(
            &server,
            "GET",
            &format!("{path}?name=shared&size=4"),
            &owner_two,
            Value::Null,
            200,
        );
        assert_eq!(selected["total"], 1);
        assert_eq!(selected["items"][0]["note"], "two");
        let reader_page = request(
            &server,
            "GET",
            &format!("{path}?size=4"),
            &reader,
            Value::Null,
            200,
        );
        assert_eq!(reader_page["total"], 1);
        request(
            &server,
            "GET",
            &format!("{path}/{}", owner["id"]),
            &reader,
            Value::Null,
            404,
        );
        request(
            &server,
            "GET",
            &format!("{path}/bad-id"),
            &reader,
            Value::Null,
            404,
        );
        let replaced = request(
            &server,
            "PUT",
            &format!("{path}/{}", owner["id"]),
            &owner_one,
            json!({"name": " renamed ", "note": null}),
            200,
        );
        assert_eq!(replaced["name"], "renamed");
        assert_eq!(replaced["note"], Value::Null);
        request(
            &server,
            "PUT",
            &format!("{path}/{}", owner["id"]),
            &owner_one,
            json!({"name": ""}),
            400,
        );
        request(
            &server,
            "POST",
            &format!("{path}/rollback"),
            &owner_one,
            json!({"id": owner["id"]}),
            403,
        );
        let persisted = request(
            &server,
            "GET",
            &format!("{path}?size=4"),
            &owner_one,
            Value::Null,
            200,
        );
        assert_eq!(persisted["total"], 1);
        assert_eq!(persisted["items"][0]["name"], "renamed");
        let identity = request(
            &server,
            "GET",
            &format!("{path}/identity"),
            &reader,
            Value::Null,
            200,
        );
        assert_eq!(identity["id"], "reader");
        assert_eq!(identity["user_id"], 2);
        assert_eq!(identity["tenant_id"], 1);
        assert_eq!(identity["site"], "admin");
        std::thread::scope(|scope| {
            let server = &server;
            let pending = [
                (&owner_one, "owner", 1, 1),
                (&owner_two, "owner", 1, 2),
                (&reader, "reader", 2, 1),
            ]
            .map(|(cookie, subject, user, tenant)| {
                scope.spawn(move || {
                    let view = request(
                        server,
                        "GET",
                        &format!("{path}/identity"),
                        cookie,
                        Value::Null,
                        200,
                    );
                    assert_eq!(view["id"], subject);
                    assert_eq!(view["user_id"], user);
                    assert_eq!(view["tenant_id"], tenant);
                    assert_eq!(view["session"], "active");
                    assert_eq!(view["site"], "admin");
                    view["request_id"].as_str().unwrap().to_owned()
                })
            });
            let ids = pending.map(|task| task.join().unwrap());
            assert_eq!(
                ids.into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                3
            );
        });
        let cookies = server
            .request(
                "GET",
                &format!("{path}/cookies"),
                &[("cookie", &owner_one), ("origin", "https://localhost")],
                "",
            )
            .unwrap();
        assert_eq!(cookies.status, 200);
        let issued = cookies
            .headers
            .iter()
            .filter(|(name, _)| name == "set-cookie")
            .collect::<Vec<_>>();
        assert_eq!(issued.len(), 2);
        assert!(
            issued
                .iter()
                .all(|(_, value)| value.contains("Secure") && value.contains("HttpOnly"))
        );
        assert!(
            issued
                .iter()
                .any(|(_, value)| value.starts_with("theme=dark;"))
        );
        assert!(issued.iter().any(|(_, value)| value.starts_with("copy=")));
        request(
            &server,
            "GET",
            "/news/article/front/browse/identity",
            &front,
            Value::Null,
            403,
        );
        request(
            &server,
            "POST",
            &format!("{path}/revoke"),
            &owner_one,
            json!({"user_id": 2, "role_id": "writer"}),
            200,
        );
        request(
            &server,
            "POST",
            path,
            &reader,
            json!({"name": "revoked"}),
            403,
        );
        // Saving a disabled role deliberately never re-enables it. Revoke reusable
        // membership between roots; test permanent role disable on the final root.
        let (action, body) = if cycle == iterations + 1 {
            ("disable", json!({"role_id": "reader"}))
        } else {
            ("revoke", json!({"user_id": 2, "role_id": "reader"}))
        };
        request(
            &server,
            "POST",
            &format!("{path}/{action}"),
            &owner_one,
            body,
            200,
        );
        request(
            &server,
            "GET",
            &format!("{path}?size=4"),
            &reader,
            Value::Null,
            403,
        );
        for (cookie, value) in [(&owner_one, &owner), (&owner_two, &second)] {
            request(
                &server,
                "DELETE",
                &format!("{path}/{}", value["id"]),
                cookie,
                Value::Null,
                200,
            );
        }
        // Owner filtering is deliberate; remove the reader's row through a new exact role.
        request(
            &server,
            "POST",
            &format!("{path}/role"),
            &owner_one,
            json!({"id": "cleaner", "keys": ["news.article.admin.delete"]}),
            200,
        );
        request(
            &server,
            "POST",
            &format!("{path}/grant"),
            &owner_one,
            json!({"user_id": 2, "role_id": "cleaner"}),
            200,
        );
        request(
            &server,
            "DELETE",
            &format!("{path}/{}", own["id"]),
            &reader,
            Value::Null,
            200,
        );
        request(
            &server,
            "POST",
            &format!("{path}/revoke"),
            &owner_one,
            json!({"user_id": 2, "role_id": "cleaner"}),
            200,
        );
        server.stop_cycle(cycle);
    }
    server.finish();
}

#[cfg(feature = "runtime-postgres")]
#[test]
#[ignore = "requires explicit PostgreSQL fixture in config/setting.json; owns control and two tenant databases"]
fn postgres_private_rest_uses_the_same_identity_permission_and_transaction_contract() {
    super::postgres::run_in_isolated_databases("llvm_api", |fixture| {
        let mut configuration = configuration();
        configuration["database"]["default"] = fixture.database_setting();
        configuration["database"]["default"]["max_page_size"] = 4.into();
        exercise_private_rest(configuration, 2);
        Ok(())
    });
}

#[test]
#[ignore = "requires explicit runtime-api/sqlite archive; owns all listener, component and tenant state"]
fn disabled_component_cannot_be_bypassed_by_tenant_owner() {
    let mut commands = tenant_bootstrap();
    for state in ["disable", "enable", "disable"] {
        commands.push(
            ["--dever-tenant-component", "1", state, "news"]
                .map(str::to_owned)
                .into(),
        );
    }
    let mut server = Server::start_with(&files(), 64, configuration(), &commands);
    let settings = Settings::load_project(server.directory.path()).unwrap();
    let disabled = credential(&settings, "admin", "owner", "active", "one");
    let allowed = credential(&settings, "admin", "owner", "active", "two");
    for cycle in 1..=65 {
        server.ready_at("/sample/echo/test/probe/increment?q=1");
        request(
            &server,
            "GET",
            "/news/article/admin/manage?size=4",
            &disabled,
            Value::Null,
            403,
        );
        request(
            &server,
            "POST",
            "/news/article/admin/manage",
            &disabled,
            json!({"name": "blocked"}),
            403,
        );
        let page = request(
            &server,
            "GET",
            "/news/article/admin/manage?size=4",
            &allowed,
            Value::Null,
            200,
        );
        assert_eq!(page["total"], 0);
        server.stop_cycle(cycle);
    }
    server.finish();
}

//! HTTP enqueues trusted references; the later worker must recheck current grants.
use super::{
    native::{Server, application_ir},
    process, security, sqlite_inspect,
};
use dever_backend_bridge::{Target, emit_object};
use dever_runtime::{config::Settings, orm::Value as SqlValue};
use serde_json::{Value, json};
use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn files() -> Vec<(String, String)> {
    let mut files = security::files()
        .into_iter()
        .map(|(path, text)| (path.to_owned(), text.to_owned()))
        .collect::<Vec<_>>();
    files.push((
        "news/article/model/delivery.dever".into(),
        "type Delivery { name: Text }".into(),
    ));
    files
        .iter_mut()
        .find(|(path, _)| path == "news/article/app.dever")
        .unwrap()
        .1
        .push_str(
            r#"
type Delivery { name: Text }
submit(name: Text) (id: Id) {
  id = dever.job.enqueue(job.publish, Delivery { name = name }, name)
}
deliver(input: Delivery) () {
  saved = model.delivery.create({ name = input.name })
}
"#,
        );
    files
        .iter_mut()
        .find(|(path, _)| path == "news/article/api/admin/manage.dever")
        .unwrap()
        .1
        .push_str("\npost submit = app.submit");
    files.push(("news/article/job.dever".into(), "database default\njob publish(input: app.Delivery) () retry(2) timeout(1000) { app.deliver(input) }".into()));
    files
}

fn configuration() -> Value {
    let mut value = security::configuration();
    value["database"]["default"]["max_connections"] = 3.into();
    value["job"] = json!({"workers": 2, "poll_ms": 10, "lease_ms": 3000, "retry_base_ms": 20, "retry_max_ms": 100});
    value
}

#[test]
fn job_application_auth_and_tenant_roots_emit_all_six_targets() {
    let files = files();
    let files = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let ir = application_ir(&files, &configuration().to_string(), false, None);
    for target in Target::ALL {
        assert!(!emit_object(&ir, target).unwrap().is_empty());
    }
}

fn queue_settled(server: &Server, tenant: i64) {
    let path = server
        .directory
        .path()
        .join(format!("data/tenants/tenant_{tenant}.db"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = sqlite_inspect::query(
            &path,
            "SELECT COUNT(*) FROM _dever_jobs WHERE state IN ('pending','running')",
        );
        if rows == [vec![SqlValue::Int(0)]] {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "tenant {tenant} jobs did not settle: {rows:?}\n{}",
            server.stderr()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires explicit runtime archive; owned loopback API/worker and two tenant SQLite databases"]
fn queued_users_recheck_roles_sessions_catalog_and_tenant_components() {
    let owned_files = files();
    let files = owned_files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let mut server = Server::start_with(&files, 0, configuration(), &security::tenant_bootstrap());
    server.ready_at("/sample/echo/test/probe/increment?q=1");
    let settings = Settings::load_project(server.directory.path()).unwrap();
    let owner_one = security::credential(&settings, "admin", "owner", "active", "one");
    let owner_two = security::credential(&settings, "admin", "owner", "active", "two");
    let reader = security::credential(&settings, "admin", "reader", "active", "one");
    let manage = "/news/article/admin/manage";
    security::request(
        &server,
        "POST",
        &format!("{manage}/role"),
        &owner_one,
        json!({"id": "publisher", "keys": ["news.article.admin.submit"]}),
        200,
    );
    security::request(
        &server,
        "POST",
        &format!("{manage}/grant"),
        &owner_one,
        json!({"user_id": 2, "role_id": "publisher"}),
        200,
    );
    for (name, cookie) in [
        ("allowed-one", &owner_one),
        ("allowed-two", &owner_two),
        ("revoked", &reader),
        ("stale-session", &owner_one),
        ("removed-permission", &owner_one),
        ("missing-subject", &owner_one),
        ("wrong-tenant", &owner_one),
    ] {
        security::request(
            &server,
            "POST",
            &format!("{manage}/submit"),
            cookie,
            json!({"name": name}),
            200,
        );
    }
    security::request(
        &server,
        "POST",
        &format!("{manage}/revoke"),
        &owner_one,
        json!({"user_id": 2, "role_id": "publisher"}),
        200,
    );
    server.stop_cycle(1);
    server.finish();

    let first = server.directory.path().join("data/tenants/tenant_1.db");
    let second = server.directory.path().join("data/tenants/tenant_2.db");
    assert_eq!(
        sqlite_inspect::query(
            &first,
            "SELECT DISTINCT auth_permission,execution_kind,state FROM _dever_jobs"
        ),
        [vec![
            SqlValue::Text("news.article.admin.submit".into()),
            SqlValue::Text("user".into()),
            SqlValue::Text("pending".into())
        ]]
    );
    // Keep stored references structurally valid so dispatch must reverify them.
    // Corrupt storage ownership is rejected earlier by the shared queue store.
    for sql in [
        "UPDATE _dever_jobs SET auth_session='stale' WHERE dedupe_key='business:stale-session'",
        "UPDATE _dever_jobs SET auth_permission='news.article.admin.removed' WHERE dedupe_key='business:removed-permission'",
        "UPDATE _dever_jobs SET auth_subject='deleted-account' WHERE dedupe_key='business:missing-subject'",
        "UPDATE _dever_jobs SET auth_tenant='two' WHERE dedupe_key='business:wrong-tenant'",
    ] {
        sqlite_inspect::execute(&first, sql);
    }

    server.restart("all");
    server.ready_at("/sample/echo/test/probe/increment?q=1");
    queue_settled(&server, 1);
    queue_settled(&server, 2);
    assert_eq!(
        sqlite_inspect::query(&first, "SELECT name FROM delivery"),
        [vec![SqlValue::Text("allowed-one".into())]]
    );
    assert_eq!(
        sqlite_inspect::query(&second, "SELECT name FROM delivery"),
        [vec![SqlValue::Text("allowed-two".into())]]
    );
    let blocked = sqlite_inspect::query(
        &first,
        "SELECT dedupe_key,state,attempt,error FROM _dever_jobs WHERE dedupe_key!='business:allowed-one' ORDER BY dedupe_key",
    );
    assert_eq!(blocked.len(), 5);
    assert!(
        blocked.iter().all(|row| row[1..]
            == [
                SqlValue::Text("blocked".into()),
                SqlValue::Int(1),
                SqlValue::Text("identity_rejected".into())
            ]),
        "{blocked:?}"
    );
    server.stop_cycle(1);
    server.finish();

    // A fresh API-only root enqueues while enabled. The same stored user must
    // be rejected after the platform disables that tenant's component.
    server.restart("api");
    server.ready_at("/sample/echo/test/probe/increment?q=1");
    security::request(
        &server,
        "POST",
        &format!("{manage}/submit"),
        &owner_one,
        json!({"name": "disabled"}),
        200,
    );
    server.stop_cycle(1);
    server.finish();
    let status = process::status(
        Command::new(server.directory.path().join("managed-kernel"))
            .env_clear()
            .args(["--dever-tenant-component", "1", "disable", "news"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(status.success());
    server.restart("worker");
    queue_settled(&server, 1);
    server.stop_cycle(1);
    server.finish();
    assert_eq!(
        sqlite_inspect::query(
            &first,
            "SELECT state,error FROM _dever_jobs WHERE dedupe_key='business:disabled'"
        ),
        [vec![
            SqlValue::Text("blocked".into()),
            SqlValue::Text("component_disabled".into())
        ]]
    );
    assert_eq!(
        sqlite_inspect::query(&first, "SELECT COUNT(*) FROM delivery"),
        [vec![SqlValue::Int(1)]]
    );
    assert!(
        !fs::read_to_string(server.directory.path().join("stderr"))
            .unwrap()
            .contains("panicked")
    );
}

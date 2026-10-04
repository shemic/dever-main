use dever_core::source::SourceMap;
use dever_runtime::api::{Inputs, QueryValue};
use dever_runtime::bytes::Bytes;
use dever_runtime::config::RuntimeProfile;
use dever_runtime::http::{Header, Request};
use std::fs;
use std::process::Command;

#[path = "support/temp.rs"]
mod temp;

fn source(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, text) in files {
        sources.add(*path, *text);
    }
    sources
}

#[test]
fn project_entry_collects_api_and_command_without_main() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "greet(name: Text) (value: Text) { value = text.trim(name) }",
        ),
        (
            "user/account/api.dever",
            "get greeting = app.greet\ncmd greeting = app.greet",
        ),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let code =
        dever_core::native::emit_project(&program, &sources, RuntimeProfile::default()).unwrap();
    assert!(code.contains("dever_runtime::api::serve(api_route).await"));
    assert!(code.contains("\"user.account.greeting\" => {"));
    assert!(code.contains("let service_start = args.is_empty()"));
    assert!(code.contains("cmd_dispatch(&args[0], &args[1]).await"));
    assert!(!code.contains("main.main"));
}

#[test]
fn project_entry_can_embed_verified_external_resources() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "greet() (value: Text) { value = \"ok\" }",
        ),
        ("user/account/api.dever", "cmd greeting = app.greet"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let code = dever_core::native::emit_project_with_resources(
        &program,
        &sources,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[dever_core::native::EmbeddedResource {
            path: "worker.bin".into(),
            bytes: b"worker".to_vec(),
            sha256: "87eba76e7f3164534045ba922e7770fb58bbd14ad732bbf5ba6f11cc56989e6e".into(),
            executable: true,
        }],
    )
    .unwrap();
    assert!(code.contains("dever_runtime::external::prepare"));
    assert!(code.contains("DEVER_EXTERNAL_RESOURCES"));
    assert!(code.contains("include_bytes!(\"external-resource-0.bin\")"));
}

#[test]
fn large_external_resources_do_not_expand_generated_source_and_reject_forged_digests() {
    use sha2::{Digest, Sha256};
    let sources = source(&[
        (
            "user/account/app.dever",
            "greet() (value: Text) { value = \"ok\" }",
        ),
        ("user/account/api.dever", "cmd greeting = app.greet"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let baseline =
        dever_core::native::emit_project(&program, &sources, RuntimeProfile::default()).unwrap();
    let bytes = vec![157; 1024 * 1024];
    let mut resource = dever_core::native::EmbeddedResource {
        path: "runtime.bin".into(),
        sha256: Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes,
        executable: true,
    };
    let code = dever_core::native::emit_project_with_resources(
        &program,
        &sources,
        RuntimeProfile::default(),
        std::slice::from_ref(&resource),
    )
    .unwrap();
    assert!(
        code.len().saturating_sub(baseline.len()) < 8 * 1024,
        "resource bytes must not become Rust literals"
    );
    assert!(code.contains("include_bytes!(\"external-resource-0.bin\")"));
    let mut copy = resource.clone();
    copy.path = "runtime-copy.bin".into();
    copy.executable = false;
    let shared = dever_core::native::emit_project_with_resources(
        &program,
        &sources,
        RuntimeProfile::default(),
        &[resource.clone(), copy],
    )
    .unwrap();
    assert_eq!(shared.matches("include_bytes!(").count(), 1);
    assert!(!shared.contains("external-resource-1.bin"));
    assert!(shared.contains("runtime-copy.bin"));
    assert_eq!(shared.matches("bytes: DEVER_EXTERNAL_BYTES_0").count(), 2);
    resource.bytes[512 * 1024] ^= 1;
    assert!(
        dever_core::native::emit_project_with_resources(
            &program,
            &sources,
            RuntimeProfile::default(),
            &[resource],
        )
        .unwrap_err()
        .contains("invalid digest")
    );
}

#[test]
fn api_and_command_inputs_share_strict_json_validation() {
    assert!(Inputs::from_json_object("{\"profile\":{\"name\":\"a\",\"name\":\"b\"}}").is_err());
    let mut inputs = Inputs::from_json_object("{}").unwrap();
    assert!(inputs.raw_json("required").is_err());
    assert_eq!(inputs.optional_raw_json("optional").unwrap(), None);
    assert!(inputs.json("required", QueryValue::Text).is_err());
    assert_eq!(
        inputs.optional_json("optional", QueryValue::Text).unwrap(),
        "null"
    );
    let request = Request {
        method: "PUT".into(),
        target: "/user/account/profile".into(),
        headers: vec![Header {
            name: "content-type".into(),
            value: Bytes::from_text("application/json"),
        }],
        body: Bytes::from_text("{\"count\":12}"),
    };
    let mut inputs = Inputs::from_request(&request).unwrap();
    assert_eq!(inputs.json("count", QueryValue::Int).unwrap(), "12");
    inputs.finish().unwrap();
}

#[test]
fn generated_write_entry_rejects_detached_concurrency() {
    let sources = source(&[
        (
            "content/article/model.dever",
            "type Article { title: Text(1, 32) }",
        ),
        (
            "content/article/app.dever",
            "write() () { article = model.create({ title = \"published\" }) }\npublish() (value: Text) { task = run(write())\nwait(task)\nvalue = \"done\" }",
        ),
        ("content/article/api.dever", "post publish = app.publish"),
    ]);
    let errors = dever_core::check(&sources).unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains("transaction cannot start concurrent work"),
        "{rendered}"
    );
}

#[test]
fn generated_write_entry_allows_structured_blocking_work() {
    let sources = source(&[
        (
            "user/account/model.dever",
            "type Account { email: Text(1, 254)\nprivate password_hash: Text(255) }",
        ),
        (
            "user/account/app.dever",
            r#"register(password: Secret) (id: model.id) {
  hash = blocking(dever.crypto.password_hash(password))
  id = persist(hash)
}
transaction persist(hash: Text) (id: model.id) {
  stored = model.create({
    email = "reader@example.com"
    password_hash = hash
  })
  id = stored.id
}"#,
        ),
        ("user/account/api.dever", "post register = app.register"),
    ]);
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let code =
        dever_core::native::emit_project(&program, &sources, RuntimeProfile::default()).unwrap();
    assert!(
        code.contains("transaction_database.begin().await"),
        "{code}"
    );
    assert!(code.contains("task::blocking_typed"), "{code}");
}

#[test]
fn http_request_capabilities_are_not_available_to_commands() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "current() (value: Text) { value = dever.api.request_id() }",
        ),
        ("user/account/api.dever", "cmd current = app.current"),
    ]);
    let errors = dever_core::check(&sources).unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains("HTTP request capabilities are not available from CMD or Job"),
        "{rendered}"
    );
}

#[test]
fn http_request_capabilities_cannot_escape_into_detached_tasks() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "read() (value: Text) { value = dever.api.request_id() }\ncurrent() (value: Text) { task = run(read())\nvalue = wait(task) }",
        ),
        ("user/account/api.dever", "get current = app.current"),
    ]);
    let errors = dever_core::check(&sources).unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(rendered.contains("HTTP request, identity, and site capabilities cannot be used inside detached or parallel work"), "{rendered}");
}

#[test]
fn generated_http_context_capabilities_compile_without_a_source_context_value() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "current() (value: Text) { value = dever.api.request_id()\ndever.api.set_header(\"x-request-id\", value)\noptions = dever.api.secure_cookie_options()\ndever.api.set_cookie(\"theme\", \"dark\", options) }",
        ),
        ("user/account/api.dever", "get current = app.current"),
    ]);
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native =
        dever_core::native::compile_project(&program, &sources, &rustc, RuntimeProfile::default())
            .unwrap();
    assert!(native.executable().is_file());
}

#[test]
fn generated_authenticated_route_compiles_with_safe_standard_error_mapping() {
    let directory = temp::TemporaryDirectory::new();
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(
        directory.path().join("config/setting.json"),
        r#"{
          "auth": {
            "providers": {
              "session": {
                "verify": "user.account.verify",
                "jwtSecret": "0123456789abcdef0123456789abcdef"
              }
            }
          },
          "sites": {
            "admin": {"path": "admin", "auth": "session"}
          }
        }"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(directory.path()).unwrap();
    let sources = source(&[
        (
            "user/account/model.dever",
            "global type Account { email: Text(1, 254) }",
        ),
        (
            "platform/tenant/model.dever",
            "global type Tenant { name: Text(1, 64) }",
        ),
        (
            "user/account/app.dever",
            r#"type Identity {
  id: Text
  user_id: user.account.model.id?
  tenant_id: platform.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity {
    id = claims.subject
    user_id = null
    tenant_id = null
  }
}
profile() (response: Text) {
  fail(dever.api.Error.Forbidden)
}
permissions() (response: List<dever.auth.Permission>) {
  response = dever.auth.permissions()
}
manage_role() (ok: Bool) {
  dever.auth.save_role("reader", "Reader", false, ["user.account.admin.profile"])
  dever.auth.grant_role(1, "reader")
  dever.auth.revoke_role(1, "reader")
  dever.auth.disable_role("reader")
  ok = true
}
type Work { value: Text }
submit() (id: Id) {
  id = dever.job.enqueue(job.deliver, Work { value = "queued" }, "profile")
}
deliver(input: Work) () {
  news.article.deliver(input.value)
}"#,
        ),
        (
            "user/account/api/admin/profile.dever",
            "get profile = app.profile\nget permissions = app.permissions\npost manage_role = app.manage_role\npost submit = app.submit",
        ),
        (
            "user/account/job.dever",
            "database default\njob deliver(input: app.Work) () retry(2) timeout(1000) { app.deliver(input) }",
        ),
        (
            "news/article/model.dever",
            "type Article { title: Text(1, 128) }",
        ),
        (
            "news/article/app.dever",
            "deliver(title: Text) () { stored = model.create({ title = title }) }",
        ),
        (
            "gateway/session/app.dever",
            "login() (response: Text) { token = dever.auth.issue(\"account:1\", \"session:1\", null)\noptions = dever.api.secure_cookie_options()\ndever.api.set_secret_cookie(\"cms_session\", token, options)\nresponse = \"ok\" }",
        ),
        (
            "gateway/session/api/admin/login.dever",
            "public post login = app.login",
        ),
    ]);
    let program = dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(generated.contains("dever_runtime::api::standard_error(403)"));
    assert!(generated.contains("dever_runtime::auth::validate_job_source(&provider, &site)"));
    assert!(generated.contains(
        "dever_runtime::auth::validate_job_identity(&verified_claims, &verified_identity)"
    ));
    assert!(generated.contains("dever_runtime::auth::store::authorize"));
    assert!(generated.contains("AUTHORIZATION_PERMISSIONS.iter().any"));
    assert!(generated.contains("permission_key"));
    assert!(generated.contains("let auth_claims ="));
    assert!(generated.contains("job_dispatch_verified(claim).await"));
    let dispatch = generated.split_once("async fn job_dispatch(").unwrap().1;
    let verify = dispatch.find("let identity:").unwrap();
    let permission = dispatch
        .find("AUTHORIZATION_PERMISSIONS.iter().any")
        .unwrap();
    let components = permission
        + dispatch[permission..]
            .find("job_require_components(&claim).await")
            .unwrap();
    let role = components
        + dispatch[components..]
            .find("dever_runtime::auth::store::authorize")
            .unwrap();
    assert!(verify < permission && permission < components && components < role);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile_project(
        &program,
        &sources,
        &rustc,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(native.executable().is_file());
}

#[test]
fn command_only_entry_requires_explicit_selection() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "greet() (value: Text) { value = \"ok\" }",
        ),
        ("user/account/api.dever", "cmd greeting = app.greet"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let code =
        dever_core::native::emit_project(&program, &sources, RuntimeProfile::default()).unwrap();
    assert!(code.contains("application has no service entry; select a CMD and JSON input"));
    assert!(code.contains("\"user.account.greeting\" => Some(false)"));
    assert!(!code.contains("dever_runtime::api::serve(api_route)"));
}

#[test]
fn application_build_rejects_unreachable_app_and_private_forwarder() {
    let sources = source(&[
        (
            "user/account/domain.dever",
            "base(value: Text) (result: Text) pure { result = text.trim(value) }\nalias(value: Text) (result: Text) pure { result = base(value) }",
        ),
        (
            "user/account/app.dever",
            "greet(value: Text) (result: Text) { result = domain.alias(value) }\nunused() (result: Text) { result = \"unused\" }",
        ),
        ("user/account/api.dever", "get greeting = app.greet"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let errors = program.application_errors().unwrap();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains("unreachable from API, CMD, or Job"),
        "{rendered}"
    );
    assert!(
        rendered.contains("only forwards unchanged inputs"),
        "{rendered}"
    );
}

#[test]
fn generated_api_and_command_binary_compiles() {
    let sources = source(&[
        (
            "user/account/app.dever",
            "greet(name: Text) (value: Text) { value = text.trim(name) }",
        ),
        (
            "user/account/api.dever",
            "get greeting = app.greet\npost greeting = app.greet\ncmd greeting = app.greet",
        ),
    ]);
    let program = dever_core::check(&sources).unwrap();
    assert!(program.application_errors().unwrap().is_empty());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let binary =
        dever_core::native::compile_project(&program, &sources, &rustc, RuntimeProfile::default())
            .unwrap();
    assert!(binary.executable().is_file());
    let directory = temp::TemporaryDirectory::new();
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(directory.path().join("config/setting.json"), "{}\n").unwrap();
    let executable = directory.path().join("app");
    binary.save(&executable).unwrap();
    let run = Command::new(&executable)
        .args(["user.account.greeting", r#"{"name":" Ada "}"#])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap().trim(),
        r#"{"code":0,"message":"ok","data":"Ada"}"#
    );
}

#[test]
fn generated_job_entry_wraps_model_writes_in_transaction() {
    let sources = source(&[
        (
            "content/article/model.dever",
            "type Article { title: Text(1, 32) }",
        ),
        (
            "content/article/app.dever",
            "apply() () { article = model.create({ title = \"published\" }) }",
        ),
        (
            "content/article/job.dever",
            "database default\njob publish() () retry(2) timeout(1000) { app.apply() }",
        ),
    ]);
    let program = dever_core::check(&sources).unwrap();
    assert!(program.application_errors().unwrap().is_empty());
    let code = dever_core::native::emit_project(
        &program,
        &sources,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(code.contains("job_serve().await"));
    assert!(code.contains("Some(&transaction)"));
    assert!(code.contains("transaction.commit().await"));
    assert!(code.contains("transaction.rollback().await"));
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let binary = dever_core::native::compile_project(
        &program,
        &sources,
        &rustc,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(binary.executable().is_file());
}

#[test]
fn postgres_profile_compiles_without_connecting() {
    let sources = source(&[
        (
            "content/article/model.dever",
            "global type Article { title: Text(1, 32) }",
        ),
        (
            "content/article/app.dever",
            "count() (total: Int) { total = model.count() }",
        ),
        ("content/article/api.dever", "cmd count = app.count"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    assert!(program.application_errors().unwrap().is_empty());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let binary = dever_core::native::compile_project(
        &program,
        &sources,
        &rustc,
        RuntimeProfile {
            sqlite: false,
            postgres: true,
        },
    )
    .unwrap();
    assert!(binary.executable().is_file());
}

#[cfg(feature = "sqlite")]
#[test]
fn generated_tenant_migration_is_explicit_idempotent_and_never_falls_back() {
    use dever_runtime::orm::Value;
    use dever_runtime::sqlite::Database;

    let directory = temp::TemporaryDirectory::new();
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(
        directory.path().join("config/setting.json"),
        r#"{
          "tenant": {
            "database": "default",
            "max_pools": 4,
            "idle_timeout_ms": 60000
          },
          "database": {
            "default": {
              "type": "sqlite",
              "path": "data/platform.db",
              "tenant_directory": "data/tenants",
              "max_connections": 2
            }
          }
        }"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(directory.path()).unwrap();
    let sources = source(&[
        (
            "content/article/model.dever",
            "type Article { title: Text(1, 160) }",
        ),
        (
            "content/article/app.dever",
            "count() (total: Int) { total = model.count() }",
        ),
        ("content/article/api.dever", "cmd count = app.count"),
    ]);
    let program = dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    assert!(program.application_errors().unwrap().is_empty());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let binary = dever_core::native::compile_project(
        &program,
        &sources,
        &rustc,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    let executable = directory.path().join("app");
    binary.save(&executable).unwrap();
    let tenant_database = directory.path().join("data/tenants/tenant_7.db");

    let unscoped = Command::new(&executable)
        .args(["content.article.count", "{}"])
        .output()
        .unwrap();
    assert!(!unscoped.status.success());
    assert!(
        String::from_utf8_lossy(&unscoped.stderr).contains("requires an explicit tenant"),
        "{}",
        String::from_utf8_lossy(&unscoped.stderr),
    );
    assert!(!tenant_database.exists());

    for _ in 0..2 {
        let migrated = Command::new(&executable)
            .args(["--dever-tenant-migrate", "7"])
            .output()
            .unwrap();
        assert!(
            migrated.status.success(),
            "{}",
            String::from_utf8_lossy(&migrated.stderr),
        );
    }
    assert!(tenant_database.is_file());

    let scoped = Command::new(&executable)
        .args(["--dever-tenant-cmd", "7", "content.article.count", "{}"])
        .output()
        .unwrap();
    assert!(
        scoped.status.success(),
        "{}",
        String::from_utf8_lossy(&scoped.stderr)
    );
    assert_eq!(
        String::from_utf8(scoped.stdout).unwrap().trim(),
        r#"{"code":0,"message":"ok","data":0}"#,
    );

    let disabled = Command::new(&executable)
        .args(["--dever-tenant-component", "7", "disable", "content"])
        .output()
        .unwrap();
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let blocked = Command::new(&executable)
        .args(["--dever-tenant-cmd", "7", "content.article.count", "{}"])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("tenant component 'content' is disabled"),
        "{}",
        String::from_utf8_lossy(&blocked.stderr),
    );
    let enabled = Command::new(&executable)
        .args(["--dever-tenant-component", "7", "enable", "content"])
        .output()
        .unwrap();
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    let restored = Command::new(&executable)
        .args(["--dever-tenant-cmd", "7", "content.article.count", "{}"])
        .output()
        .unwrap();
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let tenant = Database::open(&tenant_database, 1, 100).unwrap();
        let marker = tenant
            .query(
                "SELECT tenant_id FROM _dever_tenant_ready WHERE marker = 1",
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(marker[0].get(0).unwrap(), &Value::Int(7));
        tenant
            .query(
                "SELECT id, title, created_at FROM article LIMIT 1",
                Vec::new(),
            )
            .await
            .unwrap();

        let platform = Database::open(directory.path().join("data/platform.db"), 1, 100).unwrap();
        assert!(
            platform
                .query(
                    "SELECT id, title, created_at FROM article LIMIT 1",
                    Vec::new()
                )
                .await
                .is_err()
        );
    });
}

#[test]
fn generated_command_rolls_back_when_output_encoding_fails() {
    let sources = source(&[
        (
            "content/article/model.dever",
            "type Article { title: Text(1, 32) }",
        ),
        (
            "content/article/app.dever",
            "create() (article: model.Article) { article = model.create({ title = \"published\" }) }",
        ),
        ("content/article/api.dever", "cmd create = app.create"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let code = dever_core::native::emit_project(
        &program,
        &sources,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(code.contains("let encoded = match cmd_output_0_encode(&value)"));
    assert!(code.contains("Err(error) => { let rollback = transaction.rollback().await;"));
}

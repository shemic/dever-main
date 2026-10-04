mod support;

use dever_core::source::SourceMap;
use dever_runtime::{bytes::Bytes, crypto, secret::Secret};

fn sources(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, text) in files {
        sources.add(*path, *text);
    }
    sources
}

fn rejected(files: &[(&str, &str)], message: &str) {
    let sources = sources(files);
    let errors = dever_core::check(&sources).expect_err("must reject unsafe source");
    assert!(
        errors.iter().any(|error| error.message.contains(message)),
        "{}",
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    );
}

const ACCOUNT: &str = "type Account { email: Text\nprivate password_hash: Text(255) }";

fn checked_platform(sources: &SourceMap) -> dever_core::hir::Program {
    let root = support::temp::TemporaryDirectory::new();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/setting.json"),
        r#"{"database":{
          "default":{"type":"sqlite","path":"data/default.db"},
          "platform":{"type":"sqlite","path":"data/platform.db"}
        }}"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(root.path()).unwrap();
    let program = support::checked(sources);
    program.validate_database_settings(&settings).unwrap();
    program
}

#[test]
fn password_crypto_api_requires_a_short_explicit_write_transaction() {
    let account = format!("database platform\nglobal {ACCOUNT}");
    let sources = sources(&[
        ("user/account/model.dever", &account),
        (
            "user/account/app.dever",
            r#"
login(credential_id: model.id, password: Secret) (id: model.id) {
  stored = model.get(credential_id)
  valid = blocking(dever.crypto.password_verify(password, stored.password_hash))
  dever.api.set_header("x-login", "accepted")
  id = store(credential_id)
}
transaction store(credential_id: model.id) (id: model.id) {
  stored = model.get(credential_id)
  created = model.create({ email = stored.email
    password_hash = stored.password_hash })
  id = created.id
}
"#,
        ),
        (
            "user/account/api/front/account.dever",
            "public post login = app.login",
        ),
    ]);
    let program = checked_platform(&sources);
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(!generated.contains("begin_deferred"), "{generated}");
    assert_eq!(
        generated
            .matches("transaction_database.begin().await")
            .count(),
        1,
        "{generated}"
    );

    rejected(
        &[
            ("user/account/model.dever", &account),
            (
                "user/account/app.dever",
                r#"
login(credential_id: model.id, password: Secret) (id: model.id) {
  stored = model.get(credential_id)
  valid = blocking(dever.crypto.password_verify(password, stored.password_hash))
  created = model.create({ email = stored.email
    password_hash = stored.password_hash })
  id = created.id
}
"#,
            ),
            (
                "user/account/api/front/account.dever",
                "public post login = app.login",
            ),
        ],
        "explicit transaction function",
    );

    rejected(
        &[
            ("user/account/model.dever", &account),
            (
                "user/account/app.dever",
                r#"
transaction login(credential_id: model.id, password: Secret) (id: model.id) {
  stored = model.get(credential_id)
  valid = blocking(dever.crypto.password_verify(password, stored.password_hash))
  id = stored.id
}
"#,
            ),
        ],
        "cannot perform password hashing or verification",
    );
}

#[test]
fn ordinary_write_api_commits_before_publishing_response_metadata() {
    let sources = sources(&[
        ("user/account/model.dever", ACCOUNT),
        (
            "user/account/app.dever",
            r#"
register(email: Text, password_hash: Text) (id: model.id) {
  dever.api.set_header("x-register", "accepted")
  stored = model.create({ email = email
    password_hash = password_hash })
  id = stored.id
}
"#,
        ),
        ("user/account/api.dever", "post register = app.register"),
    ]);
    let program = support::checked(&sources);
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    let commit = generated
        .find("if let Err(error) = transaction.commit().await")
        .unwrap();
    let metadata = generated
        .find("dever_runtime::api::commit_response_metadata()")
        .unwrap();
    assert!(
        commit < metadata,
        "response metadata must be published only after the database commits"
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_work_window_does_not_hold_the_sqlite_writer_lock() {
    use std::sync::Arc;
    use std::time::Duration;

    use dever_runtime::orm::Value;
    use dever_runtime::sqlite::Database;

    let root = support::temp::TemporaryDirectory::new();
    let database = Arc::new(Database::open(root.path().join("login.db"), 2, 20).unwrap());
    database
        .execute_batch(
            "CREATE TABLE account(id INTEGER PRIMARY KEY,status TEXT NOT NULL);\
         CREATE TABLE session(id INTEGER PRIMARY KEY,account_id INTEGER NOT NULL);\
         CREATE TABLE platform_write(id INTEGER PRIMARY KEY,value INTEGER NOT NULL);\
         INSERT INTO account(id,status) VALUES(1,'active');\
         INSERT INTO platform_write(id,value) VALUES(1,0);"
                .into(),
        )
        .await
        .unwrap();

    let account = database
        .query(
            "SELECT status FROM account WHERE id=?1",
            vec![Value::Int(1)],
        )
        .await
        .unwrap();
    assert_eq!(account.len(), 1);

    let (started, waiting) = tokio::sync::oneshot::channel();
    let password_work = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        std::thread::sleep(Duration::from_millis(500));
        crypto::password_hash(&Secret::from_input(b"correct password".to_vec())).unwrap()
    });
    waiting.await.unwrap();

    tokio::time::timeout(Duration::from_millis(250), async {
        let transaction = database.begin().await.unwrap();
        transaction
            .execute(
                "UPDATE platform_write SET value=value+1 WHERE id=?1",
                vec![Value::Int(1)],
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    })
    .await
    .expect("unrelated write must not wait for password work");
    assert!(
        !password_work.is_finished(),
        "the write must finish during the password work window"
    );
    let _hash = password_work.await.unwrap();

    let transaction = database.begin().await.unwrap();
    let active = transaction
        .query(
            "SELECT status FROM account WHERE id=?1 AND status='active'",
            vec![Value::Int(1)],
        )
        .await
        .unwrap();
    assert_eq!(
        active.len(),
        1,
        "mutable account state is rechecked inside the short transaction"
    );
    transaction
        .execute(
            "INSERT INTO session(id,account_id) VALUES(?1,?2)",
            vec![Value::Int(1), Value::Int(1)],
        )
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    assert_eq!(
        database
            .query("SELECT id FROM session", vec![])
            .await
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn private_model_fields_are_app_owned_and_do_not_change_storage_schema() {
    let plain = sources(&[("user/account/model.dever", &ACCOUNT.replace("private ", ""))]);
    let private = sources(&[("user/account/model.dever", ACCOUNT)]);
    assert_eq!(
        support::checked(&plain).model_snapshot(),
        support::checked(&private).model_snapshot()
    );
    support::checked(&sources(&[
        ("user/account/model.dever", ACCOUNT),
        (
            "user/account/app.dever",
            r#"
register(password: Secret) (id: model.id) {
  hash = blocking(dever.crypto.password_hash(password))
  stored = model.create({ email = "a@example.com"
    password_hash = hash })
  changed = model.update(stored.id, { password_hash = hash })
  valid = blocking(dever.crypto.password_verify(password, stored.password_hash))
  id = stored.id
}
"#,
        ),
    ]));
    for caller in ["utility.dever", "other.dever"] {
        rejected(
            &[
                ("user/account/model.dever", ACCOUNT),
                (
                    caller,
                    "read(stored: user.account.model.Account) (hash: Text) { hash = stored.password_hash }",
                ),
            ],
            "private field",
        );
    }
    rejected(
        &[
            ("user/account/model.dever", ACCOUNT),
            (
                "utility.dever",
                "rewrite(stored: user.account.model.Account) () { stored.password_hash = \"replacement\" }",
            ),
        ],
        "private field",
    );
}

#[test]
fn private_models_cannot_cross_app_boundaries_or_render_nested_errors() {
    for declaration in [
        "leak() (stored: model.Account) { stored = model.get(1) }",
        "accept(stored: model.Account) () {}",
        "type Envelope { values: List<model.Account?> }\naccept(value: Envelope) () {}",
        "type Failure { error Failed(value: model.Account) }",
        "accept(page: model.Page) () {}",
        "accept(values: Related<List<model.Account>>) () {}",
    ] {
        rejected(
            &[
                ("user/account/model.dever", ACCOUNT),
                ("user/account/app.dever", declaration),
            ],
            "private Model",
        );
    }
}

#[test]
fn secrets_are_non_observable_at_every_container_and_error_boundary() {
    for ty in [
        "Secret",
        "Secret?",
        "List<Secret>",
        "Map<Text, Secret>",
        "Envelope",
    ] {
        let source = format!(
            "public type Envelope {{ value: Secret }}\npublic expose(value: {ty}) (out: {ty}) {{ out = value }}"
        );
        rejected(&[("utility.dever", &source)], "output Secret");
    }
    for source in [
        "type Failure { error Failed(value: Secret) }",
        "type Nested { value: List<Secret> }\ntype Failure { error Failed(value: Nested) }",
    ] {
        rejected(&[("utility.dever", source)], "error payload");
    }
    for (source, expected) in [
        (
            "compare(a: Secret, b: Secret) (same: Bool) { same = a == b }",
            "comparable",
        ),
        (
            "join(a: Secret) (value: Text) { value = a + \"x\" }",
            "numeric",
        ),
        ("bad(value: Map<Secret, Text>) () {}", "Map key"),
        ("main() () { value = secret(\"password\") }", "unknown"),
        (
            "log(value: Secret) () { dever.log.info(value, {}) }",
            "expected Text",
        ),
    ] {
        rejected(&[("utility.dever", source)], expected);
    }
    rejected(
        &[(
            "user/account/model.dever",
            "type Account { password: Secret }",
        )],
        "Model field",
    );
}

#[test]
fn public_post_does_not_guess_database_not_found_authentication_semantics() {
    let account = format!("database platform\nglobal {ACCOUNT}");
    let sources = sources(&[
        ("user/account/model.dever", &account),
        (
            "user/account/app.dever",
            "lookup(id: model.id) (email: Text) { stored = model.get(id)\nemail = stored.email }",
        ),
        (
            "user/account/api/front/account.dever",
            "public post lookup = app.lookup",
        ),
    ]);
    let program = checked_platform(&sources);
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(
        !generated.contains("api_anonymous_post_failure_response"),
        "{generated}"
    );
}

#[test]
fn removed_process_configuration_has_no_source_entry() {
    for function in [
        "dever.process.environment",
        "dever.system.process_environment",
    ] {
        support::rejected(
            &format!("public main() () {{ ignored = {function}(\"setting\") }}"),
            "unknown",
        );
    }
}

fn test_sources(test: &str) -> (support::temp::TemporaryDirectory, SourceMap) {
    let root = support::temp::TemporaryDirectory::new();
    std::fs::create_dir_all(root.path().join("module")).unwrap();
    std::fs::create_dir_all(root.path().join("test/user/account")).unwrap();
    std::fs::write(root.path().join("module/main.dever"), "main() () {}").unwrap();
    std::fs::write(
        root.path().join("test/user/account/credentials.dever"),
        test,
    )
    .unwrap();
    let sources =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    (root, sources)
}

#[test]
fn secret_fixture_authority_and_native_rendering_are_checked() {
    let (_root, sources) = test_sources(
        r#"
credentials() () {
  password = secret("test password")
  hash = dever.crypto.password_hash(password)
  assert(dever.crypto.password_verify(password, hash))
  assert(not dever.crypto.password_verify(secret("wrong"), hash))
}
"#,
    );
    let program = support::checked(&sources);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile_test_suite(&program, &sources, &rustc).unwrap();
    let output = std::process::Command::new(native.executable())
        .arg("0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    for test in [
        "credentials() () { assert_eq(secret(\"a\"), secret(\"a\")) }",
        "credentials() () {}\nsecret(value: Text) (out: Text) { out = value }",
    ] {
        let (_root, sources) = test_sources(test);
        assert!(dever_core::check(&sources).is_err());
    }
    let (_root, mut sources) = test_sources(
        "credentials() () {}\nread(stored: model.Account) (hash: Text) { hash = stored.password_hash }",
    );
    sources.add("user/account/model.dever", ACCOUNT);
    let errors = dever_core::check(&sources).expect_err("Test cannot read private Model fields");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("private field"))
    );
}

#[test]
fn model_privacy_follows_related_cycles_without_affecting_layout_validation() {
    rejected(
        &[
            (
                "user/account/model.dever",
                "type Account { private password_hash: Text\nfriend_id: user.account.model.id? }",
            ),
            (
                "user/account/app.dever",
                "accept(value: Related<model.Account>) () {}",
            ),
        ],
        "private Model",
    );
}

#[test]
fn crypto_known_answers_and_password_cost_limits() {
    let password = Secret::from_input(b"correct horse battery staple".to_vec());
    let hash = crypto::password_hash(&password).unwrap();
    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(crypto::password_verify(&password, &hash).unwrap());
    assert!(!crypto::password_verify(&Secret::from_input(b"wrong".to_vec()), &hash).unwrap());
    for bad in [
        "malformed".to_owned(),
        hash.replace("m=19456", "m=19457"),
        hash.replace("t=2", "t=3"),
        hash.replace("p=1", "p=2"),
        hash.replace("argon2id", "argon2i"),
        hash.replace("v=19", "v=16"),
    ] {
        assert_eq!(
            crypto::password_verify(&password, &bad).unwrap_err(),
            "invalid or unsupported password hash"
        );
    }
    assert_eq!(format!("{password:?}"), "Secret([REDACTED])");
    let abc = Bytes::from_text("abc");
    assert_eq!(
        crypto::sha256(&abc).values(),
        &[
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad
        ]
    );
    // RFC 4231 test case 1.
    let digest = crypto::hmac_sha256(
        &Secret::from_input(vec![0x0b; 20]),
        &Bytes::from_text("Hi There"),
    );
    assert_eq!(
        digest.values(),
        &[
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7
        ]
    );
    assert!(crypto::constant_time_eq(&digest, &digest));
    assert!(!crypto::constant_time_eq(&digest, &abc));
    let first = crypto::token(32).unwrap();
    let second = crypto::token(32).unwrap();
    assert!(!crypto::constant_time_eq(
        &crypto::hmac_sha256(&first, &abc),
        &crypto::hmac_sha256(&second, &abc)
    ));
    assert!(crypto::token(0).is_err());
    assert!(crypto::token(1025).is_err());
}

#[test]
fn native_crypto_and_direct_system_intrinsics_select_crypto_runtime() {
    let source = r#"public main() (length: Int, valid: Bool) {
  key = dever.crypto.token(32)
  hash = dever.crypto.password_hash(key)
  valid = dever.crypto.password_verify(key, hash)
  bytes = dever.system.crypto_sha256(dever.bytes.from_text("abc"))
  length = dever.bytes.length(bytes)
}"#;
    assert_eq!(support::stdout(source), "length = 32\nvalid = true\n");
    #[cfg(feature = "reference")]
    {
        let sources = support::sources(source);
        let program = support::checked(&sources);
        let output = dever_core::reference::evaluate(&program, "main.main").unwrap();
        assert!(output.fault.is_none());
        assert_eq!(output.stdout, "length = 32\nvalid = true\n");
    }
    assert_eq!(
        support::stdout(
            "public main() (length: Int) { length = dever.bytes.length(dever.system.crypto_sha256(dever.bytes.from_text(\"abc\"))) }"
        ),
        "length = 32\n"
    );
}

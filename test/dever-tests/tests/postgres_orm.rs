#[path = "support/orm_fixture.rs"]
mod orm_fixture;
#[path = "support/orm_postgres_migration.rs"]
mod orm_postgres_migration;
mod support;

use std::sync::Arc;
use std::time::Duration;

use dever_runtime::config::{PostgresTls, RuntimeProfile};
use dever_runtime::orm::{ErrorKind, Row, Value};
use dever_runtime::postgres::Database;
use orm_fixture::DatabaseState;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use support::postgres::open_database as database;

const CERTIFICATE: &[u8] = include_bytes!("../fixtures/tls/server.pem");
const PRIVATE_KEY: &[u8] = include_bytes!("../fixtures/tls/server-key.pem");
const FAST_WAIT: Duration = Duration::from_millis(100);
const FAST_IO: Duration = Duration::from_millis(100);
const NORMAL_IO: Duration = Duration::from_secs(5);

#[test]
fn shared_orm_fixture_compiles_for_postgres_without_a_database() {
    orm_fixture::compile(
        "app.user.main",
        RuntimeProfile {
            sqlite: false,
            postgres: true,
        },
    );
}

fn integer(row: &Row, index: usize) -> i64 {
    match row.get(index).unwrap() {
        Value::Int(value) => *value,
        value => panic!("expected Int at column {index}, got {value:?}"),
    }
}

async fn database_state(database: &Database) -> DatabaseState {
    let columns = database
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'user' ORDER BY ordinal_position",
            vec![],
        )
        .await
        .unwrap();
    let managed_index_count = database
        .query(
            "SELECT COUNT(*) FROM pg_indexes WHERE schemaname = current_schema() AND tablename = 'user' AND indexname LIKE '_dever_idx_%'",
            vec![],
        )
        .await
        .unwrap();
    let history = database
        .query(
            "SELECT kind, revision FROM _dever_history WHERE model = 'app.user.model' AND kind IN ('schema', 'seed') ORDER BY kind",
            vec![],
        )
        .await
        .unwrap();
    let users = database
        .query(
            "SELECT email, display_name, status, age, alias FROM \"user\" ORDER BY email",
            vec![],
        )
        .await
        .unwrap();
    orm_fixture::database_state(columns, managed_index_count, history, users)
}

async fn rejects_untrusted_system_tls() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let certificates = CertificateDer::pem_slice_iter(CERTIFICATE)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(PRIVATE_KEY).unwrap();
    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .unwrap(),
    );
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 8];
        socket.read_exact(&mut request).await.unwrap();
        assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
        socket.write_all(b"S").await.unwrap();
        assert!(TlsAcceptor::from(config).accept(socket).await.is_err());
    });
    let database = database(
        &format!("postgres://dever@127.0.0.1:{port}/dever"),
        PostgresTls::System,
        1,
        FAST_WAIT,
        Duration::from_secs(1),
    );
    let error = database.query("SELECT 1", vec![]).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Connection, "{error}");
    peer.await.unwrap();
}

async fn assert_pool_timeout_cancel_disconnect_and_shutdown(url: &str, tls: PostgresTls) {
    let pool = Arc::new(database(url, tls, 1, FAST_WAIT, NORMAL_IO));
    let transaction = pool.begin().await.unwrap();
    assert_eq!(
        pool.query("SELECT 1", vec![]).await.unwrap_err().kind(),
        ErrorKind::PoolExhausted
    );
    transaction.rollback().await.unwrap();
    assert_eq!(
        integer(&pool.query("SELECT 1", vec![]).await.unwrap()[0], 0),
        1
    );

    let timeout_pool = database(url, tls, 1, FAST_WAIT, FAST_IO);
    assert_eq!(
        timeout_pool
            .query("SELECT pg_sleep(1)", vec![])
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Timeout
    );
    assert_eq!(
        integer(&timeout_pool.query("SELECT 1", vec![]).await.unwrap()[0], 0),
        1
    );
    timeout_pool.close().await.unwrap();

    let cancel_pool = Arc::new(database(url, tls, 1, FAST_WAIT, NORMAL_IO));
    let pending = {
        let database = Arc::clone(&cancel_pool);
        tokio::spawn(async move { database.query("SELECT pg_sleep(5)", vec![]).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_eq!(
        integer(&cancel_pool.query("SELECT 1", vec![]).await.unwrap()[0], 0),
        1
    );
    cancel_pool.close().await.unwrap();

    let disconnected = Arc::new(database(url, tls, 1, FAST_WAIT, NORMAL_IO));
    let backend = integer(
        &disconnected
            .query("SELECT pg_backend_pid()", vec![])
            .await
            .unwrap()[0],
        0,
    );
    let pending = {
        let database = Arc::clone(&disconnected);
        tokio::spawn(async move { database.query("SELECT pg_sleep(5)", vec![]).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let administrator = database(url, tls, 1, FAST_WAIT, NORMAL_IO);
    administrator
        .query(
            "SELECT pg_terminate_backend($1::bigint::integer)",
            vec![Value::Int(backend)],
        )
        .await
        .unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap_err().kind(),
        ErrorKind::Connection
    );
    assert_eq!(
        integer(&disconnected.query("SELECT 1", vec![]).await.unwrap()[0], 0),
        1
    );
    administrator.close().await.unwrap();
    disconnected.close().await.unwrap();

    let draining = Arc::clone(&pool);
    let transaction = pool.begin().await.unwrap();
    let shutdown = tokio::spawn(async move { draining.close().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!shutdown.is_finished());
    transaction.rollback().await.unwrap();
    shutdown.await.unwrap().unwrap();
    assert_eq!(
        pool.query("SELECT 1", vec![]).await.unwrap_err().kind(),
        ErrorKind::Connection
    );
}

#[test]
#[ignore = "requires an isolated database configured in config/setting.json"]
fn postgres_orm_uses_the_configured_isolated_database() {
    support::postgres::run_in_isolated_schema("postgres_orm", |setting| async move {
        let scoped = setting.url();
        let tls = setting.tls();
        let profile = RuntimeProfile {
            sqlite: false,
            postgres: true,
        };
        let main = orm_fixture::compile("app.user.main", profile);
        let rollback = orm_fixture::compile("app.user.rollback", profile);
        assert_eq!(main.revisions, rollback.revisions);
        let application_setting = setting.settings_json();
        orm_fixture::configure(main.native.executable(), &application_setting);
        orm_fixture::configure(rollback.native.executable(), &application_setting);
        orm_fixture::assert_main_output(orm_fixture::run(main.native.executable()));

        let application = setting.open(2, Duration::from_secs(1), NORMAL_IO);
        let committed = database_state(&application).await;
        orm_fixture::assert_committed_state(&committed, &main.revisions);
        orm_postgres_migration::assert_named_data_migration(&application).await;

        let rollback_output = orm_fixture::run(rollback.native.executable());
        assert!(!rollback_output.status.success());
        let rollback_error = String::from_utf8(rollback_output.stderr).unwrap();
        assert!(
            rollback_error.contains("duplicate key value violates unique constraint"),
            "{rollback_error}"
        );
        assert_eq!(database_state(&application).await, committed);

        let stream_pool = setting.open(1, FAST_WAIT, NORMAL_IO);
        let stream_backend = integer(
            &stream_pool
                .query("SELECT pg_backend_pid()", vec![])
                .await
                .unwrap()[0],
            0,
        );
        let stream = stream_pool
            .stream("SELECT generate_series(1, 20)", vec![], 2)
            .await
            .unwrap();
        assert_eq!(integer(&stream.pull().await.unwrap().unwrap(), 0), 1);
        stream.close();
        let replacement_backend = integer(
            &stream_pool
                .query("SELECT pg_backend_pid()", vec![])
                .await
                .unwrap()[0],
            0,
        );
        assert_ne!(replacement_backend, stream_backend);
        stream_pool.close().await.unwrap();
        application.close().await.unwrap();

        assert_pool_timeout_cancel_disconnect_and_shutdown(scoped, tls).await;
        rejects_untrusted_system_tls().await;

        Ok(())
    });
}

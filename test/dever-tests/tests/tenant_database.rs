mod support;

use std::fs;
use std::process::Command;
use std::time::Duration;

use dever_runtime::config::{self, RuntimeProfile};
use dever_runtime::database::{self, Executor, ModelScope, Sql, StorageBinding};
use dever_runtime::orm::{self, ErrorKind, Value};

const SCHEMA: &str = "tenant-schema-v1";

#[test]
fn sqlite_tenant_databases_are_ready_bounded_and_transaction_isolated() {
    let executable = std::env::current_exe().unwrap();
    let directory = executable.parent().unwrap();
    if !directory.join("config/setting.json").is_file() {
        let project = support::temp::TemporaryDirectory::new();
        fs::create_dir(project.path().join("config")).unwrap();
        fs::write(
            project.path().join("config/setting.json"),
            r#"{
          "tenant": {
            "database": "default",
            "max_pools": 2,
            "idle_timeout_ms": 1000
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
        let child = project.path().join("tenant-database-case");
        fs::copy(executable, &child).unwrap();
        let output = Command::new(child)
            .args([
                "--exact",
                "sqlite_tenant_databases_are_ready_bounded_and_transaction_isolated",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        config::bootstrap(
            RuntimeProfile {
                sqlite: true,
                postgres: false,
            },
            &[],
            &[],
        )
        .unwrap();
        database::prepare().await.unwrap();
        dever_runtime::tenant::initialize(SCHEMA).await.unwrap();

        let tenant_binding = StorageBinding::new(None, "content", ModelScope::Tenant);
        let missing = match dever_runtime::tenant::scope(1, tenant_binding.database()).await {
            Ok(_) => panic!("an unprovisioned tenant must not resolve a database"),
            Err(error) => error,
        };
        assert_eq!(missing.kind(), ErrorKind::Migration);
        assert!(!directory.join("data/tenants/tenant_1.db").exists());

        for tenant_id in [1, 2, 3] {
            let database = dever_runtime::tenant::begin_migration("default", tenant_id, SCHEMA)
                .await
                .unwrap();
            database.execute(Sql {
                sqlite: "CREATE TABLE profile (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
                postgres: "CREATE TABLE profile (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
            }, Vec::new()).await.unwrap();
            dever_runtime::tenant::mark_database_ready(&database, tenant_id, SCHEMA)
                .await
                .unwrap();
            dever_runtime::tenant::finish_migration(tenant_id, SCHEMA)
                .await
                .unwrap();
        }

        let tenant_components = ["media", "news"];
        dever_runtime::tenant::require_components(1, &["news"], &tenant_components)
            .await
            .unwrap();
        dever_runtime::tenant::disable_component(1, "news", &tenant_components)
            .await
            .unwrap();
        let disabled = dever_runtime::tenant::require_components(
            1,
            &["media", "news", "news"],
            &tenant_components,
        )
        .await
        .unwrap_err();
        assert_eq!(disabled.kind(), ErrorKind::InvalidData);
        assert!(
            disabled
                .to_string()
                .contains("tenant component 'news' is disabled")
        );
        dever_runtime::tenant::require_components(1, &["media"], &tenant_components)
            .await
            .unwrap();

        for component in ["platform", "unknown"] {
            let error = dever_runtime::tenant::disable_component(1, component, &tenant_components)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert!(error.to_string().contains("is not a tenant component"));
        }

        let tenant_one = dever_runtime::tenant::scope(1, tenant_binding.database())
            .await
            .unwrap();
        tenant_one
            .execute(
                Sql {
                    sqlite: "INSERT INTO profile (id,name) VALUES (?1,?2)",
                    postgres: "INSERT INTO profile (id,name) VALUES ($1,$2)",
                },
                vec![Value::Int(1), Value::Text("one".into())],
            )
            .await
            .unwrap();

        let tenant_two = dever_runtime::tenant::scope(2, tenant_binding.database())
            .await
            .unwrap();
        tenant_two
            .execute(
                Sql {
                    sqlite: "INSERT INTO profile (id,name) VALUES (?1,?2)",
                    postgres: "INSERT INTO profile (id,name) VALUES ($1,$2)",
                },
                vec![Value::Int(1), Value::Text("two".into())],
            )
            .await
            .unwrap();
        let rows = tenant_two
            .query(
                Sql {
                    sqlite: "SELECT name FROM profile WHERE id = ?1",
                    postgres: "SELECT name FROM profile WHERE id = $1",
                },
                vec![Value::Int(1)],
            )
            .await
            .unwrap();
        assert_eq!(orm::text(rows[0].get(0).unwrap().clone()).unwrap(), "two");

        let rows = tenant_one
            .query(
                Sql {
                    sqlite: "SELECT name FROM profile WHERE id = ?1",
                    postgres: "SELECT name FROM profile WHERE id = $1",
                },
                vec![Value::Int(1)],
            )
            .await
            .unwrap();
        assert_eq!(orm::text(rows[0].get(0).unwrap().clone()).unwrap(), "one");
        assert!(
            tenant_one
                .query(
                    Sql {
                        sqlite: "SELECT component FROM _dever_tenant_component",
                        postgres: "SELECT component FROM _dever_tenant_component",
                    },
                    Vec::new()
                )
                .await
                .is_err()
        );

        dever_runtime::tenant::enable_component(1, "news", &tenant_components)
            .await
            .unwrap();
        dever_runtime::tenant::require_components(1, &["news"], &tenant_components)
            .await
            .unwrap();

        let transaction = tenant_one.begin().await.unwrap();
        let cross_tenant = match Executor::new(tenant_two.clone(), Some(&transaction)) {
            Ok(_) => panic!("one transaction must not cross tenant databases"),
            Err(error) => error,
        };
        assert_eq!(cross_tenant.kind(), ErrorKind::Database);
        transaction.rollback().await.unwrap();

        let pool_error = match dever_runtime::tenant::scope(3, tenant_binding.database()).await {
            Ok(_) => panic!("the tenant pool cap must be enforced"),
            Err(error) => error,
        };
        assert_eq!(pool_error.kind(), ErrorKind::PoolExhausted);
        drop(tenant_one);
        tokio::time::sleep(Duration::from_millis(1_050)).await;
        let tenant_three = dever_runtime::tenant::scope(3, tenant_binding.database())
            .await
            .unwrap();
        drop(tenant_three);

        let global = StorageBinding::new(None, "content", ModelScope::Global)
            .database()
            .await
            .unwrap();
        global.execute(Sql {
            sqlite: "CREATE TABLE platform_flag (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            postgres: "CREATE TABLE platform_flag (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
        }, Vec::new()).await.unwrap();
        global
            .execute(
                Sql {
                    sqlite: "INSERT INTO platform_flag (id,name) VALUES (?1,?2)",
                    postgres: "INSERT INTO platform_flag (id,name) VALUES ($1,$2)",
                },
                vec![Value::Int(1), Value::Text("shared".into())],
            )
            .await
            .unwrap();
        let global_from_other_tenant = dever_runtime::tenant::scope(
            2,
            StorageBinding::new(None, "content", ModelScope::Global).database(),
        )
        .await
        .unwrap();
        let rows = global_from_other_tenant
            .query(
                Sql {
                    sqlite: "SELECT name FROM platform_flag WHERE id = ?1",
                    postgres: "SELECT name FROM platform_flag WHERE id = $1",
                },
                vec![Value::Int(1)],
            )
            .await
            .unwrap();
        assert_eq!(
            orm::text(rows[0].get(0).unwrap().clone()).unwrap(),
            "shared"
        );

        database::shutdown().await.unwrap();
    });
}

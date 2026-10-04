mod support;

use std::fs;

use dever_runtime::config::{Database, LogLevel, PostgresTls, Settings};

const MIXED_SETTING: &str = r#"{
  "http": {},
  "performance": {
    "network_peer": "target/network_bench",
    "live_peer": "target/live_bench",
    "linux_ci": {"inputs": [], "required_inputs": {}, "author_outputs": []}
  },
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/cms.db",
      "max_connections": 1,
      "max_page_size": 100
    },
    "report": {
      "type": "postgres",
      "url": "postgres://example.invalid/report",
      "tls": "system",
      "min_connections": 0,
      "max_connections": 4,
      "max_page_size": 200
    }
  },
  "log": {}
}"#;

fn project(setting: &str) -> support::temp::TemporaryDirectory {
    let root = support::temp::TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/setting.json"), setting).unwrap();
    root
}

#[cfg(feature = "external")]
#[test]
fn worker_file_grants_are_explicit_and_cannot_write_executable_cache() {
    let setting = |path: &str, write: bool| {
        serde_json::json!({
            "adapter": {"sample.worker": {"files": {"content": {"path": path, "write": write}}}}
        })
        .to_string()
    };
    let capabilities = vec!["file".into()];
    let root = project(&setting("data/upload", true));
    let settings = Settings::load_project(root.path()).unwrap();
    let grants = settings
        .external_grants("sample.worker", &capabilities)
        .unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].source, root.path().join("data/upload"));
    assert_eq!(grants[0].destination, std::path::Path::new("/data/content"));
    assert!(grants[0].writable);
    assert!(
        settings
            .external_grants("sample.worker", &[])
            .unwrap_err()
            .contains("file capability")
    );
    for path in [
        "data",
        "data/cache",
        "data/cache/lib",
        "data/cache/lib/worker",
    ] {
        let root = project(&setting(path, true));
        let settings = Settings::load_project(root.path()).unwrap();
        assert!(
            settings
                .external_grants("sample.worker", &capabilities)
                .unwrap_err()
                .contains("executable resource cache")
        );
    }
    for path in ["../private", "/etc", ""] {
        let root = project(&setting(path, false));
        let settings = Settings::load_project(root.path()).unwrap();
        assert!(
            settings
                .external_grants("sample.worker", &capabilities)
                .unwrap_err()
                .contains("relative to the application")
        );
    }
}

#[test]
fn settings_resolve_connections_and_paths_from_the_application_directory() {
    let root = project(MIXED_SETTING);
    let settings = Settings::load_project(root.path()).unwrap();
    assert_eq!(settings.resolve_database(None, "cms").unwrap().0, "default");
    assert_eq!(
        settings.resolve_database(None, "report").unwrap().0,
        "report"
    );
    assert!(settings.resolve_database(Some("missing"), "cms").is_err());
    match settings.database("default").unwrap() {
        Database::Sqlite {
            path,
            max_connections,
            max_page_size,
            ..
        } => {
            assert_eq!(path, &root.path().join("data/db/cms.db"));
            assert_eq!((*max_connections, *max_page_size), (1, 100));
        }
        Database::Postgres { .. } => panic!("default must be SQLite"),
    }
    match settings.database("report").unwrap() {
        Database::Postgres {
            tls,
            wait_timeout_ms,
            io_timeout_ms,
            ..
        } => {
            assert_eq!(*tls, PostgresTls::System);
            assert_eq!((*wait_timeout_ms, *io_timeout_ms), (5_000, 30_000));
        }
        Database::Sqlite { .. } => panic!("report must be PostgreSQL"),
    }
    assert!(settings.profile().sqlite);
    assert!(settings.profile().postgres);
    assert!(
        settings
            .validate_transaction_bindings(&[&[(None, "cms"), (None, "news")]])
            .is_ok()
    );
    let error = settings
        .validate_transaction_bindings(&[&[(None, "cms"), (Some("report"), "cms")]])
        .unwrap_err();
    assert!(error.contains("multiple database connections"), "{error}");

    settings.initialize_data_directories().unwrap();
    for directory in ["db", "upload", "log", "cache", "tmp"] {
        assert!(root.path().join("data").join(directory).is_dir());
    }
}

#[test]
fn tenant_database_settings_use_only_controlled_physical_targets() {
    let root = project(
        r#"{
      "tenant": {
        "database": "default",
        "max_pools": 8,
        "idle_timeout_ms": 60000
      },
      "database": {
        "default": {
          "type": "sqlite",
          "path": "data/platform.db",
          "tenant_directory": "data/tenants"
        },
        "content": {
          "type": "postgres",
          "url": "postgres://example.invalid/platform",
          "tls": "disabled",
          "tenant_database_prefix": "cms_tenant"
        },
        "platform": {
          "type": "sqlite",
          "path": "data/platform_only.db"
        }
      }
    }"#,
    );
    let settings = Settings::load_project(root.path()).unwrap();
    let tenant = settings.tenant().unwrap();
    assert_eq!(tenant.database(), "default");
    assert_eq!(tenant.max_pools(), 8);
    assert_eq!(tenant.idle_timeout_ms(), 60_000);
    match settings.database("default").unwrap() {
        Database::Sqlite {
            tenant_directory, ..
        } => {
            assert_eq!(
                tenant_directory.as_ref().unwrap(),
                &root.path().join("data/tenants")
            );
        }
        Database::Postgres { .. } => panic!("default must be SQLite"),
    }
    match settings.database("content").unwrap() {
        Database::Postgres {
            tenant_database_prefix,
            ..
        } => {
            assert_eq!(tenant_database_prefix.as_deref(), Some("cms_tenant"));
        }
        Database::Sqlite { .. } => panic!("content must be PostgreSQL"),
    }

    settings
        .validate_scoped_database_bindings(
            &[(Some("content"), "news", true), (None, "platform", false)],
            &[],
        )
        .unwrap();
    let error = settings
        .validate_scoped_database_bindings(&[(Some("platform"), "news", true)], &[])
        .unwrap_err();
    assert!(error.contains("tenant_directory is required"), "{error}");

    let transaction = [
        (Some("default"), "news", false),
        (Some("default"), "news", true),
    ];
    let error = settings
        .validate_scoped_database_bindings(&[], &[&transaction])
        .unwrap_err();
    assert!(
        error.contains("cannot mix global and tenant Models"),
        "{error}"
    );
}

#[test]
fn final_connection_resolution_rejects_table_collisions_and_cross_database_foreign_keys() {
    let root = project(MIXED_SETTING);
    let settings = Settings::load_project(root.path()).unwrap();

    let mut collision_sources = dever_core::source::SourceMap::default();
    collision_sources.add(
        "user/model/account.dever",
        "public type Account { email: Text }",
    );
    collision_sources.add(
        "news/model/account.dever",
        "public type Account { slug: Text }",
    );
    let collision = dever_core::check(&collision_sources).unwrap();
    let error = collision.validate_database_settings(&settings).unwrap_err();
    assert!(error.contains("share table 'account'"), "{error}");

    let mut foreign_key_sources = dever_core::source::SourceMap::default();
    foreign_key_sources.add(
        "user/model/user.dever",
        "database report\npublic type User { email: Text }",
    );
    foreign_key_sources.add(
        "news/model/news.dever",
        "public type News { author_id: user.model.user.id }",
    );
    let foreign_key = dever_core::check(&foreign_key_sources).unwrap();
    let error = foreign_key
        .validate_database_settings(&settings)
        .unwrap_err();
    assert!(error.contains("crosses database connections"), "{error}");

    let long_field =
        "field_name_that_is_longer_than_postgresql_identifier_limit_of_sixty_three_bytes";
    let mut identifier_sources = dever_core::source::SourceMap::default();
    identifier_sources.add(
        "report/model/entry.dever",
        format!("database report\npublic type Entry {{ {long_field}: Text }}"),
    );
    let identifiers = dever_core::check(&identifier_sources).unwrap();
    let error = identifiers
        .validate_database_settings(&settings)
        .unwrap_err();
    assert!(error.contains("exceeds 63 bytes"), "{error}");
}

#[test]
fn settings_reject_unknown_fields_missing_default_and_escaping_paths() {
    for (setting, expected) in [
        (r#"{"linux_ci":{}}"#, "unknown field `linux_ci`"),
        (
            r#"{"database":{"default":{"type":"sqlite","path":"data/app.db","extra":true}}}"#,
            "unknown field",
        ),
        (r#"{"database":{}}"#, "database.default is required"),
        (
            r#"{"database":{"default":{"type":"sqlite","path":"../outside.db"}}}"#,
            "must stay below",
        ),
        (
            r#"{"database":{"default":{"type":"postgres","url":"postgres://example.invalid/app","tls":"prefer"}}}"#,
            "unknown variant",
        ),
        (
            r#"{"database":{"default":{"type":"sqlite","path":"data/app.db","max_connections":0}}}"#,
            "must be between",
        ),
        (
            r#"{"database":{"default":{"type":"postgres","url":"postgres://example.invalid/app","tls":"disabled","wait_timeout_ms":0}}}"#,
            "wait_timeout_ms must be between",
        ),
        (
            r#"{"database":{"default":{"type":"postgres","url":"postgres://example.invalid/app","tls":"disabled","io_timeout_ms":300001}}}"#,
            "io_timeout_ms must be between",
        ),
        (
            r#"{"database":{"default":{"type":"postgres","url":"postgres://example.invalid/app","tls":"disabled","tenant_database_prefix":"tenant_database_prefix_that_exceeds_forty_three_bytes"}}}"#,
            "at most 43 lower_snake_case bytes",
        ),
        (
            r#"{"database":{"default":{"type":"sqlite","path":"data/app.db","tenant_directory":"data/tenants"}}}"#,
            "require top-level tenant settings",
        ),
        (
            r#"{"tenant":{"database":"missing"},"database":{"default":{"type":"sqlite","path":"data/app.db","tenant_directory":"data/tenants"}}}"#,
            "unknown database",
        ),
        (
            r#"{"tenant":{"mode":"database","control_database":"default"},"database":{"default":{"type":"sqlite","path":"data/app.db","tenant_directory":"data/tenants"}}}"#,
            "unknown field",
        ),
        (
            r#"{"tenant":{"max_pools":8},"database":{"default":{"type":"sqlite","path":"data/app.db","tenant_directory":"data/tenants"}}}"#,
            "missing field `database`",
        ),
        (
            r#"{"tenant":{"database":"default","max_pools":0},"database":{"default":{"type":"sqlite","path":"data/app.db","tenant_directory":"data/tenants"}}}"#,
            "max_pools must be between",
        ),
    ] {
        let root = project(setting);
        let error = Settings::load_project(root.path())
            .err()
            .expect("invalid setting must fail");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn sites_map_api_parent_directories_to_strict_auth_providers() {
    let root = project(
        r#"{
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
        "admin": {"path": "admin", "auth": "session", "hosts": ["ADMIN.EXAMPLE.COM:443"], "origin": "https://admin.example.com"},
        "front": {"path": "front", "auth": "session", "origin": "https://front.example.com:8443"}
      },
      "log": {"level": "warn"}
    }"#,
    );
    let settings = Settings::load_project(root.path()).unwrap();
    let (key, site) = settings
        .site_for_directory(&["admin", "article"])
        .unwrap()
        .unwrap();
    assert_eq!(key, "admin");
    assert_eq!(site.path(), "admin");
    assert_eq!(site.auth(), "session");
    assert_eq!(site.hosts(), ["admin.example.com"]);
    let origin = site.origin().unwrap();
    assert_eq!(
        (origin.scheme(), origin.host(), origin.port()),
        ("https", "admin.example.com", 443)
    );
    let (_, front) = settings.site_for_directory(&["front"]).unwrap().unwrap();
    assert_eq!(front.hosts(), ["front.example.com"]);
    let origin = front.origin().unwrap();
    assert_eq!(
        (origin.scheme(), origin.host(), origin.port()),
        ("https", "front.example.com", 8443)
    );
    assert!(settings.site_for_directory(&[]).unwrap().is_none());
    assert!(
        settings
            .site_for_directory(&["administrator"])
            .unwrap()
            .is_none()
    );
    assert_eq!(
        settings.auth().provider("session").unwrap().verify(),
        "user.account.verify"
    );
    assert_eq!(settings.log().level, LogLevel::Warn);
}

#[test]
fn sites_reject_ambiguous_paths_unknown_providers_and_removed_app_settings() {
    for (setting, expected) in [
        (r#"{"app":{"name":"cms"}}"#, "unknown field"),
        (
            r#"{"sites":{"admin":{"path":"admin","auth":"missing"}}}"#,
            "unknown provider",
        ),
        (
            r#"{
              "auth":{"providers":{"session":{"verify":"user.account.verify","jwtSecret":"0123456789abcdef0123456789abcdef"}}},
              "sites":{
                "admin":{"path":"admin","auth":"session"},
                "nested":{"path":"admin/reports","auth":"session"}
              }
            }"#,
            "overlap",
        ),
        (
            r#"{
              "auth":{"providers":{"session":{"verify":"user.account.verify","jwtSecret":"0123456789abcdef0123456789abcdef"}}},
              "sites":{"front":{"path":"front","auth":"session","origin":"ftp://cms.example.com"}}
            }"#,
            "scheme must be http or https",
        ),
        (
            r#"{
              "auth":{"providers":{"session":{"verify":"user.account.verify","jwtSecret":"0123456789abcdef0123456789abcdef"}}},
              "sites":{"front":{"path":"front","auth":"session","hosts":["api.example.com"],"origin":"https://cms.example.com"}}
            }"#,
            "origin host must be present",
        ),
    ] {
        let root = project(setting);
        let error = Settings::load_project(root.path())
            .err()
            .expect("invalid setting must fail");
        assert!(error.contains(expected), "{error}");
    }
}

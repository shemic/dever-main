use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dever_runtime::config::{Database as DatabaseSetting, PostgresTls, Settings};
use dever_runtime::orm::Value;
use dever_runtime::postgres::{ConnectionOptions, Database};

const POSTGRES_TEST_CONNECTION: &str = "postgres_test";
const MAX_PAGE_SIZE: usize = 100;

static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);

pub struct IsolatedDatabase {
    document: serde_json::Value,
    url: String,
    tls: PostgresTls,
}

impl IsolatedDatabase {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn tls(&self) -> PostgresTls {
        self.tls
    }

    pub fn open(
        &self,
        max_connections: usize,
        wait_timeout: Duration,
        io_timeout: Duration,
    ) -> Database {
        open_database(
            &self.url,
            self.tls,
            max_connections,
            wait_timeout,
            io_timeout,
        )
    }

    pub fn settings_json(&self) -> String {
        let mut connection = self.document["database"][POSTGRES_TEST_CONNECTION].clone();
        connection["url"] = serde_json::Value::String(self.url.clone());
        let mut document = serde_json::Map::new();
        for name in ["http", "log"] {
            if let Some(value) = self.document.get(name) {
                document.insert(name.to_owned(), value.clone());
            }
        }
        if let Some(value) = self.document.get("tenant") {
            let mut tenant = value.clone();
            tenant["database"] = serde_json::Value::String("default".into());
            document.insert("tenant".into(), tenant);
        }
        document.insert(
            "database".into(),
            serde_json::json!({"default": connection}),
        );
        serde_json::to_string_pretty(&document).unwrap() + "\n"
    }
}

pub struct IsolatedTenantDatabases {
    database_setting: serde_json::Value,
}

impl IsolatedTenantDatabases {
    pub fn database_setting(&self) -> serde_json::Value {
        self.database_setting.clone()
    }
}

pub fn run_in_isolated_databases<F, T>(case_name: &str, test: F) -> T
where
    F: FnOnce(IsolatedTenantDatabases) -> Result<T, String>,
{
    let setting = load_setting(case_name);
    let suffix = unique_namespace();
    let control_case = format!("{case_name}_{suffix}");
    let template = setting.document["database"][POSTGRES_TEST_CONNECTION]["url"]
        .as_str()
        .unwrap();
    let control_url = case_url(template, &control_case);
    let control_name = database_name(&control_url);
    let tenant_prefix = format!("dever_t_{suffix}");
    assert!(control_name.len() <= 63 && valid_case_name(&control_name));
    assert!(tenant_prefix.len() <= 43 && valid_case_name(&tenant_prefix));

    let mut cleanup_required = false;
    let setup = catch_unwind(AssertUnwindSafe(|| {
        dever_runtime::task::run_entry(async {
            let administrator = open_database(
                &setting.url,
                setting.tls,
                1,
                Duration::from_secs(1),
                Duration::from_secs(5),
            );
            let result = async {
                let existing = database_names(&administrator).await?;
                if existing.iter().any(|name| {
                    name == &control_name || name.starts_with(&format!("{tenant_prefix}_"))
                }) {
                    return Err("isolated PostgreSQL database names already exist".into());
                }
                cleanup_required = true;
                administrator
                    .query_owned(format!("CREATE DATABASE \"{control_name}\""), vec![])
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
            .await;
            let close = administrator
                .close()
                .await
                .map_err(|error| error.to_string());
            result.and(close)
        })
    }));

    let mut database_setting = setting.document["database"][POSTGRES_TEST_CONNECTION].clone();
    database_setting["url"] = serde_json::Value::String(control_url);
    database_setting["tenant_database_prefix"] = serde_json::Value::String(tenant_prefix.clone());
    let result = match setup {
        Ok(Ok(())) => catch_unwind(AssertUnwindSafe(|| {
            test(IsolatedTenantDatabases { database_setting })
        })),
        Ok(Err(error)) => Ok(Err(error)),
        Err(panic) => Err(panic),
    };
    let cleanup = cleanup_required.then(|| {
        catch_unwind(AssertUnwindSafe(|| {
            dever_runtime::task::run_entry(cleanup_databases(
                &setting.url,
                setting.tls,
                &control_name,
                &tenant_prefix,
            ))
        }))
    });
    let cleanup_error = match cleanup {
        None | Some(Ok(Ok(()))) => None,
        Some(Ok(Err(error))) => Some(error),
        Some(Err(_)) => Some("PostgreSQL database cleanup panicked".into()),
    };
    match result {
        Ok(Ok(value)) => {
            if let Some(error) = cleanup_error {
                panic!("PostgreSQL database cleanup failed: {error}");
            }
            value
        }
        Ok(Err(error)) => match cleanup_error {
            Some(cleanup_error) => {
                panic!("{error}; PostgreSQL database cleanup failed: {cleanup_error}")
            }
            None => panic!("{error}"),
        },
        Err(panic) => {
            if let Some(error) = cleanup_error {
                eprintln!("PostgreSQL database cleanup failed after panic: {error}");
            }
            resume_unwind(panic);
        }
    }
}

async fn database_names(administrator: &Database) -> Result<Vec<String>, String> {
    let rows = administrator
        .query("SELECT datname::text FROM pg_database", vec![])
        .await
        .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(|row| match row.get(0).map_err(|error| error.to_string())? {
            Value::Text(name) => Ok(name.clone()),
            _ => Err("PostgreSQL database name was not Text".into()),
        })
        .collect()
}

async fn cleanup_databases(
    administrator_url: &str,
    tls: PostgresTls,
    control_name: &str,
    tenant_prefix: &str,
) -> Result<(), String> {
    let administrator = open_database(
        administrator_url,
        tls,
        1,
        Duration::from_secs(1),
        Duration::from_secs(5),
    );
    let result = async {
        let mut failures = Vec::new();
        match database_names(&administrator).await {
            Ok(names) => {
                for name in owned_tenant_database_names(names, tenant_prefix) {
                    if let Err(error) = administrator
                        .query_owned(format!("DROP DATABASE \"{name}\""), vec![])
                        .await
                    {
                        failures.push(format!("cannot drop owned tenant database {name}: {error}"));
                    }
                }
            }
            Err(error) => failures.push(format!("cannot list owned tenant databases: {error}")),
        }
        if let Err(error) = administrator
            .query_owned(
                format!("DROP DATABASE IF EXISTS \"{control_name}\""),
                vec![],
            )
            .await
        {
            failures.push(format!(
                "cannot drop owned control database {control_name}: {error}"
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
    .await;
    let close = administrator
        .close()
        .await
        .map_err(|error| error.to_string());
    result.and(close)
}

fn owned_tenant_database_names(names: Vec<String>, prefix: &str) -> Vec<String> {
    let marker = format!("{prefix}_");
    names
        .into_iter()
        .filter(|name| {
            name.strip_prefix(&marker)
                .and_then(|id| {
                    id.parse::<u64>()
                        .ok()
                        .filter(|value| *value > 0)
                        .map(|value| value.to_string() == id)
                })
                .unwrap_or(false)
        })
        .collect()
}

pub fn run_in_isolated_schema<F, Fut>(case_name: &str, test: F)
where
    F: FnOnce(IsolatedDatabase) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let setting = load_setting(case_name);
    let schema = unique_schema();
    let cleanup_url = setting.url.clone();
    let cleanup_schema = schema.clone();
    let tls = setting.tls;
    let result = catch_unwind(AssertUnwindSafe(|| {
        dever_runtime::task::run_entry(async move {
            let administrator = open_database(
                &setting.url,
                tls,
                2,
                Duration::from_secs(1),
                Duration::from_secs(5),
            );
            administrator
                .query_owned(format!("CREATE SCHEMA \"{schema}\""), vec![])
                .await
                .map_err(|error| error.to_string())?;
            administrator
                .close()
                .await
                .map_err(|error| error.to_string())?;
            test(IsolatedDatabase {
                document: setting.document,
                url: scoped_url(&setting.url, &schema),
                tls,
            })
            .await
        })
    }));
    let cleanup = catch_unwind(AssertUnwindSafe(|| {
        dever_runtime::task::run_entry(async move {
            let administrator = open_database(
                &cleanup_url,
                tls,
                1,
                Duration::from_secs(1),
                Duration::from_secs(5),
            );
            let drop_result = administrator
                .query_owned(
                    format!("DROP SCHEMA IF EXISTS \"{cleanup_schema}\" CASCADE"),
                    vec![],
                )
                .await;
            let close_result = administrator.close().await;
            drop_result.map_err(|error| error.to_string())?;
            close_result.map_err(|error| error.to_string())?;
            Ok(())
        })
    }));
    match result {
        Ok(result) => {
            cleanup.unwrap().unwrap();
            result.unwrap();
        }
        Err(panic) => {
            let _ = cleanup;
            resume_unwind(panic);
        }
    }
}

struct PostgresTestSetting {
    document: serde_json::Value,
    url: String,
    tls: PostgresTls,
}

fn load_setting(case_name: &str) -> PostgresTestSetting {
    assert!(
        valid_case_name(case_name),
        "PostgreSQL acceptance case names must use lower_snake_case"
    );
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let settings = Settings::load_project(&workspace).unwrap_or_else(|error| {
        panic!(
            "PostgreSQL acceptance requires '{}': {error}",
            workspace.join("config/setting.json").display()
        )
    });
    let (url, tls) = match settings.database(POSTGRES_TEST_CONNECTION) {
        Some(DatabaseSetting::Postgres { url, tls, .. }) => (case_url(url, case_name), *tls),
        Some(DatabaseSetting::Sqlite { .. }) => {
            panic!("database.{POSTGRES_TEST_CONNECTION} must use type 'postgres'")
        }
        None => panic!("database.{POSTGRES_TEST_CONNECTION} is required"),
    };
    let document = serde_json::from_str(
        &std::fs::read_to_string(workspace.join("config/setting.json")).unwrap(),
    )
    .unwrap();
    PostgresTestSetting { document, url, tls }
}

fn case_url(template: &str, case_name: &str) -> String {
    let (url, query) = template
        .split_once('?')
        .map_or((template, ""), |(url, query)| (url, query));
    let database = url.rsplit_once('/').map_or("", |(_, database)| database);
    assert!(
        matches!(
            template.split_once("://"),
            Some(("postgres" | "postgresql", _))
        ) && template.matches("{case}").count() == 1
            && database.matches("{case}").count() == 1
            && !query_overrides_database(query),
        concat!(
            "config/setting.json database.postgres_test.url must contain exactly one ",
            "{{case}} placeholder in its database name and must not override it with dbname"
        )
    );
    template.replace("{case}", case_name)
}

fn query_overrides_database(query: &str) -> bool {
    query.split('&').any(|parameter| {
        let key = parameter.split_once('=').map_or(parameter, |(key, _)| key);
        percent_decode(key).as_deref() == Some(b"dbname")
    })
}

fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = hex_digit(*bytes.get(index + 1)?)?;
        let low = hex_digit(*bytes.get(index + 2)?)?;
        decoded.push(high * 16 + low);
        index += 3;
    }
    Some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn valid_case_name(case_name: &str) -> bool {
    !case_name.is_empty()
        && case_name.starts_with(|character: char| character.is_ascii_lowercase())
        && case_name.split('_').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn unique_schema() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "dever_t_{:x}_{:x}_{:x}",
        std::process::id(),
        nanos,
        NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed)
    )
}

fn unique_namespace() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "{:x}_{:x}_{:x}",
        std::process::id(),
        nanos,
        NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed)
    )
}

fn database_name(url: &str) -> String {
    let path = url.split_once('?').map_or(url, |(path, _)| path);
    path.rsplit_once('/').unwrap().1.to_owned()
}

fn scoped_url(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-csearch_path%3D{schema}")
}

pub(crate) fn open_database(
    url: &str,
    tls: PostgresTls,
    max_connections: usize,
    wait_timeout: Duration,
    io_timeout: Duration,
) -> Database {
    Database::open(ConnectionOptions {
        url,
        database_name: None,
        tls,
        min_connections: 0,
        max_connections,
        max_page_size: MAX_PAGE_SIZE,
        wait_timeout,
        io_timeout,
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{case_url, owned_tenant_database_names};

    #[test]
    fn case_placeholder_must_select_the_effective_database() {
        assert_eq!(
            case_url(
                "postgresql://dever@localhost/dever_{case}?sslmode=disable",
                "authorization_postgres",
            ),
            "postgresql://dever@localhost/dever_authorization_postgres?sslmode=disable"
        );
        for url in [
            "postgresql://dever@localhost/dever_{case}?dbname=production",
            "postgresql://dever@localhost/dever_{case}?%64bname=production",
            "postgresql://dever@localhost/dever?application_name={case}",
        ] {
            assert!(
                std::panic::catch_unwind(|| case_url(url, "postgres_orm")).is_err(),
                "URL must be rejected: {url}"
            );
        }
    }

    #[test]
    fn cleanup_only_selects_this_run_and_canonical_positive_tenant_ids() {
        let names = [
            "dever_t_run_1",
            "dever_t_run_2",
            "dever_t_run_0",
            "dever_t_run_01",
            "dever_t_run_-1",
            "dever_t_run_1_extra",
            "dever_t_run2_1",
            "dever_t_run",
        ];
        assert_eq!(
            owned_tenant_database_names(names.map(str::to_owned).to_vec(), "dever_t_run"),
            ["dever_t_run_1", "dever_t_run_2"],
        );
    }
}

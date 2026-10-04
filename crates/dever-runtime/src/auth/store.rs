use std::collections::BTreeSet;

use crate::database::{Database, Executor, Sql, Transaction};
use crate::orm::{self, Error, Row, Value};

const OWNER_ROLE_PREFIX: &str = "_dever_owner:";
const ROLE_SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permission {
    pub key: &'static str,
    pub component: &'static str,
    pub domain: &'static str,
    pub site: &'static str,
    pub action: &'static str,
    pub method: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogPermission {
    pub key: String,
    pub component: String,
    pub domain: String,
    pub site: String,
    pub action: String,
    pub method: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Role {
    pub id: String,
    pub site: String,
    pub name: String,
    pub all_permissions: bool,
}

pub async fn initialize_catalog(database: Database) -> Result<(), Error> {
    require_control_database(&database)?;
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = initialize_catalog_in(&executor).await;
    finish_transaction(transaction, result).await
}

pub async fn sync_catalog(database: Database, permissions: &[Permission]) -> Result<(), Error> {
    require_control_database(&database)?;
    validate_catalog(permissions)?;
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = sync_catalog_in(&executor, permissions).await;
    finish_transaction(transaction, result).await
}

pub async fn list_permissions(database: Database) -> Result<Vec<CatalogPermission>, Error> {
    require_control_database(&database)?;
    let rows = database.query(Sql {
        sqlite: "SELECT key,component,domain,site,action,method FROM _dever_permission WHERE active=1 ORDER BY component,domain,site,action",
        postgres: "SELECT key,component,domain,site,action,method FROM _dever_permission WHERE active=1 ORDER BY component,domain,site,action",
    }, vec![]).await?;
    rows.into_iter().map(catalog_permission).collect()
}

pub async fn initialize_roles(database: Database) -> Result<(), Error> {
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = initialize_roles_in(&executor).await;
    finish_transaction(transaction, result).await
}

pub async fn authorize(
    database: Database,
    user_id: i64,
    site: &str,
    permission_key: &str,
) -> Result<bool, Error> {
    validate_user_id(user_id)?;
    validate_name("site", site)?;
    validate_permission_key(permission_key, Some(site))?;
    let rows = database.query(Sql {
        sqlite: "SELECT 1 FROM _dever_auth_user_role AS ur JOIN _dever_auth_role AS r ON r.site=ur.site AND r.id=ur.role_id LEFT JOIN _dever_auth_role_permission AS rp ON rp.site=r.site AND rp.role_id=r.id AND rp.permission_key=?3 WHERE ur.user_id=?1 AND ur.site=?2 AND r.enabled=1 AND (r.all_permissions=1 OR rp.permission_key=?3) LIMIT 1",
        postgres: "SELECT 1 FROM _dever_auth_user_role AS ur JOIN _dever_auth_role AS r ON r.site=ur.site AND r.id=ur.role_id LEFT JOIN _dever_auth_role_permission AS rp ON rp.site=r.site AND rp.role_id=r.id AND rp.permission_key=$3 WHERE ur.user_id=$1 AND ur.site=$2 AND r.enabled=1 AND (r.all_permissions=1 OR rp.permission_key=$3) LIMIT 1",
    }, vec![number(user_id), text(site), text(permission_key)]).await?;
    Ok(!rows.is_empty())
}

pub async fn save_role(
    catalog_database: Database,
    authorization_database: Database,
    role: &Role,
    permission_keys: &[String],
) -> Result<(), Error> {
    require_control_database(&catalog_database)?;
    validate_role(role)?;
    let permissions = validate_role_permissions(&catalog_database, role, permission_keys).await?;
    let transaction = authorization_database.begin().await?;
    let executor = Executor::new(authorization_database, Some(&transaction))?;
    let result = save_role_in(&executor, role, &permissions).await;
    finish_transaction(transaction, result).await
}

pub async fn grant_role(
    database: Database,
    user_id: i64,
    site: &str,
    role_id: &str,
) -> Result<(), Error> {
    validate_user_id(user_id)?;
    validate_name("site", site)?;
    validate_mutable_role_id(role_id)?;
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = grant_role_in(&executor, user_id, site, role_id).await;
    finish_transaction(transaction, result).await
}

pub async fn revoke_role(
    database: Database,
    user_id: i64,
    site: &str,
    role_id: &str,
) -> Result<(), Error> {
    validate_user_id(user_id)?;
    validate_name("site", site)?;
    validate_mutable_role_id(role_id)?;
    database.execute(Sql {
        sqlite: "DELETE FROM _dever_auth_user_role WHERE user_id=?1 AND site=?2 AND role_id=?3",
        postgres: "DELETE FROM _dever_auth_user_role WHERE user_id=$1 AND site=$2 AND role_id=$3",
    }, vec![number(user_id), text(site), text(role_id)]).await?;
    Ok(())
}

pub async fn disable_role(database: Database, site: &str, role_id: &str) -> Result<(), Error> {
    validate_name("site", site)?;
    validate_mutable_role_id(role_id)?;
    let changed = database
        .execute(
            Sql {
                sqlite: "UPDATE _dever_auth_role SET enabled=0 WHERE id=?1 AND site=?2",
                postgres: "UPDATE _dever_auth_role SET enabled=0 WHERE id=$1 AND site=$2",
            },
            vec![text(role_id), text(site)],
        )
        .await?;
    if changed == 0 {
        return Err(Error::invalid_data("role does not exist"));
    }
    Ok(())
}

pub async fn provision_owner(
    database: Database,
    user_id: i64,
    site: &str,
) -> Result<String, Error> {
    validate_user_id(user_id)?;
    validate_name("site", site)?;
    let role_id = format!("{OWNER_ROLE_PREFIX}{site}");
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = async {
        executor.execute(Sql {
            sqlite: "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES (?1,?2,'Owner',1,1) ON CONFLICT(site,id) DO UPDATE SET name=excluded.name,all_permissions=1,enabled=1",
            postgres: "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES ($1,$2,'Owner',1,1) ON CONFLICT(site,id) DO UPDATE SET name=excluded.name,all_permissions=1,enabled=1",
        }, vec![text(&role_id), text(site)]).await?;
        grant_role_in(&executor, user_id, site, &role_id).await
    }.await;
    finish_transaction(transaction, result).await?;
    Ok(role_id)
}

async fn initialize_catalog_in(executor: &Executor<'_>) -> Result<(), Error> {
    lock_private_schema(executor).await?;
    executor.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_permission (key TEXT PRIMARY KEY, component TEXT NOT NULL, domain TEXT NOT NULL, site TEXT NOT NULL, action TEXT NOT NULL, method TEXT NOT NULL, active BIGINT NOT NULL CHECK(active IN (0,1)), updated_at BIGINT NOT NULL)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_permission (key TEXT PRIMARY KEY, component TEXT NOT NULL, domain TEXT NOT NULL, site TEXT NOT NULL, action TEXT NOT NULL, method TEXT NOT NULL, active BIGINT NOT NULL CHECK(active IN (0,1)), updated_at BIGINT NOT NULL)",
    }, vec![]).await?;
    executor.execute(Sql {
        sqlite: "CREATE INDEX IF NOT EXISTS _dever_permission_catalog ON _dever_permission(active,component,domain,site,action)",
        postgres: "CREATE INDEX IF NOT EXISTS _dever_permission_catalog ON _dever_permission(active,component,domain,site,action)",
    }, vec![]).await?;
    Ok(())
}

async fn sync_catalog_in(executor: &Executor<'_>, permissions: &[Permission]) -> Result<(), Error> {
    initialize_catalog_in(executor).await?;
    let updated_at = crate::time::now().map_err(Error::migration)?;
    executor
        .execute(
            Sql {
                sqlite: "UPDATE _dever_permission SET active=0,updated_at=?1 WHERE active=1",
                postgres: "UPDATE _dever_permission SET active=0,updated_at=$1 WHERE active=1",
            },
            vec![number(updated_at)],
        )
        .await?;
    for permission in permissions {
        executor.execute(Sql {
            sqlite: "INSERT INTO _dever_permission(key,component,domain,site,action,method,active,updated_at) VALUES (?1,?2,?3,?4,?5,?6,1,?7) ON CONFLICT(key) DO UPDATE SET component=excluded.component,domain=excluded.domain,site=excluded.site,action=excluded.action,method=excluded.method,active=1,updated_at=excluded.updated_at",
            postgres: "INSERT INTO _dever_permission(key,component,domain,site,action,method,active,updated_at) VALUES ($1,$2,$3,$4,$5,$6,1,$7) ON CONFLICT(key) DO UPDATE SET component=excluded.component,domain=excluded.domain,site=excluded.site,action=excluded.action,method=excluded.method,active=1,updated_at=excluded.updated_at",
        }, vec![
            text(permission.key),
            text(permission.component),
            text(permission.domain),
            text(permission.site),
            text(permission.action),
            text(permission.method),
            number(updated_at),
        ]).await?;
    }
    Ok(())
}

async fn initialize_roles_in(executor: &Executor<'_>) -> Result<(), Error> {
    lock_private_schema(executor).await?;
    executor.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_auth_version (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version BIGINT NOT NULL)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_auth_version (singleton BIGINT PRIMARY KEY CHECK(singleton=1), version BIGINT NOT NULL)",
    }, vec![]).await?;
    let versions = executor
        .query(
            Sql {
                sqlite: "SELECT version FROM _dever_auth_version",
                postgres: "SELECT version FROM _dever_auth_version",
            },
            vec![],
        )
        .await?;
    if let Some(version) = versions.first() {
        if versions.len() != 1 || integer(version, 0)? != ROLE_SCHEMA_VERSION {
            return Err(Error::migration(
                "unsupported private authorization schema version",
            ));
        }
        validate_role_schema_v2(executor).await?;
        create_role_indexes(executor).await?;
        return Ok(());
    }

    let role = private_table_exists(executor, "_dever_auth_role").await?;
    let permission = private_table_exists(executor, "_dever_auth_role_permission").await?;
    let user = private_table_exists(executor, "_dever_auth_user_role").await?;
    match (role, permission, user) {
        (false, false, false) => create_role_schema_v2(executor).await?,
        (true, true, true) => {
            let permission_site =
                private_column_exists(executor, "_dever_auth_role_permission", "site").await?;
            let user_site =
                private_column_exists(executor, "_dever_auth_user_role", "site").await?;
            match (permission_site, user_site) {
                (false, false) => migrate_role_schema_v1(executor).await?,
                (true, true) => {
                    validate_role_schema_v2(executor).await?;
                    create_role_indexes(executor).await?;
                }
                _ => {
                    return Err(Error::migration(
                        "inconsistent private authorization schema",
                    ));
                }
            }
        }
        _ => return Err(Error::migration("incomplete private authorization schema")),
    }
    executor
        .execute(
            Sql {
                sqlite: "INSERT INTO _dever_auth_version(singleton,version) VALUES (1,?1)",
                postgres: "INSERT INTO _dever_auth_version(singleton,version) VALUES (1,$1)",
            },
            vec![number(ROLE_SCHEMA_VERSION)],
        )
        .await?;
    Ok(())
}

async fn create_role_schema_v2(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "CREATE TABLE IF NOT EXISTS _dever_auth_role (id TEXT NOT NULL, site TEXT NOT NULL, name TEXT NOT NULL, all_permissions BIGINT NOT NULL CHECK(all_permissions IN (0,1)), enabled BIGINT NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(site,id))",
        "CREATE TABLE IF NOT EXISTS _dever_auth_role_permission (site TEXT NOT NULL, role_id TEXT NOT NULL, permission_key TEXT NOT NULL, PRIMARY KEY(site,role_id,permission_key), FOREIGN KEY(site,role_id) REFERENCES _dever_auth_role(site,id) ON DELETE CASCADE)",
        "CREATE TABLE IF NOT EXISTS _dever_auth_user_role (user_id BIGINT NOT NULL, site TEXT NOT NULL, role_id TEXT NOT NULL, PRIMARY KEY(user_id,site,role_id), FOREIGN KEY(site,role_id) REFERENCES _dever_auth_role(site,id) ON DELETE CASCADE)",
    ] {
        executor
            .execute(
                Sql {
                    sqlite: sql,
                    postgres: sql,
                },
                vec![],
            )
            .await?;
    }
    create_role_indexes(executor).await
}

async fn create_role_indexes(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "CREATE INDEX IF NOT EXISTS _dever_auth_role_site ON _dever_auth_role(site,enabled)",
        "CREATE INDEX IF NOT EXISTS _dever_auth_user_role_user ON _dever_auth_user_role(user_id,site,role_id)",
    ] {
        executor
            .execute(
                Sql {
                    sqlite: sql,
                    postgres: sql,
                },
                vec![],
            )
            .await?;
    }
    Ok(())
}

async fn migrate_role_schema_v1(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "CREATE TABLE _dever_auth_role_v2 (id TEXT NOT NULL, site TEXT NOT NULL, name TEXT NOT NULL, all_permissions BIGINT NOT NULL CHECK(all_permissions IN (0,1)), enabled BIGINT NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(site,id))",
        "CREATE TABLE _dever_auth_role_permission_v2 (site TEXT NOT NULL, role_id TEXT NOT NULL, permission_key TEXT NOT NULL, PRIMARY KEY(site,role_id,permission_key), FOREIGN KEY(site,role_id) REFERENCES _dever_auth_role_v2(site,id) ON DELETE CASCADE)",
        "CREATE TABLE _dever_auth_user_role_v2 (user_id BIGINT NOT NULL, site TEXT NOT NULL, role_id TEXT NOT NULL, PRIMARY KEY(user_id,site,role_id), FOREIGN KEY(site,role_id) REFERENCES _dever_auth_role_v2(site,id) ON DELETE CASCADE)",
        "INSERT INTO _dever_auth_role_v2(id,site,name,all_permissions,enabled) SELECT id,site,name,all_permissions,enabled FROM _dever_auth_role",
        "INSERT INTO _dever_auth_role_permission_v2(site,role_id,permission_key) SELECT role.site,permission.role_id,permission.permission_key FROM _dever_auth_role_permission AS permission JOIN _dever_auth_role AS role ON role.id=permission.role_id",
        "INSERT INTO _dever_auth_user_role_v2(user_id,site,role_id) SELECT assignment.user_id,role.site,assignment.role_id FROM _dever_auth_user_role AS assignment JOIN _dever_auth_role AS role ON role.id=assignment.role_id",
        "DROP INDEX IF EXISTS _dever_auth_role_site",
        "DROP INDEX IF EXISTS _dever_auth_user_role_user",
        "DROP TABLE _dever_auth_role_permission",
        "DROP TABLE _dever_auth_user_role",
        "DROP TABLE _dever_auth_role",
        "ALTER TABLE _dever_auth_role_v2 RENAME TO _dever_auth_role",
        "ALTER TABLE _dever_auth_role_permission_v2 RENAME TO _dever_auth_role_permission",
        "ALTER TABLE _dever_auth_user_role_v2 RENAME TO _dever_auth_user_role",
    ] {
        executor
            .execute(
                Sql {
                    sqlite: sql,
                    postgres: sql,
                },
                vec![],
            )
            .await?;
    }
    create_role_indexes(executor).await
}

async fn validate_role_schema_v2(executor: &Executor<'_>) -> Result<(), Error> {
    for table in [
        "_dever_auth_role",
        "_dever_auth_role_permission",
        "_dever_auth_user_role",
    ] {
        if !private_table_exists(executor, table).await? {
            return Err(Error::migration("incomplete private authorization schema"));
        }
    }
    for table in ["_dever_auth_role_permission", "_dever_auth_user_role"] {
        if !private_column_exists(executor, table, "site").await? {
            return Err(Error::migration(
                "invalid private authorization schema version",
            ));
        }
    }
    Ok(())
}

async fn private_table_exists(executor: &Executor<'_>, table: &str) -> Result<bool, Error> {
    let rows = executor.query(Sql {
        sqlite: "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        postgres: "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name=$1",
    }, vec![text(table)]).await?;
    Ok(integer(&orm::required_row(rows, "authorization table lookup")?, 0)? == 1)
}

async fn private_column_exists(
    executor: &Executor<'_>,
    table: &str,
    column: &str,
) -> Result<bool, Error> {
    let rows = executor.query(Sql {
        sqlite: "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name=?2",
        postgres: "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 AND column_name=$2",
    }, vec![text(table), text(column)]).await?;
    Ok(integer(&orm::required_row(rows, "authorization column lookup")?, 0)? == 1)
}

async fn save_role_in(
    executor: &Executor<'_>,
    role: &Role,
    permission_keys: &BTreeSet<String>,
) -> Result<(), Error> {
    executor.execute(Sql {
        sqlite: "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES (?1,?2,?3,?4,1) ON CONFLICT(site,id) DO UPDATE SET name=excluded.name,all_permissions=excluded.all_permissions",
        postgres: "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES ($1,$2,$3,$4,1) ON CONFLICT(site,id) DO UPDATE SET name=excluded.name,all_permissions=excluded.all_permissions",
    }, vec![
        text(&role.id),
        text(&role.site),
        text(&role.name),
        number(i64::from(role.all_permissions)),
    ]).await?;
    executor
        .execute(
            Sql {
                sqlite: "DELETE FROM _dever_auth_role_permission WHERE site=?1 AND role_id=?2",
                postgres: "DELETE FROM _dever_auth_role_permission WHERE site=$1 AND role_id=$2",
            },
            vec![text(&role.site), text(&role.id)],
        )
        .await?;
    for permission_key in permission_keys {
        executor.execute(Sql {
            sqlite: "INSERT INTO _dever_auth_role_permission(site,role_id,permission_key) VALUES (?1,?2,?3)",
            postgres: "INSERT INTO _dever_auth_role_permission(site,role_id,permission_key) VALUES ($1,$2,$3)",
        }, vec![text(&role.site), text(&role.id), text(permission_key)]).await?;
    }
    Ok(())
}

async fn grant_role_in(
    executor: &Executor<'_>,
    user_id: i64,
    site: &str,
    role_id: &str,
) -> Result<(), Error> {
    let rows = executor
        .query(
            Sql {
                sqlite: "SELECT enabled FROM _dever_auth_role WHERE id=?1 AND site=?2",
                postgres: "SELECT enabled FROM _dever_auth_role WHERE id=$1 AND site=$2 FOR SHARE",
            },
            vec![text(role_id), text(site)],
        )
        .await?;
    let role = orm::required_row(rows, "grant role")?;
    if integer(&role, 0)? != 1 {
        return Err(Error::invalid_data("disabled role cannot be granted"));
    }
    executor.execute(Sql {
        sqlite: "INSERT INTO _dever_auth_user_role(user_id,site,role_id) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
        postgres: "INSERT INTO _dever_auth_user_role(user_id,site,role_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    }, vec![number(user_id), text(site), text(role_id)]).await?;
    Ok(())
}

async fn validate_role_permissions(
    catalog_database: &Database,
    role: &Role,
    permission_keys: &[String],
) -> Result<BTreeSet<String>, Error> {
    if role.all_permissions && !permission_keys.is_empty() {
        return Err(Error::invalid_data(
            "all-permissions role cannot store individual permissions",
        ));
    }
    let requested = permission_keys.iter().cloned().collect::<BTreeSet<_>>();
    if requested.len() != permission_keys.len() {
        return Err(Error::invalid_data("role permissions contain duplicates"));
    }
    for permission_key in &requested {
        validate_permission_key(permission_key, Some(&role.site))?;
    }
    if requested.is_empty() {
        return Ok(requested);
    }
    let available = catalog_database
        .query(
            Sql {
                sqlite: "SELECT key FROM _dever_permission WHERE site=?1 AND active=1",
                postgres: "SELECT key FROM _dever_permission WHERE site=$1 AND active=1",
            },
            vec![text(&role.site)],
        )
        .await?
        .into_iter()
        .map(|row| row_text(&row, 0))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if let Some(unknown) = requested.iter().find(|key| !available.contains(*key)) {
        return Err(Error::invalid_data(format!(
            "permission '{unknown}' is not active for site '{}'",
            role.site
        )));
    }
    Ok(requested)
}

fn validate_catalog(permissions: &[Permission]) -> Result<(), Error> {
    let mut keys = BTreeSet::new();
    for permission in permissions {
        validate_name("component", permission.component)?;
        validate_name("domain", permission.domain)?;
        validate_name("site", permission.site)?;
        validate_name("action", permission.action)?;
        if !matches!(permission.method, "GET" | "POST" | "PUT" | "DELETE") {
            return Err(Error::invalid_data(
                "permission method must be GET, POST, PUT, or DELETE",
            ));
        }
        let expected = format!(
            "{}.{}.{}.{}",
            permission.component, permission.domain, permission.site, permission.action
        );
        if permission.key != expected {
            return Err(Error::invalid_data(format!(
                "permission key '{}' must be '{expected}'",
                permission.key
            )));
        }
        if !keys.insert(permission.key) {
            return Err(Error::invalid_data(format!(
                "duplicate permission key '{}'",
                permission.key
            )));
        }
    }
    Ok(())
}

fn validate_role(role: &Role) -> Result<(), Error> {
    validate_mutable_role_id(&role.id)?;
    validate_name("site", &role.site)?;
    validate_text("role name", &role.name, 128)
}

fn validate_mutable_role_id(role_id: &str) -> Result<(), Error> {
    validate_text("role id", role_id, 128)?;
    if role_id.starts_with(OWNER_ROLE_PREFIX) {
        return Err(Error::invalid_data("reserved role id"));
    }
    Ok(())
}

fn validate_permission_key(key: &str, expected_site: Option<&str>) -> Result<(), Error> {
    if key.len() > 512 {
        return Err(Error::invalid_data("permission key exceeds 512 bytes"));
    }
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 4 || parts.iter().any(|part| !valid_name(part)) {
        return Err(Error::invalid_data(
            "permission key must contain component.domain.site.action",
        ));
    }
    if expected_site.is_some_and(|site| parts[2] != site) {
        return Err(Error::invalid_data(
            "permission key belongs to another site",
        ));
    }
    Ok(())
}

fn validate_name(label: &str, value: &str) -> Result<(), Error> {
    if !valid_name(value) {
        return Err(Error::invalid_data(format!(
            "{label} must use lower_snake_case"
        )));
    }
    Ok(())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.starts_with(|character: char| character.is_ascii_lowercase())
        && value.split('_').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn validate_text(label: &str, value: &str, maximum: usize) -> Result<(), Error> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(Error::invalid_data(format!(
            "{label} must contain 1..={maximum} non-control bytes"
        )));
    }
    Ok(())
}

fn validate_user_id(user_id: i64) -> Result<(), Error> {
    if user_id <= 0 {
        return Err(Error::invalid_data("user id must be positive"));
    }
    Ok(())
}

fn require_control_database(database: &Database) -> Result<(), Error> {
    if database.key().tenant_id().is_some() {
        return Err(Error::invalid_data(
            "permission catalog requires the global control database",
        ));
    }
    Ok(())
}

async fn lock_private_schema(executor: &Executor<'_>) -> Result<(), Error> {
    executor
        .query(
            Sql {
                sqlite: "SELECT 1",
                postgres: "SELECT pg_advisory_xact_lock(1684371045, 1635087476) IS NULL",
            },
            vec![],
        )
        .await?;
    Ok(())
}

fn catalog_permission(row: Row) -> Result<CatalogPermission, Error> {
    Ok(CatalogPermission {
        key: row_text(&row, 0)?,
        component: row_text(&row, 1)?,
        domain: row_text(&row, 2)?,
        site: row_text(&row, 3)?,
        action: row_text(&row, 4)?,
        method: row_text(&row, 5)?,
    })
}

fn row_text(row: &Row, index: usize) -> Result<String, Error> {
    orm::text(row.get(index)?.clone())
}

fn integer(row: &Row, index: usize) -> Result<i64, Error> {
    orm::int(row.get(index)?.clone())
}

fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}

fn number(value: i64) -> Value {
    Value::Int(value)
}

async fn finish_transaction<T>(
    transaction: Transaction,
    result: Result<T, Error>,
) -> Result<T, Error> {
    match result {
        Ok(value) => {
            transaction.commit().await?;
            Ok(value)
        }
        Err(error) => match transaction.rollback().await {
            Ok(()) => Err(error),
            Err(rollback) => Err(error.cause(rollback)),
        },
    }
}

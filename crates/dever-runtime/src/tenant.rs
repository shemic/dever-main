use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::database::{Database, OwnedSql, Sql};
use crate::orm::{self, Error, ErrorKind, Value};

tokio::task_local! {
    static CURRENT_TENANT: i64;
}

static MANAGER: OnceLock<Arc<Manager>> = OnceLock::new();

pub(crate) struct Manager {
    control: Database,
    schema_fingerprint: String,
    max_pools: usize,
    idle_timeout: Duration,
    state: Mutex<State>,
}

struct State {
    accepting: bool,
    pools: BTreeMap<(String, i64), PoolEntry>,
}

struct PoolEntry {
    database: Database,
    last_used: Instant,
    ready: bool,
}

impl State {
    async fn retire_pool(&mut self, key: &(String, i64)) -> Result<(), Error> {
        let Some(entry) = self.pools.get_mut(key) else {
            return Ok(());
        };
        entry.ready = false;
        let result = entry.database.close_pool().await;
        self.pools.remove(key);
        result
    }
}

pub async fn scope<T>(tenant_id: i64, future: impl Future<Output = T>) -> T {
    CURRENT_TENANT.scope(tenant_id, future).await
}

pub fn current_id() -> Result<i64, Error> {
    CURRENT_TENANT
        .try_with(|tenant_id| *tenant_id)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "tenant context is required"))
        .and_then(|tenant_id| {
            if tenant_id > 0 {
                Ok(tenant_id)
            } else {
                Err(Error::new(
                    ErrorKind::InvalidData,
                    "tenant id must be positive",
                ))
            }
        })
}

pub fn schema_fingerprint() -> Result<String, Error> {
    Ok(manager()?.schema_fingerprint.clone())
}

pub async fn initialize(schema_fingerprint: &str) -> Result<(), Error> {
    if schema_fingerprint.is_empty() || schema_fingerprint.len() > 65_536 {
        return Err(Error::migration("invalid tenant schema fingerprint"));
    }
    let configuration = crate::config::current_settings();
    let settings = configuration
        .tenant()
        .ok_or_else(|| Error::new(ErrorKind::Pool, "tenant settings are not configured"))?;
    let control = crate::database::database_for(Some(settings.database()), "")?;
    control.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_tenant_storage (tenant_id INTEGER PRIMARY KEY, state TEXT NOT NULL, schema_fingerprint TEXT NOT NULL, updated_at INTEGER NOT NULL)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_tenant_storage (tenant_id BIGINT PRIMARY KEY, state TEXT NOT NULL, schema_fingerprint TEXT NOT NULL, updated_at BIGINT NOT NULL)",
    }, Vec::new()).await?;
    control.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_tenant_component (tenant_id INTEGER NOT NULL, component TEXT NOT NULL, disabled_at INTEGER NOT NULL, PRIMARY KEY (tenant_id, component), FOREIGN KEY (tenant_id) REFERENCES _dever_tenant_storage (tenant_id) ON DELETE CASCADE)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_tenant_component (tenant_id BIGINT NOT NULL, component TEXT NOT NULL, disabled_at BIGINT NOT NULL, PRIMARY KEY (tenant_id, component), FOREIGN KEY (tenant_id) REFERENCES _dever_tenant_storage (tenant_id) ON DELETE CASCADE)",
    }, Vec::new()).await?;
    let manager = Arc::new(Manager {
        control,
        schema_fingerprint: schema_fingerprint.to_owned(),
        max_pools: settings.max_pools(),
        idle_timeout: Duration::from_millis(settings.idle_timeout_ms()),
        state: Mutex::new(State {
            accepting: true,
            pools: BTreeMap::new(),
        }),
    });
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return session.set_tenant(manager);
    }
    MANAGER
        .set(manager)
        .map_err(|_| Error::migration("tenant storage was initialized more than once"))
}

pub async fn database(connection: &str) -> Result<Database, Error> {
    let tenant_id = current_id()?;
    let manager = manager()?;
    manager.require_ready(tenant_id).await?;
    let key = (connection.to_owned(), tenant_id);
    let mut state = manager.state.lock().await;
    if !state.accepting {
        return Err(Error::new(
            ErrorKind::Pool,
            "tenant storage is shutting down",
        ));
    }
    if let Some(entry) = state.pools.get_mut(&key)
        && entry.ready
    {
        entry.last_used = Instant::now();
        return Ok(entry.database.clone());
    }
    // An interrupted preparation remains owned, but must never become a usable pool.
    state.retire_pool(&key).await?;
    let now = Instant::now();
    let expired = state
        .pools
        .iter()
        .filter(|(_, entry)| {
            entry.database.exclusively_held()
                && now.duration_since(entry.last_used) >= manager.idle_timeout
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for expired_key in expired {
        state.retire_pool(&expired_key).await?;
    }
    let mut reusable = state
        .pools
        .iter()
        .filter(|(_, entry)| entry.database.exclusively_held())
        .map(|(key, entry)| (key.clone(), entry.last_used))
        .collect::<Vec<_>>();
    reusable.sort_by_key(|(_, last_used)| *last_used);
    for (reusable_key, _) in reusable {
        if state.pools.len() < manager.max_pools {
            break;
        }
        state.retire_pool(&reusable_key).await?;
    }
    if state.pools.len() >= manager.max_pools {
        return Err(Error::new(
            ErrorKind::PoolExhausted,
            "tenant database pool limit reached",
        ));
    }
    let database = crate::database::tenant_database(connection, tenant_id, false).await?;
    state.pools.insert(
        key.clone(),
        PoolEntry {
            database: database.clone(),
            last_used: now,
            ready: false,
        },
    );
    let preparation = async {
        database.prepare_pool().await?;
        manager.require_local_marker(&database, tenant_id).await
    }
    .await;
    if let Err(error) = preparation {
        let cleanup = state.retire_pool(&key).await;
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => error.cause(cleanup),
        });
    }
    state
        .pools
        .get_mut(&key)
        .expect("registered tenant pool")
        .ready = true;
    Ok(database)
}

pub async fn ready_ids_after(after: i64, limit: usize) -> Result<Vec<i64>, Error> {
    if after < 0 || !(1..=256).contains(&limit) {
        return Err(Error::invalid_data("invalid tenant worker page"));
    }
    let manager = manager()?;
    let rows = manager.control.query(Sql {
        sqlite: "SELECT tenant_id FROM _dever_tenant_storage WHERE state='ready' AND schema_fingerprint=?1 AND tenant_id>?2 ORDER BY tenant_id LIMIT ?3",
        postgres: "SELECT tenant_id FROM _dever_tenant_storage WHERE state='ready' AND schema_fingerprint=$1 AND tenant_id>$2 ORDER BY tenant_id LIMIT $3",
    }, vec![
        Value::Text(manager.schema_fingerprint.clone()),
        Value::Int(after),
        Value::Int(limit as i64),
    ]).await?;
    rows.into_iter()
        .map(|row| {
            let tenant_id = orm::int(row.get(0)?.clone())?;
            if tenant_id <= 0 {
                return Err(Error::migration(
                    "tenant registry contains an invalid tenant id",
                ));
            }
            Ok(tenant_id)
        })
        .collect()
}

pub async fn disable_component(
    tenant_id: i64,
    component: &str,
    tenant_components: &[&str],
) -> Result<(), Error> {
    require_tenant_component(component, tenant_components)?;
    let manager = manager()?;
    manager.require_ready(tenant_id).await?;
    let disabled_at = crate::time::now().map_err(Error::migration)?;
    manager.control.execute(Sql {
        sqlite: "INSERT INTO _dever_tenant_component (tenant_id,component,disabled_at) VALUES (?1,?2,?3) ON CONFLICT(tenant_id,component) DO UPDATE SET disabled_at=excluded.disabled_at",
        postgres: "INSERT INTO _dever_tenant_component (tenant_id,component,disabled_at) VALUES ($1,$2,$3) ON CONFLICT(tenant_id,component) DO UPDATE SET disabled_at=excluded.disabled_at",
    }, vec![
        Value::Int(tenant_id),
        Value::Text(component.to_owned()),
        Value::Int(disabled_at),
    ]).await.map(|_| ())
}

pub async fn enable_component(
    tenant_id: i64,
    component: &str,
    tenant_components: &[&str],
) -> Result<(), Error> {
    require_tenant_component(component, tenant_components)?;
    let manager = manager()?;
    manager.require_ready(tenant_id).await?;
    manager
        .control
        .execute(
            Sql {
                sqlite: "DELETE FROM _dever_tenant_component WHERE tenant_id=?1 AND component=?2",
                postgres: "DELETE FROM _dever_tenant_component WHERE tenant_id=$1 AND component=$2",
            },
            vec![Value::Int(tenant_id), Value::Text(component.to_owned())],
        )
        .await
        .map(|_| ())
}

pub async fn require_components(
    tenant_id: i64,
    components: &[&str],
    tenant_components: &[&str],
) -> Result<(), Error> {
    let components = components.iter().copied().collect::<BTreeSet<_>>();
    for component in &components {
        require_tenant_component(component, tenant_components)?;
    }
    if components.is_empty() {
        return Ok(());
    }

    let manager = manager()?;
    manager.require_ready(tenant_id).await?;
    let sqlite_placeholders = (2..=components.len() + 1)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let postgres_placeholders = (2..=components.len() + 1)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut parameters = Vec::with_capacity(components.len() + 1);
    parameters.push(Value::Int(tenant_id));
    parameters.extend(
        components
            .iter()
            .map(|component| Value::Text((*component).to_owned())),
    );
    let rows = manager.control.query_owned(OwnedSql {
        sqlite: format!("SELECT component FROM _dever_tenant_component WHERE tenant_id=?1 AND component IN ({sqlite_placeholders}) ORDER BY component LIMIT 1"),
        postgres: format!("SELECT component FROM _dever_tenant_component WHERE tenant_id=$1 AND component IN ({postgres_placeholders}) ORDER BY component LIMIT 1"),
    }, parameters).await?;
    if let Some(row) = orm::optional_row(rows, "tenant component lookup")? {
        let component = orm::text(row.get(0)?.clone())?;
        return Err(Error::invalid_data(format!(
            "tenant component '{component}' is disabled"
        )));
    }
    Ok(())
}

pub async fn begin_migration(
    connection: &str,
    tenant_id: i64,
    schema_fingerprint: &str,
) -> Result<Database, Error> {
    if tenant_id <= 0 {
        return Err(Error::migration("tenant id must be positive"));
    }
    let manager = manager()?;
    if schema_fingerprint != manager.schema_fingerprint {
        return Err(Error::migration(
            "tenant migration schema fingerprint does not match this application",
        ));
    }
    manager
        .write_state(tenant_id, "migrating", schema_fingerprint)
        .await?;
    let database = crate::database::tenant_database(connection, tenant_id, true).await?;
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        session.own_migration_database(database.clone());
    }
    if let Err(error) = database.prepare_pool().await {
        return Err(match database.close_pool().await {
            Ok(()) => error,
            Err(cleanup) => error.cause(cleanup),
        });
    }
    Ok(database)
}

pub async fn mark_database_ready(
    database: &Database,
    tenant_id: i64,
    schema_fingerprint: &str,
) -> Result<(), Error> {
    let manager = manager()?;
    if schema_fingerprint != manager.schema_fingerprint {
        return Err(Error::migration(
            "tenant migration schema fingerprint does not match this application",
        ));
    }
    if database.key().tenant_id() != Some(tenant_id) {
        return Err(Error::migration(
            "tenant migration database key does not match tenant id",
        ));
    }
    database.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_tenant_ready (marker INTEGER PRIMARY KEY CHECK (marker = 1), tenant_id INTEGER NOT NULL, schema_fingerprint TEXT NOT NULL)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_tenant_ready (marker BIGINT PRIMARY KEY CHECK (marker = 1), tenant_id BIGINT NOT NULL, schema_fingerprint TEXT NOT NULL)",
    }, Vec::new()).await?;
    database.execute(Sql {
        sqlite: "INSERT INTO _dever_tenant_ready (marker,tenant_id,schema_fingerprint) VALUES (1,?1,?2) ON CONFLICT(marker) DO UPDATE SET tenant_id=excluded.tenant_id,schema_fingerprint=excluded.schema_fingerprint",
        postgres: "INSERT INTO _dever_tenant_ready (marker,tenant_id,schema_fingerprint) VALUES (1,$1,$2) ON CONFLICT(marker) DO UPDATE SET tenant_id=excluded.tenant_id,schema_fingerprint=excluded.schema_fingerprint",
    }, vec![Value::Int(tenant_id), Value::Text(schema_fingerprint.to_owned())]).await?;
    Ok(())
}

pub async fn finish_migration(tenant_id: i64, schema_fingerprint: &str) -> Result<(), Error> {
    let manager = manager()?;
    if schema_fingerprint != manager.schema_fingerprint {
        return Err(Error::migration(
            "tenant migration schema fingerprint does not match this application",
        ));
    }
    manager
        .write_state(tenant_id, "ready", schema_fingerprint)
        .await
}

pub async fn shutdown() -> Result<(), Error> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    manager.close().await
}

impl Manager {
    pub(crate) async fn close(&self) -> Result<(), Error> {
        let databases = {
            let mut state = self.state.lock().await;
            state.accepting = false;
            std::mem::take(&mut state.pools)
                .into_values()
                .map(|entry| entry.database)
                .collect::<Vec<_>>()
        };
        for database in &databases {
            database.stop_accepting();
        }
        let mut first_error = None;
        for database in databases {
            if let Err(error) = database.close_pool().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn manager() -> Result<Arc<Manager>, Error> {
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return session
            .tenant()
            .ok_or_else(|| Error::new(ErrorKind::Pool, "tenant storage is not initialized"));
    }
    MANAGER
        .get()
        .cloned()
        .ok_or_else(|| Error::new(ErrorKind::Pool, "tenant storage is not initialized"))
}

fn require_tenant_component(component: &str, tenant_components: &[&str]) -> Result<(), Error> {
    if tenant_components.contains(&component) {
        Ok(())
    } else {
        Err(Error::invalid_data(format!(
            "component '{component}' is not a tenant component"
        )))
    }
}

impl Manager {
    async fn require_ready(&self, tenant_id: i64) -> Result<(), Error> {
        let rows = self.control.query(Sql {
            sqlite: "SELECT state, schema_fingerprint FROM _dever_tenant_storage WHERE tenant_id = ?1",
            postgres: "SELECT state, schema_fingerprint FROM _dever_tenant_storage WHERE tenant_id = $1",
        }, vec![Value::Int(tenant_id)]).await?;
        let row = orm::optional_row(rows, "tenant storage lookup")?
            .ok_or_else(|| Error::migration("tenant storage is not provisioned"))?;
        let state = orm::text(row.get(0)?.clone())?;
        let fingerprint = orm::text(row.get(1)?.clone())?;
        if state != "ready" {
            return Err(Error::migration("tenant storage is not ready"));
        }
        if fingerprint != self.schema_fingerprint {
            return Err(Error::migration(
                "tenant storage schema fingerprint is stale",
            ));
        }
        Ok(())
    }

    async fn write_state(
        &self,
        tenant_id: i64,
        state: &str,
        fingerprint: &str,
    ) -> Result<(), Error> {
        let updated_at = crate::time::now().map_err(Error::migration)?;
        self.control.execute(Sql {
            sqlite: "INSERT INTO _dever_tenant_storage (tenant_id,state,schema_fingerprint,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(tenant_id) DO UPDATE SET state=excluded.state,schema_fingerprint=excluded.schema_fingerprint,updated_at=excluded.updated_at",
            postgres: "INSERT INTO _dever_tenant_storage (tenant_id,state,schema_fingerprint,updated_at) VALUES ($1,$2,$3,$4) ON CONFLICT(tenant_id) DO UPDATE SET state=excluded.state,schema_fingerprint=excluded.schema_fingerprint,updated_at=excluded.updated_at",
        }, vec![
            Value::Int(tenant_id),
            Value::Text(state.to_owned()),
            Value::Text(fingerprint.to_owned()),
            Value::Int(updated_at),
        ]).await.map(|_| ())
    }

    async fn require_local_marker(&self, database: &Database, tenant_id: i64) -> Result<(), Error> {
        let rows = database.query(Sql {
            sqlite: "SELECT tenant_id, schema_fingerprint FROM _dever_tenant_ready WHERE marker = 1",
            postgres: "SELECT tenant_id, schema_fingerprint FROM _dever_tenant_ready WHERE marker = 1",
        }, Vec::new()).await.map_err(|error| {
            Error::migration(format!("tenant database ready marker is missing: {error}"))
        })?;
        let row = orm::optional_row(rows, "tenant database ready marker")?
            .ok_or_else(|| Error::migration("tenant database ready marker is missing"))?;
        if orm::int(row.get(0)?.clone())? != tenant_id {
            return Err(Error::migration(
                "tenant database ready marker has the wrong tenant id",
            ));
        }
        if orm::text(row.get(1)?.clone())? != self.schema_fingerprint {
            return Err(Error::migration(
                "tenant database ready marker has a stale schema fingerprint",
            ));
        }
        Ok(())
    }
}

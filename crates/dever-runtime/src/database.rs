use crate::config::Database as DatabaseConfig;
use crate::orm::{Error, ErrorKind, Row, RowStream, Value};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct Database {
    key: DatabaseKey,
    backend: Backend,
}

/// One application entry's database owners. Native process roots retain their
/// existing global initialization; repeated embedded roots own this session.
pub struct Session {
    settings: Arc<crate::config::Settings>,
    databases: std::collections::BTreeMap<String, Database>,
}

impl Session {
    pub fn load(
        bundled: crate::config::RuntimeProfile,
        bindings: &[(Option<&str>, &str, bool)],
        transactions: &[&[(Option<&str>, &str, bool)]],
    ) -> Result<Self, String> {
        let settings = crate::config::Settings::load_for_executable()?;
        settings.validate_database_entry()?;
        Self::from_settings(Arc::new(settings), bundled, bindings, transactions)
    }

    pub fn from_settings(
        settings: Arc<crate::config::Settings>,
        bundled: crate::config::RuntimeProfile,
        bindings: &[(Option<&str>, &str, bool)],
        transactions: &[&[(Option<&str>, &str, bool)]],
    ) -> Result<Self, String> {
        settings.prepare_databases(bundled, bindings, transactions)?;
        let mut databases = std::collections::BTreeMap::new();
        #[cfg(feature = "sqlite")]
        for (name, database) in crate::sqlite::configured_databases(&settings)? {
            databases.insert(
                name.clone(),
                Database {
                    key: DatabaseKey {
                        connection: name,
                        tenant_id: None,
                    },
                    backend: Backend::Sqlite(database),
                },
            );
        }
        #[cfg(feature = "postgres")]
        for (name, database) in crate::postgres::configured_databases(&settings)? {
            databases.insert(
                name.clone(),
                Database {
                    key: DatabaseKey {
                        connection: name,
                        tenant_id: None,
                    },
                    backend: Backend::Postgres(database),
                },
            );
        }
        Ok(Self {
            settings,
            databases,
        })
    }

    pub fn database(&self, explicit: Option<&str>, root: &str) -> Result<Database, Error> {
        let (name, _) = self
            .settings
            .resolve_database(explicit, root)
            .map_err(|message| Error::new(ErrorKind::Pool, message))?;
        self.databases.get(name).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::Pool,
                format!("database '{name}' is not initialized"),
            )
        })
    }

    pub async fn prepare(&self) -> Result<(), Error> {
        for database in self.databases.values() {
            database.prepare_pool().await?;
        }
        Ok(())
    }

    /// Called after the entry's structured scope has drained, before its
    /// runtime is destroyed. Stop every pool before awaiting any driver.
    pub async fn close(&self) -> Result<(), Error> {
        self.stop_accepting();
        let mut failure: Option<Error> = None;
        for database in self.databases.values() {
            if let Err(error) = database.close_pool().await {
                failure = Some(match failure {
                    Some(previous) => previous.cause(error),
                    None => error,
                });
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(crate) fn stop_accepting(&self) {
        for database in self.databases.values() {
            database.stop_accepting();
        }
    }
}

#[derive(Clone)]
enum Backend {
    #[cfg(feature = "sqlite")]
    Sqlite(Arc<crate::sqlite::Database>),
    #[cfg(feature = "postgres")]
    Postgres(Arc<crate::postgres::Database>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DatabaseKey {
    connection: String,
    tenant_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelScope {
    Global,
    Tenant,
}

#[derive(Clone, Copy)]
pub struct StorageBinding {
    explicit: Option<&'static str>,
    package_root: &'static str,
    scope: ModelScope,
}

impl StorageBinding {
    pub const fn new(
        explicit: Option<&'static str>,
        package_root: &'static str,
        scope: ModelScope,
    ) -> Self {
        Self {
            explicit,
            package_root,
            scope,
        }
    }

    pub async fn database(&self) -> Result<Database, Error> {
        let settings = crate::config::current_settings();
        if self.scope == ModelScope::Global || settings.tenant().is_none() {
            return database_for(self.explicit, self.package_root);
        }
        let (connection, _) = settings
            .resolve_database(self.explicit, self.package_root)
            .map_err(|error| Error::new(ErrorKind::Pool, error))?;
        crate::tenant::database(connection).await
    }

    pub(crate) fn selector(&self) -> (Option<&str>, &str) {
        (self.explicit, self.package_root)
    }
}

impl DatabaseKey {
    pub fn connection(&self) -> &str {
        &self.connection
    }
    pub fn tenant_id(&self) -> Option<i64> {
        self.tenant_id
    }
}

pub enum Transaction {
    #[cfg(feature = "sqlite")]
    Sqlite {
        key: DatabaseKey,
        lease: Arc<crate::sqlite::Database>,
        transaction: crate::sqlite::Transaction,
    },
    #[cfg(feature = "postgres")]
    Postgres {
        key: DatabaseKey,
        lease: Arc<crate::postgres::Database>,
        transaction: crate::postgres::Transaction,
    },
}

/// An internal shared transaction owner for foreign coroutine frames. Sequential
/// operations borrow the same transaction; finish takes it exactly once. No
/// task-local ambient transaction is installed or inherited by spawned tasks.
pub struct TransactionState(tokio::sync::Mutex<Option<Transaction>>);

pub struct TransactionLease<'a> {
    transaction: tokio::sync::MutexGuard<'a, Option<Transaction>>,
    completed: bool,
}

impl std::ops::Deref for TransactionLease<'_> {
    type Target = Transaction;
    fn deref(&self) -> &Self::Target {
        self.transaction
            .as_ref()
            .expect("validated live transaction lease")
    }
}

impl TransactionLease<'_> {
    /// The driver returned a known result. Cancellation before this point must
    /// discard the shared transaction even if another frame still owns an alias.
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for TransactionLease<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.transaction.take();
        }
    }
}

impl TransactionState {
    pub fn new(transaction: Transaction) -> Self {
        Self(tokio::sync::Mutex::new(Some(transaction)))
    }

    pub async fn borrow(state: Option<&Self>) -> Result<Option<TransactionLease<'_>>, Error> {
        let Some(state) = state else {
            return Ok(None);
        };
        let guard = state.0.lock().await;
        if guard.is_none() {
            return Err(Error::new(
                ErrorKind::Database,
                "transaction was already finished",
            ));
        }
        Ok(Some(TransactionLease {
            transaction: guard,
            completed: false,
        }))
    }

    pub async fn finish(&self, commit: bool) -> Result<(), Error> {
        let transaction =
            self.0.lock().await.take().ok_or_else(|| {
                Error::new(ErrorKind::Database, "transaction was already finished")
            })?;
        if commit {
            transaction.commit().await
        } else {
            transaction.rollback().await
        }
    }
}

pub enum Executor<'a> {
    Pool(Database),
    #[cfg(feature = "sqlite")]
    Sqlite(&'a crate::sqlite::Transaction),
    #[cfg(feature = "postgres")]
    Postgres(&'a crate::postgres::Transaction),
}

pub struct Sql {
    pub sqlite: &'static str,
    pub postgres: &'static str,
}

pub struct OwnedSql {
    pub sqlite: String,
    pub postgres: String,
}

pub fn relation_sql(
    prefix: &'static str,
    before_limit: &'static str,
    suffix: &'static str,
    ids: usize,
) -> OwnedSql {
    let sqlite = (1..=ids)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let postgres = (1..=ids)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    OwnedSql {
        sqlite: format!("{prefix}{sqlite}{before_limit}?{}{suffix}", ids + 1),
        postgres: format!("{prefix}{postgres}{before_limit}${}{suffix}", ids + 1),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub nullable: bool,
    pub generated: bool,
    pub default: Option<String>,
    pub rename_from: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    pub fields: Vec<String>,
    pub unique: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    pub name: String,
    pub revision: String,
    pub drops: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub model: String,
    pub table: String,
    pub revision: String,
    pub seed_revision: String,
    pub fields: Vec<Field>,
    pub indexes: Vec<Index>,
    pub migrations: Vec<Migration>,
}

pub(crate) fn validate_migration_order(previous: &Schema, desired: &Schema) -> Result<(), Error> {
    let previous_names = previous
        .migrations
        .iter()
        .map(|migration| migration.name.as_str())
        .collect::<Vec<_>>();
    let retained = desired
        .migrations
        .iter()
        .map(|migration| migration.name.as_str())
        .filter(|name| previous_names.contains(name))
        .collect::<Vec<_>>();
    if retained != previous_names {
        return Err(Error::migration(format!(
            "applied migrations for '{}' changed order",
            desired.model,
        )));
    }
    Ok(())
}

pub struct SchemaSql {
    pub sqlite: String,
    pub postgres: String,
}

pub struct Seed {
    pub sql: Sql,
    pub parameters: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationPhase {
    Before,
    After,
}

pub struct DataMigration {
    pub name: String,
    pub phase: MigrationPhase,
    pub sql: Sql,
    pub parameters: Vec<Value>,
}

pub struct PostgresConstraint {
    pub name: String,
    pub definition: String,
    pub foreign_key: bool,
}

pub struct PostgresColumn {
    pub name: String,
    pub definition: String,
    pub sql_type: String,
    pub default: Option<String>,
}

pub struct Model {
    pub schema: Schema,
    pub create_table: SchemaSql,
    pub create_temporary_table: String,
    pub create_indexes: Vec<SchemaSql>,
    pub postgres_columns: Vec<PostgresColumn>,
    pub postgres_constraints: Vec<PostgresConstraint>,
    pub seeds: Vec<Seed>,
    pub data_migrations: Vec<DataMigration>,
}

impl Database {
    pub fn name(&self) -> &str {
        self.key.connection()
    }

    pub fn key(&self) -> &DatabaseKey {
        &self.key
    }

    pub fn max_page_size(&self) -> usize {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.max_page_size(),
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.max_page_size(),
        }
    }

    pub async fn begin(&self) -> Result<Transaction, Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => Ok(Transaction::Sqlite {
                key: self.key.clone(),
                lease: Arc::clone(database),
                transaction: database.begin().await?,
            }),
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => Ok(Transaction::Postgres {
                key: self.key.clone(),
                lease: Arc::clone(database),
                transaction: database.begin().await?,
            }),
        }
    }

    pub async fn execute(&self, sql: Sql, parameters: Vec<Value>) -> Result<usize, Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.execute(sql.sqlite, parameters).await,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.execute(sql.postgres, parameters).await,
        }
    }

    pub async fn query(&self, sql: Sql, parameters: Vec<Value>) -> Result<Vec<Row>, Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.query(sql.sqlite, parameters).await,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.query(sql.postgres, parameters).await,
        }
    }

    pub async fn query_bounded(
        &self,
        sql: Sql,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => {
                database
                    .query_bounded(sql.sqlite, parameters, maximum)
                    .await
            }
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => {
                database
                    .query_bounded(sql.postgres, parameters, maximum)
                    .await
            }
        }
    }

    pub async fn query_owned(
        &self,
        sql: OwnedSql,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.query_owned(sql.sqlite, parameters).await,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.query_owned(sql.postgres, parameters).await,
        }
    }

    pub async fn stream(
        &self,
        sql: Sql,
        parameters: Vec<Value>,
        capacity: usize,
    ) -> Result<RowStream<Row>, Error> {
        let stream = match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.stream(sql.sqlite, parameters, capacity).await?,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => {
                database.stream(sql.postgres, parameters, capacity).await?
            }
        };
        Ok(stream.hold_database(self.lease_owner()))
    }

    pub async fn migrate(&self, model: Model) -> Result<(), Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.migrate(model).await,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.migrate(model).await,
        }
    }

    pub async fn migrate_foreign_keys(
        &self,
        model: String,
        table: String,
        constraints: Vec<PostgresConstraint>,
    ) -> Result<(), Error> {
        let _ = (&model, &table, &constraints);
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => Ok(()),
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => {
                database
                    .migrate_foreign_keys(model, table, constraints)
                    .await
            }
        }
    }

    fn lease_owner(&self) -> Arc<dyn Send + Sync> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => Arc::clone(database) as Arc<dyn Send + Sync>,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => Arc::clone(database) as Arc<dyn Send + Sync>,
        }
    }

    pub(crate) fn exclusively_held(&self) -> bool {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => Arc::strong_count(database) == 1,
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => Arc::strong_count(database) == 1,
        }
    }

    pub(crate) async fn prepare_pool(&self) -> Result<(), Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => Ok(()),
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.prepare().await,
        }
    }

    pub(crate) async fn close_pool(&self) -> Result<(), Error> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => {
                database.close();
                Ok(())
            }
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.close().await,
        }
    }

    pub(crate) fn stop_accepting(&self) {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(database) => database.close(),
            #[cfg(feature = "postgres")]
            Backend::Postgres(database) => database.stop_accepting(),
        }
    }
}

impl Transaction {
    pub fn key(&self) -> &DatabaseKey {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite { key, .. } => key,
            #[cfg(feature = "postgres")]
            Self::Postgres { key, .. } => key,
        }
    }

    pub async fn commit(self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite {
                transaction, lease, ..
            } => {
                let result = transaction.commit().await;
                drop(lease);
                result
            }
            #[cfg(feature = "postgres")]
            Self::Postgres {
                transaction, lease, ..
            } => {
                let result = transaction.commit().await;
                drop(lease);
                result
            }
        }
    }

    pub async fn rollback(self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite {
                transaction, lease, ..
            } => {
                let result = transaction.rollback().await;
                drop(lease);
                result
            }
            #[cfg(feature = "postgres")]
            Self::Postgres {
                transaction, lease, ..
            } => {
                let result = transaction.rollback().await;
                drop(lease);
                result
            }
        }
    }
}

impl<'a> Executor<'a> {
    pub fn new(database: Database, transaction: Option<&'a Transaction>) -> Result<Self, Error> {
        let Some(transaction) = transaction else {
            return Ok(Self::Pool(database));
        };
        if transaction.key() != database.key() {
            return Err(Error::new(
                ErrorKind::Database,
                format!(
                    "transaction for database key {:?} cannot access database key {:?}",
                    transaction.key(),
                    database.key()
                ),
            ));
        }
        match (&database.backend, transaction) {
            #[cfg(feature = "sqlite")]
            (Backend::Sqlite(_), Transaction::Sqlite { transaction, .. }) => {
                Ok(Self::Sqlite(transaction))
            }
            #[cfg(feature = "postgres")]
            (Backend::Postgres(_), Transaction::Postgres { transaction, .. }) => {
                Ok(Self::Postgres(transaction))
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::new(
                ErrorKind::Database,
                "transaction database driver does not match the Model driver",
            )),
        }
    }

    pub fn max_page_size(&self) -> usize {
        match self {
            Self::Pool(database) => database.max_page_size(),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(transaction) => {
                crate::sqlite::Executor::Transaction(transaction).max_page_size()
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(transaction) => {
                crate::postgres::Executor::Transaction(transaction).max_page_size()
            }
        }
    }

    pub async fn execute(&self, sql: Sql, parameters: Vec<Value>) -> Result<usize, Error> {
        match self {
            Self::Pool(database) => database.execute(sql, parameters).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(transaction) => {
                crate::sqlite::Executor::Transaction(transaction)
                    .execute(sql.sqlite, parameters)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(transaction) => {
                crate::postgres::Executor::Transaction(transaction)
                    .execute(sql.postgres, parameters)
                    .await
            }
        }
    }

    pub async fn query(&self, sql: Sql, parameters: Vec<Value>) -> Result<Vec<Row>, Error> {
        match self {
            Self::Pool(database) => database.query(sql, parameters).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(transaction) => {
                crate::sqlite::Executor::Transaction(transaction)
                    .query(sql.sqlite, parameters)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(transaction) => {
                crate::postgres::Executor::Transaction(transaction)
                    .query(sql.postgres, parameters)
                    .await
            }
        }
    }

    pub async fn query_bounded(
        &self,
        sql: Sql,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        match self {
            Self::Pool(database) => database.query_bounded(sql, parameters, maximum).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(transaction) => {
                transaction
                    .query_bounded(sql.sqlite, parameters, maximum)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(transaction) => {
                transaction
                    .query_bounded(sql.postgres, parameters, maximum)
                    .await
            }
        }
    }

    pub async fn query_owned(
        &self,
        sql: OwnedSql,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        match self {
            Self::Pool(database) => database.query_owned(sql, parameters).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(transaction) => transaction.query_owned(sql.sqlite, parameters).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(transaction) => transaction.query_owned(sql.postgres, parameters).await,
        }
    }

    pub async fn stream(
        &self,
        sql: Sql,
        parameters: Vec<Value>,
        capacity: usize,
    ) -> Result<RowStream<Row>, Error> {
        match self {
            Self::Pool(database) => database.stream(sql, parameters, capacity).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => Err(Error::new(
                ErrorKind::Database,
                "row streams cannot escape a transaction",
            )),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => Err(Error::new(
                ErrorKind::Database,
                "row streams cannot escape a transaction",
            )),
        }
    }
}

pub fn database_for(explicit: Option<&str>, package_root: &str) -> Result<Database, Error> {
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return session.databases().database(explicit, package_root);
    }
    let (name, config) = crate::config::settings()
        .resolve_database(explicit, package_root)
        .map_err(|error| Error::new(ErrorKind::Pool, error))?;
    let backend = match config {
        DatabaseConfig::Sqlite { .. } => {
            #[cfg(feature = "sqlite")]
            {
                Backend::Sqlite(crate::sqlite::database(name)?)
            }
            #[cfg(not(feature = "sqlite"))]
            {
                return Err(Error::new(ErrorKind::Pool, "SQLite support is not bundled"));
            }
        }
        DatabaseConfig::Postgres { .. } => {
            #[cfg(feature = "postgres")]
            {
                Backend::Postgres(crate::postgres::database(name)?)
            }
            #[cfg(not(feature = "postgres"))]
            {
                return Err(Error::new(
                    ErrorKind::Pool,
                    "PostgreSQL support is not bundled",
                ));
            }
        }
    };
    Ok(Database {
        key: DatabaseKey {
            connection: name.to_owned(),
            tenant_id: None,
        },
        backend,
    })
}

pub(crate) async fn tenant_database(
    connection: &str,
    tenant_id: i64,
    create: bool,
) -> Result<Database, Error> {
    let settings = crate::config::current_settings();
    let config = settings.database(connection).ok_or_else(|| {
        Error::new(
            ErrorKind::Pool,
            format!("database connection '{connection}' is not configured"),
        )
    })?;
    let backend = match config {
        DatabaseConfig::Sqlite {
            tenant_directory: Some(directory),
            max_connections,
            max_page_size,
            ..
        } => {
            #[cfg(not(feature = "sqlite"))]
            let _ = (directory, max_connections, max_page_size, create);
            #[cfg(feature = "sqlite")]
            {
                if create {
                    std::fs::create_dir_all(directory).map_err(|error| {
                        Error::migration(format!(
                            "cannot create tenant database directory: {error}"
                        ))
                    })?;
                }
                let path = directory.join(format!("tenant_{tenant_id}.db"));
                if !create && !path.is_file() {
                    return Err(Error::migration("tenant database is not provisioned"));
                }
                Backend::Sqlite(Arc::new(crate::sqlite::Database::open(
                    path,
                    *max_connections,
                    *max_page_size,
                )?))
            }
            #[cfg(not(feature = "sqlite"))]
            {
                return Err(Error::new(ErrorKind::Pool, "SQLite support is not bundled"));
            }
        }
        DatabaseConfig::Postgres {
            url,
            tenant_database_prefix: Some(prefix),
            tls,
            min_connections,
            max_connections,
            max_page_size,
            wait_timeout_ms,
            io_timeout_ms,
            ..
        } => {
            #[cfg(not(feature = "postgres"))]
            let _ = (
                url,
                prefix,
                tls,
                min_connections,
                max_connections,
                max_page_size,
                wait_timeout_ms,
                io_timeout_ms,
                create,
            );
            #[cfg(feature = "postgres")]
            {
                let database_name = format!("{prefix}_{tenant_id}");
                if create {
                    crate::postgres::create_database(
                        url,
                        &database_name,
                        *tls,
                        std::time::Duration::from_millis(*wait_timeout_ms),
                        std::time::Duration::from_millis(*io_timeout_ms),
                    )
                    .await?;
                }
                Backend::Postgres(Arc::new(crate::postgres::Database::open(
                    crate::postgres::ConnectionOptions {
                        url,
                        database_name: Some(&database_name),
                        tls: *tls,
                        min_connections: *min_connections,
                        max_connections: *max_connections,
                        max_page_size: *max_page_size,
                        wait_timeout: std::time::Duration::from_millis(*wait_timeout_ms),
                        io_timeout: std::time::Duration::from_millis(*io_timeout_ms),
                    },
                )?))
            }
            #[cfg(not(feature = "postgres"))]
            {
                return Err(Error::new(
                    ErrorKind::Pool,
                    "PostgreSQL support is not bundled",
                ));
            }
        }
        DatabaseConfig::Sqlite {
            tenant_directory: None,
            ..
        } => {
            return Err(Error::new(
                ErrorKind::Pool,
                format!(
                    "database.{connection}.tenant_directory is required for tenant database mode"
                ),
            ));
        }
        DatabaseConfig::Postgres {
            tenant_database_prefix: None,
            ..
        } => {
            return Err(Error::new(
                ErrorKind::Pool,
                format!(
                    "database.{connection}.tenant_database_prefix is required for tenant database mode"
                ),
            ));
        }
    };
    Ok(Database {
        key: DatabaseKey {
            connection: connection.to_owned(),
            tenant_id: Some(tenant_id),
        },
        backend,
    })
}

pub async fn prepare() -> Result<(), Error> {
    #[cfg(feature = "postgres")]
    crate::postgres::prepare().await?;
    Ok(())
}

pub async fn shutdown() -> Result<(), Error> {
    crate::tenant::shutdown().await?;
    #[cfg(feature = "sqlite")]
    crate::sqlite::shutdown();
    #[cfg(feature = "postgres")]
    crate::postgres::shutdown().await?;
    Ok(())
}

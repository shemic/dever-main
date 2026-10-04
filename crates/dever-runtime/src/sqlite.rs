use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use deadpool_sqlite::rusqlite::types::{Value as SqliteValue, ValueRef};
use deadpool_sqlite::{Config, Hook, HookError, Pool, PoolConfig, Runtime, Timeouts};
use tokio::sync::OnceCell;

use crate::config::{Database as DatabaseConfig, Settings};
use crate::orm::{Error, ErrorKind, Row, RowStream, Value};
use crate::pool::{CREATE_TIMEOUT, PoolObjectGuard, RECYCLE_TIMEOUT, WAIT_TIMEOUT};

mod migration;

const CONNECTION_SETUP: &str = "\
PRAGMA foreign_keys = ON;\n\
PRAGMA synchronous = NORMAL;\n\
PRAGMA busy_timeout = 5000;";

static DATABASES: OnceLock<BTreeMap<String, Arc<Database>>> = OnceLock::new();

pub struct Database {
    pool: Pool,
    max_page_size: usize,
    initialized: OnceCell<()>,
}

pub struct Transaction {
    connection: Option<deadpool_sqlite::Object>,
    max_page_size: usize,
}

pub enum Executor<'a> {
    Pool(&'a Database),
    Transaction(&'a Transaction),
}

impl Database {
    pub fn open(
        path: impl Into<std::path::PathBuf>,
        max_connections: usize,
        max_page_size: usize,
    ) -> Result<Self, Error> {
        let config = Config::new(path);
        let manager = deadpool_sqlite::Manager::from_config(&config, Runtime::Tokio1);
        let mut pool_config = PoolConfig::new(max_connections);
        pool_config.timeouts = Timeouts {
            wait: Some(WAIT_TIMEOUT),
            create: Some(CREATE_TIMEOUT),
            recycle: Some(RECYCLE_TIMEOUT),
        };
        let pool = Pool::builder(manager)
            .config(pool_config)
            .runtime(Runtime::Tokio1)
            .post_create(Hook::async_fn(|connection, _| {
                Box::pin(async move {
                    connection
                        .interact(|connection| connection.execute_batch(CONNECTION_SETUP))
                        .await
                        .map_err(|error| HookError::message(error.to_string()))?
                        .map_err(HookError::Backend)
                })
            }))
            .build()
            .map_err(|error| Error::new(ErrorKind::Pool, error.to_string()))?;
        Ok(Self {
            pool,
            max_page_size,
            initialized: OnceCell::new(),
        })
    }

    pub fn max_page_size(&self) -> usize {
        self.max_page_size
    }

    pub(crate) fn close(&self) {
        self.pool.close();
    }

    pub async fn execute(&self, sql: &'static str, parameters: Vec<Value>) -> Result<usize, Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = execute_on(connection.object(), sql, parameters).await;
        drop(connection.release());
        result
    }

    pub async fn query(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = query_on(connection.object(), sql, parameters).await;
        drop(connection.release());
        result
    }

    pub async fn query_bounded(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = query_bounded_on(connection.object(), sql, parameters, maximum).await;
        drop(connection.release());
        result
    }

    pub async fn query_owned(
        &self,
        sql: String,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = query_owned_on(connection.object(), sql, parameters).await;
        drop(connection.release());
        result
    }

    pub async fn stream(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        capacity: usize,
    ) -> Result<RowStream<Row>, Error> {
        self.prepare().await?;
        if capacity == 0 || capacity > self.max_page_size {
            return Err(Error::invalid_data(format!(
                "row stream capacity must be between 1 and {}",
                self.max_page_size
            )));
        }
        let connection = self.connection().await?;
        let (rows, receiver) = tokio::sync::mpsc::channel(capacity);
        let (ready, initialized) = tokio::sync::oneshot::channel();
        let (worker, spawner, worker_fault) =
            crate::task::scoped_owner(()).map_err(|error| Error::new(ErrorKind::Pool, error))?;
        spawner.spawn(async move {
            let connection = guarded(connection);
            let result = connection
                .object()
                .interact(move |connection| stream_rows(connection, sql, parameters, rows, ready))
                .await;
            if matches!(result, Ok(true)) {
                drop(connection.release());
            }
        });
        initialized
            .await
            .map_err(|_| Error::new(ErrorKind::Pool, "SQLite row stream worker stopped"))??;
        Ok(RowStream::from_receiver(receiver, worker, worker_fault))
    }

    pub async fn execute_batch(&self, sql: String) -> Result<(), Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = connection
            .object()
            .interact(move |connection| connection.execute_batch(&sql))
            .await
            .map_err(interact_error)?
            .map_err(database_error);
        drop(connection.release());
        result
    }

    pub async fn begin(&self) -> Result<Transaction, Error> {
        self.begin_with("BEGIN IMMEDIATE").await
    }

    async fn begin_with(&self, statement: &'static str) -> Result<Transaction, Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        batch_on(connection.object(), statement).await?;
        Ok(Transaction {
            connection: Some(connection.release()),
            max_page_size: self.max_page_size,
        })
    }

    pub async fn migrate(&self, model: crate::database::Model) -> Result<(), Error> {
        self.prepare().await?;
        let connection = guarded(self.connection().await?);
        let result = migration::apply(connection.object(), model).await;
        if result.is_ok() {
            drop(connection.release());
        }
        result
    }

    async fn prepare(&self) -> Result<(), Error> {
        self.initialized
            .get_or_try_init(|| async {
                let connection = guarded(self.connection().await?);
                let result = connection
                    .object()
                    .interact(|connection| connection.pragma_update(None, "journal_mode", "WAL"))
                    .await
                    .map_err(interact_error)?
                    .map_err(database_error);
                if result.is_ok() {
                    drop(connection.release());
                }
                result
            })
            .await
            .copied()
    }

    async fn connection(&self) -> Result<deadpool_sqlite::Object, Error> {
        self.pool
            .get()
            .await
            .map_err(|error| Error::new(ErrorKind::Pool, error.to_string()))
    }
}

impl Transaction {
    pub async fn execute(&self, sql: &'static str, parameters: Vec<Value>) -> Result<usize, Error> {
        execute_on(self.connection(), sql, parameters).await
    }

    pub async fn query(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        query_on(self.connection(), sql, parameters).await
    }

    pub async fn query_bounded(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        query_bounded_on(self.connection(), sql, parameters, maximum).await
    }

    pub async fn query_owned(
        &self,
        sql: String,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        query_owned_on(self.connection(), sql, parameters).await
    }

    pub async fn commit(mut self) -> Result<(), Error> {
        self.finish("COMMIT").await
    }

    pub async fn rollback(mut self) -> Result<(), Error> {
        self.finish("ROLLBACK").await
    }

    fn connection(&self) -> &deadpool_sqlite::Object {
        self.connection.as_ref().expect("open SQLite transaction")
    }

    async fn finish(&mut self, sql: &'static str) -> Result<(), Error> {
        let connection = guarded(self.connection.take().expect("open SQLite transaction"));
        let result = batch_on(connection.object(), sql).await;
        if result.is_ok() {
            drop(connection.release());
        }
        result
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(deadpool_sqlite::Object::take(connection));
        }
    }
}

fn guarded(connection: deadpool_sqlite::Object) -> PoolObjectGuard<deadpool_sqlite::Object> {
    PoolObjectGuard::new(connection, detach)
}

fn detach(connection: deadpool_sqlite::Object) {
    drop(deadpool_sqlite::Object::take(connection));
}

impl Executor<'_> {
    pub fn max_page_size(&self) -> usize {
        match self {
            Self::Pool(database) => database.max_page_size(),
            Self::Transaction(transaction) => transaction.max_page_size,
        }
    }

    pub async fn execute(&self, sql: &'static str, parameters: Vec<Value>) -> Result<usize, Error> {
        match self {
            Self::Pool(database) => database.execute(sql, parameters).await,
            Self::Transaction(transaction) => transaction.execute(sql, parameters).await,
        }
    }

    pub async fn query(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        match self {
            Self::Pool(database) => database.query(sql, parameters).await,
            Self::Transaction(transaction) => transaction.query(sql, parameters).await,
        }
    }
}

pub fn initialize(settings: &Settings) -> Result<(), String> {
    DATABASES
        .set(configured_databases(settings)?)
        .map_err(|_| "SQLite databases were initialized more than once".to_owned())
}

pub(crate) fn configured_databases(
    settings: &Settings,
) -> Result<BTreeMap<String, Arc<Database>>, String> {
    let mut databases = BTreeMap::new();
    for (name, config) in settings.databases() {
        if let DatabaseConfig::Sqlite {
            path,
            max_connections,
            max_page_size,
            ..
        } = config
        {
            let database = Database::open(path, *max_connections, *max_page_size)
                .map_err(|error| format!("cannot initialize database.{name}: {error}"))?;
            databases.insert(name.clone(), Arc::new(database));
        }
    }
    Ok(databases)
}

pub fn database(name: &str) -> Result<Arc<Database>, Error> {
    DATABASES
        .get()
        .and_then(|databases| databases.get(name))
        .cloned()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Pool,
                format!("SQLite database '{name}' is not initialized"),
            )
        })
}

pub fn shutdown() {
    if let Some(databases) = DATABASES.get() {
        for database in databases.values() {
            database.close();
        }
    }
}

pub fn database_for(explicit: Option<&str>, package_root: &str) -> Result<Arc<Database>, Error> {
    let (name, config) = crate::config::settings()
        .resolve_database(explicit, package_root)
        .map_err(|error| Error::new(ErrorKind::Pool, error))?;
    if !matches!(config, DatabaseConfig::Sqlite { .. }) {
        return Err(Error::new(
            ErrorKind::Pool,
            format!("database connection '{name}' is not SQLite"),
        ));
    }
    database(name)
}

fn sqlite_values(parameters: Vec<Value>) -> Vec<SqliteValue> {
    parameters
        .into_iter()
        .map(|value| match value {
            Value::Null => SqliteValue::Null,
            Value::Bool(value) => SqliteValue::Integer(i64::from(value)),
            Value::Int(value) => SqliteValue::Integer(value),
            Value::Float(value) => SqliteValue::Real(value),
            Value::Decimal(value) => SqliteValue::Text(value),
            Value::Text(value) => SqliteValue::Text(value),
            Value::Bytes(value) => SqliteValue::Blob(value),
            Value::Uuid(value) => SqliteValue::Blob(value.into_bytes().to_vec()),
        })
        .collect()
}

async fn execute_on(
    connection: &deadpool_sqlite::Object,
    sql: &'static str,
    parameters: Vec<Value>,
) -> Result<usize, Error> {
    connection
        .interact(move |connection| {
            let parameters = sqlite_values(parameters);
            connection.execute(sql, deadpool_sqlite::rusqlite::params_from_iter(parameters))
        })
        .await
        .map_err(interact_error)?
        .map_err(database_error)
}

async fn query_on(
    connection: &deadpool_sqlite::Object,
    sql: &'static str,
    parameters: Vec<Value>,
) -> Result<Vec<Row>, Error> {
    connection
        .interact(move |connection| {
            let parameters = sqlite_values(parameters);
            let mut statement = connection.prepare_cached(sql)?;
            let column_count = statement.column_count();
            let mut rows =
                statement.query(deadpool_sqlite::rusqlite::params_from_iter(parameters))?;
            let mut output = Vec::new();
            while let Some(row) = rows.next()? {
                let values = (0..column_count)
                    .map(|index| value(index, row.get_ref(index)?))
                    .collect::<Result<Vec<_>, _>>()?;
                output.push(Row::new(values));
            }
            Ok(output)
        })
        .await
        .map_err(interact_error)?
        .map_err(database_error)
}

async fn query_bounded_on(
    connection: &deadpool_sqlite::Object,
    sql: &'static str,
    parameters: Vec<Value>,
    maximum: usize,
) -> Result<Vec<Row>, Error> {
    let rows = connection
        .interact(move |connection| {
            let parameters = sqlite_values(parameters);
            let mut statement = connection.prepare_cached(sql)?;
            let column_count = statement.column_count();
            let mut rows =
                statement.query(deadpool_sqlite::rusqlite::params_from_iter(parameters))?;
            let mut output = Vec::with_capacity(maximum.saturating_add(1));
            while output.len() <= maximum {
                let Some(row) = rows.next()? else {
                    break;
                };
                let values = (0..column_count)
                    .map(|index| value(index, row.get_ref(index)?))
                    .collect::<Result<Vec<_>, _>>()?;
                output.push(Row::new(values));
            }
            Ok(output)
        })
        .await
        .map_err(interact_error)?
        .map_err(database_error)?;
    crate::orm::bounded_rows(rows, maximum)
}

async fn query_owned_on(
    connection: &deadpool_sqlite::Object,
    sql: String,
    parameters: Vec<Value>,
) -> Result<Vec<Row>, Error> {
    connection
        .interact(move |connection| {
            let parameters = sqlite_values(parameters);
            let mut statement = connection.prepare(&sql)?;
            let column_count = statement.column_count();
            let mut rows =
                statement.query(deadpool_sqlite::rusqlite::params_from_iter(parameters))?;
            let mut output = Vec::new();
            while let Some(row) = rows.next()? {
                let values = (0..column_count)
                    .map(|index| value(index, row.get_ref(index)?))
                    .collect::<Result<Vec<_>, _>>()?;
                output.push(Row::new(values));
            }
            Ok(output)
        })
        .await
        .map_err(interact_error)?
        .map_err(database_error)
}

fn stream_rows(
    connection: &mut deadpool_sqlite::rusqlite::Connection,
    sql: &'static str,
    parameters: Vec<Value>,
    rows: tokio::sync::mpsc::Sender<Result<Row, Error>>,
    ready: tokio::sync::oneshot::Sender<Result<(), Error>>,
) -> bool {
    let parameters = sqlite_values(parameters);
    let mut statement = match connection.prepare_cached(sql) {
        Ok(statement) => statement,
        Err(error) => {
            let _ = ready.send(Err(database_error(error)));
            return false;
        }
    };
    let column_count = statement.column_count();
    let mut source = match statement.query(deadpool_sqlite::rusqlite::params_from_iter(parameters))
    {
        Ok(source) => source,
        Err(error) => {
            let _ = ready.send(Err(database_error(error)));
            return false;
        }
    };
    if ready.send(Ok(())).is_err() {
        return true;
    }
    loop {
        let row = match source.next() {
            Ok(Some(row)) => row,
            Ok(None) => return true,
            Err(error) => {
                let _ = rows.blocking_send(Err(database_error(error)));
                return false;
            }
        };
        let values = (0..column_count)
            .map(|index| value(index, row.get_ref(index)?))
            .collect::<Result<Vec<_>, deadpool_sqlite::rusqlite::Error>>();
        let item = values.map(Row::new).map_err(database_error);
        let healthy = item.is_ok();
        if rows.blocking_send(item).is_err() {
            return true;
        }
        if !healthy {
            return false;
        }
    }
}

async fn batch_on(connection: &deadpool_sqlite::Object, sql: &'static str) -> Result<(), Error> {
    connection
        .interact(move |connection| connection.execute_batch(sql))
        .await
        .map_err(interact_error)?
        .map_err(database_error)
}

fn value(column: usize, value: ValueRef<'_>) -> Result<Value, deadpool_sqlite::rusqlite::Error> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Int(value),
        ValueRef::Real(value) => Value::Float(value),
        ValueRef::Text(value) => Value::Text(
            std::str::from_utf8(value)
                .map_err(|error| deadpool_sqlite::rusqlite::Error::Utf8Error(column, error))?
                .to_owned(),
        ),
        ValueRef::Blob(value) => Value::Bytes(value.to_vec()),
    })
}

fn interact_error(error: deadpool_sqlite::InteractError) -> Error {
    Error::new(ErrorKind::Pool, error.to_string())
}

fn database_error(error: deadpool_sqlite::rusqlite::Error) -> Error {
    let kind = match &error {
        deadpool_sqlite::rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == deadpool_sqlite::rusqlite::ErrorCode::ConstraintViolation =>
        {
            ErrorKind::Constraint
        }
        _ => ErrorKind::Database,
    };
    Error::new(kind, error.to_string())
}

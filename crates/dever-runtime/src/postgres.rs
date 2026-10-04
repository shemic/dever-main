use std::collections::BTreeMap;
use std::error::Error as _;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use deadpool_postgres::{
    Manager, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime, Timeouts,
};
use futures_util::TryStreamExt;
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type, to_sql_checked};

use crate::config::{Database as DatabaseConfig, PostgresTls, Settings};
use crate::orm::{Error, ErrorKind, Row, RowStream, Uuid, Value};
use crate::pool::{CREATE_TIMEOUT, PoolObjectGuard, RECYCLE_TIMEOUT};

mod migration;

const STATEMENT_CACHE_CAPACITY: usize = 64;

static DATABASES: OnceLock<BTreeMap<String, Arc<Database>>> = OnceLock::new();

pub struct Database {
    pool: Pool,
    min_connections: usize,
    max_page_size: usize,
    io_timeout: Duration,
}

pub struct ConnectionOptions<'a> {
    pub url: &'a str,
    pub database_name: Option<&'a str>,
    pub tls: PostgresTls,
    pub min_connections: usize,
    pub max_connections: usize,
    pub max_page_size: usize,
    pub wait_timeout: Duration,
    pub io_timeout: Duration,
}

pub struct Transaction {
    connection: Option<deadpool_postgres::Object>,
    max_page_size: usize,
    io_timeout: Duration,
}

pub enum Executor<'a> {
    Pool(&'a Database),
    Transaction(&'a Transaction),
}

struct Parameter(Value);
struct NumericText(String);
#[derive(Debug)]
struct UuidBytes(Uuid);

impl Database {
    pub fn open(options: ConnectionOptions<'_>) -> Result<Self, Error> {
        let ConnectionOptions {
            url,
            database_name,
            tls,
            min_connections,
            max_connections,
            max_page_size,
            wait_timeout,
            io_timeout,
        } = options;
        let mut config = tokio_postgres::Config::from_str(url)
            .map_err(|error| Error::new(ErrorKind::Pool, error.to_string()))?;
        if let Some(database_name) = database_name {
            config.dbname(database_name);
        }
        config.connect_timeout(CREATE_TIMEOUT);
        let pool = match tls {
            PostgresTls::Disabled => {
                config.ssl_mode(tokio_postgres::config::SslMode::Disable);
                build_pool(config, tokio_postgres::NoTls, max_connections, wait_timeout)?
            }
            PostgresTls::System => {
                config.ssl_mode(tokio_postgres::config::SslMode::Require);
                let (tls, _) = tokio_postgres_rustls::MakeRustlsConnect::with_native_certs()
                    .map_err(|errors| {
                        Error::new(
                            ErrorKind::Pool,
                            errors
                                .into_iter()
                                .map(|error| error.to_string())
                                .collect::<Vec<_>>()
                                .join("; "),
                        )
                    })?;
                build_pool(config, tls, max_connections, wait_timeout)?
            }
        };
        Ok(Self {
            pool,
            min_connections,
            max_page_size,
            io_timeout,
        })
    }

    pub fn max_page_size(&self) -> usize {
        self.max_page_size
    }

    pub(crate) async fn prepare(&self) -> Result<(), Error> {
        let mut connections = Vec::with_capacity(self.min_connections);
        for _ in 0..self.min_connections {
            connections.push(self.connection().await?);
        }
        Ok(())
    }

    pub async fn execute(&self, sql: &'static str, parameters: Vec<Value>) -> Result<usize, Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            execute_on(connection.object(), sql, parameters),
        )
        .await;
        finish(connection, result)
    }

    pub async fn query(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            query_on(connection.object(), sql, parameters),
        )
        .await;
        finish(connection, result)
    }

    pub async fn query_bounded(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            query_bounded_on(connection.object(), sql, parameters, maximum),
        )
        .await;
        finish(connection, result)
    }

    pub async fn query_owned(
        &self,
        sql: String,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            query_owned_on(connection.object(), &sql, parameters),
        )
        .await;
        finish(connection, result)
    }

    pub async fn stream(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        capacity: usize,
    ) -> Result<RowStream<Row>, Error> {
        if capacity == 0 || capacity > self.max_page_size {
            return Err(Error::invalid_data(format!(
                "row stream capacity must be between 1 and {}",
                self.max_page_size
            )));
        }
        let connection = self.connection().await?;
        let io_timeout = self.io_timeout;
        let (rows, receiver) = tokio::sync::mpsc::channel(capacity);
        let (ready, initialized) = tokio::sync::oneshot::channel();
        let (worker, spawner, worker_fault) =
            crate::task::scoped_owner(()).map_err(|error| Error::new(ErrorKind::Pool, error))?;
        spawner.spawn(async move {
            let connection = guarded(connection);
            let mut ready = Some(ready);
            let result = async {
                let statement = io(io_timeout, prepare_cached(connection.object(), sql)).await?;
                let parameters = parameters.into_iter().map(Parameter).collect::<Vec<_>>();
                let references = parameters
                    .iter()
                    .map(|value| value as &(dyn ToSql + Sync))
                    .collect::<Vec<_>>();
                let source = io(io_timeout, async {
                    connection
                        .object()
                        .query_raw(&statement, references)
                        .await
                        .map_err(database_error)
                })
                .await?;
                tokio::pin!(source);
                ready
                    .take()
                    .expect("pending row stream initialization")
                    .send(Ok(()))
                    .ok();
                while let Some(row) = io(io_timeout, async {
                    source.as_mut().try_next().await.map_err(database_error)
                })
                .await?
                {
                    if rows.send(decode_row(row)).await.is_err() {
                        return Err(Error::new(
                            ErrorKind::Cancelled,
                            "PostgreSQL row stream consumer closed",
                        ));
                    }
                }
                Ok::<_, Error>(())
            }
            .await;
            match result {
                Ok(()) => drop(connection.release()),
                Err(error) => {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Err(error));
                    } else {
                        let _ = rows.send(Err(error)).await;
                    }
                }
            }
        });
        initialized
            .await
            .map_err(|_| Error::new(ErrorKind::Pool, "PostgreSQL row stream worker stopped"))??;
        Ok(RowStream::from_receiver(receiver, worker, worker_fault))
    }

    pub async fn begin(&self) -> Result<Transaction, Error> {
        let connection = guarded(self.connection().await?);
        io(self.io_timeout, async {
            connection
                .object()
                .batch_execute("BEGIN")
                .await
                .map_err(database_error)
        })
        .await?;
        Ok(Transaction {
            connection: Some(connection.release()),
            max_page_size: self.max_page_size,
            io_timeout: self.io_timeout,
        })
    }

    pub async fn migrate(&self, model: crate::database::Model) -> Result<(), Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            migration::apply(connection.object(), model),
        )
        .await;
        finish(connection, result)
    }

    pub async fn migrate_foreign_keys(
        &self,
        model: String,
        table: String,
        constraints: Vec<crate::database::PostgresConstraint>,
    ) -> Result<(), Error> {
        let connection = guarded(self.connection().await?);
        let result = io(
            self.io_timeout,
            migration::apply_foreign_keys(connection.object(), &model, &table, &constraints),
        )
        .await;
        finish(connection, result)
    }

    async fn connection(&self) -> Result<deadpool_postgres::Object, Error> {
        self.pool.get().await.map_err(pool_error)
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.stop_accepting();
        self.drain().await
    }

    pub(crate) fn stop_accepting(&self) {
        self.pool.close();
    }

    async fn drain(&self) -> Result<(), Error> {
        io(self.io_timeout, async {
            while self.pool.status().size != 0 {
                tokio::task::yield_now().await;
            }
            Ok(())
        })
        .await
    }
}

pub(crate) async fn create_database(
    url: &str,
    database_name: &str,
    tls: PostgresTls,
    wait_timeout: Duration,
    io_timeout: Duration,
) -> Result<(), Error> {
    if database_name.is_empty()
        || database_name.len() > 63
        || !database_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(Error::migration("invalid PostgreSQL tenant database name"));
    }
    let database = Database::open(ConnectionOptions {
        url,
        database_name: None,
        tls,
        min_connections: 0,
        max_connections: 1,
        max_page_size: 1,
        wait_timeout,
        io_timeout,
    })?;
    let connection = guarded(database.connection().await?);
    let sql = format!("CREATE DATABASE \"{database_name}\"");
    let result = io(io_timeout, async {
        match connection.object().batch_execute(&sql).await {
            Ok(()) => Ok(()),
            Err(error) if error.code().is_some_and(|code| code.code() == "42P04") => Ok(()),
            Err(error) => Err(database_error(error)),
        }
    })
    .await;
    let result = finish(connection, result);
    database.close().await?;
    result
}

impl Transaction {
    pub async fn execute(&self, sql: &'static str, parameters: Vec<Value>) -> Result<usize, Error> {
        io(
            self.io_timeout,
            execute_on(self.connection(), sql, parameters),
        )
        .await
    }

    pub async fn query(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        io(
            self.io_timeout,
            query_on(self.connection(), sql, parameters),
        )
        .await
    }

    pub async fn query_bounded(
        &self,
        sql: &'static str,
        parameters: Vec<Value>,
        maximum: usize,
    ) -> Result<Vec<Row>, Error> {
        io(
            self.io_timeout,
            query_bounded_on(self.connection(), sql, parameters, maximum),
        )
        .await
    }

    pub async fn query_owned(
        &self,
        sql: String,
        parameters: Vec<Value>,
    ) -> Result<Vec<Row>, Error> {
        io(
            self.io_timeout,
            query_owned_on(self.connection(), &sql, parameters),
        )
        .await
    }

    pub async fn commit(mut self) -> Result<(), Error> {
        self.finish("COMMIT").await
    }

    pub async fn rollback(mut self) -> Result<(), Error> {
        self.finish("ROLLBACK").await
    }

    fn connection(&self) -> &deadpool_postgres::Object {
        self.connection
            .as_ref()
            .expect("open PostgreSQL transaction")
    }

    async fn finish(&mut self, sql: &'static str) -> Result<(), Error> {
        let connection = guarded(self.connection.take().expect("open PostgreSQL transaction"));
        let result = io(self.io_timeout, async {
            connection
                .object()
                .batch_execute(sql)
                .await
                .map_err(database_error)
        })
        .await;
        finish(connection, result)
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(deadpool_postgres::Object::take(connection));
        }
    }
}

fn guarded(connection: deadpool_postgres::Object) -> PoolObjectGuard<deadpool_postgres::Object> {
    PoolObjectGuard::new(connection, detach)
}

fn detach(connection: deadpool_postgres::Object) {
    drop(deadpool_postgres::Object::take(connection));
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
        .map_err(|_| "PostgreSQL databases were initialized more than once".to_owned())
}

pub(crate) fn configured_databases(
    settings: &Settings,
) -> Result<BTreeMap<String, Arc<Database>>, String> {
    let mut databases = BTreeMap::new();
    for (name, config) in settings.databases() {
        if let DatabaseConfig::Postgres {
            url,
            tls,
            min_connections,
            max_connections,
            max_page_size,
            wait_timeout_ms,
            io_timeout_ms,
            ..
        } = config
        {
            let database = Database::open(ConnectionOptions {
                url,
                database_name: None,
                tls: *tls,
                min_connections: *min_connections,
                max_connections: *max_connections,
                max_page_size: *max_page_size,
                wait_timeout: Duration::from_millis(*wait_timeout_ms),
                io_timeout: Duration::from_millis(*io_timeout_ms),
            })
            .map_err(|error| format!("cannot initialize database.{name}: {error}"))?;
            databases.insert(name.clone(), Arc::new(database));
        }
    }
    Ok(databases)
}

pub async fn prepare() -> Result<(), Error> {
    for database in DATABASES
        .get()
        .expect("PostgreSQL databases are initialized before prepare")
        .values()
    {
        database.prepare().await?;
    }
    Ok(())
}

pub async fn shutdown() -> Result<(), Error> {
    let databases = DATABASES
        .get()
        .expect("PostgreSQL databases are initialized before shutdown")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for database in &databases {
        database.stop_accepting();
    }
    let mut first_error = None;
    for database in databases {
        if let Err(error) = database.drain().await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub fn database(name: &str) -> Result<Arc<Database>, Error> {
    DATABASES
        .get()
        .and_then(|databases| databases.get(name))
        .cloned()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Pool,
                format!("PostgreSQL database '{name}' is not initialized"),
            )
        })
}

fn build_pool<T>(
    config: tokio_postgres::Config,
    tls: T,
    max_connections: usize,
    wait_timeout: Duration,
) -> Result<Pool, Error>
where
    T: tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket> + Clone + Send + Sync + 'static,
    T::Stream: Send + Sync,
    T::TlsConnect: Send + Sync,
    <T::TlsConnect as tokio_postgres::tls::TlsConnect<tokio_postgres::Socket>>::Future: Send,
{
    let manager = Manager::from_config(
        config,
        tls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    let mut pool_config = PoolConfig::new(max_connections);
    pool_config.timeouts = Timeouts {
        wait: Some(wait_timeout),
        create: Some(CREATE_TIMEOUT),
        recycle: Some(RECYCLE_TIMEOUT),
    };
    Pool::builder(manager)
        .config(pool_config)
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|error| Error::new(ErrorKind::Pool, error.to_string()))
}

async fn execute_on(
    connection: &deadpool_postgres::Object,
    sql: &'static str,
    parameters: Vec<Value>,
) -> Result<usize, Error> {
    let statement = prepare_cached(connection, sql).await?;
    let parameters = parameters.into_iter().map(Parameter).collect::<Vec<_>>();
    let references = parameters
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    connection
        .execute(&statement, &references)
        .await
        .map(|affected| affected as usize)
        .map_err(database_error)
}

async fn query_on(
    connection: &deadpool_postgres::Object,
    sql: &'static str,
    parameters: Vec<Value>,
) -> Result<Vec<Row>, Error> {
    let statement = prepare_cached(connection, sql).await?;
    let parameters = parameters.into_iter().map(Parameter).collect::<Vec<_>>();
    let references = parameters
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    connection
        .query(&statement, &references)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(decode_row)
        .collect()
}

async fn query_bounded_on(
    connection: &deadpool_postgres::Object,
    sql: &'static str,
    parameters: Vec<Value>,
    maximum: usize,
) -> Result<Vec<Row>, Error> {
    let statement = prepare_cached(connection, sql).await?;
    let parameters = parameters.into_iter().map(Parameter).collect::<Vec<_>>();
    let references = parameters
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    let rows = connection
        .query_raw(&statement, references)
        .await
        .map_err(database_error)?;
    tokio::pin!(rows);
    let mut output = Vec::with_capacity(maximum.saturating_add(1));
    while output.len() <= maximum {
        let Some(row) = rows.try_next().await.map_err(database_error)? else {
            break;
        };
        output.push(decode_row(row)?);
    }
    crate::orm::bounded_rows(output, maximum)
}

async fn query_owned_on(
    connection: &deadpool_postgres::Object,
    sql: &str,
    parameters: Vec<Value>,
) -> Result<Vec<Row>, Error> {
    let statement = connection.prepare(sql).await.map_err(database_error)?;
    let parameters = parameters.into_iter().map(Parameter).collect::<Vec<_>>();
    let references = parameters
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    connection
        .query(&statement, &references)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(decode_row)
        .collect()
}

async fn prepare_cached(
    connection: &deadpool_postgres::Object,
    sql: &'static str,
) -> Result<tokio_postgres::Statement, Error> {
    let statement = connection
        .prepare_cached(sql)
        .await
        .map_err(database_error)?;
    if connection.statement_cache.size() > STATEMENT_CACHE_CAPACITY {
        connection.statement_cache.remove(sql, &[]);
    }
    Ok(statement)
}

fn decode_row(row: tokio_postgres::Row) -> Result<Row, Error> {
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        let value = match *column.type_() {
            Type::BOOL => optional(&row, index, |value: bool| Value::Bool(value))?,
            Type::INT2 => optional(&row, index, |value: i16| Value::Int(i64::from(value)))?,
            Type::INT4 => optional(&row, index, |value: i32| Value::Int(i64::from(value)))?,
            Type::INT8 => optional(&row, index, Value::Int)?,
            Type::FLOAT4 => optional(&row, index, |value: f32| Value::Float(f64::from(value)))?,
            Type::FLOAT8 => optional(&row, index, Value::Float)?,
            Type::BYTEA => optional(&row, index, Value::Bytes)?,
            Type::NUMERIC => optional(&row, index, |value: NumericText| Value::Decimal(value.0))?,
            Type::UUID => optional(&row, index, |value: UuidBytes| Value::Uuid(value.0))?,
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => {
                optional(&row, index, Value::Text)?
            }
            ref ty => {
                return Err(Error::invalid_data(format!(
                    "unsupported PostgreSQL column type '{ty}'"
                )));
            }
        };
        values.push(value);
    }
    Ok(Row::new(values))
}

fn optional<T>(
    row: &tokio_postgres::Row,
    index: usize,
    convert: impl FnOnce(T) -> Value,
) -> Result<Value, Error>
where
    T: for<'a> tokio_postgres::types::FromSql<'a>,
{
    row.try_get::<_, Option<T>>(index)
        .map(|value| value.map(convert).unwrap_or(Value::Null))
        .map_err(database_error)
}

impl ToSql for Parameter {
    fn to_sql(
        &self,
        ty: &Type,
        output: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        match &self.0 {
            Value::Null => Ok(IsNull::Yes),
            Value::Bool(value) => value.to_sql(ty, output),
            Value::Int(value) => value.to_sql(ty, output),
            Value::Float(value) => value.to_sql(ty, output),
            Value::Decimal(value) => crate::orm::decimal_text(value)?.to_sql(ty, output),
            Value::Text(value) => value.to_sql(ty, output),
            Value::Bytes(value) => value.to_sql(ty, output),
            Value::Uuid(value) => UuidBytes(*value).to_sql(ty, output),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

impl<'a> FromSql<'a> for NumericText {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        decode_numeric(raw)
            .map(Self)
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message).into())
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
}

impl<'a> FromSql<'a> for UuidBytes {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        raw.try_into().map(Uuid::from_bytes).map(Self).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "PostgreSQL UUID is not 16 bytes",
            )
            .into()
        })
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::UUID
    }
}

impl ToSql for UuidBytes {
    fn to_sql(
        &self,
        _: &Type,
        output: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        output.extend_from_slice(&self.0.into_bytes());
        Ok(IsNull::No)
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::UUID
    }

    to_sql_checked!();
}

fn decode_numeric(raw: &[u8]) -> Result<String, &'static str> {
    if raw.len() < 8 || !(raw.len() - 8).is_multiple_of(2) {
        return Err("invalid PostgreSQL NUMERIC payload");
    }
    let read = |offset: usize| i16::from_be_bytes([raw[offset], raw[offset + 1]]);
    let digit_count = read(0);
    let weight = read(2);
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let scale = read(6);
    if digit_count < 0
        || scale < 0
        || raw.len() != 8 + usize::try_from(digit_count).unwrap_or(usize::MAX) * 2
        || !matches!(sign, 0x0000 | 0x4000)
    {
        return Err("unsupported PostgreSQL NUMERIC payload");
    }
    let digits = raw[8..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|digit| u16::from_be_bytes([digit[0], digit[1]]))
        .collect::<Vec<_>>();
    if digits.iter().any(|digit| *digit >= 10_000) {
        return Err("invalid PostgreSQL NUMERIC digit");
    }
    let digit_at = |exponent: i16| {
        let index = i32::from(weight) - i32::from(exponent);
        usize::try_from(index)
            .ok()
            .and_then(|index| digits.get(index))
            .copied()
            .unwrap_or(0)
    };
    let mut text = String::new();
    if sign == 0x4000 && digits.iter().any(|digit| *digit != 0) {
        text.push('-');
    }
    if weight >= 0 {
        for exponent in (0..=weight).rev() {
            let digit = digit_at(exponent);
            if exponent == weight {
                text.push_str(&digit.to_string());
            } else {
                use std::fmt::Write as _;
                write!(text, "{digit:04}").expect("string formatting");
            }
        }
    } else {
        text.push('0');
    }
    let scale = usize::try_from(scale).expect("validated NUMERIC scale");
    if scale > 0 {
        text.push('.');
        let groups = scale.div_ceil(4);
        for group in 1..=groups {
            use std::fmt::Write as _;
            write!(text, "{:04}", digit_at(-(group as i16))).expect("string formatting");
        }
        text.truncate(text.len() - (groups * 4 - scale));
    }
    Ok(text)
}

impl std::fmt::Debug for Parameter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Parameter(<value>)")
    }
}

fn database_error(error: tokio_postgres::Error) -> Error {
    let kind = match error.code() {
        Some(
            &tokio_postgres::error::SqlState::UNIQUE_VIOLATION
            | &tokio_postgres::error::SqlState::FOREIGN_KEY_VIOLATION
            | &tokio_postgres::error::SqlState::CHECK_VIOLATION
            | &tokio_postgres::error::SqlState::NOT_NULL_VIOLATION,
        ) => ErrorKind::Constraint,
        Some(code) if code == &tokio_postgres::error::SqlState::QUERY_CANCELED => {
            ErrorKind::Cancelled
        }
        Some(
            &tokio_postgres::error::SqlState::ADMIN_SHUTDOWN
            | &tokio_postgres::error::SqlState::CRASH_SHUTDOWN
            | &tokio_postgres::error::SqlState::CANNOT_CONNECT_NOW
            | &tokio_postgres::error::SqlState::DATABASE_DROPPED,
        ) => ErrorKind::Connection,
        Some(code) if code.code().starts_with("08") => ErrorKind::Connection,
        _ if error.is_closed() => ErrorKind::Connection,
        _ => ErrorKind::Database,
    };
    let mut result = Error::new(kind, error.to_string());
    if let Some(source) = error.source() {
        result = result.cause(source);
    }
    result
}

async fn io<T>(
    timeout: Duration,
    operation: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| Error::new(ErrorKind::Timeout, "PostgreSQL I/O timed out"))?
}

fn finish<T>(
    connection: PoolObjectGuard<deadpool_postgres::Object>,
    result: Result<T, Error>,
) -> Result<T, Error> {
    if match &result {
        Ok(_) => true,
        Err(error) => error.keeps_connection(),
    } {
        drop(connection.release());
    }
    result
}

fn pool_error(error: deadpool_postgres::PoolError) -> Error {
    let kind = match &error {
        deadpool_postgres::PoolError::Timeout(deadpool_postgres::TimeoutType::Wait) => {
            ErrorKind::PoolExhausted
        }
        deadpool_postgres::PoolError::Timeout(_) => ErrorKind::Timeout,
        deadpool_postgres::PoolError::Backend(_) => ErrorKind::Connection,
        deadpool_postgres::PoolError::Closed => ErrorKind::Connection,
        _ => ErrorKind::Pool,
    };
    Error::new(kind, error.to_string())
}

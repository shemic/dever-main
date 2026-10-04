//! Compiler-owned durable queue bridge. All persistence uses the ordinary database executor.
use crate::database::{Database, Executor, Sql, Transaction};
use crate::orm::{self, Error, Row, Value};
use crate::wire::Encoded;
use std::cell::RefCell;
use std::future::Future;

pub mod resources;
pub mod store;
pub mod worker;

pub use crate::time::Clock;

tokio::task_local! { static EXECUTION_CONTEXT: ExecutionContext; }

struct ExecutionContext {
    identity: ExecutionIdentity,
    permission_key: RefCell<Option<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionIdentity {
    System {
        tenant_id: Option<i64>,
    },
    User {
        tenant_id: Option<i64>,
        provider: String,
        site: String,
        subject: String,
        session: String,
        tenant_claim: Option<String>,
        permission_key: String,
    },
}

impl ExecutionIdentity {
    pub fn tenant_id(&self) -> Option<i64> {
        match self {
            Self::System { tenant_id } | Self::User { tenant_id, .. } => *tenant_id,
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.tenant_id().is_some_and(|tenant_id| tenant_id <= 0) {
            return Err(Error::invalid_data(
                "Job execution tenant id must be positive",
            ));
        }
        if let Self::User {
            provider,
            site,
            subject,
            session,
            tenant_claim,
            permission_key,
            ..
        } = self
        {
            for (value, label) in [
                (provider.as_str(), "provider"),
                (site.as_str(), "site"),
                (subject.as_str(), "subject"),
                (session.as_str(), "session"),
            ] {
                bounded(value, 256, &format!("Job execution {label}"))?;
            }
            if let Some(tenant_claim) = tenant_claim {
                bounded(tenant_claim, 256, "Job execution tenant claim")?;
            }
            if !valid_permission_key(permission_key) {
                return Err(Error::invalid_data(
                    "Job execution permission key is invalid",
                ));
            }
        }
        Ok(())
    }
}

pub fn scope_execution<T>(
    identity: ExecutionIdentity,
    future: impl Future<Output = T>,
) -> impl Future<Output = T> {
    // Pin before constructing the context wrapper so its poll frame does not
    // carry another copy of a large business Future.
    let future = Box::pin(future);
    async move {
        let permission_key = match &identity {
            ExecutionIdentity::User { permission_key, .. } if !permission_key.is_empty() => {
                Some(permission_key.clone())
            }
            _ => None,
        };
        EXECUTION_CONTEXT
            .scope(
                ExecutionContext {
                    identity,
                    permission_key: RefCell::new(permission_key),
                },
                future,
            )
            .await
    }
}

pub fn scope_system<T>(
    tenant_id: Option<i64>,
    future: impl Future<Output = T>,
) -> impl Future<Output = T> {
    scope_execution(ExecutionIdentity::System { tenant_id }, future)
}

pub fn execution_identity() -> Result<ExecutionIdentity, Error> {
    EXECUTION_CONTEXT
        .try_with(|context| {
            let mut identity = context.identity.clone();
            if let ExecutionIdentity::User { permission_key, .. } = &mut identity {
                *permission_key = context.permission_key.borrow().clone().unwrap_or_default();
            }
            identity
        })
        .map_err(|_| Error::invalid_data("trusted Job execution identity is required"))
}

/// Compiled component ownership does not enable tenant isolation; deployment settings do.
pub async fn require_components(
    identity: &ExecutionIdentity,
    components: &[&str],
    manifest: &[&str],
) -> Result<(), store::Outcome> {
    if components.is_empty() || crate::config::current_settings().tenant().is_none() {
        return Ok(());
    }
    let tenant_id = identity
        .tenant_id()
        .ok_or(store::Outcome::InvalidIdentity)?;
    crate::tenant::require_components(tenant_id, components, manifest)
        .await
        .map_err(|error| {
            if error.kind() == orm::ErrorKind::InvalidData {
                store::Outcome::ComponentDisabled
            } else {
                store::Outcome::Retry
            }
        })
}

#[cfg(feature = "api")]
pub fn bind_permission(permission_key: &str) -> Result<(), Error> {
    if !valid_permission_key(permission_key) {
        return Err(Error::invalid_data(
            "Job execution permission key is invalid",
        ));
    }
    EXECUTION_CONTEXT
        .try_with(|context| match &context.identity {
            ExecutionIdentity::User { .. } => {
                let mut current = context.permission_key.borrow_mut();
                match current.as_deref() {
                    Some(value) if value != permission_key => Err(Error::invalid_data(
                        "Job execution permission cannot change within one request",
                    )),
                    Some(_) => Ok(()),
                    None => {
                        *current = Some(permission_key.to_owned());
                        Ok(())
                    }
                }
            }
            ExecutionIdentity::System { .. } => Ok(()),
        })
        .map_err(|_| Error::invalid_data("trusted Job execution identity is required"))?
}

#[derive(Clone)]
pub struct Spec {
    pub target: &'static str,
    pub schema: &'static str,
    pub attempts: u32,
    pub timeout_ms: u32,
}

impl Spec {
    pub fn validate(&self) -> Result<(), Error> {
        bounded(self.target, 1024, "Job target")?;
        bounded(self.schema, 65536, "Job schema")?;
        if !(1..=100).contains(&self.attempts)
            || !(1..=crate::config::MAX_JOB_TIMEOUT_MS).contains(&self.timeout_ms)
        {
            return Err(Error::invalid_data("invalid Job retry or timeout policy"));
        }
        Ok(())
    }
}

pub fn now() -> Result<i64, Error> {
    crate::time::now().map_err(Error::invalid_data)
}

fn bounded(value: &str, maximum: usize, label: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > maximum {
        return Err(Error::invalid_data(format!(
            "{label} must contain 1..={maximum} bytes"
        )));
    }
    Ok(())
}

fn valid_permission_key(permission_key: &str) -> bool {
    let parts = permission_key.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts.iter().all(|part| {
            part.starts_with(|character: char| character.is_ascii_lowercase())
                && part.split('_').all(|word| {
                    !word.is_empty()
                        && word.chars().all(|character| {
                            character.is_ascii_lowercase() || character.is_ascii_digit()
                        })
                })
        })
}

fn text(row: &Row, index: usize) -> Result<String, Error> {
    orm::text(row.get(index)?.clone())
}
fn int(row: &Row, index: usize) -> Result<i64, Error> {
    orm::int(row.get(index)?.clone())
}
fn t(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}
fn n(value: i64) -> Value {
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

pub async fn enqueue(
    database: Database,
    ambient: Option<&Transaction>,
    spec: &Spec,
    payload: Encoded,
    key: &str,
    run_at: i64,
) -> Result<crate::Id, Error> {
    spec.validate()?;
    bounded(key, 1024, "Job idempotency key")?;
    // Business keys and scheduler keys occupy disjoint namespaces.
    let key = format!("business:{key}");
    let tenant_id = database.key().tenant_id().unwrap_or(0);
    let identity = execution_identity()?;
    identity.validate()?;
    if tenant_id > 0 && identity.tenant_id() != Some(tenant_id) {
        return Err(Error::invalid_data(
            "Job execution tenant does not match tenant storage",
        ));
    }
    if let Some(transaction) = ambient {
        let executor = Executor::new(database, Some(transaction))?;
        return insert(
            &executor,
            tenant_id,
            &identity,
            spec,
            payload.as_str(),
            &key,
            run_at,
        )
        .await
        .map(crate::Id);
    }
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = insert(
        &executor,
        tenant_id,
        &identity,
        spec,
        payload.as_str(),
        &key,
        run_at,
    )
    .await;
    finish_transaction(transaction, result).await.map(crate::Id)
}

async fn insert(
    executor: &Executor<'_>,
    tenant_id: i64,
    identity: &ExecutionIdentity,
    spec: &Spec,
    payload: &str,
    key: &str,
    run_at: i64,
) -> Result<String, Error> {
    let id = orm::Uuid::new_v7()?.to_string();
    let (kind, scope_tenant_id, provider, site, subject, session, tenant_claim, permission_key) =
        match identity {
            ExecutionIdentity::System { tenant_id } => {
                ("system", tenant_id.unwrap_or(0), "", "", "", "", "", "")
            }
            ExecutionIdentity::User {
                tenant_id,
                provider,
                site,
                subject,
                session,
                tenant_claim,
                permission_key,
            } => (
                "user",
                tenant_id.unwrap_or(0),
                provider.as_str(),
                site.as_str(),
                subject.as_str(),
                session.as_str(),
                tenant_claim.as_deref().unwrap_or(""),
                permission_key.as_str(),
            ),
        };
    // RETURNING holds the conflicting row through this transaction. A separate
    // SELECT could miss it if a worker completed the active job in between.
    let rows = executor.query(Sql {
        sqlite: "INSERT INTO _dever_jobs (id,tenant_id,scope_tenant_id,execution_kind,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_permission,target,schema,payload,dedupe_key,state,run_at,attempt,max_attempts,timeout_ms,claim_token,lease_until,error,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,'pending',?15,0,?16,?17,'',0,'',?15) ON CONFLICT(tenant_id,scope_tenant_id,target,dedupe_key) WHERE state IN ('pending','running') DO UPDATE SET dedupe_key=excluded.dedupe_key RETURNING id",
        postgres: "INSERT INTO _dever_jobs (id,tenant_id,scope_tenant_id,execution_kind,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_permission,target,schema,payload,dedupe_key,state,run_at,attempt,max_attempts,timeout_ms,claim_token,lease_until,error,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'pending',$15,0,$16,$17,'',0,'',$15) ON CONFLICT(tenant_id,scope_tenant_id,target,dedupe_key) WHERE state IN ('pending','running') DO UPDATE SET dedupe_key=excluded.dedupe_key RETURNING id",
    }, vec![t(id), n(tenant_id), n(scope_tenant_id), t(kind), t(provider), t(site), t(subject), t(session), t(tenant_claim), t(permission_key), t(spec.target), t(spec.schema), t(payload), t(key), n(run_at), n(i64::from(spec.attempts)), n(i64::from(spec.timeout_ms))]).await?;
    text(&orm::required_row(rows, "Job enqueue")?, 0)
}

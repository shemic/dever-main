use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use futures_util::{StreamExt, stream::FuturesUnordered};

use super::{Clock, Spec, store};
use crate::{
    config::JobSettings,
    database::{Database, ModelScope, StorageBinding},
    orm::ErrorKind,
};

#[derive(Clone)]
enum BindingStorage {
    Fixed(Database),
    Scoped(StorageBinding),
}

#[derive(Clone)]
pub struct Binding {
    storage: BindingStorage,
    tenant: bool,
    pub spec: Spec,
    pub schedule: Option<crate::cron::Cron>,
}

impl Binding {
    pub fn fixed(
        database: Database,
        spec: &'static Spec,
        schedule: Option<crate::cron::Cron>,
    ) -> Self {
        Self {
            storage: BindingStorage::Fixed(database),
            tenant: false,
            spec: spec.clone(),
            schedule,
        }
    }

    pub fn scoped(
        storage: &'static StorageBinding,
        scope: ModelScope,
        spec: &'static Spec,
        schedule: Option<crate::cron::Cron>,
    ) -> Self {
        Self::owned(*storage, scope, spec.clone(), schedule)
    }

    pub fn owned(
        storage: StorageBinding,
        scope: ModelScope,
        spec: Spec,
        schedule: Option<crate::cron::Cron>,
    ) -> Self {
        Self {
            storage: BindingStorage::Scoped(storage),
            tenant: scope == ModelScope::Tenant,
            spec,
            schedule,
        }
    }

    pub(crate) fn uses_tenant_storage(&self) -> bool {
        self.tenant && crate::config::current_settings().tenant().is_some()
    }

    pub(crate) fn selector(&self) -> Option<(Option<&str>, &str)> {
        match &self.storage {
            BindingStorage::Fixed(_) => None,
            BindingStorage::Scoped(storage) => Some(storage.selector()),
        }
    }

    pub(crate) async fn database(
        &self,
        tenant_id: Option<i64>,
    ) -> Result<Database, crate::orm::Error> {
        match &self.storage {
            BindingStorage::Fixed(database) => Ok(database.clone()),
            BindingStorage::Scoped(storage) => match tenant_id {
                Some(tenant_id) => crate::tenant::scope(tenant_id, storage.database()).await,
                None => storage.database().await,
            },
        }
    }
}

async fn global_connections(bindings: &[Binding]) -> Result<Vec<Database>, String> {
    let mut databases = BTreeMap::new();
    for binding in bindings
        .iter()
        .filter(|binding| !binding.uses_tenant_storage())
    {
        let database = binding
            .database(None)
            .await
            .map_err(|error| error.to_string())?;
        databases.insert(database.key().clone(), database);
    }
    Ok(databases.into_values().collect())
}

async fn tenant_connections(bindings: &[Binding], tenant_id: i64) -> Result<Vec<Database>, String> {
    let mut databases = BTreeMap::new();
    for binding in bindings
        .iter()
        .filter(|binding| binding.uses_tenant_storage())
    {
        let database = match binding.database(Some(tenant_id)).await {
            Ok(database) => database,
            Err(error) if error.kind() == ErrorKind::PoolExhausted => continue,
            Err(error) => return Err(error.to_string()),
        };
        databases.insert(database.key().clone(), database);
    }
    Ok(databases.into_values().collect())
}

async fn schedule_global(bindings: &[Binding], clock: &Clock) -> Result<(), String> {
    let now = clock.now().map_err(|error| error.to_string())?;
    for binding in bindings
        .iter()
        .filter(|binding| !binding.uses_tenant_storage())
    {
        if let Some(cron) = &binding.schedule {
            let database = binding
                .database(None)
                .await
                .map_err(|error| error.to_string())?;
            store::materialize(database, &binding.spec, cron, now)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn schedule_tenant(
    bindings: &[Binding],
    clock: &Clock,
    tenant_id: i64,
) -> Result<(), String> {
    let now = clock.now().map_err(|error| error.to_string())?;
    for binding in bindings
        .iter()
        .filter(|binding| binding.uses_tenant_storage())
    {
        let Some(cron) = &binding.schedule else {
            continue;
        };
        let database = match binding.database(Some(tenant_id)).await {
            Ok(database) => database,
            Err(error) if error.kind() == ErrorKind::PoolExhausted => continue,
            Err(error) => return Err(error.to_string()),
        };
        store::materialize(database, &binding.spec, cron, now)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn execute<F, Fut>(
    database: Database,
    claim: store::Claim,
    settings: JobSettings,
    clock: Clock,
    dispatch: F,
) -> Result<(), String>
where
    F: Fn(store::Claim) -> Fut + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    if database.key().tenant_id() != claim.tenant_id {
        return Err("Job claim tenant does not match its physical database".into());
    }
    let current_settings = crate::config::current_settings();
    let connection = current_settings
        .database(database.name())
        .ok_or("Job database is not configured")?;
    let outcome = if settings
        .validate_lease(connection, claim.timeout_ms)
        .is_err()
        || !(1..=100).contains(&claim.max_attempts)
    {
        store::Outcome::InvalidPolicy
    } else {
        let identity = claim.execution.clone();
        let tenant_id = identity.tenant_id();
        let input = claim.clone();
        let task = crate::task::run(async move {
            let execution = async move {
                match tenant_id {
                    Some(tenant_id) => crate::tenant::scope(tenant_id, dispatch(input)).await,
                    None => dispatch(input).await,
                }
            };
            let outcome = crate::job::scope_execution(identity, execution).await;
            Ok(outcome)
        })
        .await?;
        match crate::task::wait_lease(task, claim.timeout_ms, settings.lease_ms / 3, || async {
            store::renew(database.clone(), &claim, &clock, settings.lease_ms)
                .await
                .map_err(|error| error.to_string())
        })
        .await?
        {
            crate::task::LeaseResult::Completed(outcome) => outcome,
            crate::task::LeaseResult::Timeout => store::Outcome::Timeout,
            crate::task::LeaseResult::Lost => return Ok(()),
        }
    };
    store::complete(
        database,
        &claim,
        outcome,
        &clock,
        settings.retry_base_ms,
        settings.retry_max_ms,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn drain_connections<F, Fut>(
    databases: Vec<Database>,
    settings: JobSettings,
    clock: &Clock,
    limit: i64,
    count: &mut i64,
    dispatch: &F,
) -> Result<bool, String>
where
    F: Fn(store::Claim) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    let mut progress = false;
    for database in databases {
        if *count == limit {
            return Ok(progress);
        }
        for claim in store::claim(database.clone(), clock, settings.lease_ms, 1)
            .await
            .map_err(|error| error.to_string())?
        {
            execute(
                database.clone(),
                claim,
                settings,
                clock.clone(),
                dispatch.clone(),
            )
            .await?;
            *count += 1;
            progress = true;
        }
    }
    Ok(progress)
}

pub async fn drain<F, Fut>(
    bindings: Vec<Binding>,
    clock: Clock,
    limit: i64,
    dispatch: F,
) -> Result<i64, String>
where
    F: Fn(store::Claim) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    if !(1..=10_000).contains(&limit) {
        return Err("test Job drain limit must be between 1 and 10000".into());
    }
    let settings = JobSettings::for_test();
    schedule_global(&bindings, &clock).await?;
    let tenant_id = if bindings.iter().any(Binding::uses_tenant_storage) {
        let tenant_id = crate::tenant::current_id().map_err(|error| error.to_string())?;
        schedule_tenant(&bindings, &clock, tenant_id).await?;
        Some(tenant_id)
    } else {
        None
    };
    let mut count = 0;
    loop {
        let mut progress = drain_connections(
            global_connections(&bindings).await?,
            settings,
            &clock,
            limit,
            &mut count,
            &dispatch,
        )
        .await?;
        if count == limit {
            return Ok(count);
        }
        if let Some(tenant_id) = tenant_id {
            progress |= drain_connections(
                tenant_connections(&bindings, tenant_id).await?,
                settings,
                &clock,
                limit,
                &mut count,
                &dispatch,
            )
            .await?;
        }
        if !progress {
            return Ok(count);
        }
    }
}

type ActiveJob = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

async fn claim_into<F, Fut>(
    databases: Vec<Database>,
    settings: JobSettings,
    clock: &Clock,
    dispatch: &F,
    active: &mut FuturesUnordered<ActiveJob>,
) -> Result<(), String>
where
    F: Fn(store::Claim) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    for database in databases {
        if crate::lifecycle::stopping() {
            break;
        }
        let available = settings.workers.saturating_sub(active.len());
        if available == 0 {
            break;
        }
        let claims = store::claim(database.clone(), clock, settings.lease_ms, available)
            .await
            .map_err(|error| error.to_string())?;
        if crate::lifecycle::stopping() {
            break;
        }
        for claim in claims {
            let task = crate::task::run(execute(
                database.clone(),
                claim,
                settings,
                clock.clone(),
                dispatch.clone(),
            ))
            .await?;
            active.push(Box::pin(crate::task::wait(task)));
        }
    }
    Ok(())
}

async fn claim_ready_tenants<F, Fut>(
    bindings: &[Binding],
    settings: JobSettings,
    clock: &Clock,
    dispatch: &F,
    active: &mut FuturesUnordered<ActiveJob>,
    after: &mut i64,
) -> Result<(), String>
where
    F: Fn(store::Claim) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    if active.len() >= settings.workers {
        return Ok(());
    }
    let tenant_ids = crate::tenant::ready_ids_after(*after, 256)
        .await
        .map_err(|error| error.to_string())?;
    if tenant_ids.is_empty() {
        *after = 0;
        return Ok(());
    }
    for tenant_id in tenant_ids {
        if crate::lifecycle::stopping() || active.len() >= settings.workers {
            break;
        }
        schedule_tenant(bindings, clock, tenant_id).await?;
        claim_into(
            tenant_connections(bindings, tenant_id).await?,
            settings,
            clock,
            dispatch,
            active,
        )
        .await?;
        *after = tenant_id;
    }
    Ok(())
}

pub async fn serve<F, Fut>(
    bindings: Vec<Binding>,
    clock: Clock,
    settings: JobSettings,
    dispatch: F,
) -> Result<(), String>
where
    F: Fn(store::Claim) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = store::Outcome> + Send + 'static,
{
    let settings = settings.validate()?;
    let has_tenant_jobs = bindings.iter().any(Binding::uses_tenant_storage);
    let mut tenant_after = 0;
    let mut prefer_tenant = has_tenant_jobs;
    let mut active = FuturesUnordered::new();
    loop {
        if crate::lifecycle::stopping() {
            break;
        }
        schedule_global(&bindings, &clock).await?;
        if prefer_tenant {
            claim_ready_tenants(
                &bindings,
                settings,
                &clock,
                &dispatch,
                &mut active,
                &mut tenant_after,
            )
            .await?;
            claim_into(
                global_connections(&bindings).await?,
                settings,
                &clock,
                &dispatch,
                &mut active,
            )
            .await?;
        } else {
            claim_into(
                global_connections(&bindings).await?,
                settings,
                &clock,
                &dispatch,
                &mut active,
            )
            .await?;
            if has_tenant_jobs {
                claim_ready_tenants(
                    &bindings,
                    settings,
                    &clock,
                    &dispatch,
                    &mut active,
                    &mut tenant_after,
                )
                .await?;
            }
        }
        prefer_tenant = has_tenant_jobs && !prefer_tenant;
        tokio::select! {
            _ = crate::lifecycle::shutdown_requested() => break,
            Some(result) = active.next(), if !active.is_empty() => result?,
            _ = tokio::time::sleep(std::time::Duration::from_millis(settings.poll_ms as u64)) => {}
        }
    }
    while let Some(result) = active.next().await {
        result?;
    }
    Ok(())
}

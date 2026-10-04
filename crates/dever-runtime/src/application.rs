//! Invocation-owned resources shared by generated application roots and requests.
use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

use crate::config::{RuntimeProfile, Settings};

tokio::task_local! {
    static CURRENT: RefCell<Option<Arc<Session>>>;
}

pub struct Session {
    settings: Arc<Settings>,
    lifecycle: std::sync::OnceLock<Arc<crate::lifecycle::Lifecycle>>,
    authorization: std::sync::OnceLock<bool>,
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    databases: Arc<crate::database::Session>,
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    tenant: std::sync::OnceLock<Arc<crate::tenant::Manager>>,
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    migrations: std::sync::Mutex<Vec<crate::database::Database>>,
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    jobs: std::sync::OnceLock<Arc<crate::job::resources::Resources>>,
}

impl Session {
    pub fn load(
        profile: RuntimeProfile,
        bindings: &[(Option<&str>, &str, bool)],
        transactions: &[&[(Option<&str>, &str, bool)]],
    ) -> Result<Arc<Self>, String> {
        let settings = Arc::new(Settings::load_for_executable()?);
        #[cfg(any(feature = "sqlite", feature = "postgres"))]
        let databases = Arc::new(crate::database::Session::from_settings(
            settings.clone(),
            profile,
            bindings,
            transactions,
        )?);
        #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
        settings.prepare_databases(profile, bindings, transactions)?;
        Ok(Arc::new(Self {
            settings,
            lifecycle: std::sync::OnceLock::new(),
            authorization: std::sync::OnceLock::new(),
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            databases,
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            tenant: std::sync::OnceLock::new(),
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            migrations: std::sync::Mutex::new(Vec::new()),
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            jobs: std::sync::OnceLock::new(),
        }))
    }

    pub fn settings(&self) -> &Arc<Settings> {
        &self.settings
    }

    pub fn initialize_authorization(&self, enabled: bool) -> Result<(), String> {
        self.authorization
            .set(enabled)
            .map_err(|_| "application authorization initialized twice".into())
    }

    pub fn has_authorization(&self) -> bool {
        self.authorization.get().copied().unwrap_or(false)
    }

    pub(crate) fn lifecycle(&self) -> Option<Arc<crate::lifecycle::Lifecycle>> {
        self.lifecycle.get().cloned()
    }

    pub(crate) fn set_lifecycle(
        &self,
        state: Arc<crate::lifecycle::Lifecycle>,
    ) -> Result<(), String> {
        self.lifecycle
            .set(state)
            .map_err(|_| "application lifecycle initialized twice".into())
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn databases(&self) -> &Arc<crate::database::Session> {
        &self.databases
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn jobs(&self) -> Option<&Arc<crate::job::resources::Resources>> {
        self.jobs.get()
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn initialize_jobs(
        &self,
        jobs: Arc<crate::job::resources::Resources>,
    ) -> Result<(), String> {
        self.jobs
            .set(jobs)
            .map_err(|_| "application Jobs initialized twice".into())
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn own_migration_database(&self, database: crate::database::Database) {
        self.migrations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(database);
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn tenant(&self) -> Option<Arc<crate::tenant::Manager>> {
        self.tenant.get().cloned()
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn set_tenant(
        &self,
        manager: Arc<crate::tenant::Manager>,
    ) -> Result<(), crate::orm::Error> {
        self.tenant.set(manager).map_err(|_| {
            crate::orm::Error::migration("tenant storage was initialized more than once")
        })
    }

    pub async fn scope<T>(self: Arc<Self>, future: impl Future<Output = T>) -> T {
        CURRENT.scope(RefCell::new(Some(self)), future).await
    }

    pub async fn close(&self) -> Result<(), String> {
        #[cfg(any(feature = "sqlite", feature = "postgres"))]
        {
            self.databases.stop_accepting();
            let migrations = std::mem::take(
                &mut *self
                    .migrations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
            for database in &migrations {
                database.stop_accepting();
            }
            let mut migration_error = None;
            for database in migrations {
                if let Err(error) = database.close_pool().await {
                    migration_error = Some(match migration_error {
                        Some(previous) => crate::orm::Error::cause(previous, error),
                        None => error,
                    });
                }
            }
            let tenant = match self.tenant.get() {
                Some(manager) => manager.close().await,
                None => Ok(()),
            };
            let databases = self.databases.close().await;
            for result in [tenant, databases] {
                if let Err(error) = result {
                    migration_error = Some(match migration_error {
                        Some(previous) => previous.cause(error),
                        None => error,
                    });
                }
            }
            crate::log::flush();
            migration_error.map_or(Ok(()), |error| Err(error.to_string()))
        }
        #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
        {
            crate::log::flush();
            Ok(())
        }
    }
}

pub fn current() -> Option<Arc<Session>> {
    CURRENT
        .try_with(|current| current.borrow().clone())
        .ok()
        .flatten()
}

/// Installation occurs inside the generated root, before any application work.
pub fn install(session: Arc<Session>) -> Result<(), String> {
    CURRENT
        .try_with(|current| {
            let mut current = current.borrow_mut();
            if current.is_some() {
                return Err("application session was initialized more than once".into());
            }
            *current = Some(session);
            Ok(())
        })
        .map_err(|_| "application session requires an application root".to_owned())?
}

pub async fn scope_entry<T>(future: impl Future<Output = T>) -> T {
    CURRENT.scope(RefCell::new(None), future).await
}

/// Carry application resources across a task boundary, never request identity.
pub async fn scope_resources<T>(
    session: Option<Arc<Session>>,
    future: impl Future<Output = T>,
) -> T {
    CURRENT.scope(RefCell::new(session), future).await
}

pub fn with_resources<T>(session: Option<Arc<Session>>, apply: impl FnOnce() -> T) -> T {
    CURRENT.sync_scope(RefCell::new(session), apply)
}

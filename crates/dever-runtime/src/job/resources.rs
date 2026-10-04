//! Application-owned Job bindings; storage and scheduling remain in their original owners.
use std::collections::{BTreeMap, BTreeSet};

use super::{Clock, store, worker::Binding};
use crate::{database::Database, orm::Error};

pub struct Resources {
    pub bindings: Vec<Binding>,
    pub clock: Clock,
}

impl Resources {
    pub fn validate(&self, worker: bool) -> Result<(), String> {
        let settings = crate::config::current_settings();
        for binding in &self.bindings {
            binding.spec.validate().map_err(|error| error.to_string())?;
            if worker && let Some((explicit, root)) = binding.selector() {
                settings.validate_job_connection(explicit, root, binding.spec.timeout_ms)?;
            }
        }
        Ok(())
    }

    pub async fn initialize(&self) -> Result<(), Error> {
        let mut initialized = BTreeSet::new();
        for binding in &self.bindings {
            if binding.uses_tenant_storage() {
                continue;
            }
            let database = binding.database(None).await?;
            if initialized.insert(database.key().clone()) {
                store::initialize(database.clone()).await?;
            }
            Self::validate_schedule(binding, database).await?;
        }
        Ok(())
    }

    pub fn has_tenant_jobs(&self) -> bool {
        self.bindings.iter().any(Binding::uses_tenant_storage)
    }

    pub async fn migrate_tenant(
        &self,
        tenant_id: i64,
        fingerprint: &str,
        databases: &mut BTreeMap<String, Database>,
    ) -> Result<(), Error> {
        let settings = crate::config::current_settings();
        let mut initialized = BTreeSet::new();
        for binding in self
            .bindings
            .iter()
            .filter(|binding| binding.uses_tenant_storage())
        {
            let (explicit, root) = binding
                .selector()
                .ok_or_else(|| Error::migration("tenant Job requires scoped storage"))?;
            let connection = settings
                .resolve_database(explicit, root)
                .map_err(Error::migration)?
                .0
                .to_owned();
            if !databases.contains_key(&connection) {
                let database =
                    crate::tenant::begin_migration(&connection, tenant_id, fingerprint).await?;
                databases.insert(connection.clone(), database);
            }
            let database = databases[&connection].clone();
            if initialized.insert(connection) {
                store::initialize(database.clone()).await?;
            }
            Self::validate_schedule(binding, database).await?;
        }
        Ok(())
    }

    async fn validate_schedule(binding: &Binding, database: Database) -> Result<(), Error> {
        if let Some(schedule) = &binding.schedule {
            store::validate_schedule(database, &binding.spec, schedule).await?;
        }
        Ok(())
    }
}

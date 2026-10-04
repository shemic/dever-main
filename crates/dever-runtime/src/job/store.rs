use super::*;

/// A fencing token belongs to exactly one claim, including reclaim by the same process.
#[derive(Clone, Debug)]
pub struct Claim {
    pub id: String,
    /// Physical queue database tenant, not the execution scope.
    pub tenant_id: Option<i64>,
    pub execution: ExecutionIdentity,
    pub target: String,
    pub schema: String,
    pub payload: String,
    pub attempt: i64,
    pub max_attempts: i64,
    pub timeout_ms: i64,
    pub token: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Success,
    Retry,
    Timeout,
    UnknownTarget,
    SchemaMismatch,
    InvalidPayload,
    InvalidPolicy,
    IdentityRejected,
    InvalidIdentity,
    ComponentDisabled,
}

impl Outcome {
    fn error(self) -> &'static str {
        match self {
            Self::Success => "",
            Self::Retry => "handler_failed",
            Self::Timeout => "handler_timeout",
            Self::UnknownTarget => "unknown_target",
            Self::SchemaMismatch => "schema_mismatch",
            Self::InvalidPayload => "invalid_payload",
            Self::InvalidPolicy => "invalid_policy",
            Self::IdentityRejected => "identity_rejected",
            Self::InvalidIdentity => "invalid_identity",
            Self::ComponentDisabled => "component_disabled",
        }
    }
}

pub async fn initialize(database: Database) -> Result<(), Error> {
    let tenant_id = database.key().tenant_id();
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = initialize_in(&executor, tenant_id).await;
    finish_transaction(transaction, result).await
}

async fn initialize_in(executor: &Executor<'_>, tenant_id: Option<i64>) -> Result<(), Error> {
    // Serialize private schema installation independently of Model migrations.
    executor
        .query(
            Sql {
                sqlite: "SELECT 1",
                postgres: "SELECT pg_advisory_xact_lock(1684371045, 1785684595) IS NULL",
            },
            vec![],
        )
        .await?;
    executor.execute(Sql {
        sqlite: "CREATE TABLE IF NOT EXISTS _dever_job_version (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version BIGINT NOT NULL)",
        postgres: "CREATE TABLE IF NOT EXISTS _dever_job_version (singleton BIGINT PRIMARY KEY CHECK(singleton=1), version BIGINT NOT NULL)",
    }, vec![]).await?;
    let versions = executor
        .query(
            Sql {
                sqlite: "SELECT version FROM _dever_job_version",
                postgres: "SELECT version FROM _dever_job_version",
            },
            vec![],
        )
        .await?;
    if let Some(version) = versions.first() {
        if versions.len() != 1 {
            return Err(Error::migration("invalid private Job schema version state"));
        }
        return match int(version, 0)? {
            5 => Ok(()),
            4 => migrate_v4(executor).await,
            3 => {
                migrate_v3(executor).await?;
                migrate_v4(executor).await
            }
            2 => {
                migrate_v2(executor).await?;
                migrate_v3(executor).await?;
                migrate_v4(executor).await
            }
            1 => {
                migrate_v1(executor, tenant_id).await?;
                migrate_v2(executor).await?;
                migrate_v3(executor).await?;
                migrate_v4(executor).await
            }
            _ => Err(Error::migration("unsupported private Job schema version")),
        };
    }
    const JOBS: &str = "CREATE TABLE _dever_jobs (id TEXT PRIMARY KEY, tenant_id BIGINT NOT NULL, scope_tenant_id BIGINT NOT NULL, execution_kind TEXT NOT NULL CHECK(execution_kind IN ('system','user','legacy')), auth_provider TEXT NOT NULL, auth_site TEXT NOT NULL, auth_subject TEXT NOT NULL, auth_session TEXT NOT NULL, auth_tenant TEXT NOT NULL, auth_permission TEXT NOT NULL, target TEXT NOT NULL, schema TEXT NOT NULL, payload TEXT NOT NULL, dedupe_key TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('pending','running','succeeded','dead','blocked')), run_at BIGINT NOT NULL, attempt BIGINT NOT NULL, max_attempts BIGINT NOT NULL, timeout_ms BIGINT NOT NULL, claim_token TEXT NOT NULL, lease_until BIGINT NOT NULL, error TEXT NOT NULL, updated_at BIGINT NOT NULL)";
    const ACTIVE: &str = "CREATE UNIQUE INDEX _dever_jobs_active ON _dever_jobs(tenant_id,scope_tenant_id,target,dedupe_key) WHERE state IN ('pending','running')";
    const DUE: &str = "CREATE INDEX _dever_jobs_due ON _dever_jobs(state,run_at,lease_until)";
    const SCHEDULE: &str = "CREATE TABLE _dever_job_schedule (identity TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, last_minute BIGINT NOT NULL)";
    for sql in [
        JOBS,
        ACTIVE,
        DUE,
        SCHEDULE,
        "INSERT INTO _dever_job_version(singleton,version) VALUES (1,5)",
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

async fn migrate_v4(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "ALTER TABLE _dever_jobs ADD COLUMN auth_permission TEXT NOT NULL DEFAULT ''",
        "UPDATE _dever_jobs SET state='blocked',error='authorization_contract_changed',claim_token='',lease_until=0 WHERE execution_kind='user' AND state IN ('pending','running')",
        "ALTER TABLE _dever_jobs DROP COLUMN auth_capabilities",
        "UPDATE _dever_job_version SET version=5 WHERE singleton=1",
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

async fn migrate_v3(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "ALTER TABLE _dever_jobs ADD COLUMN auth_capabilities TEXT NOT NULL DEFAULT '[]'",
        "UPDATE _dever_jobs SET state='blocked',error='missing_execution_capabilities',claim_token='',lease_until=0 WHERE execution_kind='user' AND state IN ('pending','running')",
        "UPDATE _dever_job_version SET version=4 WHERE singleton=1",
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

async fn migrate_v2(executor: &Executor<'_>) -> Result<(), Error> {
    for sql in [
        "ALTER TABLE _dever_jobs ADD COLUMN scope_tenant_id BIGINT NOT NULL DEFAULT 0",
        "ALTER TABLE _dever_jobs ADD COLUMN execution_kind TEXT NOT NULL DEFAULT 'legacy'",
        "ALTER TABLE _dever_jobs ADD COLUMN auth_provider TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE _dever_jobs ADD COLUMN auth_site TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE _dever_jobs ADD COLUMN auth_subject TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE _dever_jobs ADD COLUMN auth_session TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE _dever_jobs ADD COLUMN auth_tenant TEXT NOT NULL DEFAULT ''",
        "UPDATE _dever_jobs SET state='blocked',error='missing_execution_identity',claim_token='',lease_until=0 WHERE state IN ('pending','running')",
        "DROP INDEX IF EXISTS _dever_jobs_active",
        "CREATE UNIQUE INDEX _dever_jobs_active ON _dever_jobs(tenant_id,scope_tenant_id,target,dedupe_key) WHERE state IN ('pending','running')",
        "UPDATE _dever_job_version SET version=3 WHERE singleton=1",
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

async fn migrate_v1(executor: &Executor<'_>, tenant_id: Option<i64>) -> Result<(), Error> {
    if tenant_id.is_some() {
        let rows = executor
            .query(
                Sql {
                    sqlite: "SELECT COUNT(*) FROM _dever_jobs",
                    postgres: "SELECT COUNT(*) FROM _dever_jobs",
                },
                vec![],
            )
            .await?;
        if int(&orm::required_row(rows, "legacy Job row count")?, 0)? != 0 {
            return Err(Error::migration(
                "legacy Job rows have no tenant ownership; drain or explicitly migrate them before enabling tenant Jobs",
            ));
        }
    }
    for sql in [
        "ALTER TABLE _dever_jobs ADD COLUMN tenant_id BIGINT NOT NULL DEFAULT 0",
        "DROP INDEX IF EXISTS _dever_jobs_active",
        "CREATE UNIQUE INDEX _dever_jobs_active ON _dever_jobs(tenant_id,target,dedupe_key) WHERE state IN ('pending','running')",
        "UPDATE _dever_job_version SET version=2 WHERE singleton=1",
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

pub async fn claim(
    database: Database,
    clock: &Clock,
    lease_ms: i64,
    available: usize,
) -> Result<Vec<Claim>, Error> {
    if !(1..=256).contains(&available)
        || !(100..=crate::config::MAX_JOB_LEASE_MS).contains(&lease_ms)
    {
        return Err(Error::invalid_data("invalid Job claim capacity or lease"));
    }
    let tenant_id = database.key().tenant_id().unwrap_or(0);
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = claim_in(&executor, tenant_id, clock, lease_ms, available).await;
    finish_transaction(transaction, result).await
}

async fn claim_in(
    executor: &Executor<'_>,
    expected_tenant: i64,
    clock: &Clock,
    lease_ms: i64,
    available: usize,
) -> Result<Vec<Claim>, Error> {
    let now = clock.now()?;
    // Exhausted rows consume bounded cleanup work, never an execution slot. In
    // particular, an empty claim must not hide eligible jobs behind dead rows.
    executor.execute(Sql {
        sqlite: "UPDATE _dever_jobs SET state='dead',error='attempts_exhausted',updated_at=?1 WHERE id IN (SELECT id FROM _dever_jobs WHERE attempt>=max_attempts AND ((state='pending' AND run_at<=?1) OR (state='running' AND lease_until<=?1)) ORDER BY run_at,id LIMIT ?2)",
        postgres: "UPDATE _dever_jobs SET state='dead',error='attempts_exhausted',updated_at=$1 WHERE id IN (SELECT id FROM _dever_jobs WHERE attempt>=max_attempts AND ((state='pending' AND run_at<=$1) OR (state='running' AND lease_until<=$1)) ORDER BY run_at,id LIMIT $2 FOR UPDATE SKIP LOCKED)",
    }, vec![n(now), n(available as i64)]).await?;
    let rows = executor.query(Sql {
        sqlite: "SELECT id,tenant_id,scope_tenant_id,execution_kind,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_permission,target,schema,payload,attempt,max_attempts,timeout_ms FROM _dever_jobs WHERE attempt<max_attempts AND ((state='pending' AND run_at<=?1) OR (state='running' AND lease_until<=?1)) ORDER BY run_at,id LIMIT ?2",
        postgres: "SELECT id,tenant_id,scope_tenant_id,execution_kind,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_permission,target,schema,payload,attempt,max_attempts,timeout_ms FROM _dever_jobs WHERE attempt<max_attempts AND ((state='pending' AND run_at<=$1) OR (state='running' AND lease_until<=$1)) ORDER BY run_at,id LIMIT $2 FOR UPDATE SKIP LOCKED",
    }, vec![n(now), n(available as i64)]).await?;
    // BEGIN/row locking may wait. Start the lease only after ownership is locked.
    let now = clock.now()?;
    let lease_until = now
        .checked_add(lease_ms)
        .ok_or_else(|| Error::invalid_data("Job lease overflow"))?;
    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let id = text(&row, 0)?;
        let tenant = int(&row, 1)?;
        if tenant != expected_tenant {
            return Err(Error::migration(
                "Job tenant ownership does not match its physical database",
            ));
        }
        let scope_tenant_id = int(&row, 2)?;
        if scope_tenant_id < 0 {
            return Err(Error::migration("Job execution tenant id is invalid"));
        }
        let execution = match text(&row, 3)?.as_str() {
            "system" => ExecutionIdentity::System {
                tenant_id: (scope_tenant_id > 0).then_some(scope_tenant_id),
            },
            "user" => ExecutionIdentity::User {
                tenant_id: (scope_tenant_id > 0).then_some(scope_tenant_id),
                provider: text(&row, 4)?,
                site: text(&row, 5)?,
                subject: text(&row, 6)?,
                session: text(&row, 7)?,
                tenant_claim: match text(&row, 8)? {
                    value if value.is_empty() => None,
                    value => Some(value),
                },
                permission_key: text(&row, 9)?,
            },
            _ => {
                return Err(Error::migration(
                    "active Job has no trusted execution identity",
                ));
            }
        };
        execution.validate()?;
        if tenant > 0 && execution.tenant_id() != Some(tenant) {
            return Err(Error::migration(
                "Job execution tenant does not match tenant storage",
            ));
        }
        let token = orm::Uuid::new_v7()?.to_string();
        let changed = executor.execute(Sql {
            sqlite: "UPDATE _dever_jobs SET state='running',attempt=attempt+1,claim_token=?2,lease_until=?3,updated_at=?4 WHERE id=?1 AND attempt<max_attempts AND ((state='pending' AND run_at<=?4) OR (state='running' AND lease_until<=?4))",
            postgres: "UPDATE _dever_jobs SET state='running',attempt=attempt+1,claim_token=$2,lease_until=$3,updated_at=$4 WHERE id=$1 AND attempt<max_attempts AND ((state='pending' AND run_at<=$4) OR (state='running' AND lease_until<=$4))",
        }, vec![t(&id), t(&token), n(lease_until), n(now)]).await?;
        if changed == 1 {
            claims.push(Claim {
                id,
                tenant_id: (tenant > 0).then_some(tenant),
                execution,
                target: text(&row, 10)?,
                schema: text(&row, 11)?,
                payload: text(&row, 12)?,
                attempt: int(&row, 13)? + 1,
                max_attempts: int(&row, 14)?,
                timeout_ms: int(&row, 15)?,
                token,
            });
        }
    }
    Ok(claims)
}

pub async fn renew(
    database: Database,
    claim: &Claim,
    clock: &Clock,
    lease_ms: i64,
) -> Result<bool, Error> {
    if !(100..=crate::config::MAX_JOB_LEASE_MS).contains(&lease_ms) {
        return Err(Error::invalid_data("invalid Job lease"));
    }
    let tenant_id = database.key().tenant_id().unwrap_or(0);
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = renew_in(&executor, tenant_id, claim, clock, lease_ms).await;
    finish_transaction(transaction, result).await
}

async fn lock_claim(
    executor: &Executor<'_>,
    expected_tenant: i64,
    claim: &Claim,
) -> Result<bool, Error> {
    let tenant_id = claim.tenant_id.unwrap_or(0);
    if expected_tenant != tenant_id {
        return Err(Error::migration(
            "Job claim tenant does not match its physical database",
        ));
    }
    // PostgreSQL needs the same ordering as SQLite's BEGIN IMMEDIATE: obtain the
    // write lock before reading time, so lock wait cannot revive an expired lease.
    executor.query(Sql {
        sqlite: "SELECT id FROM _dever_jobs WHERE id=?1 AND claim_token=?2 AND tenant_id=?3 AND state='running'",
        postgres: "SELECT id FROM _dever_jobs WHERE id=$1 AND claim_token=$2 AND tenant_id=$3 AND state='running' FOR UPDATE",
    }, vec![t(&claim.id), t(&claim.token), n(tenant_id)]).await.map(|rows| !rows.is_empty())
}

async fn renew_in(
    executor: &Executor<'_>,
    expected_tenant: i64,
    claim: &Claim,
    clock: &Clock,
    lease_ms: i64,
) -> Result<bool, Error> {
    if !lock_claim(executor, expected_tenant, claim).await? {
        return Ok(false);
    }
    let now = clock.now()?;
    let until = now
        .checked_add(lease_ms)
        .ok_or_else(|| Error::invalid_data("Job lease overflow"))?;
    executor.execute(Sql {
        sqlite: "UPDATE _dever_jobs SET lease_until=?4,updated_at=?3 WHERE id=?1 AND claim_token=?2 AND state='running' AND lease_until>?3",
        postgres: "UPDATE _dever_jobs SET lease_until=$4,updated_at=$3 WHERE id=$1 AND claim_token=$2 AND state='running' AND lease_until>$3",
    }, vec![t(&claim.id), t(&claim.token), n(now), n(until)]).await.map(|count| count == 1)
}

pub async fn complete(
    database: Database,
    claim: &Claim,
    outcome: Outcome,
    clock: &Clock,
    retry_base_ms: i64,
    retry_max_ms: i64,
) -> Result<bool, Error> {
    if retry_base_ms < 1 || retry_max_ms < retry_base_ms || retry_max_ms > 86_400_000 {
        return Err(Error::invalid_data("invalid Job retry bounds"));
    }
    let tenant_id = database.key().tenant_id().unwrap_or(0);
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = complete_in(
        &executor,
        tenant_id,
        claim,
        outcome,
        clock,
        retry_base_ms,
        retry_max_ms,
    )
    .await;
    finish_transaction(transaction, result).await
}

async fn complete_in(
    executor: &Executor<'_>,
    expected_tenant: i64,
    claim: &Claim,
    outcome: Outcome,
    clock: &Clock,
    retry_base_ms: i64,
    retry_max_ms: i64,
) -> Result<bool, Error> {
    if !lock_claim(executor, expected_tenant, claim).await? {
        return Ok(false);
    }
    let now = clock.now()?;
    let state = match outcome {
        Outcome::Success => "succeeded",
        Outcome::UnknownTarget
        | Outcome::SchemaMismatch
        | Outcome::InvalidPayload
        | Outcome::InvalidPolicy
        | Outcome::IdentityRejected
        | Outcome::InvalidIdentity
        | Outcome::ComponentDisabled => "blocked",
        _ if claim.attempt >= claim.max_attempts => "dead",
        _ => "pending",
    };
    let exponent = u32::try_from(claim.attempt.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(62);
    let delay = retry_base_ms
        .saturating_mul(1i64 << exponent)
        .min(retry_max_ms);
    let run_at = now
        .checked_add(delay)
        .ok_or_else(|| Error::invalid_data("Job retry time overflow"))?;
    executor.execute(Sql {
        sqlite: "UPDATE _dever_jobs SET state=?4,error=?5,run_at=?6,lease_until=0,claim_token='',updated_at=?3 WHERE id=?1 AND claim_token=?2 AND state='running' AND lease_until>?3",
        postgres: "UPDATE _dever_jobs SET state=$4,error=$5,run_at=$6,lease_until=0,claim_token='',updated_at=$3 WHERE id=$1 AND claim_token=$2 AND state='running' AND lease_until>$3",
    }, vec![t(&claim.id), t(&claim.token), n(now), t(state), t(outcome.error()), n(run_at)]).await.map(|count| count == 1)
}

pub async fn materialize(
    database: Database,
    spec: &Spec,
    cron: &crate::cron::Cron,
    now: i64,
) -> Result<usize, Error> {
    spec.validate()?;
    let tenant_id = database.key().tenant_id().unwrap_or(0);
    let transaction = database.begin().await?;
    let executor = Executor::new(database, Some(&transaction))?;
    let result = materialize_in(&executor, tenant_id, spec, cron, now.div_euclid(60_000)).await;
    finish_transaction(transaction, result).await
}

/// Validate persisted schedule identity before any application listener starts.
pub async fn validate_schedule(
    database: Database,
    spec: &Spec,
    cron: &crate::cron::Cron,
) -> Result<(), Error> {
    let rows = database
        .query(
            Sql {
                sqlite: "SELECT fingerprint FROM _dever_job_schedule WHERE identity=?1",
                postgres: "SELECT fingerprint FROM _dever_job_schedule WHERE identity=$1",
            },
            vec![t(spec.target)],
        )
        .await?;
    if let Some(row) = rows.first() {
        validate_fingerprint(row, &cron.fingerprint())?;
    }
    Ok(())
}

fn validate_fingerprint(cursor: &Row, expected: &str) -> Result<(), Error> {
    if text(cursor, 0)? != expected {
        return Err(Error::migration("Job schedule fingerprint changed"));
    }
    Ok(())
}

async fn materialize_in(
    executor: &Executor<'_>,
    tenant_id: i64,
    spec: &Spec,
    cron: &crate::cron::Cron,
    minute: i64,
) -> Result<usize, Error> {
    let fingerprint = cron.fingerprint();
    executor.execute(Sql {
        sqlite: "INSERT INTO _dever_job_schedule(identity,fingerprint,last_minute) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
        postgres: "INSERT INTO _dever_job_schedule(identity,fingerprint,last_minute) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    }, vec![t(spec.target), t(&fingerprint), n(minute)]).await?;
    let rows = executor.query(Sql {
        sqlite: "SELECT fingerprint,last_minute FROM _dever_job_schedule WHERE identity=?1",
        postgres: "SELECT fingerprint,last_minute FROM _dever_job_schedule WHERE identity=$1 FOR UPDATE",
    }, vec![t(spec.target)]).await?;
    let cursor = orm::required_row(rows, "Job schedule cursor")?;
    validate_fingerprint(&cursor, &fingerprint)?;
    let previous = int(&cursor, 1)?;
    let end = minute.min(previous.saturating_add(256));
    let mut inserted = 0;
    for slot in previous.saturating_add(1)..=end {
        if cron.matches_minute(slot) {
            let run_at = slot
                .checked_mul(60_000)
                .ok_or_else(|| Error::invalid_data("Job schedule time overflow"))?;
            let identity = ExecutionIdentity::System {
                tenant_id: (tenant_id > 0).then_some(tenant_id),
            };
            insert(
                executor,
                tenant_id,
                &identity,
                spec,
                "null",
                &format!("schedule:{}:{slot}", spec.target),
                run_at,
            )
            .await?;
            inserted += 1;
        }
    }
    if end > previous {
        executor
            .execute(
                Sql {
                    sqlite: "UPDATE _dever_job_schedule SET last_minute=?2 WHERE identity=?1",
                    postgres: "UPDATE _dever_job_schedule SET last_minute=$2 WHERE identity=$1",
                },
                vec![t(spec.target), n(end)],
            )
            .await?;
    }
    Ok(inserted)
}

# Database Guidelines

## 1. Scope

Dever owns a static ORM. Application code declares persistence in `module/<component>/<domain>/model.dever` or, for multiple cohesive models, `model/<topic>.dever`; Markdown uses the same identity and schema rules. It does not add CRUD App/API wrappers or pass database/transaction handles. SQLite and PostgreSQL consume the same checked `ModelSchema`, `QueryPlan`, bind values, row shapes and error propagation.

## 2. Model Contract

- A role-file Model has one automatically visible record matching the domain in UpperCamelCase; a topic Model record matches its topic. Source does not write `public`.
- `id` and `created_at` are synthesized, generated and cannot be assigned by source code.
- Persistent fields use lower_snake_case and the checked types `Bool/Int/Float/Decimal/Text/Bytes/Uuid/DateTime/Date/Time/Duration/Json`, Model choices or `<model-package>.id`.
- Persistent metadata includes `generated`, constant `default`, `index`, `unique` and `from <old_field>`. REST-only `owner`, `create = expr`, `replace = expr` and `search = expr` are checked API contracts and do not alter ordinary ORM semantics or storage fingerprints.
- `private` is language visibility metadata: only the owning domain App accesses that field, including generated create/update blocks. It does not enter ModelSchema, SQL column order or migration fingerprints. Private Model snapshots and containers cannot cross App contracts; expose a View. Secret is never a persistent type; password hashes use private Text with enough room for PHC encoding.
- Default and Seed literals share storage validation: integer/date values fit i64 without a fractional part, Float is finite, Decimal satisfies exact precision/scale and Text lengths count Unicode scalars. Invalid values fail checking rather than Rust emission or database startup.
- Composite `index(...)`, `unique(...)`, reverse `relation`, one `seed` block and stable named `migrate` blocks live in the Model file.
- Model IDs are opaque nominal values. A foreign Model ID is not interchangeable with `Int` or another Model ID.

## 3. Runtime And Query Contract

Use compiler-synthesized short Model functions: `create/create_many/get/first/list/cursor/count/exists/stream/update/delete/upsert`. Only the owning domain App may call them, as `model.<operation>` or `model.<topic>.<operation>`. Main, API and other domains call the owning App instead. Anonymous create/update/query blocks are contextual syntax and never become runtime maps or reusable dynamic query objects.

`where` builds a static condition tree; values are always bind parameters. Equality on nullable values is null-safe (`IS`/`IS NOT` on SQLite, `IS [NOT] DISTINCT FROM` on PostgreSQL). `order` is static: an omitted order defaults to `id DESC`, while an explicit order receives a stable `id` tie-breaker when needed. `list` defaults to page 1 and size 20, and runtime rejects a size above `database.<name>.max_page_size`. Do not add an Active Record type, chainable query object, reflection decoder or duplicated backend CRUD implementation.

`cursor` uses the same ordered Model fields as a static composite keyset. Its nullable `next` value is the last returned Model snapshot and is passed back as nullable `after`; the compiler binds the ordered field values in declaration order and rejects nullable cursor-order fields. SQLite uses `IS` and PostgreSQL uses `IS NOT DISTINCT FROM` for equal key prefixes. The nullable guard must cast the first PostgreSQL placeholder to the ordered field's SQL type before `IS NULL`; an untyped `$n IS NULL` leaves PostgreSQL unable to infer the parameter type. `stream` remains the existing `AsyncStream<Model>` source surface and uses a bounded runtime producer. Its database worker is registered in the current structured task scope; full consumption, `close`, drop, cancellation and decoder/driver failure all release or discard the owned connection according to its known state.

Connection selection order is explicit `database <name>`, root package name, then required `database.default`. `config/setting.json` is strict and external to the binary. SQLite paths remain below the executable directory; invalid or missing settings fail startup.

Repository PostgreSQL acceptance tests and performance tools use the repository-root `config/setting.json` entry `database.postgres_test`. They do not accept connection URLs or TLS modes from environment variables or command-line flags. The local file is ignored by Git; tests stage only the selected connection as `database.default` beside their owned executable.

`test/dever-tests/tests/support/postgres.rs` owns both schema-level and physical-tenant-database fixtures. The latter requires an explicitly isolated test connection with `{case}` in the database name and permission to create/drop test databases. It creates one random control database and a fresh tenant prefix, rejects a pre-existing namespace, and only drops that exact control name plus the generated prefix with canonical positive tenant IDs. Cleanup failures fail acceptance, including when the test itself panics. No configured URL means no connection; these real-service tests remain ignored by default.

`postgres_api` runs stock CMS from a test-owned project copy and validates Owner HTTP requests plus two physical tenant databases. Its explicit test-only member fixture also exercises real non-Owner sessions, two admin roles, a same-named front role and immediate grant/revoke effects; never change reserved Owner mutability or production CMS to manufacture this evidence. Permission catalogs are site-filtered: look up each site's keys through that site's authenticated API. GET/DELETE arguments use query, not JSON bodies. Its `cms.py postgres-case` helper stages its own temporary copy: private ports, secrets, config changes and compiled artifacts must never modify the supplied source project, and persistent reports must omit credentials. A standalone Owner-only run without `--rbac-fixture` still does not prove non-Owner authorization.

The PostgreSQL driver receives named `ConnectionOptions`, shared by initial and tenant connections. These are internal constructor arguments, not an additional configuration layer or environment-variable path.

Application tests never read or copy the project's deployment database settings. A test with reachable Model effects receives a runner-generated `config/setting.json` in its per-case temporary project, containing SQLite `database.default` plus SQLite entries for every explicit Model connection. It uses the normal settings validation, complete schema initialization, migrations, Seed, transactions and shutdown. A suite containing only tests without Model effects uses the base runtime. A mixed suite links the SQLite-capable runtime once, but a selected non-database case receives no generated setting and does not load settings or initialize Models. Neither path accepts database environment variables, PostgreSQL URLs or writes project `data/`.

Global Models resolve their connection once during startup into a `OnceLock<Database>`. When top-level `tenant.database` is configured, ordinary Models resolve through the hidden trusted tenant scope to a bounded, idle-evicted physical pool; there is no mode switch and they never fall back to the global connection. SQLite and PostgreSQL are independent optional runtime features; native build profiles contain only configured drivers.

`global type` marks platform data. With top-level tenant settings, every other persistent Model and its Job queue is tenant scoped. SQLite derives a private database file below `tenant_directory`; PostgreSQL derives a database name from `tenant_database_prefix` and the positive internal tenant id. Only explicit `dever tenant migrate <project-root> <id>` creates/migrates storage and records matching local/control readiness markers. For PostgreSQL that command connects through the configured control URL, creates the derived database when absent, treats SQLSTATE `42P04` as the idempotent already-exists case, and then migrates it; ordinary requests/workers never create or fall back to another database. Requests, Jobs and tenant CMD execution require a ready marker and current schema fingerprint. Table/field modes are not accepted until their entire query, relation, unique, migration and native-SQL contracts exist.

`tenant` accepts only `database`, `max_pools` and `idle_timeout_ms`; `database` selects the control connection. The control database owns `_dever_tenant_storage`, `_dever_tenant_component` and the global `_dever_permission` catalog. Tenant components are the sorted component names owning a tenant Model or tenant Job; platform/global-only and unknown names cannot be toggled. A mixed component is wholly tenant scoped: every Job it owns uses tenant storage even when that Job's own body touches only global data. After migration, `dever tenant component disable|enable <root> <id> <component>` writes only the control table. API, CMD and Job check all tenant components in their reachable execution graph before business work. A `public` API reaching any tenant component is rejected statically.

Authorization role storage is framework-private: `_dever_auth_role`, `_dever_auth_role_permission` and `_dever_auth_user_role`, versioned by `_dever_auth_version`. Platform identities use the control database; tenant identities use their tenant database. Role identity and both relation tables use the composite `(site, role_id)` boundary, so sites may reuse role names without collision and grants cannot cross sites. The v1 single-ID schema migrates transactionally by copying each relation's site from its role before replacing the three tables; an incomplete or unknown private schema fails startup. Disabled roles do not authorize, multiple roles form a union and an `all_permissions` role covers only its site. `_dever_owner:<site>` is reserved and can be provisioned only by `dever tenant owner` after tenant migration; ordinary save, grant, revoke and disable operations must reject it. Applications may expose thin App/API adapters over the core role operations but must not create duplicate business permission tables.

SQLite uses `deadpool-sqlite 0.13` with bundled SQLite, WAL, foreign keys, normal synchronous mode, a busy timeout and a bounded pool. PostgreSQL uses `deadpool-postgres 0.14.2` with `tokio-postgres 0.7.18`, cached static statements and rustls system roots; `min_connections` is prepared before Model initialization. Pool create/recycle and PostgreSQL socket connection have fixed runtime bounds. `wait_timeout_ms` bounds pool wait, while independent `io_timeout_ms` bounds SQL, transaction and migration I/O; both accept 1–300000 milliseconds.

PostgreSQL parameter types must match the runtime's binary bind codec. Dever `Int` binds as i64 and therefore requires a `BIGINT` context in typed native SQL; `Uuid` binds as the native PostgreSQL UUID type and generated placeholders use `$n::uuid`, never `$n::text::uuid`. Casts resolve PostgreSQL parameter types and must not change the runtime value representation.

Model operations, row-stream pulls and transaction begin/commit propagate `dever.database.Error` categories (Pool, PoolExhausted, Connection, Timeout, Cancelled, Database, Constraint, NotFound, InvalidData, Migration). Generated code converts runtime ErrorKind directly to source variants with message payloads; it never flattens recoverable errors into Fault or guesses their category from text. A service capture can use `type ReadResult { Found(value: User) error Failed(error: dever.database.Error) }`. The inferred set conservatively includes every database category. Startup failures remain process failures before application code begins.

## 4. Validation And Error Matrix

| Condition | Required result |
| --- | --- |
| Missing/invalid `setting.json`, unresolved database, conflicting tables or cross-database FK | Startup or compile failure before serving work |
| Transaction has no database effect, spans physical connections/scopes, or starts escaping concurrent work | Compile failure; validated sequential `blocking` remains allowed |
| Bound handler specializes to database work | Specialized effect set and hidden transaction context are retained |
| `run`/thread/parallel/HTTP callback performs database work | Detached context; it cannot inherit an ambient transaction |
| Operation, `BEGIN`, migration or transaction finish is cancelled before confirmed completion | Detach the pool object; never recycle an unknown connection state |
| Actual catalog differs from recorded catalog | Migration failure before schema mutation |
| Constraint tightening conflicts with stored rows | Database error and full transaction rollback |
| Business body fails and rollback also fails | Preserve the business error and append rollback failure as a cause |
| Missing get row or duplicate unique key | Recoverable NotFound / Constraint, including through task wait and transaction rollback |
| PostgreSQL cursor nullable guard leaves `$n` untyped | Generate `CAST($n AS <field SQL type>) IS NULL` |
| PostgreSQL reports admin/crash shutdown, cannot-connect-now or database-dropped SQLSTATE | Classify as a connection error so pool recovery semantics apply |
| Typed native SQL uses an `Int` parameter where PostgreSQL infers int4 | Give the placeholder a `BIGINT` context; do not change the runtime codec |
| Job claim/renew/complete waits for a database write lock | Read the injected Clock only after the lock is owned; never extend or complete an already expired lease with a cached timestamp |
| SQLite `job.lease_ms <= Job timeout_ms` | Fail startup before listen/claim; a persisted incompatible timeout becomes `blocked/invalid_policy` before App dispatch |
| An expired Job already consumed `max_attempts` | Move it to dead with bounded cleanup; it must not occupy an execution slot or hide a later eligible Job |
| Concurrent enqueue repeats an active `(target, key)` | Return the same Job Id from the atomic upsert; never create a second pending/running row |
| Job enqueue has no trusted execution identity | Reject before persistence; absence never means System |
| User Job identity cannot be re-verified or changes tenant | Block with a redacted identity category before business dispatch |
| User Job permission is absent from the current compiled catalog or current role grant | Block as `identity_rejected` before business dispatch |
| A reachable tenant component is disabled | API 403, CMD failure or Job `blocked/component_disabled`; no business dispatch |
| Component name is global-only or absent from the compiled tenant manifest | Reject the control operation; do not create an arbitrary row |
| Legacy active Job lacks identity or crosses the v4 permission-contract upgrade | Block it as `missing_execution_identity` / `authorization_contract_changed`; never upgrade it to System |
| PostgreSQL tenant database is absent during explicit tenant migration | Create the derived database through the configured control connection, then migrate; concurrent already-exists is success |

## 5. Required Patterns

Declare a business function with `transaction name(...) (...)`. The compiler propagates its database effects through ordinary calls and specialized handler bindings, then validates the concrete call graph. The generated hidden context owns one pool object at the outer boundary; nested transaction calls reuse it without savepoints.

Success commits. Error/fault rolls back before propagation. A rollback failure is appended as a cause and does not replace the original error. Commit and rollback consume the transaction, immediately returning a one-connection SQLite pool object after completion. SQLite write transactions use `BEGIN IMMEDIATE`. An API call graph containing password verification or hashing receives no entry-wide transaction; every write in that graph must be owned by an explicit short transaction reached after the expensive work, and that transaction must recheck the mutable state on which the write depends.

Use one checked query planner and shared value model for both database executors. Backend-specific code is limited to SQL dialect, bind/row codecs, pool operations and catalog migration. Keep business transactions in the owning domain App; Model CRUD remains compiler-generated.

Typed Model SQL may return its private result record or an explicit owning-domain App View (`app.ArticleView`, including App topic files), as one row, optional row or list. Reuse ordinary App alias resolution and existing scalar row decoding. App Views must contain only visible supported storage scalar fields; Secret, nested Model/record values and cross-domain or Domain-owned result records are rejected. All regular SQL result records reject field bounds: row decoding does not prove source range refinements. This supports bounded list projections without exporting private Model snapshots or duplicating View types; existing App/API sensitivity checks remain mandatory.

GET may reach typed SQL only when both dialects are proved to be a complete single-own-table field projection: SELECT fields FROM own_table, optional WHERE field = numbered-bind joined by AND, optional ORDER BY own fields, optional LIMIT numbered-bind. `check/sql_read.rs` owns this conservative proof. Unknown syntax, functions, CTEs, subqueries, other tables or extra statements retain the existing write effect; a SELECT prefix alone never establishes safety. Test both dialects independently and preserve POST SQL write behavior.

## 6. Schema, Migration, Seed And Tests

Every database has `_dever_history` rows for schema, named migrations and Seed revisions. Initialization takes a backend lock and updates schema, indexes, Seed and history in one transaction.

- A missing table is created directly at the final schema; old `from` and drop history are not replayed.
- The normalized actual catalog must match recorded history before any update. SQLite fingerprints table DDL and managed index DDL. PostgreSQL fingerprints columns, types, nullability, identity/default metadata, constraints and managed indexes.
- `from` maps one old field to one new field. Name similarity is never inferred.
- Removed fields require a new named `migrate ... { drop field }`; applied migrations cannot be removed or modified.
- Named migrations may contain repeated `before` / `after` blocks with explicit `sqlite`, `postgres` and literal `parameters`. Reuse checked placeholder scanning and existing bind codecs; accept one INSERT/UPDATE/DELETE only, reject reserved internal identifiers, DDL and transaction control. Unclosed quoted regions are diagnostics, including Unicode input. Context-free parameters support Bool/Int/Text/null and finite exponent-form Float; do not silently reinterpret Decimal as Float.
- PostgreSQL dollar-quote recognition must respect identifier boundaries and validate both delimiter and body termination. Do not eagerly construct an EOF slice inside `then_some`; malformed SQL must produce C014, never panic. The same scanner serves typed SQL and migration SQL.
- For an existing table, validate history/order/catalog first, run pending before blocks in source order, apply automatic DDL, then pending after blocks, Seed and history in the same Model transaction. Fresh databases skip old data transforms and record the final migrations as applied. SQL revisions cover phase, source order, both dialects and parameter types/values. Keep drop-only revision calculation stable. Applied migration order is immutable.
- The migration boundary is one Model on one database. Data transforms may reference tables already present at that point; do not promise cross-Model atomicity or arbitrary table initialization order.
- Text/Bytes constraint changes and nullable/unique/FK/choice tightening rely on database validation and roll back without truncating or inventing values.
- Cross-family or lossy type changes are rejected. The currently automatic widening is `Int -> Float`; Text and Bytes bound changes stay within their storage family.
- SQLite rebuilds the table transactionally when fields change and synchronizes managed indexes directly for index-only changes.
- PostgreSQL uses transactional `ALTER TABLE`, named `_dever_` CHECK/FK constraints, advisory locks and managed index replacement. Fresh schemas create tables first, then add foreign keys in a second phase so source order and cyclic references do not break initialization.
- Seed requires a covered unique key and uses parameterized `ON CONFLICT DO NOTHING`. It inserts missing initialization rows and never overwrites existing business data.

Date/time values use integer milliseconds: DateTime is Unix epoch milliseconds, Date is UTC midnight epoch milliseconds, Time is milliseconds since midnight in [0, 86400000), and Duration is signed milliseconds. External wire strings do not change the storage codec, logical Model type or schema fingerprint. JSON uses checked text. Decimal uses a fixed-width sortable text key on SQLite and native `NUMERIC(P,S)` on PostgreSQL. UUID uses a 16-byte SQLite BLOB and native PostgreSQL `UUID`; generated UUID fields receive UUIDv7 values from the runtime.

Long-term tests live under repository-root `test/`. Focused verification must cover database-effect propagation through helpers, nested transactions and handler specialization; rejection of concurrent transaction work; nullable equality with both present and absent values; typed cursor placeholder casts; Decimal/UUID round trips; cancellation isolation; migration catalog drift; settings and Model syntax. PostgreSQL catalog, migration, TLS, native codecs, disconnect classification and cancellation require an explicitly supplied isolated test database; never probe or mutate a local/development PostgreSQL service by assumption.

Durable Job changes select `durable_jobs`. Assert atomic business-write/enqueue commit and rollback, active-key concurrency, claim-time attempt, fresh fencing tokens, old-token CAS rejection, expired-lease recovery, bounded exhausted-row cleanup, blocked target/schema/payload/policy/identity/component, cursor rollback/restart and completed-occurrence dedupe. Cover System/User persistence, missing-identity rejection, v2 identity migration, v4 User authorization-contract blocking, physical/execution tenant dedupe isolation, chained inheritance and generated User re-verification/exact-permission checks. The SQLite regression must hold `BEGIN IMMEDIATE` across a lease renewal window and advance the injected Clock while claim/renew/complete waits, proving timestamps are sampled after lock ownership. Mode/signal tests use only test-owned processes, loopback listeners and temporary SQLite databases.

`model_syntax` covers invalid default/Seed values; `markdown_source` covers Model metadata and schema parity; `sqlite_orm` verifies captured error identity and zero persisted rows after rollback. `orm_data_migration` covers bound SQL, revisions, catalog checks, source order, once-only execution, fresh databases, rollback and native upgrades. `contract_execution` compares both CMS source declarations, Model/API snapshots and settings; `cms_project` checks both source formats and isolated packaged executables. Native row decoding must return its ORM Result inside its own closure; a bare generated block lets `?`/`return` escape into the outer application error type, breaking `first()`.

## Durable Job Store

`job/store.rs` owns versioned private `_dever_jobs`/cursor schema, separate from Model history. Use the existing Database/Executor/Transaction and pool. Private schema v5 stores physical `tenant_id`, execution `scope_tenant_id`, `execution_kind`, provider/site/subject/session/tenant references and one `auth_permission` key inherited from the protected HTTP entry. It never stores a token, Cookie, Header, role or permission collection. SQLite initialization/claims use the ordinary write transaction; PostgreSQL schema installation uses a transaction advisory lock and claims use bounded `FOR UPDATE SKIP LOCKED` rows.

An active partial unique index is authoritative for physical tenant + execution tenant + target + business key. Atomic upsert with RETURNING retains the active row lock until commit and avoids an insert/select completion race. Ambient enqueue shares the caller's physical transaction; standalone enqueue owns a short transaction. Every claim increments attempt and creates a fresh token. Renew and every completion require id/token/running/unexpired lease. Unknown target, wire fingerprint mismatch and decode failure become blocked with fixed redacted categories; retry and timeout use bounded backoff, then dead.

CMD, cron and compiler-owned Test entries explicitly establish System identity. Protected API and restored User Jobs establish User identity from verified Claims; nested enqueue inherits it. On every User attempt generated dispatch selects the configured provider/site verify App, reconstructs the trusted Claims reference, validates the returned subject and tenant, confirms the persisted key still belongs to the compiled site catalog, checks current tenant components and performs the latest role query, and only then begins the business transaction. Invalid identity is blocked; transient verify database failures retain normal retry behavior. A missing task-local identity is always an enqueue error.

Private migration v2/v3 preserves the existing fail-closed identity behavior. Migration v4 to v5 adds `auth_permission`, blocks every active User row as `authorization_contract_changed`, and drops the obsolete `auth_capabilities` column because a previous capability list cannot be guessed into one entry permission. Active System rows and terminal history remain unchanged. No migration guesses System or reconstructs authorization from payload data.

Scheduler identity/fingerprint/cursor are durable. Occurrence insert and cursor advance share a transaction; first enable records the current minute, each pass considers at most 256 elapsed minutes. Completed occurrences cannot be recreated. Job database effects participate in settings-aware transaction validation and per-case SQLite test isolation without manufacturing Model metadata or a second storage stack.

Read the injected Clock after obtaining the write lock: claims establish their deadline after row selection, and renew/completion lock the claim row before checking expiry. A timestamp captured before pool/lock wait cannot prove a lease is still valid. SQLite worker preflight requires `lease_ms > Job timeout_ms`, because a business write transaction blocks heartbeat writes even with a spare connection; PostgreSQL retains shorter renewable leases.

Validate persisted claim policy before dispatch too: older timeout policies incompatible with the current SQLite lease become blocked with `invalid_policy`, without running App. Clean exhausted rows in a separate bounded statement within the claim transaction; they must not occupy execution slots or make test drain mistake an eligible queue for an empty one.

## 7. Good / Base / Bad

Base: a System CMD enqueues a tenant Job and persists its controlled execution tenant. Good: protected `post publish` automatically authorizes and binds `news.article.admin.publish`; each attempt re-verifies the session/membership, compiled permission, component state and current role. Bad: derive actor/tenant from payload fields, treat a missing identity as System, persist JWT/Cookie/roles/current permissions, write manual authorization strings, allow two execution tenants to share a global-queue dedupe key, or create a tenant database during an ordinary request.

## 8. Wrong Versus Correct

- Wrong: hand-written CRUD Service/API wrappers, dynamic query maps, reflection decoders or database handles in source code. Correct: Model methods lower to a checked `QueryPlan` and thin backend executors.
- Wrong: infer a renamed column from similarity or silently accept catalog drift. Correct: require `from <old_field>` and compare recorded actual catalog before migration.
- Wrong: carry an ambient transaction into `run` or another concurrent boundary. Correct: concurrent callbacks use detached database context; only sequential calls inherit the transaction.
- Wrong: release a connection whose async/blocking operation did not report completion. Correct: detach it so the pool replaces it.
- Wrong: make PostgreSQL accept an i64/UUID bind by converting the runtime value to text. Correct: keep the native binary codec and emit the exact `BIGINT`/`UUID` placeholder context.
- Wrong: claim cursor/stream, bulk create/upsert, relation loading or live PostgreSQL behavior from AOT compilation. Correct: claim each only after its dedicated runtime tests exist.
- Wrong: assume an extra SQLite pool connection can renew a lease while a handler holds `BEGIN IMMEDIATE`, or capture `now` before waiting for that lock. Correct: require the SQLite lease to exceed the bounded handler timeout and read Clock after acquiring the write/claim lock.
- Wrong: let exhausted rows consume `LIMIT` and return an empty claim batch while eligible work remains. Correct: clean exhausted rows with a separate bounded statement and query execution slots only from `attempt < max_attempts` rows.
- Wrong: snapshot authorization into payload/queue columns or let legacy rows become System. Correct: persist only a trusted re-verifiable identity reference and the protected entry's exact permission key, then re-verify the current catalog, components and role before each attempt.
- Wrong: let password hashing hold SQLite's writer reservation or switch the whole flow to a deferred snapshot that later upgrades to a write. Correct: perform password work outside a transaction, then enter an explicit short `BEGIN IMMEDIATE` transaction that rechecks mutable state and writes.
- Wrong: silently connect to the control/global database when a tenant database is missing. Correct: create/migrate it only through explicit `tenant migrate`, require readiness/fingerprint markers, and fail ordinary work closed.

AOT compilation proves feature and generated-code compatibility, not live PostgreSQL semantics.

## 9. LLVM Model Application Boundary

### Scope / Trigger

Checked Model/SQL/transaction lowering serves LLVM CMD, API, Job and case-local Test roots. The API Tenant Session section below defines identity, authorization and tenant-pool ownership. Missing tenant identity/readiness fails closed; it never silently selects the platform database. External Worker and default CLI integration remain separate work.

### Signatures

- Core `llvm::emit_application(&Program, &SourceMap)` emits typed Model functions and the owned database application root. Pure SQL/DDL builders remain owned by `native/orm.rs` and shared by both backends.
- Runtime `database::Session::load(profile, bindings, transactions)`, `prepare()`, `database(explicit, root)` and `close()` own only this invocation's Settings and database pools.
- `task::run_entry_with_typed_cleanup(config, future, cleanup)` returns the original typed result after descendants drain and cleanup completes inside the same runtime.
- Canonical `dever_rt_v1_db_*` declarations live only in `crates/dever-backend-bridge/include/runtime.h`; `runtime-sqlite` / `runtime-postgres` add actual drivers independently of `embedded`.

### Contracts

Deployment settings still come only from executable-local `config/setting.json`; no connection environment variable or PATH language runtime is introduced. Ordinary resolution is explicit connection → component root → required default. Persistence ABI accepts only fixed storage scalar rows; business Model records, nullable fields, Related and DbError are concrete typed owners, not dynamic business values. Related ToMany groups at most 998 parent keys per SQL batch and retains the existing per-parent row limit. Migrations group by resolved database, create all tables before foreign keys and preserve existing history/Seed validation.

Writing CMD order is input decode → begin → sequential App/handler calls → output encode → commit → stdout. Nested transaction shares the outer owner, without savepoints; Task and parallel boundaries detach. Failed or cancelled unconfirmed operations discard the transaction connection. Rollback must move the original fault, retain its type/source/payload and only append cleanup cause; commit failure must release the already-initialized output. Drain children, stop every pool, await pool drivers, then destroy the runtime. Never reuse the process-global Settings/pools for repeated embedded roots.

### Validation & Error Matrix

| Condition | Required result |
|---|---|
| Invalid/misaligned output or typed callback slot | ABI fault before I/O, receiver advancement or fault consumption |
| Missing/invalid settings or missing tenant identity/readiness | Explicit root failure, no default connection fallback |
| Database error | Exact one of the ten checked DbError variants with owned Text payload |
| Output encoding or numeric fault after a write | Original runtime fault; rollback; no stdout |
| Captured PostgreSQL SQL constraint error | Current transaction remains aborted; no implicit savepoint/continued-query promise |
| Partial decode, stream stop/cancel or root exit | Paired typed cleanup and connection release/discard in its known state |

### Good / Base / Bad Cases

Base: a CMD creates, reads and deletes a row in its owned SQLite database. Good: real PostgreSQL round-trips storage scalars, captures NotFound without losing identity, and rolls back an uncaught unique violation before the next command inspects persisted state. Bad: use SQLite's ability to continue after a SQL constraint error as a PostgreSQL contract, close a pool after its runtime disappears or reuse a closed OnceLock pool on the next entry.

### Tests Required

Select root `llvm_database_source`, `runtime_entry_cleanup` and the canonical C database driver in `native_runtime_link`. Static cases must pass the existing checker and emit objects for all six targets. Opt-in native cases link the explicitly prepared runtime-only archive, execute with a cleared environment, and check output/fault plus allocation balance over warmup and 64 repeats. Cover CRUD/bulk/upsert, nullable/codec values, cursor/offset, stream cancellation with one connection, ToOne/ToMany, typed SQL, nested/sequential transactions, uncaught constraint/numeric/encoding rollback and successive schema/Seed revisions. Live PostgreSQL requires only the shared `database.postgres_test` isolated fixture, not an environment URL. These checks do not prove target runtime packs, throughput/RSS, tenant/API equivalence or injected commit/close failures.

### Wrong Versus Correct

Wrong: publish Session settings into global OnceLock, run pool cleanup in a second runtime, copy SQL/driver code for LLVM or add savepoints to make a SQLite test pass on PostgreSQL. Correct: invocation-owned Session, original shared SQL/drivers, same-runtime drain/close ordering and tests respecting each driver's existing transaction semantics.

### LLVM API Tenant Session

Job bindings and clock belong to the same invocation Session. Enqueue uses the caller's physical database and ambient transaction; handler transactions and timeout rollback reuse the existing store/worker and transaction owners. Tenant migrations prepare the queue in each selected tenant database before readiness. Component membership alone never enables tenant isolation: Job component checks apply only when deployment settings configure tenant storage. Without tenant configuration, ordinary SQLite/System Jobs must run without a tenant ID. Protected API authorization must bind its exact permission to the trusted Job execution context before enqueue; workers recheck the current subject/session, tenant, catalog, component and role rather than trusting a stored grant.

HTTP/REST and log-enabled CMD use `application::Session` as the single config,
lifecycle, database, tenant and migration-pool owner. The fixed route descriptors
only reference checked Models and auth contracts; SQL/DDL/role storage remain
shared with the original backend. Configured database-only tenant storage is
supported, not a reason to reject every root. Missing tenant identity/readiness
still fails closed; `global` Models use control storage, other Models require the
tenant physical database. A disabled component also denies Tenant Owner.

Tenant pool creation registers a manager entry with `ready=false` before awaiting
schema preparation/marker validation. Interrupted preparation never becomes a
usable pool, and retirement remains owned through `close_pool().await`. Migration
pools register with the Session before their first prepare await. After child
drain, close migration pools, tenant manager and platform pools on the same
runtime; preserve the original fault and append cleanup causes rather than
replacing it. Do not cache a failed or incomplete pool as ready.

`llvm_api_source` uses 65 SQLite roots with two tenant files and the unchanged
allocation ledger; it asserts exact site roles, component denial, owner row
filtering, concurrent identity isolation and POST rollback. Its opt-in PostgreSQL
case shares `support/postgres::run_in_isolated_databases`, supplies only
`config/setting.json.database.postgres_test`, and cleans one random control and
two tenant databases. Neither cross-target objects nor prior Model-only PG
success proves API/tenant execution. Commit/close failure injection and
throughput/RSS remain separate acceptance work.

REST SQL snapshots may inspect HIR without deployment settings, but executable
REST fixtures must use `check_with_settings` with a real site/provider binding
and a checked identity App. Reuse one owned fixture for this setup, as in
`rest_model`; missing auth metadata or generated authorization helpers must not
be worked around by disabling permissions.

### Dependency Lock-Order Regression

The old bundled SQLite 3.51.1 had an ABBA VFS lock inversion: replacement connection creation (`findReusableFd`) acquired global → inode, while WAL connection close (`unixLock` → `unixIsSharingShmNode`) acquired inode → global. A cancelled stream correctly discarded its unknown connection; concurrent background close/replacement then hung runtime shutdown. Native stdout already contained a successful CMD response, so checking only SQL success or Scope completion missed this dependency defect. Repeated opt-in process-bounded stream/root execution and both blocking-worker stacks exposed the exact cycle; no timeout extension, reusable-unknown connection or checkpoint suppression is an acceptable workaround.

Keep the fixed dependency chain (`deadpool-sqlite` 0.14.0, locked `rusqlite` 0.40.2 / `libsqlite3-sys` 0.38.2, bundled SQLite 3.53.2), without editing registry cache or shipping a private SQLite fork. The author workspace requires Rust 1.95. `sqlite::value(column, ValueRef)` receives the real index in ordinary, bounded, owned-SQL and stream decoders; malformed Text retains `ErrorKind::Database` and the column diagnostic. Select the native early-close/repeated-root case and `sqlite_runtime` invalid-Text/replacement tests on dependency changes. An earlier successful run is not sufficient evidence against an observed intermittent shutdown hang.

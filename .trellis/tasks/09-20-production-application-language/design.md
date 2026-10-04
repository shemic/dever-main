# Dever 生产级大型应用语言支持设计

## 1. Delivery Shape

父任务只拥有跨阶段合同、子任务顺序和最终集成验收，不直接实施源码。六个子任务按下列依赖执行：

```text
secure-data-contracts ---+
                         +--> port-adapter --------+
time-typed-codec --------+                         +--> large-cms-acceptance
                         +--> durable-jobs --------+
                         +--> production-api ------+
```

Port 的类型化配置依赖 Secret 和 typed codec；Job 依赖 DateTime、typed codec 和数据库事务；生产 API 依赖 Secret、typed codec 和既有 HTTP 引擎。CMS 只在所有语言能力分别通过后扩展。

## 2. Source Architecture

最终角色矩阵：

| Caller | Allowed application targets |
| --- | --- |
| Generated entry | configured API/Job startup and shutdown |
| handwritten API | own-domain App |
| generated REST | own-domain Model operations under checked field/access rules |
| Job | own-domain App |
| App | own Domain, own Model operations, own Port, any App |
| Domain | own Domain |
| Model | no business functions; field bindings are checked at generated REST entry |
| Port | declarations only |
| Adapter | implemented Port target, same-file helpers, bundled packages |
| Test | same-domain App/Domain, any App, test-local Port fakes |

Type visibility is narrower than call visibility. App contracts may use public business records and Model IDs, but cannot expose private Model fields, Adapter/Domain implementation types or non-observable runtime resources. Port types remain domain-private. Job/API types must pass the shared wire-schema validator.

## 3. Shared Static Value Schema

The checker derives one internal wire schema from existing concrete `Type` and `Definition` data. It is compiler metadata, not a source-visible reflection API. Three emitters consume it:

- setting decoder: strict JSON object to concrete Adapter/runtime settings;
- Job codec: concrete payload to/from persisted JSON bytes plus schema fingerprint;
- API codec: query/JSON request decode and response encode.

Private fields, Secret and resources are represented in the schema policy, not rediscovered by each emitter. The runtime may reuse bounded JSON token parsing and scalar conversion, but generated programs retain concrete Rust values; no universal application `Value` interpreter crosses business calls.

## 4. Secure Values And Model Privacy

`Secret` is an opaque source scalar with no Render, comparison, ordinary JSON, Map-key or Model storage implementation. Approved constructors are secure random/token generation and typed setting/API decoding. Approved consumers are password/HMAC operations and narrowly typed transport sinks such as secure Cookie or bearer-auth construction. Secret cannot be converted to Text by a generic helper.

Model `private` fields remain stored normally but receive domain-owned access metadata. Generated CRUD blocks in the owning App may assign them; field projection is available only to that App. A type containing a private Model field is rejected at App public signatures, API/Job wire boundaries and test assertions. This keeps the existing Model record internally useful without adding DTO directories; App declares explicit view records beside capabilities.

Password hashing uses a maintained Argon2id implementation with runtime-selected secure salt and encoded hash text. Verification accepts Secret plaintext plus stored private hash and returns Bool/failure without exposing intermediate bytes. Security parameters are named runtime constants with tests, not application configuration knobs.

## 5. Port And Adapter Dispatch

Port role declarations use bodyless function signatures and explicit allowed failure choices. Adapter/Test sources may define a qualified Port implementation; ordinary unqualified functions remain local helpers. The checker builds one Port contract and statically verifies implementation inputs, named outputs and effective failure subsets.

Single implementation binds directly. Multiple implementations receive a compiler-generated closed enum and direct match; `config/setting.json` selects one stable implementation identity. This is runtime selection among compiled functions, not dynamic linking or trait-object business dispatch. Only the selected implementation's setting object is required and decoded.

Test-local fakes are associated with the owning TestCase. The single compiled test suite contains all fakes, while the selected case installs only its own compile-checked binding before entry. Production bindings and project deployment settings are not read by application tests.

## 6. Durable Job Data Flow

Each Job contract has a stable logical identity, concrete input shape, schema fingerprint, logical database binding, timeout and retry policy. Job inputs are zero or one wire-safe record; outputs are empty. Job handlers are registered entries and cannot be called as ordinary functions.

```text
App write call under an automatically owned entry transaction
  -> typed enqueue/schedule(job target, payload, dedupe key)
  -> hidden transaction context writes _dever_job row
  -> business rows and job row commit together

Job worker
  -> atomically claims due row with lease
  -> decodes exact schema
  -> calls own-domain App
  -> marks success, schedules retry, or moves to dead-letter
```

SQLite uses a short write transaction and compare-and-update claim. PostgreSQL uses row locking/skip-locked semantics and the existing typed driver boundary. Leases have owner and expiry so a terminated worker does not permanently lose a task. A completed external side effect followed by process failure may execute again; this is documented at-least-once behavior.

Immediate and one-shot scheduled tasks are inserted by App calls. Recurring UTC schedules are compile-time validated declarations and materialize deterministic slot keys so multiple workers do not create duplicate occurrences. Payload signature drift never falls back to best-effort decoding; incompatible work is retained with an explicit failure reason.

## 7. Runtime Modes And Shutdown

`config/setting.json` gains strict runtime/job sections. A single compiled program supports:

- `api`: API server active, Job worker does not claim work;
- `worker`: Job worker active, API listener is not opened;
- `all`: both active for local/small deployments.

The target source contract no longer requires `main.dever` or source-level `serve()` calls. The compiler generates API/Job startup for the configured mode and owns their bounded group, shutdown and resource cleanup. Disabled services return without opening resources. This is a planned migration; the current compiler still uses Main.

The process signal owner is runtime-level and broadcasts to active services. API closes its listener and reuses existing connection drain. Job stops polling/claiming and drains active leases to its deadline. The generated entry then closes databases and flushes logs. Service failure cancels and drains its Group siblings.

## 8. Production API

API route discovery remains compiler-owned. The target syntax uses `get/post/put/delete/cmd/rest`; `rest` lowers plain CRUD to the same-domain Model, while custom actions bind the same-domain App signature. POST/PUT accept typed bodies; query routes accept named wire-safe scalars. A request Context is hidden in the runtime rather than declared as a source parameter. Validated transport observations and bounded Cookie/Header mutation are available through controlled API capabilities; no raw request/response handle reaches App. The detailed contract is owned by the `09-20-production-api` child task.

Handler success is encoded into the existing `{code,message,data}` envelope. Recoverable App failures must be captured and mapped to a standard `dever.api.Error`; uncaptured failures produce a generic internal envelope and structured, secret-free log. This keeps business errors independent of HTTP while allowing explicit 400/401/403/404/409/429 mapping.

Uploads are streamed to an owned temporary file under runtime data/tmp with checked total size, part count, filename and MIME metadata. API receives an opaque upload handle/metadata, and a storage Port consumes it. Drop, cancellation and handler failure remove unclaimed temporary files. The CMS does not expose arbitrary filesystem paths.

## 9. CMS Integration

The maintained CMS is the cross-feature acceptance application, not a second implementation of framework logic. Plain and Markdown forms retain equal normalized declarations. Plain CRUD uses generated REST; domains own explicit App views and commands when business behavior requires them. No dto/service/repository folders are introduced.

Authentication uses Secret password input, private password hash storage, hashed session token storage and secure Cookie output. Media uses a storage Port with a local production Adapter and test fake. Publishing writes the state transition, audit row and scheduled Job atomically. Job execution rechecks state/idempotency before publishing and can retry safely.

## 10. Compatibility And Rollback

This language is pre-release and repository callers migrate atomically; no legacy aliases, environment fallback, old JSON date encoding or dual role matrix remains. Each child task is a rollback boundary. A child is not accepted while canonical docs, Markdown contracts or maintained CMS callers use two protocols.

No database destructive migration is run against user data during compiler tests. Runtime private Job schema uses additive/versioned initialization and test-owned databases. Real PostgreSQL verification requires an explicitly configured isolated database.

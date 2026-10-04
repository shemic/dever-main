# Research: Durable Job implementation map

- Query: 核实持久化 Job 的源码接入点、最小复用架构、SQL/生命周期风险和定向验收。
- Scope: mixed；以当前本地源码为准，外部资料仅核对 SQL/信号基础能力。
- Date: 2026-09-20

## Findings

### 当前事实和实施前须解决的契约矛盾

1. 当前没有 Job source role、Job runtime、cron parser 或 SIGTERM/Ctrl-C owner。`rg signal|SIGTERM|ctrl_c|cron|Job crates library` 仅发现 `core/wire.rs` 的 `Policy::Job`。不可将设计中的这些能力当作已有实现。
2. `wire::Policy::Job` 已存在，拒绝 Secret/private Model；现有 wire 同样拒绝 ModelId。设计示例 `PublishInput.article_id: content.article.model.id` 因而不成立。需要明确选择：扩展共享 wire owner 支持保持 nominal 身份的 ModelId，或修改示例使用当前 wire-safe DTO；不得只给 Job 私加类型白名单。考虑 Job 最常见用途正是实体异步操作，建议在共享 schema/codegen 增加 ModelId 节点，协议用字符串，fingerprint 保留所属 Model 身份，同时明确适用 policy。当前 `native/wire.rs:32` 的普通 `Id` 已映射 runtime `Id`，不可把 ModelId 在类型系统里降级成普通 Text/Id。
3. `hir.rs:21`、`contracts/effects.rs:18` 的 database effects 是 Model ID 集合；`native.rs:436` 的 transaction 取第一个 Model 找连接。纯 Job enqueue 无 Model 时无法运行。必须推广为连接资源 effect，不能伪造 Model 或复制第二套 transaction context。
4. 现有跨连接检查在 `model.rs:256` 的 `validate_database_settings`，依赖设置解析后连接名，而非仅 AST check。明确显式不同连接可在 check 拒绝；PackageRoot fallback 仍需要 settings-aware CLI 检查/启动校验。不要错误拒绝不同 selector 最终落在同一个 default 的合法情形。
5. `retry(5)` 的含义必须固定为最多 5 次 attempt，还是首次之外 5 次重试。PRD 用 max attempts，建议前者并准确写明；不能只在失败后加 attempt，否则每次执行中崩溃会无限重领。
6. 设计只规定 UTC cron，未规定字段语法、DOM/DOW 组合、停机补跑和首次部署起点。最小实现应固定五字段数字语法、范围/列表/步长和 DOM/DOW 规则；持久 checkpoint、每轮有界补齐。拒绝不能满足的日期组合或规定有界搜索失败，不能无穷找下个时间。

### Owner 文件与复用入口

| 文件/符号 | 当前职责与 Job 接入 |
| --- | --- |
| `crates/dever-core/src/source.rs:25` SourceRole/from_name/name | 当前 App/Domain/Model/Port/Adapter/Api；增加 Job，沿用 path-derived identity。 |
| `check/layout.rs:14,56` RoleFiles | 已有 role file/directory 排他、非 API 平铺、目录至少两文件；增加枚举后复用，不另写 Job scanner。 |
| `syntax.rs:169,184` FunctionClause/FunctionKind | 已有 ordinary/transaction、fails/bodyless；Job kind 和 bounded policy 作为声明元信息。schedule 用独立声明。 |
| `parser.rs:210,707,761` declaration/function/function_contracts | contextual transaction 的 dispatch 可复用；增加 job/retry/timeout/schedule；勿让 retry/timeout 成普通函数调用。 |
| `format.rs:120,258` declaration/function | 共享 AST formatter，输出 job 前缀和稳定 policy 顺序。 |
| `markdown/contract/parse.rs:198,237,261` / `validate.rs:247` | function 元信息、非 function directive 标识已有 owner；schedule 需加入 directive 分支；Markdown policy 元信息应与 source 一致。 |
| `check.rs:321,423,438` Program/test registration | 注册 Job metadata 和 target map；test case closed graph 与 fakes 已完成，必须沿用。 |
| `check/symbols.rs:222` can_call / `:277` can_access_type | API 不可调用的现有模式适用于 Job；普通 function resolution 和 handler resolution 都必须拒绝 Job。Job 仅调用同域 App，DTO 类型访问另定规则。 |
| `check/expressions.rs:521,620,655` static call/handler reference | 可复用 named-path 解析形态，但 enqueue target 不是 Handler 参数，不能转发。新增专用 checked enqueue path，参数先解 target 再按 payload schema 检查。 |
| `hir.rs:164,507,524` ExpressionKind/static_call/CallArgument | 用 JobEnqueue {target,payload,key,run_at} 明确表达入队；遍历/变更遍历要覆盖三个值表达式。enqueue 不等价同步 call。 |
| `contracts/dependencies.rs`, `contracts/effects.rs:61,505`, `contracts/error_effects.rs` | enqueue 引入本 Job 连接、database failures 和 suspension；不得传播 Job handler 的业务失败/网络 effect 到 enqueue 的 App。 |
| `specialize.rs:77,120,178` database_effects/concrete/successors | 独立执行根收集 Job handlers；不要把 enqueue->handler 当普通递归 edge，App 入队自己未来工作的循环是正常队列行为。 |
| `native.rs:292,418,434,569` DB setup/function/call | 统一隐藏 `_database: Option<&Transaction>`；普通 enqueue 继承，Job handler 执行从 None 开始。 |
| `native/orm.rs` | 现有 Executor/参数/错误转换供 Job emitter 复用；不复制数据库错误到字符串再猜类别。 |
| `native/api.rs`, `native/ports.rs` | 静态 dispatcher 和 startup initialization 可参考；Job 生成 concrete decoder + direct function invocation，不造 runtime Value 解释器。 |
| `native/build.rs:120,309,349` | suite profile、runtime features、缓存身份；Job 无 Model 仍需 database+wire+已选驱动。feature/cache 与源码变化同步。 |
| `crates/dever-runtime/src/database.rs:167,317,458` | Database/begin、Executor::new、database_for；所有 Job SQL 通过既有 Sql 双方言和 Value/Row 编解码。 |
| `sqlite.rs:174,223,279` | BEGIN IMMEDIATE、transaction 方法、未确认完成 Drop 丢弃 pool object。可直接用于 claim 短事务。 |
| `postgres.rs:236,302,383` | BEGIN、带 I/O deadline 的事务、Drop 丢弃连接；claim 使用 SKIP LOCKED。 |
| `sqlite/migration.rs`, `postgres/migration.rs:73,109` | Model 自己的 schema/history owner；Job 私有 schema 不进入它们，但复用事务/advisory lock 纪律。 |
| `config.rs:11,70,144,325,356` | 单一 Settings，严格 top-level/duplicate wire 验证，bootstrap/http lazy read。扩展 typed runtime/job config。 |
| `task.rs:78,488`, `task/scoped.rs` | 单 runtime、Scope supervisor、Group；worker/heartbeat/signal 等所有活动必须有结构化 owner。 |
| `api.rs:13`, `http/mod.rs:147,176`, `net.rs:47,232` | API 内创建 listener；HTTP 已响应 listener.close 并 graceful drain。新增 signal owner 触发此路径，不用取消 serve 代替优雅关闭。 |
| `database.rs:496,502`, `postgres.rs:462,473` | prepare/shutdown 当前只负责 PostgreSQL；SQLite 没有显式 pool shutdown 路径。若 PRD 保证最终关闭 SQLite，需补 owner。 |
| `core/wire.rs:10,55,64`, `native/wire.rs:8` | Job policy、稳定版本 schema identity、concrete codec；当前 fingerprint 是确定性描述串，不是固定长度加密 hash。持久字段可存完整 identity，不误称 SHA。 |
| `cli/test_runner.rs:73,126` / `check.rs:466` | 每 case 新进程/临时目录，runner 生成 SQLite setting；无 DB case 无 setting。加入 Job connection、harness，仍不读部署配置。 |

### 建议最小架构和数据流

保留 Job body 为 Function/HIR，额外 Program.jobs 持有 identity/function/connection/payload schema/policy/schedule。compiler 专用 enqueue 表达式只保存静态 target ID 和普通参数；不将 target 暴露成值类型。Job entry 的 ordinary/handler 调用拒绝统一放符号访问 owner。

数据库 effect 推荐抽为统一 `DatabaseBinding`（selector + fallback root 的稳定值或 intern ID），Model 与 Job 都引用。Model 的 CRUD schema/index 仍保持 Model ID，仅 effects/transaction/startup/test 配置改用连接 binding。为最小改动也可用明确的资源枚举 Model/Job 并统一 `binding()` accessor，但不应让消费者每处重复 match。

源码 Job → checked metadata/payload Schema → enqueue HIR → 编码 Encoded JSON → 同一 Executor 写入 → commit → worker claim → 校验 target/schema → concrete decode → direct App 调用 → owner-CAS finish/retry。enqueue 的调用图不包含 handler 业务 effect；worker 启动根和 test harness 根要独立包含 handler/App/Port fake graph，避免漏链接及错误递归判定。

runtime 可由 `job.rs` 拥有 public compiler bridge，`job/store.rs` 拥有 DB 状态机和 SQL，`job/schedule.rs` 拥有 cron/slot，`job/worker.rs` 拥有有界执行；只有职责确实需要时拆这些文件，不添加通用 queue framework。compile-time cron 验证可调用纯 runtime cron parser，避免编译期/运行时双实现。chrono std-only 已存在，复用 UTC calendar；无须引入时区库。

### SQL/租约策略

- runtime private version 表 + job 表 + schedule checkpoint 表。SQLite BEGIN IMMEDIATE，PG 独立 runtime namespace advisory transaction lock；version 不认识时明确失败，不能 CREATE IF NOT EXISTS 后假装兼容。
- enqueue 的 active-only 唯一索引覆盖 `(target,dedupe_key)` 且 predicate 只含待执行/执行中状态；参数化 INSERT ON CONFLICT DO NOTHING，再返回已有 active ID。不能用 SELECT-before-INSERT 作为唯一保障。业务失败 rollback 必须撤销同物理事务的入队。
- PostgreSQL claim：短事务选取到期 pending 或过期 running，`FOR UPDATE SKIP LOCKED LIMIT ...`，写 owner、lease_until、attempt 后提交。SQLite 同过程已有 BEGIN IMMEDIATE 序列化。只 claim 有空闲执行容量的条目，不能先批量占 lease 再在无界内存队列等候。
- attempts 建议在成功 claim 时递增；超时/失败/崩溃恢复都耗费次数。过期且 attempts 达上限的条目需进入 dead-letter，不能永远排除于查询而残留 running。
- owner token 每次 claim 新生成，不能仅以进程 worker ID 作为 fence；旧执行在同 worker 中过期重领后仍可能迟到。finish/renew/retry 带 id + token + state，必要时判断 lease_until；影响行数为 0 即丢失租约，不可继续确认成功。复用现有 OS randomness/Uuid owner，不硬编码 ID。
- renewal 不与 handler 持有同一 transaction；需预算 worker 数与 pool 容量，避免单连接长事务使 heartbeat 饿死。取消后复用当前丢弃未知连接的规则。若 handler 无法强制立即停止，最多只能保证至少一次，外部副作用依赖业务 idempotency。
- PG runtime Int bind 是 i64，SQL 数值列和参数使用 BIGINT 语境；时间全部 epoch ms，勿调用 SQL now 导致测试时钟不一致。错误摘要写固定错误类别/有界安全信息，不保存 AppError Display（业务 message 可含 payload）。
- cron occurrence 幂等不能只用 active-only job 索引：已经成功的 occurrence 可能被另一 scheduler 再建。锁定 checkpoint 行，插入 jobs 与推进 last slot 同事务；或者永久唯一 occurrence ledger。checkpoint 必须按规范化 cron identity，计划变更不能回放全部历史。每轮扫描/补跑有界。
- key/payload/target/schema/error 等都有确定上限；wire 16 MiB 默认上限已经有界，但 worker 并发乘以该值仍需评估，不宣称低内存性能。

### Mode、启动与关闭

当前 RuntimeProfile 只有 sqlite/postgres；Tokio RuntimeConfig 只有线程/任务容量，均没有 api/worker/all。严格 runtime/job 文档必须进入 Settings 的真实解析，不能仅在 serve 里临时读 JSON。主程序调用 serve 的能力图用于校验 mode：worker-only 不要求 HTTP；API-only 不 claim；all 按 main 的 Group 并行拥有两个 server。启动前校验对应模式的缺失配置及所有启用 Job timeout/lease 关系，不能 API 已监听后才发现 Job 不可用。

API runtime::serve 需在创建 listener 后把关闭与 drain 接到同一 shutdown token。signal watcher 是 generated entry/运行时生命周期拥有的 future，不可 detached spawn；SIGINT/SIGTERM 一次广播，停止 accept/claim/schedule 后等待在途工作到统一 deadline。deadline 到达使用现有 Task/Group cancel-and-drain，再执行 database shutdown 和 log flush。`Group::stop` 当前直接 abort body，正常收到 signal 时先合作排空，超时才 stop。

Tokio 1.53.1 当前 features 不含 signal（根 Cargo.toml:20），Cargo.lock 未找到 signal-hook。需启用 Tokio signal feature 并确认 offline 所需依赖已缓存；不要用 unsafe libc handler（workspace 禁 unsafe）或 shell signal 轮询做替代。PG driver shutdown 当前存在；SQLite pool explicit close 和 startup failure 清理须单独覆盖。generated entry 当前仅 profile.postgres 时调用 shutdown，需按完成的生命周期设计调整。

### Application test 隔离

保持每 case 单独 native module、closed fake graph、进程和临时目录。test metadata 收集 enqueue 与 harness 执行的 Job 连接以及 handler 访问的 Model；不能只看 test 主函数的直接 Model effects。每 case SQLite default/explicit connections 由 runner 生成。Job-only case 也生成 setting；非数据库 case 仍无 setting。

设计还没有给 compiler-owned test harness 的源码调用协议，不能只实现 Rust store 单元测试便声称应用测试可用。建议最小 test-only drain-due/advance-clock 操作，返回有界观察结果或保证已处理 due work，测试通过 App 查业务状态；不开放 DB handle 或普通直接 Job entry 调用。假时钟作为 case-owned Clock 注入 store/scheduler，不改全局系统时间、不真实 sleep。

### 定向验收建议

1. 新 `durable_jobs` 源码/formatter/Markdown 定向 target：合法声明、错误 role/直接 call/handler reference、0/1 record 参数、Secret/private/非法 payload、policy 上下界、cron 非法、纯 Job transaction 与跨连接。
2. runtime SQLite temp DB：业务写入+入队 commit/rollback；并发 active key 去重；终结后 key 重用；run_at；多 claimant；租约过期、旧 token finish/renew 拒绝、崩溃耗尽 attempts；有界退避/溢出；损坏/schema/unknown target 不执行。
3. cron 两个独立 scheduler 同 slot、已完成 job 后再次调度、checkpoint rollback/restart、停机补跑上限、闰年/月底/DOM-DOW 规则。
4. `application_testing` 精确 case：同二进制不同 case fake 完全隔离；Job-only SQLite setting；部署 PostgreSQL 配置未读取；受控时钟/harness 触达 App。
5. config 三 mode 缺失/未知/重复字段、timeout/lease/poll/shutdown 约束。测试专有子进程 signal，明确 deadline；仅所属随机回环 listener（若批准）验证停止新请求和 HTTP drain；不向已有服务发送信号。
6. `structured_concurrency`/`pooled_network graceful_shutdown` 受影响 case；SQLite one-connection cancellation 后可恢复；PG 仅 feature cargo check 与 SQL 生成断言，真实 DB 另行授权条件。

## External references

- PostgreSQL SELECT locking 官方说明：`SKIP LOCKED` 适合多消费者 queue 场景，但仅跳过 row lock，仍有表级锁：https://www.postgresql.org/docs/10/sql-select.html 。这是 SQL 策略依据，不是对本仓库 PostgreSQL 实测。
- SQLite 官方 partial index 文档：https://www2.sqlite.org/partialindex.html ，UNIQUE 可只约束满足 predicate 的行。
- Tokio signal 官方 Rustdoc：https://docs.rs/tokio/latest/tokio/signal/unix/fn.signal.html ，Unix signal API 要求 signal feature。仓库实际 pin 1.53.1，应以本地锁定版编译验证依赖。
- 本地版本：Tokio 1.53.1、chrono 0.4.45 std-only；SQLite/Postgres pool 沿现有锁定依赖，不建议增加队列或 cron 框架。

## Related specs

已查 `.trellis/workflow.md`、任务 prd/design/implement/task.json，以及 backend `index.md`、`directory-structure.md`、`error-handling.md`、`quality-guidelines.md`、`compiler-contracts.md`、`database-guidelines.md`、`toolchain-and-library.md`、`logging-guidelines.md`。Job 新约束尤其会改变 compiler-contracts 的 Model-only effect 描述及 test runner 数据库触发条件。

## Caveats / Not Found

- 子 agent `task.py current --source` 返回 none；父 dispatch 明确提供本任务绝对路径，本研究仅写此 research/，不改 active state。
- 未改源码、spec、任务状态；未运行 build、测试、服务、外部数据库或性能测量。
- 当前研究证明复用边界和缺失点，不证明 Job 可执行。policy、cron、payload ModelId 和 test harness 上述未落实契约必须在实施/验收中明确，不能悄悄删需求。

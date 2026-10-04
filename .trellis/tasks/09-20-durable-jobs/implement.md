# Dever 持久化 Job 实施计划

## Start Gate

- [x] time-typed-codec 与 port-adapter 完成。
- [x] 用户批准父任务规划并允许继续实现。
- [x] 读取 structured concurrency、ORM transaction/settings、native profile、application test runner 和 HTTP drain owner。
- [x] 完成源码 implementation map 与独立设计复核。
- [x] 收紧 ModelId、connection effect、claim attempt、cron cursor、lifecycle 和测试 harness 契约。

## 1. Source And Static Contract

- [x] 增加 Job SourceRole、layout、contextual declaration、policy、schedule、formatter 和 Markdown parity。
- [x] 增加 Program Job metadata、静态 target identity、App-owned payload schema 和 nominal ModelId wire codec。
- [x] 增加 enqueue/enqueue_at 专用检查与 HIR；拒绝普通 call、handler/run/group 转发和非法 role/type/signature。
- [x] 将 Job handler 作为独立生成 root，不把 enqueue 视为同步依赖或递归 edge。

Gate：新增 `durable_jobs` 源码/Markdown测试覆盖合法声明、路径、签名、可见性、payload、policy、cron和target正反例。

## 2. Connection Effect And Atomic Enqueue

- [x] 将 Model-only database effect 推广为统一的有类型 connection owner，并集中解析 ConnectionSelector。
- [x] enqueue 只传播队列连接 effect，不传播 handler 业务 effect/failure。
- [x] native emitter 复用 `_database`、Executor 和 wire codec，实现 standalone short transaction 与 ambient transaction enqueue。
- [x] settings-aware bootstrap/test runner 使用统一 connection effect；拒绝解析后的跨连接 transaction。

Gate：enqueue-only transaction 可生成；Model write + enqueue 同时 commit/rollback；跨连接拒绝；同 transaction 不二次 checkout。

## 3. Private Store State Machine

- [x] 实现 SQLite/PostgreSQL runtime private schema、版本校验和初始化锁，不进入 Model history。
- [x] 实现 active-key 原子幂等 enqueue、run-at、bounded row/value limits。
- [x] 实现 bounded claim、claim-time attempt、随机 token、lease renew/expiry 与 owner CAS。
- [x] 实现 success、retry/backoff、dead、blocked 及受控错误分类/摘要。

Gate：SQLite temp DB 覆盖并发去重、终结后 key 重用、到期、双 claimant、崩溃耗尽、过期重领、旧 token 拒绝和 schema/decode/target blocked；PostgreSQL 验证 feature/SQL 生成。

## 4. Schedule, Worker And Test Harness

- [x] 实现共享五字段 UTC cron parser、匹配与 bounded next-slot 搜索（每轮扫描最多 256 个分钟槽，不做无界寻找）。
- [x] 实现持久 cursor、同 transaction occurrence materialize/advance、首次启用和每轮最多 256 个补偿窗口。
- [x] 实现 bounded worker pool、timeout、heartbeat 和静态 payload dispatch。
- [x] 扩展 application test closed graph、case-local clock 和 bounded due-job drain，复用生产状态机。

Gate：多 scheduler 同 slot、completed occurrence 不重建、cursor rollback/restart、cron fingerprint 冲突和补偿上限；不同 case fake/clock/database 不泄漏且不读取部署配置。

## 5. Mode And Lifecycle

- [x] 增加严格 runtime/job settings、source capability 预检和 runtime profile feature/cache identity。
- [x] 实现 api/worker/all gating，保留同 binary 的 API/Job dispatch roots。
- [x] 增加结构化 SIGTERM/Ctrl-C owner；API stop-accept/drain 与 Job stop-claim/drain 共享 deadline。
- [x] generated entry 在应用汇合后统一 database shutdown、log flush，覆盖启动失败和超时取消。

Gate：缺失/非法配置在 listen/claim 前失败；三 mode 只启动应有服务；test-owned 进程验证信号后停止新工作、drain/超时取消和关闭顺序。

## 6. Documentation And Focused Verification

- [x] 更新 LANGUAGE、MARKDOWN-SYNTAX、compiler/database/toolchain 中的 application-testing 契约。
- [x] 检查无环境变量入口、动态 target、第二套 codec/transaction/pool、脱管 task 或生产 Adapter 测试回退。
- [x] 完成独立 Trellis check，修复 in-scope findings。
- [x] 主代理最终复核并更新父任务 checklist。

```bash
cargo check --locked --offline -p dever-core -p dever-runtime -p dever-cli
cargo check --locked --offline -p dever-runtime --features sqlite,postgres
cargo test --locked --offline -p dever-tests --test durable_jobs
cargo test --locked --offline -p dever-tests --test source_architecture
cargo test --locked --offline -p dever-tests --test error_effects
cargo test --locked --offline -p dever-tests --test orm_query
cargo test --locked --offline -p dever-tests --test sqlite_orm
cargo test --locked --offline -p dever-tests --test application_config
cargo test --locked --offline -p dever-tests --test application_testing
cargo test --locked --offline -p dever-tests --test structured_concurrency
cargo test --locked --offline -p dever-tests --test pooled_network graceful_shutdown
git diff --check
```

所有 Cargo 命令使用无增量、debug info 关闭和串行测试。默认不运行 workspace 全量测试、真实性能套件或持久服务。真实 PostgreSQL 仅在仓库隔离 `config/setting.json` fixture 明确存在且用户知情时运行；否则明确记录为未运行。

## 实施记录

- 源码、共享 wire/connection owner、生产 store/worker、native/test roots、mode/lifecycle 已接通；以上勾选表示实现完成，最终 Gate 证据记录在下方，不代替独立 Trellis check。
- 启用既有 Tokio `signal` feature；经主代理明确授权解析依赖，Cargo.lock 仅新增 `errno 0.3.14` 与 `signal-hook-registry 1.4.8` 两个传递项。后续命令均 `--locked --offline`。
- API-only 保留既有省略 runtime 配置的兼容行为（api、30000 ms）；有 worker capability 的应用必须显式配置 runtime。worker pool 比执行槽至少多一个连接，以免 handler 事务耗尽 heartbeat/claim 容量。
- enqueue 使用单条 active-key upsert + RETURNING，避免 PostgreSQL 上冲突 insert 后、查询 Id 前任务终结的竞态。
- 新增测试发现 case 裁剪不可达 transaction 后被误报空事务；修复共同 effects validator，真实空事务仍有 clause 并继续被拒绝。
- 数据库服务、真实 PostgreSQL、全量测试、性能/内存压测未运行；rustfmt/clippy 未安装，不安装。

### 最终实施验证

- `cargo check --locked --offline -p dever-core -p dever-runtime -p dever-cli`：通过。
- 同一 check 加 `--features dever-runtime/sqlite,dever-runtime/postgres,dever-runtime/api,dever-core/reference`：通过。PostgreSQL 仅 feature 编译证据，未连接真实数据库，不把它算作锁、并发或 SQL 执行验收。
- `durable_jobs`（sqlite）：14/14。覆盖源码/Markdown、跨连接拒绝、nominal ModelId native 往返、同物理事务 commit/rollback、active-key 并发幂等、定时到期、claim-time attempt、旧 token CAS、崩溃耗尽、schema/corrupt/unknown 的 generated dispatch blocked、多 scheduler、完成窗口不重建、cursor 中途失败回滚/重启、256 窗口上限、case-local SQLite/clock/fake 与无数据库 empty drain。
- 同一 `durable_jobs` 目标包含真实 native 二进制的 api/worker/all 三 mode：临时回环监听和临时 SQLite；api 不 claim、worker 不监听、all 同时工作，均 SIGTERM 正常退出。独立 runtime 子进程另覆盖 heartbeat、停止 claim、在途排空与 shutdown deadline 取消后关闭数据库/flush 日志。
- `application_config`：3/3；`application_testing`：8/8；`source_architecture`：8/8。
- `pooled_network graceful_shutdown`：1/1，验证已有 HTTP 停止 accept 并完成在途响应。
- `time_codec wire_schemas_do_not_change_model_time_storage_identity`：1/1。该条旧断言随 nominal ModelId wire 合同更新；无 private/bounded 字段的 Model record 也可通过共享 codec，存储 snapshot 不变。该 fixture 已调整为当前 App 路径，未顺带改造其余旧测试。
- 上述测试均串行；`CARGO_INCREMENTAL=0`，dev/test debug info 为 0，固定 stable toolchain PATH。`git diff --check` 通过；新 Job/lifecycle 源码检查未发现环境变量读取、动态 target、脱管 spawn 或 unsafe。
- 独立 Trellis check 结果见下方；父任务 checklist 由主代理收口，任务仍保持 `in_progress`。

## 独立检查与修复

- 复现 SQLite handler 持写事务时短租约无法续租：业务已提交，但任务仍为 running。当前源码的启动预检要求 SQLite `lease_ms > timeout_ms`；PostgreSQL 继续支持短租约续租。领取在获得锁后建立 lease，续租/完成先锁 claim 行再读注入 Clock，避免使用锁等待前的旧时间。
- worker 在调用业务前重新验证持久化记录的实际 timeout/max attempts；旧策略不满足当前 SQLite lease 或已越界时进入 `blocked/invalid_policy`，不执行 App。公共 timeout 上限仍为 3,600,000 ms，lease 上限为 7,200,000 ms；测试使用最大 lease，合法长任务不会因测试固定短租约被误拒绝。
- 耗尽 attempts 的到期记录改为同一 claim 事务内独立有界清理；eligible 查询跳过耗尽记录，避免它占据执行槽并使 drain 误判队列为空。
- 信号测试原来对同一 run_at 的任务假定按业务 key 执行，但实际稳定排序使用 run_at/UUID；同毫秒 UUID 带随机部分。改用不同到期时间表达本测试的执行次序，同时保留实际 heartbeat、持事务 drain 和超时取消检查。
- 保持既有已审阅的 API-only 默认配置契约；有 worker capability 时 runtime 必填。统一 cron 文档为每轮扫描/推进最多 256 个分钟槽，只有匹配 cron 的槽生成任务。
- 同步 LANGUAGE、design、compiler 目录 owner 图以及 database/toolchain 规范；未增加环境变量、第二套 pool/transaction/codec、动态 target 或脱管任务。

### 独立验证证据

- 最终 `cargo check --locked --offline -p dever-core -p dever-runtime -p dever-cli --features dever-runtime/sqlite,dever-runtime/postgres,dever-runtime/api,dever-core/reference`：通过。
- 最终 `cargo test --locked --offline -p dever-tests --features sqlite --test durable_jobs -- --test-threads=1`：14/14，通过，122.86 秒。包含最大合法 timeout 的 case-local SQLite/fake、claim/renew/complete 锁等待时钟回归、exhausted 后正常任务 drain、旧持久策略 blocked、三 mode 和 SIGTERM 的 grace/timeout/持事务三场景。
- 主代理最终复核再次运行 `sqlite_durable_state_machine`：1/1 通过；core/runtime/CLI 的 SQLite+PostgreSQL+API+reference feature check 再次通过。
- Cargo 命令统一固定 stable PATH、`CARGO_INCREMENTAL=0`、dev/test debug info 为 0。`git diff --check` 和改动 Job 文件尾空白检查通过。
- 一次中间 suite 为 13/14：信号 fixture 的同到期时间随机顺序断言失败；已按上述原因修复，最终完整定向 target 14/14。
- rustfmt/clippy 二进制未安装，未安装或运行；全量测试、真实 PostgreSQL、性能/内存压测未运行。PostgreSQL 证据仅 feature 编译与代码审查，不宣称已完成真实锁/并发/SQL 执行验收。
- 本次独立检查没有剩余已知实现阻塞；任务状态按调度要求保持 `in_progress`，交由主代理最终复核。

## 待复核风险

- 业务 commit 与 Job insert 必须共享同一 physical transaction，不能 after-commit 补写。
- attempt 必须在 claim 时增加；finish/renew/retry 必须比较独立 claim token 和有效 lease。
- cron materialization 必须由 cursor transaction 保证，不能复用 active-only key 或单进程内存 timer。
- enqueue 不得引入 handler 同步 effect/递归；worker/test root 必须包含 handler 的真实 Model/Port/fake 闭包。
- runtime mode 必须在副作用前统一预检；signal drain 不能调用立即 abort 的 Group::stop 伪装优雅退出。
- payload/schema identity 是持久协议；未知版本必须 blocked/fail closed，不能 best-effort decode。

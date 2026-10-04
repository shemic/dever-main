# 类型安全 ORM 实施清单

## 开始前门槛

- [x] 用户在最终规划摘要之后明确批准进入实现，并要求增加 `examples/dever/cms` 项目。
- [x] 运行 `python3 ./.trellis/scripts/task.py start 09-13-typed-orm`，加载 `trellis-before-dev` 上下文。
- [x] 核对 `Cargo.lock` 中待加入驱动与当前 Rust 1.85/MSRV、license 和 feature；只锁定实际使用的最小 feature。
- [x] 为每个阶段建立对应的根目录 `test/` 定向测试目标；不运行 workspace 全量测试或集成压测。

## 阶段 1：隐式挂起语言表面

目标：删除 Dever 源码 `async`/`await`，保留 Rust runtime 的真实异步执行。

- [x] 调整 lexer/parser/syntax/formatter，删除 function/handler async 与 `await(...)`，保留 `run`/`wait`/`stop`。
- [x] 将 `FunctionClause.is_async` 拆为编译器推导的 HIR `suspends`，复用现有调用依赖图传递基础挂起 effect。
- [x] 更新 checker：普通调用自动按目标 effect 顺序等待；`run` 接受可启动调用，Task/Group 仍保持结构化所有权。
- [x] 更新 native emitter：同步可达图生成直接入口，挂起可达图生成唯一 Tokio 入口和内部 await。
- [x] 更新 API snapshot、Markdown 合同、官方 `dever.task/net/http/websocket/sse` 源码、examples 与相关 fixture，不保留旧语法。
- [x] 清理旧 `AwaitCall`、`TaskStart`、公开 async 专属诊断和死分支，保留 runtime Rust async 节点。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test structured_concurrency
cargo test -p dever-tests --test async_native
cargo test -p dever-tests --test async_network
cargo test -p dever-tests --test http_library
cargo test -p dever-tests --test live_library
```

完成条件：HTTP/TCP/WS/SSE 的 Dever fixture 不含源码 async/await，协议、背压、取消和同步程序无 Tokio 入口的定向测试通过。

## 阶段 2：默认错误传播与执行边界

目标：建立 `fail`、`result(call)`、推导错误集合和运行时统一兜底。

- [x] 在 AST/HIR 增加 `Fail`、`CaptureResult` 和静态 `FailureSet`，通过现有 dependency worklist 传递。
- [x] 将普通调用降低为自动传播，`result(call)` 降低为编译器内部 captured result；不增加用户泛型或每函数 error 声明。
- [x] 拆分官方库中“正常状态 choice”与 error effect，迁移现有 Result choice 的调用方。
- [x] 扩展 C012，覆盖默认传播、captured result 穷尽消费、覆盖、丢弃、仅日志和 `recover`。
- [x] 生成可达应用的具体 Rust error enum、cause chain 和 source span；API snapshot 显示每函数精确错误集合。
- [x] 建立 HTTP、Task、CLI 根边界：事务回滚后分别返回 500/Task failure/非零退出，且只记录一次。
- [x] 增加 `dever.log` level API、共享诊断上下文和有界队列；文件 sink 格式/轮转仍不在本阶段冻结。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test contract_syntax
cargo test -p dever-tests --test contract_semantics
cargo test -p dever-tests --test contract_execution
cargo test -p dever-tests --test hello_native
cargo test -p dever-tests --test http_engine
```

完成条件：默认传播、主动捕获、自定义 fail、C012、HTTP/Task/CLI 边界及一次日志都有正反 fixture。

## 阶段 3：Model 语法与静态 Schema

目标：编译器能从一个 Model 文件得到完整静态持久化合同。

- [x] 增加 Model 路径识别和 Model 专属 declaration/field metadata AST。
- [x] 实现文件名/package/唯一公开 record 校验，合成 `id`、`created_at`、创建输入和更新输入。
- [x] 实现 `Bool/Int/Float/Decimal/Text/Bytes/Uuid/DateTime/Date/Time/Duration/Json/choice/model.id` 及 nullable/static bounds。
- [x] 实现 `generated/default/index/unique/from`、复合 index/unique、relation、seed、migrate 的解析、格式化和源码诊断。
- [x] 实现不透明 Model ID 的解析与类型检查、隐式 FK/index、choice labels 与 `Choice.options`。
- [x] 建立与方言无关的 `ModelSchema` 规范化和稳定 checksum；检查同连接内表/对象名冲突。
- [x] API/Markdown 输出 Model、隐式字段、choice options 和持久化元数据，但不暴露内部输入类型噪音。

建议新增测试目标：`model_syntax`、`model_semantics`、`model_schema`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test model_syntax
cargo test -p dever-tests --test model_semantics
cargo test -p dever-tests --test model_schema
```

完成条件：字段/索引/关联/Seed/migrate 的合法和非法边界在 native build 前有稳定诊断，schema checksum 不受格式或方言影响。

## 阶段 4：配置、目录与 RuntimeProfile

目标：严格加载外部 setting，并按实际能力构建最小 runtime。

- [x] 增加严格 `setting.json` 结构化解析和可执行文件目录定位。
- [x] CLI 应用命令以项目根为输入，统一把 `<project>/module` 交给 SourceMap；fixture 的显式源码根入口保持内部测试用途，不做应用级自动回退。
- [x] 创建固定 `data/db|upload|log|cache|tmp` 目录；setting 缺失/无效直接失败。
- [x] 实现显式连接、根包连接、default 的一次性解析和生成 Model 句柄缓存。
- [x] 将 SQLite/PostgreSQL/config 依赖拆为 runtime optional features。
- [x] native build 从 setting 提取驱动 capability，构建对应 runtime rlib；capability 进入 cache identity，敏感配置不进入生成源码或 identity。
- [x] 启动时复核实际 setting 的驱动集合和 transaction binding；未打包驱动明确失败。
- [x] 增加 base/sqlite/postgres/both 四种产物的依赖和二进制体积报告。

建议新增测试目标：`application_config`、`native_profiles`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test application_config
cargo test -p dever-tests --test native_cache
cargo test -p dever-tests --test native_profiles
```

完成条件：cwd 改变不影响定位，缺失配置无回退；SQLite-only/PostgreSQL-only 产物不含另一驱动。

## 阶段 5：共享 QueryPlan 与 SQLite 纵向闭环

目标：先用一个真实数据库打通静态 CRUD 和有界查询。

- [x] 实现 Model 短函数符号与匿名 create/update/query 块的上下文类型检查。
- [x] 建立共享 QueryPlan、条件树、参数槽、稳定排序、page/cursor/stream 和结果 shape。
- [x] 接入 `deadpool-sqlite` + `rusqlite bundled` 和有界 pool。
- [x] 实现当前 CRUD 所需 SQLite dialect：标识符、占位符、类型/值编码、RETURNING、CHECK/FK/index。
- [x] 生成 Model 专属 bind/row decoder；所有值参数化，不使用反射或动态 Map。
- [x] 实现 create/get/first/list/cursor/count/exists/stream/update/delete/upsert 与有界 create_many。
- [x] 验证 stream/取消/超时/pool exhaustion 后连接归还。

建议新增测试目标：`orm_query`、`sqlite_orm`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test orm_query
cargo test -p dever-tests --test sqlite_orm
```

完成条件：临时 SQLite 数据库完成 CRUD、分页、游标、stream、约束错误和清理；目标机器路径不需要系统 SQLite。

## 阶段 6：Schema Diff、迁移与 Seed

目标：SQLite 先完成空库、升级、危险变化拒绝和幂等初始化。

- [x] 建立 catalog 列 introspection 与 history 中的规范化 actual schema。
- [x] 实现内部 history 表、schema/seed checksum 和并发启动锁。
- [x] 实现空库最终 schema 创建，不回放 `from`/历史 migrate。
- [x] 实现安全字段 diff、约束数据库验证和 SQLite 表重建。
- [x] 实现字段 `from`、稳定命名 migrate、内容不可改和事务性 history。
- [x] 实现 Seed 唯一身份检查、缺失插入、非唯一冲突失败和 checksum 快速跳过。
- [x] 对每一种自动/拒绝/显式变化保留真实 catalog 和数据断言，不只断言 SQL 字符串。

建议新增测试目标：`orm_migration`、`orm_seed`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test orm_migration
cargo test -p dever-tests --test orm_seed
```

完成条件：新库、旧库 rename、长度放宽/收紧、唯一/FK、重建失败回滚、重复启动和 Seed 追加全部通过。

## 阶段 7：PostgreSQL 复用接入

目标：只增加方言和 executor，不复制 Model/QueryPlan/schema 语义。

- [x] 接入 `deadpool-postgres` + `tokio-postgres`；使用 `tokio-postgres-rustls` 支持系统根 TLS 校验和显式禁用。
- [x] 实现当前 CRUD 所需 PostgreSQL dialect、值编码/解码、RETURNING、catalog introspection 和 advisory migration lock。
- [x] 对固定 QueryPlan 使用 statement cache；缓存 key 只来自编译器生成的静态 SQL shape。
- [x] 复用 SQLite 同一 CRUD/schema/Seed fixture 数据集和断言，仅替换隔离数据库适配。
- [x] 验证 pool wait、连接断开、取消、timeout、constraint code 映射、TLS 主机名/证书拒绝和 shutdown drain。

建议新增测试目标：`postgres_orm`，默认标记为 ignored；仅在仓库根 `config/setting.json` 的 `database.postgres_test` 明确配置隔离测试 PostgreSQL 时手动运行，不接触现有服务。

定向验证：

```bash
cargo fmt --all -- --check
cargo test --offline -p dever-tests --features postgres --test postgres_orm -- --ignored --exact postgres_orm_uses_the_configured_isolated_database
```

完成条件：PostgreSQL 与 SQLite 对共享 fixture 返回相同逻辑结果和 schema revision，差异仅存在于 dialect/executor。

## 阶段 8：Transaction 与多数据库

目标：实现无显式句柄、单连接静态约束的 transaction 函数。

- [x] 实现 `FunctionKind::Transaction` 检查和 `DatabaseEffect(ModelId)` 传播。
- [x] build/runtime 分别解析 transaction Model 集合到唯一 connection。
- [x] native 生成隐藏数据库上下文参数和最外层 begin/commit/rollback guard。
- [x] 同连接嵌套 transaction 复用 handle；不创建 savepoint。
- [x] 拒绝无数据库 effect、跨连接、动态切库、transaction context 逃逸和并发数据库子任务。
- [x] 验证 error/fault 时先 rollback，再传播原始失败；rollback failure 只追加 cause。
- [x] 验证 default、根包、显式 report 三种连接绑定和多连接启动顺序/幂等恢复。

建议新增测试目标：`orm_transaction`、`orm_connections`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test orm_transaction
cargo test -p dever-tests --test orm_connections
cargo test --offline -p dever-tests --features postgres --test postgres_orm -- --ignored --exact postgres_orm_uses_the_configured_isolated_database
```

完成条件：两种数据库都验证提交、回滚、嵌套复用、取消和连接归还，跨数据库在执行前被拒绝。

## 阶段 9：Relation 与原生 SQL 逃生口

目标：完成后端常用关联读取和复杂查询能力。

- [x] 实现 to-one relation 推导、显式反向 to-many relation 和 `with` 计划。
- [x] to-one 使用受控 JOIN；to-many 按父页批量加载、稳定排序、数量和参数批次上限。
- [x] 加入生成代码 query-shape 探针，断言父记录数增长不会产生 N+1。
- [x] 实现参数化 typed native SQL，显式 SQLite/PostgreSQL 方言和具体 Model/record decoder。
- [x] 拒绝 SQL 插值、动态列 Map、结果列缺失和类型不匹配。
- [x] 建立 `examples/dever/cms/module/{user,news}/{model,service}`，用用户注册、新闻发布和 author 外键覆盖 Model/Service/transaction 纵向流程。

建议新增测试目标：`orm_relations`、`orm_native_sql`。

定向验证：

```bash
cargo fmt --all -- --check
cargo test -p dever-tests --test orm_relations
cargo test -p dever-tests --test orm_native_sql
```

完成条件：两种数据库返回相同关系结果，to-many SQL 数量有固定上界，原生 SQL 保持参数化和静态解码。

## 阶段 10：小内存与压力验证

目标：在正确性完成后冻结小机器默认值并找出真实瓶颈。

- [x] 扩展 `test/performance` 构建器，产出 sync/base、SQLite-only、PostgreSQL-only 和 HTTP+JSON+DB fixture。
- [x] 构建和测量分离；运行阶段只启动已打包二进制。
- [x] 添加 SQLite/PG idle、CRUD、list/cursor/stream、pool saturation、transaction cancel 场景；PG 仅在根 `config/setting.json` 显式配置 `database.postgres_test` 时运行。
- [x] 报告 binary size、RSS/PSS/VmHWM、cgroup current/peak/OOM、线程/FD、配置 pool capacity、吞吐和 p50/p95/p99；当前无法直接观测的连接数、pool wait、SQL 数明确标记 unavailable。
- [x] 先跑 1 核/128 MiB，再跑 64 MiB；不设置假 QPS 门槛，以无 OOM、无错误、内存有界、连接回收和相对对照为硬条件。
- [x] 根据报告收敛 SQLite pool、PG min/max、page size、row buffer、IN batch 和 statement cache 上限；两档最大 RSS 高水位均约 7 MiB、OOM 为 0，现有有界默认值有充分余量，因此不做无数据依据的调参。
- [x] 最后做 cleanup：删除临时 fixture、重复驱动分支、旧 async/await 代码和未使用 feature。
- [x] 以 `examples/dever/cms` 验证项目级 check/run/build 和 SQLite-only 打包产物。
- [x] 以 `examples/dever/cms` 验证真实业务内存基线。

最小基准检查：

```bash
python3 -m unittest discover -s test/performance -p 'test_runner.py' -v
python3 test/performance/run.py run \
  --artifacts target/performance/orm-current \
  --output target/performance/orm-128m \
  --suite orm --duration 10 --warmup 2 --repeats 3 \
  --cgroup-parent /sys/fs/cgroup/system.slice \
  --memory-mib 128 --cpu-quota 1
```

该运行会启动隔离的本地测试进程并使用 cgroup；执行前必须说明，确认不会影响现有服务。PostgreSQL 场景只使用专门测试实例。

## 最终自检

- [x] `rg` 确认 Dever 官方库、examples、fixture 不再出现源码 `async`/`await`。
- [x] `rg` 确认没有旧 ORM、链式 Query、显式 Database/Transaction 或双 CRUD 表面。
- [x] 检查生成 SQL 的所有值都通过 bind 参数，只有编译器生成的标识符进入 SQL。
- [x] 检查 SQLite/PostgreSQL 共享 QueryPlan/schema/error 流程，没有复制业务算法。
- [x] 检查所有队列、池、page、batch、stream、日志和 blocking 路径有明确上限。
- [x] 检查最终改动没有调试输出、TODO、临时测试、无用依赖或未使用兼容分支。
- [x] 只报告实际运行的定向验证；未运行 PostgreSQL、压测或受权限限制的 cgroup 检查必须明确标注。

## 高风险文件与回滚点

- 编译器语法/HIR：`crates/dever-core/src/{lexer,parser,syntax,hir,format}.rs`。
- 效果与错误：`crates/dever-core/src/contracts/*`、`check/*`、`native/*`、`api.rs`。
- native profile/cache：`crates/dever-core/src/native/build.rs`、`native/build/cache.rs`、workspace/runtime Cargo 配置。
- Runtime：新增 config/database/orm/dialect 模块与 `crates/dever-runtime/src/task.rs` 的 bounded blocking 接口。
- 官方库和示例：`library/dever/*`、`examples/dever/*`。
- 所有长期测试只能放在仓库根 `test/`。

阶段 1、2 属于语言合同迁移，完成后不回退旧语法；阶段 3-9 每阶段保持编译器与 runtime 同步可用。schema 变更只靠数据库事务/history 回滚，已成功提交的命名迁移不由程序版本回退。

## 2026-09-17 收口验证

- `cargo check --offline -p dever-runtime --features postgres` 通过。
- `application_config` 3/3 通过；PostgreSQL 配置默认值和超时范围已覆盖。
- 原 `postgres_orm` 无环境变量时提前返回却显示 1/1 通过，该证据无效。测试现已改为默认 ignored，并只从根 `config/setting.json` 读取 `database.postgres_test`；真实 PostgreSQL 验收仍未运行。
- PostgreSQL 配置入口回归：performance runner 27/27 定向单测通过，`cargo check --offline -p dever-tests --features postgres --test postgres_orm` 通过；未连接数据库。
- handler effect 替换回归通过；全部维护中示例按 `<project>/module` 统一源码根通过静态检查。
- `cms_project` 已完成 1/1 check/run/build/打包程序验证；最终复验因根文件系统空间不足在链接测试二进制前中止，不是测试逻辑失败。
- rustfmt、clippy 组件未安装；全量测试、真实 PostgreSQL、二进制体积、64/128 MiB 和 cgroup 压测未运行。

## 2026-09-17 最终验收

- 共享 fixture 已落在 `test/dever-tests/tests/support/orm_fixture.rs`；SQLite 与 PostgreSQL 使用同一 schema/Seed、CRUD、create_many、upsert、list/cursor/stream、transaction 和 catalog 断言。SQLite 1/1、隔离 PostgreSQL 1/1 最终复验通过。
- PostgreSQL 真实验收修复并覆盖三处仅靠编译无法发现的问题：cursor 可空参数显式 SQL 类型 cast、UUID 原生二进制参数直接使用 `::uuid`、shutdown SQLSTATE 映射为 connection error。pool wait、断连、timeout、constraint、TLS 拒绝和 drain 均通过。
- PostgreSQL 测试与性能工具只从仓库根 `config/setting.json` 读取 `database.postgres_test`；没有数据库 URL/TLS 环境变量或命令行覆盖入口。测试实例为独立临时 PostgreSQL，未接触现有 5432/5433 服务。
- 四 profile 使用同一最小源码：base 413,880 B、sqlite 2,562,808 B、postgres 2,711,632 B、both 4,676,472 B。依赖报告确认单驱动产物不包含另一驱动，证据见 `target/performance/typed-orm-build-v2/profile-report.json`。
- 1 CPU、无 swap、128 MiB 与 64 MiB 各完成 48/48 ORM case（SQLite/PostgreSQL × 8 场景 × 3 重复），两档 OOM/OOM kill 均为 0，最大 RSS 高水位分别 7,184,384 B 与 7,102,464 B；所有 cancel case 均确认池饱和、零提交和超时前连接回收。报告见 `target/performance/typed-orm-128m-v2/report.json` 与 `target/performance/typed-orm-64m/report.json`。
- 现有 SQLite pool=1、PostgreSQL min/max=0/4、page size=100 以及有界 row/IN/statement 路径均有充分内存余量；本轮数据不支持进一步缩减，保持默认值。
- CMS 128 MiB 与 64 MiB 各 3/3 完成，业务基线每次为 2 条记录，OOM 为 0，RSS 高水位中位数分别 5,513,216 B 与 5,562,368 B。报告见 `target/performance/cms-128m/report.json` 与 `target/performance/cms-64m/report.json`。
- 定向回归：performance runner 32/32、`application_config` 3/3、`orm_query` 8/8、`cms_project` 1/1（实际 check/run/build/打包程序）通过；`cargo check --offline -p dever-runtime --features postgres` 通过。
- 最终独立复核修正了两个收口问题：普通 SQLite `setting.json` 缺少 `database.postgres_test` 时现在按未启用 PostgreSQL 处理，只有显式但非法的测试连接才失败；真实 PostgreSQL fixture 使用 PID+纳秒+计数隔离 schema，并在断言失败/panic 后清理再恢复原始失败。修正后 runner 32/32、PostgreSQL cargo check 与真实验收 1/1 通过，结束后隔离 schema=0、活动连接=0。
- rustfmt、clippy 组件未安装，未运行全量 workspace 测试；以上限制不影响本任务已列出的定向验收。

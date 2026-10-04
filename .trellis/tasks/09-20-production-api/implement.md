# Dever 生产 API 边界实施计划

## 2026-09-28 自动权限与租户收口

- [x] 用户确认最终方案：API 自动权限、分层权限目录、标准角色关系、database-only 租户、组件禁用例外和 Job 当前权限复核。
- [x] 修改前完整备份：`/data/project/dever-backups/pre-auto-rbac-20260928-105925/dever.tar.gz`；gzip 校验通过，SHA-256 `700c2756dfcb4c5bceaff9dc1a16b86cfe94599fdeea798740ebc5fe3a461eb0`，约 1.5 MiB。
- [x] HIR/API checker 生成 custom/REST 权限元数据，public 不生成权限，重复 action 静态拒绝。
- [x] Runtime 控制库同步 `_dever_permission`，并提供按 component/domain/site/action 分组的只读目录。
- [x] 官方 auth runtime 建立角色、角色权限和用户角色私有表，提供事务化授权写入与单 key EXISTS 校验；verify Identity 删除 permissions。
- [x] 删除 `dever.auth.require`、`AuthRequire` intrinsic、源码权限字符串和 Job capability 集合；所有非 public API/REST 在 handler 前自动授权。
- [x] Job schema 升级并保存单一 permission key；执行时重新 verify 和查询当前角色，旧活跃 User Job 安全阻断。
- [x] 租户配置改为 `tenant.database`，删除 mode/control_database，不保留兼容分支。
- [x] 自动识别租户组件，生成入口可达组件集合；实现 `_dever_tenant_component` 和 API/CMD/Job 统一禁用检查及 CLI enable/disable。
- [x] CMS Dever/Markdown 改为标准角色分配和自动权限；删除 membership 硬编码权限列表及所有手工 require。
- [x] 增加编译器、配置、Runtime SQLite、Job、组件闭包和双源码 CMS 定向正反测试。
- [x] 运行允许的最小 cargo check/定向测试、diff 检查和独立终审；真实 PostgreSQL 作为需要外部隔离实例的单独验收项记录。
- [x] 抽取只读根 `config/setting.json` 的共享 PostgreSQL fixture，并让 ORM 与授权验收复用同一隔离数据库/随机 schema/失败清理合同；SQLite 与 PostgreSQL 授权目标复用同一数据库无关断言。
- [x] 在真实 PostgreSQL 上运行授权目录、v1→v2 角色迁移、跨站同名角色、多角色、Owner 与失效权限的 ignored 定向验收；2026-09-30 再次通过，使用显式自有实例和临时根配置，结束后恢复配置文件不存在，不探测默认服务。

## 2026-09-26 已授权执行清单

- [x] 用户批准完整修订方案，并要求修改前备份；认证优先复用现有账号领域。
- [x] 备份完整工作目录（包含 untracked 源码与 .git，排除 target）到 `/data/project/dever-backups/pre-auth-tenancy-20260926-rm6kL6/dever.tar.gz`；gzip 检查和 tar 对源逐项比较均通过。SHA-256 `e4f0666d054ca241484e8333c16270f039a26c3c616b7ddd0f0bea50476ca9b7`，约 1.4 MiB。
- [x] 读取现有调用链和规范；确认站点 path 必须匹配 API 父目录，认证 hook 需进入生产根，Model/Job/Adapter 当前启动固定绑定。
- [x] 配置、目录站点归属、public HTTP 声明、静态认证 hook、只读身份和标准错误。
- [x] global/tenant Model、database 模式按租户选择物理库、有界池、显式初始化迁移和单库事务。
- [x] 多用户成员角色示例、能力检查、REST owner 行范围与字段来源；普通 ORM 保持显式业务范围。
- [x] Job/CMD 可信租户、日志配置、runtime 缺省、app 配置消费者迁移；CMD 不接受 JSON 自报租户/用户。
- [x] 租户组件启停由核心私有表和受控 CLI 完成；table/field 隔离及租户级动态 Adapter 已被后续 database-only 决策取消，不保留兼容模式。
- [x] 完成普通/Secret Cookie、标准错误和受限 Upload 合同。
- [x] Dever/Markdown CMS 同步、定向正反验收和现有规范同步。
- [x] 独立最终审查收口；限定复核外部 Origin、密码事务、public POST 错误、契约夹具和 Job 身份撤销，未发现 P0/P1 阻断。

实施与检查使用 Trellis 子代理；最多两个独立代理并行，共享文件必须先分配唯一拥有者。主会话负责合同、整合及最终证据。现有用户改动不回退、不提交 Git。测试仅限当前改动相关定向目标。

## Start Gate

- [x] secure-data-contracts、time-typed-codec、port-adapter完成。
- [x] 用户批准父任务规划，并批准 2026-09-25 的文件/函数/入口/API 改造方案。
- [x] 核对路径冲突、主 Model 推导、CMD 输入合同及现有 API/native/runtime、SourceLayout、ORM、HTTP 边界。

## Implementation

- [x] 收紧 role 布局：Model/API 根文件可与主题目录共存，其余 role 保持互斥；主 Model 身份沿用现有规则。
- [x] 实现 `get/post/put/delete/cmd/rest` 声明、App 绑定、路径生成与 `{id}` 路由；应用严格模式拒绝旧前缀 API 函数体。
- [x] 实现生成的 run/build 入口与 CMD JSON 选择；移除维护中 CMS 的手写 main。
- [x] 实现主 Model 基础 REST：公开字段、GET 列表/详情、POST/PUT/DELETE、静态 SQL 和有界分页。
- [x] 实现 Model 字段 `create/replace/search` 来源、可信 `owner` 身份绑定与所有自动 REST 读写同语句行范围；普通 App ORM 不受隐式修改。
- [x] 实现 GET 写 Effect 拒绝和写入口自动事务；覆盖 API/CMD/Job 生成路径，持续补足提交、回滚与取消的定向运行验证。
- [x] 基于生产调用图拒绝可证明的无用私有函数、无契约纯转发、简单 Model CRUD App 包装及应用构建不可达 App；保留警告和库检查。
- [x] 扩展 API input shape 检查和 shared decoder generation，CMD 使用同一严格 JSON 对象解析。
- [x] 增加 runtime 请求级隐藏作用域，不暴露可保存或伪造的 Context 源参数；HTTP/CMD/Job 调用链静态区分。
- [x] 增加受控 Header 读取/响应写入、Secret Cookie 原始字节的版本化 base64url 读取/写入和 typed CookieOptions；禁止敏感 Header 经 Text 读取、Header 注入及 Secret Cookie 缺少 Secure/HttpOnly。
- [x] `client_address()` 由 HTTP/1、HTTP/2 的 TCP peer 提供，不信任转发 Header；HTTP/1 loopback 定向验证通过。
- [x] 拒绝 CMD/Job 或脱离请求的 Task/parallel/blocking 调用 HTTP Context；HTTP/2 并发流的 request id、path、peer 和响应 Header 隔离定向测试通过。
- [x] 完成普通 Text Cookie 的安全读策略；Secret Cookie 保持独立非 Text 通道。
- [x] 增加官方API Error及native status/envelope映射。
- [x] 重构 request read boundary，认证/路由先于正文，multipart 使用 bounded streaming/spool。
- [x] 增加 affine Upload runtime owner、App→Port→Adapter 单次传递、storage consume/close/cleanup 和 checker 规则。
- [x] JSON request、request id 与现有 HTTP/1、HTTP/2 dispatch 路径共享；后续仍需定向运行验证 HTTP/2/drain。
- [x] 更新 README、LANGUAGE、Markdown 语法与 CMS 两套源码，迁移至编译器生成入口；其余协议文档随 Context/Error/Upload 实施更新。
- [x] Job 私有存储最终升级到 v5：只持久化可复核身份引用和单一入口 permission key，拒绝无身份入队；CMD/cron/Test 显式 System，User 每次执行重新 verify，v4 活跃 User 行阻断并删除旧 capability 列。
- [x] 响应 Header/Cookie 改为 pending/committed 两阶段，序列化和数据库提交成功后才发布；登录类请求使用严格同源校验，Cookie 签发/清理统一读取配置。
- [x] PostgreSQL database 租户由显式 `tenant migrate` 通过控制连接幂等创建后迁移；普通请求/worker 保持无创建、无回退。

## 2026-09-25 定向结果

- `cargo check --offline -p dever-core -p dever-runtime -p dever-cli` 通过。
- API 声明测试 10/10、语法/格式 22/22、基础 REST 4/4、目录规则 12/12、旧 HTTP 路由 7/7 通过。
- CMS Dever/Markdown `check`、`fmt --check` 通过；CMS 同源与生成入口测试 1/1 通过。
- 无 main 的 API/CMD AOT 编译和实际 CMD JSON 调用通过，输出固定 envelope；Job AOT 编译通过。生成入口定向测试 11/11 通过。独立审查指出的必填 Json、事务并发、日志泄漏和 REST Text 边界已修复，并增加定向反例。请求上下文和 TCP peer 测试 5/5、HTTP/2 多路复用定向测试 1/1 通过；CMS 同源测试 1/1 通过。
- 仍需确定 `owner` 的可信业务 actor 来源，以及声明式 API 如何把 App 业务失败映射到官方 API Error；两项不能用客户端 Header/Cookie 自报身份或让 App 隐式依赖 HTTP 状态来代替。Model `create/replace/search` 绑定、普通 Text Cookie 策略及 multipart Upload 还未实现。
- 未运行全量测试、服务级 CMS/HTTP 集成测试、真实数据库或性能/内存压测；rustfmt/clippy 组件未安装。

## 2026-09-26 身份与配置定向结果

- typed `auth/sites/log` 配置、API 物理目录站点归属、单声明 `public`、静态 verify hook、只读 `site/auth` 上下文、HS256 凭据、Cookie Origin 约束和标准 API Error 映射已实现。
- `app` 顶层配置已移除，性能工具迁移到顶层 `performance`；runtime 缺省按 API/Job 能力推导。UUID 官方 parse/to_text 能力已补齐，供业务会话与租户标识使用。
- `/root/.cargo/bin/cargo check --offline -p dever-core -p dever-cli` 通过；`/root/.cargo/bin/cargo check --offline -p dever-runtime --features api` 通过；`api_routes` 7/7 通过（含 native/AOT）；此前 `auth_runtime` 2/2、`application_config` 5/5、API 声明 14/14、性能工具 Python 测试 40/40 通过。
- database 模式多租户物理存储、Job/CMD 可信租户、普通 Text Cookie、Upload、REST owner 与字段绑定已经完成；CMS 双源码正在最终复验。
- 此处当时记录的 table/field 隔离与租户级动态 Adapter 后续已由 database-only 决策取消；当前不保留这些模式或兼容分支。

## 2026-09-26 多租户、REST 字段与 Upload 定向结果

- database-only 租户已贯通 global/tenant Model、无回退物理库选择、ready/fingerprint、显式 `tenant migrate`、有界池回收、单库事务和 tenant Job 恢复；当前配置入口是 `tenant.database`。SQLite 无回退/幂等迁移 CMD 1/1、tenant Job 物理库隔离 1/1 通过。
- REST `owner/create/replace/search` 已进入 checked HIR、生产可达图与 native；owner+search 复合索引、双静态 SQL、pure Domain/可信上下文正例和数据库/时间/响应/并发反例通过。SQLite AOT 1/1 通过（74.46 秒）。
- multipart/Upload 边界 5/5 通过，包含流式分块、limits/重复字段/路径穿越、取消清理、affine 静态反例和 loopback POST→App→Port→Adapter→storage 实际 AOT；临时目录在成功后为空。
- 写入口事务允许等待完成且目标无数据库/并发的 `blocking`，仍拒绝 `run` 等逃逸并发；自动事务、拒绝反例和显式 transaction 定向 3/3 通过。
- API AOT 路由复验 1/1 通过；认证运行时此前 2/2、CMD tenant migration 1/1 通过。CMS Plain/Markdown 使用部署 settings 的同源/application reachability 复验 1/1 通过。
- 未运行全量测试、真实 PostgreSQL、HTTP/2 上传 reset、外部对象存储原子发布、服务压测或性能/内存压测；rustfmt/clippy 组件未安装。

## 2026-09-26 可信身份与事务收口结果

- CMS Plain/Markdown 的发布操作统一调用 `dever.auth.owns_user(stored.author_id)`；租户 verify 同时要求 `platform.tenant.active`，避免仅凭有效会话绕过资源所有者或停用租户。
- public API 不再从浏览器 Cookie 建立身份，显式无效 Authorization 仍拒绝；Origin 与 Host 按站点配置的外部 HTTP/HTTPS scheme、主机和有效端口精确匹配，不根据内部监听协议或转发 Header猜测。Cookie 名称、TTL、签发和清理均来自 `config/setting.json`，没有环境变量入口。
- `_dever_jobs` 当前 v5 区分队列物理租户与执行租户，保存 provider/site/subject/session/tenant 引用及单一入口 permission key；不保存 token、Cookie、角色或权限集合。User Job 每次重试均重新 verify 并查询当前角色，legacy v4 活跃 User 行转为 `blocked/authorization_contract_changed`，匿名/无身份路径不能退化为 System。
- SQLite 写事务继续 `BEGIN IMMEDIATE`。含 password verify/hash 的 HTTP 调用链不再生成外层事务，编译器要求全部写入进入显式短 `transaction` 并禁止事务内密码计算；CMS 登录在短事务内重新检查账户、租户和成员关系后创建 session。响应元数据只在响应序列化和数据库提交后转为 committed。
- PostgreSQL 租户迁移会通过配置的控制连接创建派生数据库，SQLSTATE `42P04` 作为并发/重复创建成功处理；尚未连接真实 PostgreSQL 验证权限和服务端行为。
- 定向结果：核心/runtime 的 API、SQLite、PostgreSQL feature `cargo check` 通过；durable Job 静态、生成入口、SQLite 状态机、v5 迁移/持久化、public 边界和 tenant 隔离各 1/1 通过；认证 runtime 3/3、配置 6/6、安全契约 12/12、生成 User Job AOT 1/1 通过。
- 新增撤销回归通过 1/1：测试拥有的临时 SQLite 应用先以有效会话入队 User Job，再撤销 session 并启动 worker；任务转为 `blocked/identity_rejected`，业务副作用表保持为空。该验证使用临时 loopback 与子进程，无外部服务。
- 密码窗口并发回归包含在安全契约中：账号读取与 session 短事务之间模拟密码计算，同时另一连接可完成无关写入；短事务重新读取状态后正常创建 session，不持有跨密码计算的 SQLite 写锁或旧读快照。
- CMS Plain/Markdown `check` 与 `fmt --check` 均通过；两套源码/Model/API/config 同源检查 1/1 通过。两套 `check` 各保留 3 个预期 W001，不影响编译。
- 独立终审只读复核上述五项实现与执行路径，未发现 P0/P1 阻断；真实 PostgreSQL、性能、table/field 和动态 Adapter 不属于本轮收口范围。
- 未运行全量测试、持久服务、真实 PostgreSQL、HTTP 集成、性能/内存压测；rustfmt/clippy 组件未安装。table/field 租户隔离和租户级动态 Adapter 仍按已确认范围延期。

## 2026-09-28 自动权限与租户收口结果

- 权限由 checked HIR 自动生成，custom 使用 action，REST 使用 `read/create/replace/delete`；method 仅作元数据。跨 method/path 重复 key、public 触达租户组件、public/GET 调用角色管理能力均在 native 前拒绝。
- `_dever_permission` 在服务启动前同步；授权库使用版本化 `_dever_auth_role`、`_dever_auth_role_permission`、`_dever_auth_user_role`，三表以 `(site, role_id)` 隔离同名角色，旧单列 ID schema 事务迁移并保留授权数据。精确权限使用索引化 EXISTS，多角色取并集，`all_permissions` 只覆盖同站点。保留 `_dever_owner:<site>` 只能由 `tenant owner` provision，普通 save/grant/revoke/disable 全部拒绝。
- Identity 已固定为 `id/user_id/tenant_id`；源码 `auth.require`、AuthRequire intrinsic、权限列表和 Job capability 集合已删除。受保护 API 顺序固定为 verify、tenant scope、组件检查、精确权限、输入解码和 App。
- Job 已升级到 v5，只保存一个入口 permission key；每次执行按 verify、编译目录、租户组件、当前角色的顺序复核。v4 活跃 User Job 迁移为 `blocked/authorization_contract_changed` 并删除旧 capability 列，System 任务保持可执行。
- 租户只支持 `tenant.database/max_pools/idle_timeout_ms`。租户组件从 tenant Model/Job 自动推导，混合组件拥有的全部 Job 统一使用租户队列，并通过 `deverc tenant component enable|disable` 控制；public 入口不能静态触达租户组件。
- CMS Plain/Markdown 已同步为核心角色管理薄适配，admin/front 各有同站点管理入口，启动说明要求迁移后分别 provision Owner；membership 只表达业务成员关系。两套 `test` 此前各 1/1 通过；本次最终源码改动后，两套 `check`、`fmt --check` 及当前编译器双源码同源/完整应用可达检查通过，check 仅保留 3 个既有 W001。受磁盘空间限制未重复链接两套完整 CMS 测试程序。
- 定向验证通过：core/runtime/CLI cargo check；`application_config` 6/6、`api_declarations` 16/16、`contract_api` 9/9、`auth_runtime` 3/3、`authorization_runtime` 1/1（含旧角色 schema 迁移与跨站同名角色）；REST 权限、Identity、Job v4->v5 删列、撤销后 Job 阻断、混合组件 Job 租户 AOT、租户组件 disable/enable、CMS 双源码同源及受保护 auth/Job 顺序 AOT 各 1/1。
- 两轮独立终审指出的站点角色主键、混合组件 Job、CMS 首次 Owner、v5 旧列、Job 复验顺序和旧授权 schema 升级问题均已修复并复验；最终未发现新的 P0/P1。
- 未运行全量 workspace 测试、真实 PostgreSQL、持久服务或性能/内存压测；rustfmt/clippy 组件未安装。本轮实现没有环境变量配置入口。

## 2026-09-28 PostgreSQL 授权验收准备结果

- 新增共享 PostgreSQL 测试 fixture，只接受仓库根 `config/setting.json` 的 `database.postgres_test`；URL 的数据库名必须包含且只包含一个 `{case}`。fixture 为每个目标派生专用数据库名并创建随机 schema，测试成功或 panic 后都从独立 runtime 尽力清理。
- `postgres_orm` 已迁移到共享 fixture；新增 ignored `authorization_postgres` 目标。SQLite 与 PostgreSQL 共用唯一授权合同，覆盖权限目录、旧 v1 schema 数据保留迁移、跨站同 role id、多角色并集、角色禁用/撤销、保留 Owner 限制和失效权限拒绝。
- `/root/.cargo/bin/cargo check --offline -p dever-tests --features api,postgres --test postgres_orm --test authorization_postgres` 通过；SQLite 授权 exact 1/1 通过。无 feature 的 `syntax_and_format` 与仅 SQLite 无 API 的 `sqlite_orm` 编译检查由实施子任务验证通过。
- 独立检查发现 PostgreSQL URL 的 `dbname` 查询参数可覆盖路径数据库，已拒绝明文和百分号编码的覆盖形式；纯 URL 隔离回归 1/1 通过。复验期间磁盘一度归零，两个重复定向链接已中止并清理本轮生成的可重建测试二进制/增量对象，未删除源码、配置或用户数据。
- 真实 PostgreSQL 未运行：仓库根当前没有 `config/setting.json`。测试不会读取环境变量或命令行连接覆盖，也不会自动连接、创建或清理未明确配置的数据库。
- `cargo fmt --all --check` 未运行成功，因为 rustfmt 组件未安装；新增 Rust 已手工按现有样式整理。

## 2026-09-29 真实 PostgreSQL 定向验收

- 使用仓库根 `config/setting.json` 的 `database.postgres_test` 配置（测试凭据仅存在于被 `.gitignore` 忽略的临时文件，验收后已删除），在本机隔离 PostgreSQL 数据库上运行 `authorization_postgres` ignored 定向目标，1/1 通过。
- 本次实际覆盖权限目录同步、v1 角色 schema 迁移、跨站点同名角色、多角色权限并集、角色禁用、用户角色撤销、Owner 保留限制和失效权限拒绝；测试使用随机 schema，临时数据库在验收后清理。
- `postgres_orm` 未通过，失败发生在数据库连接前的共享 fixture 编译阶段：旧 fixture 仍从 `main.dever` 调用 `user.create`，当前 Model 规则要求 Model 操作只在 owning-domain App 中调用。SQLite 共享 fixture 同样复现该编译失败，因此这是独立的 ORM fixture/语言契约迁移项，不是 PostgreSQL 实例或配置问题。
- 没有读取环境变量或命令行连接覆盖；仓库工作区未保留真实 PostgreSQL 凭据或 `config/setting.json`。

## 2026-09-30 当前 PostgreSQL 组合补验

- 共享 ORM fixture 已按 owning-domain App/Model 合同迁移，真实 PostgreSQL ORM/授权 Store ignored 各 1/1 通过，旧 09-29 ORM 失败已关闭。
- `postgres_api` exact ignored 1/1 通过 stock Dever CMS 的真实 HTTP、两个物理租户数据库、同 slug 独立发布、跨站 Cookie/跨租户登录拒绝、普通非 Owner 的多角色/跨站同名角色及同一会话撤销立即失效。测试注册仅在副本追加 `test/dever-tests/fixtures/postgres_api/` 片段，不修改生产 CMS 或 Owner 保留角色规则。
- 新报告 `target/performance/postgres-http-09-30-v1/report-1968019-1790739069361983512.json`；`test-rbac-final.log` 和 `server-rbac.log` 保留真实结果与服务日志。失败轮的站点目录读取和 DELETE body 工具错误已按现行合同修正，未放宽 checker/runtime。
- 临时控制库/租户库全部清理，自有 PG 已停，根 `config/setting.json` 恢复原先不存在；PG 下载/数据目录移入回收站可恢复。Python 工具 57/57、Rust postgres_api 非 ignored 3/3；不宣称 Markdown PostgreSQL/HTTP2 所有组合或全 workspace 复验已运行。

## Focused Verification

```bash
cargo check --offline -p dever-core -p dever-cli
cargo check --offline -p dever-core -p dever-runtime --features dever-runtime/api,dever-runtime/sqlite,dever-runtime/postgres
cargo test --offline -p dever-tests --features api,crypto,sqlite --test secure_contracts --test auth_runtime --test application_config
cargo test --offline -p dever-tests --features api,crypto,sqlite --test durable_jobs revoked_user_job_is_blocked_before_business_dispatch -- --exact
cargo test --offline -p dever-tests --features api,crypto,sqlite --test contract_execution cms_formats_share_source_model_api_and_configuration_contracts -- --exact
cargo test --offline -p dever-tests --features api,postgres --test authorization_postgres postgres_authorization_preserves_site_scoped_roles_and_catalog -- --ignored --exact
cargo run --offline --quiet -p dever-cli -- check examples/cms/dever
cargo run --offline --quiet -p dever-cli -- check examples/cms/md
cargo run --offline --quiet -p dever-cli -- fmt examples/cms/dever --check
cargo run --offline --quiet -p dever-cli -- fmt examples/cms/md --check
cargo test --offline -p dever-tests --features api --test api_routes
cargo test --offline -p dever-tests --test http_engine
cargo test --offline -p dever-tests --test http2_server
cargo test --offline -p dever-tests --test logging
cargo test --offline -p dever-tests --test pooled_network
cargo test --offline -p dever-tests --test source_architecture
cargo test --offline -p dever-tests --test markdown_source
git diff --check
```

上传测试只使用test-owned临时目录和bounded loopback peer，提前说明后运行；不启动持久服务、外部网络、全量测试或性能基准。

## Review Risks

- Context/Upload不能通过record、Task、Channel、App参数或输出逃逸request scope。
- multipart错误/取消所有路径必须删除已创建part，且不能信任filename作为路径。
- Secret cookie不能生成可被Render/log捕获的中间Text。
- API Error必须与未知App failure区分，不能把内部错误message返回客户端。
- HTTP/2并发stream的Context/temporary ownership不能串请求。
- 目录或文件重排不得隐式改变 Model identity、表名、App 调用名和 API 路由；旧 API/Main 方案不能与新方案共存。

# 类型安全 ORM 技术设计

## 设计目标

实现一个静态、无反射、默认有界的 ORM，并让数据库能力自然接入 Dever 的顺序调用、结构化并发、错误传播和 AOT 产物裁剪。Model 只描述持久化事实和数据访问；数据库驱动不拥有语言层查询语义。

## 总体边界

```text
Dever source
  -> parser / formatter
  -> model + function semantic checking
  -> effect / error / database binding inference
  -> typed QueryPlan + normalized ModelSchema
  -> native emitter
       -> shared ORM runtime
       -> SQLite dialect/executor
       -> PostgreSQL dialect/executor
  -> packaged binary + external config/setting.json
```

- 编译器拥有 Model 识别、字段类型、查询表达式、静态关系、错误集合和 transaction 合法性。
- ORM runtime 拥有配置、连接生命周期、绑定值、行流、事务、schema 执行和统一错误分类。
- dialect 只拥有 SQL/DDL 差异；driver executor 只拥有具体连接、参数编码和行解码入口。
- 生成代码拥有每个 Model 的具体创建输入、更新输入、查询返回、字段绑定和行 decoder。
- 不建立通用 Repository/BaseModel 层，也不让 SQLite/PostgreSQL 分别复制 CRUD 流程。

## 源码合同

### Model 文件

```dever
package app.model.user exposes (
  User
)

type UserStatus {
  Active = "启用"
  Disabled = "禁用"
}

type User {
  uuid: Uuid generated unique
  email: Text(254) unique
  display_name: Text(1, 64) from nickname
  status: UserStatus default UserStatus.Active index
  organization_id: app.model.organization.id?
}

index(status, created_at)

seed {
  {
    email = "admin@example.com"
    display_name = "Admin"
    status = UserStatus.Active
  }
}

migrate remove_legacy_code {
  drop legacy_code
}
```

`database <name>`、`index`、`unique`、`relation`、`seed` 和 `migrate` 只在 Model 文件中合法。需要显式使用报告库时，在 package header 后增加 `database report`；未写时按根包/default 规则绑定。普通文件继续只有普通 type/function 语义。

### 查询与事务

```dever
users = user.list({
  where = status == UserStatus.Active and organization_id == organization_id
  order = [created_at.desc, id.desc]
  page = 1
  size = 20
  with = [organization]
})

transaction disable_organization(
  target_organization_id: app.model.organization.id
) () {
  user.update({
    where = organization_id == target_organization_id
  }, {
    status = UserStatus.Disabled
  })
  organization.update(target_organization_id, {
    active = false
  })
}
```

匿名创建/更新/查询块只在对应参数位置出现，由目标 Model 提供字段命名空间。它们不会成为可传递的动态 Map 或持久查询对象。

### 类型化原生 SQL

复杂查询通过 Model 文件内的专用 `sql` 声明提供，不引入通用泛型或 type-as-value：

```dever
type UserSummary {
  status: UserStatus
  total: Int
}

sql active(status: UserStatus) (users: List<User>) {
  sqlite = "SELECT id, created_at, email, status FROM user WHERE status = ?1"
  postgres = "SELECT id, created_at, email, status FROM user WHERE status = $1"
}

sql summary(status: UserStatus) (summary: UserSummary?) {
  sqlite = "SELECT status, COUNT(*) FROM user WHERE status = ?1 GROUP BY status"
  postgres = "SELECT status, COUNT(*) FROM user WHERE status = $1 GROUP BY status"
}
```

- 两种方言都必须是静态 Text 字面量。SQLite 显式使用 `?N`，PostgreSQL 显式使用 `$N`；编译器不重写 SQL。
- 参数按声明顺序和具体字段类型绑定；禁止插值、动态列名及动态行 `Map`。
- 结果只能是一个具体 `Model`、`Model?`、`List<Model>`，或同文件私有辅助 record 的对应形状。
- Model 文件仍只有文件名对应且 exposed 的 record 注册为 Model。辅助 record 必须被 SQL 结果使用，不生成表，也不能 exposed。
- Model 结果复用生成的 Model decoder；辅助 record 生成具体字段顺序 decoder。缺列、多列、类型不匹配，以及单行结果的零行/多行都作为运行时数据库数据错误传播。
- 调用形式与内置 CRUD 一致，例如 `user.active(status)`。执行复用既有 statement cache、值绑定和隐藏 transaction executor；`List<Record>` 只物化该静态 SQL 实际返回的行，不建立额外动态缓存。

## 编译器设计

### 1. 语法与格式化

- 根据源文件相对路径判定是否属于 `model/`；Model 文件仍解析一个普通 record type，不新增通用 type 种类。
- 在 AST 中增加目的明确的 Model 元数据节点：数据库绑定、字段修饰、复合索引、关系、Seed 和命名迁移。不要把它们编码为字符串或普通函数调用后再反解析。
- 函数声明使用 `FunctionKind::Ordinary | Transaction`，取代继续叠加多个布尔修饰位。
- 删除源码 `async` 和 `await` 节点；`run`、`wait`、`stop` 保留为明确的并发上下文表达式。
- `result(call)` 和 `fail(error_variant)` 作为两个目的单一的核心表达式进入 AST/HIR，不借此加入 try/catch、annotation 或 macro。
- formatter、Markdown 合同解析和 API snapshot 与新语法同步更新；不保留旧语法分支。

### 2. Model 注册与静态元数据

- SourceMap 加载后先建立 `ModelId`，验证文件名、package 路径、唯一公开 record 和 Model 表名。
- 为每个 Model 合成：
  - 不透明 `<model-package>.id`；
  - 完整 record 中的 `id`/`created_at`；
  - 创建和更新输入 shape；
  - CRUD/query/relationship 符号；
  - choice options 常量；
  - 具体 row decoder 与 bind encoder 描述。
- 同一逻辑连接中发生表名、索引名或内部对象名冲突时，在生成 SQL 前报告源码诊断。
- Model CRUD 是编译器合成的包能力，不要求开发者在 `exposes` 中逐个列出，也不扩展成通用 type-as-value。

### 3. 规范化 Schema

新增与方言无关的 `ModelSchema`：

- 稳定 Model/package/table identity；
- 有序字段、逻辑类型、可空性、默认值、generated、旧字段来源；
- 主键、外键、choice CHECK、单列/复合索引；
- relation、Seed 和命名迁移规范化表示。

revision 只对逻辑 schema 和迁移意图计算，不包含 SQLite/PostgreSQL SQL 文本、连接串、文件绝对路径或源码格式。这样两个方言完成后记录相同逻辑身份。

### 4. QueryPlan

所有短函数共用一个静态 IR：

- operation：select/insert/update/delete/upsert/count/exists；
- projection 与具体结果 shape；
- condition tree、稳定 order、page/cursor、limit；
- relation load plan；
- 参数槽位及 Dever 类型；
- transaction requirement 与 Model binding。

查询块在 checker 中解析为 Model 字段 ID 和类型化运算，不把字段名字符串留到 runtime。native emitter 将固定 SQL shape 和参数提取代码写入生成程序。

类型化原生 SQL 作为同一 Model operation HIR 的专用分支：HIR 只保存已校验的 operation 索引和类型化参数，方言文本保留在 `ModelSchema`。native emitter 选择 runtime profile 对应文本，并复用普通 ORM 的 bind encoder、database executor 与 row decoder；不会把 SQL 或结果 shape 推迟到 runtime 解释。`Record`/`Record?` 最多拉取两行以判定基数错误；`List<Record>` 最多保留 `database.max_page_size` 行并额外探测一行，超限失败而不静默截断，因此漏写 SQL `LIMIT` 也不会形成无界进程内结果集。

#### Relation 结果与加载计划

- 编译器公开内置 `Related<T>`，只有 `Unloaded` 和 `Loaded(value)` 两态。ORM 负责构造值；业务代码可以用 `Related.Unloaded<T>()` 和 `Related.Loaded<T>(value)` 分句穷尽匹配。
- `Related` 使用间接 native 布局 `Loaded(Box<T>)`。Model 类型环只有穿过 `Related` 才合法，基础 row decoder 始终把合成关系字段初始化为 `Unloaded`。
- 持久化字段 `<name>_id: target.id` 自动合成 `<name>: Related<Target>`；可空外键合成 `Related<Target?>`。显式 `relation children = target.parent_id` 合成 `children: Related<List<Target>>`。持久化 schema、bind encoder 和基础 decoder 不包含这些合成字段。
- `with = [organization, children]` 在 checker 中解析为 `QueryPlan` 的静态 relation 索引，只允许一层裸 relation 名称；未知、重复和嵌套关系均拒绝。`with` 只允许 `first`、`list`、`cursor`，`stream` 明确拒绝。
- to-one 使用受控 `LEFT JOIN` 或 `INNER JOIN`，支持同一父查询的多个关系；`list` 的 count 查询保持只访问父表，`cursor` 不生成 count。
- to-many 在父结果确定后按父 ID 批量加载并归组，不逐父查询。子记录按 `id ASC` 稳定排序，每个父记录最多加载 `database.max_page_size` 条；父 ID 每 998 个一批，为 SQLite 的第 999 个参数保留 relation limit。空父列表不会生成 `IN ()`。
- 关系查询数量由生成代码 shape 测试验证：每个 relation 只有一个按固定参数批次循环的 `query_owned` 调用点，不在生产热路径加入计数原子。

### 5. 挂起 Effect

- intrinsic 和官方库函数标记基础挂起 effect；调用、静态 handler 和 collection handler 通过现有依赖图传递。
- AST 不再存 `is_async`，HIR 在 effect 收敛后记录内部 `suspends`。native emitter 仅对可达挂起函数生成 Rust `async fn` 和内部 `.await`。
- 普通调用根据目标 effect 降低为同步 call 或内部 await call；源码语义始终是顺序完成。
- 同步入口的完整可达图没有挂起 effect 时沿用直接 `main`；否则生成唯一 Tokio runtime 入口。

### 6. Error Effect

- `error` choice 变体仍是命名、带类型 payload 的领域失败；`fail(...)` 把对应变体加入当前函数错误集合。
- 调用图为每个函数推导封闭错误集合。普通调用在 HIR 中自动传播失败，native 使用具体 Rust `Result<Success, AppError>`；源码不写 `?`。
- 对存在多个正常状态的操作，把正常状态保留为非 error choice，例如 `ReadState.Read/End`，错误集合与成功类型分开。
- `result(call)` 阻止当前调用自动传播，并生成编译器内部 `CapturedResult<SuccessShape, ErrorSet>`。它不引入用户泛型；checker 只允许通过现有穷尽函数分句消费、继续返回或传递，且必须覆盖成功形状和全部 error 变体。
- native 为当前可达程序生成一个具体的错误 enum，并为函数记录精确子集；API/docs/editor 显示每个函数的静态错误集合。没有动态错误对象、字符串类型分派或按变体名猜 HTTP 状态。
- 现有 C012 改为检查 error effect 和 captured result 的消费，继续拒绝丢弃、覆盖、仅打印或仅记录。

### 7. Database Effect 与 Transaction

- 每个 Model operation 在 HIR 标记 `DatabaseEffect(ModelId)`；普通 helper 通过调用图传递 Model 集合。
- 每个 Model 保存声明式 `ConnectionSelector::Explicit(name) | PackageRoot(root)`。source check 可拒绝显然不同的显式绑定；native build 读取 setting 后做最终连接解析和 transaction 单连接验证。
- Transaction 函数必须具有至少一个数据库 effect，且所有 effect 在构建配置中解析为同一连接；并发子调用中的数据库 effect 直接诊断。
- HIR 为 transaction 调用链增加隐藏 `DatabaseContext` 参数。普通路径使用 registry slot，事务路径使用同一 connection 的 transaction handle；不查询线程局部或任务局部状态。

## 配置与构建能力

### 项目根与源码根

- 应用命令接收项目根；项目根内的 `module/` 是唯一 SourceMap 根，package 按文件相对 `module/` 的路径校验和派生。
- `module/user/model/user.dever` 因而对应 `user.model.user`，根包数据库绑定解析为 `database.user`；不存在时才使用 `database.default`。
- `config/`、`data/` 与输出二进制都在项目根下，但不参与 Dever 源码扫描。编译器内部只传递规范化后的项目根和源码根，不在 parser/checker 中重复猜目录。
- 旧的单目录语言 fixture 由测试工具显式传入源码根；应用 CLI 不增加自动回退或第二套模糊目录探测。

### setting.json

配置路径固定为 `<executable-dir>/config/setting.json`。数据库对象直接按连接名组织：

```json
{
  "app": {},
  "http": {},
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/app.db",
      "max_connections": 1,
      "max_page_size": 100
    },
    "report": {
      "type": "postgres",
      "url": "postgres://user:password@db.example.com/report",
      "tls": "system",
      "min_connections": 0,
      "max_connections": 4,
      "max_page_size": 100,
      "wait_timeout_ms": 5000,
      "io_timeout_ms": 30000
    }
  },
  "log": {}
}
```

- 使用严格的结构化 JSON 解析；未知数据库 type、缺失 default、无效数值或被引用连接不存在均失败。
- PostgreSQL 的 `tls` 必须显式为 `system` 或 `disabled`；`system` 使用现有 rustls 系统根并校验证书与主机名，不提供从 TLS 静默回落明文的 `prefer` 模式。
- PostgreSQL 的 `wait_timeout_ms` 限制池等待，`io_timeout_ms` 独立限制 SQL、事务和迁移 I/O；二者范围均为 1–300000 毫秒。
- 相对数据库/日志/upload/cache/tmp 路径都相对可执行文件目录，不相对 cwd。
- 启动时创建 `data/db`、`data/upload`、`data/log`、`data/cache`、`data/tmp`；不创建或修复 setting。
- 连接 selector 按“显式名称 -> 根包同名键 -> default”解析一次，Model 保存连接 slot。

### RuntimeProfile

发布构建读取 setting 中全部数据库 `type`，生成 `RuntimeProfile { sqlite, postgres, async, log }`：

- 只读取能力类型，不把 URL、密码、路径或池参数嵌入二进制或缓存 key。
- runtime 的 SQLite/PostgreSQL 依赖使用 optional Cargo features。
- profile、runtime rlib/dependency 快照和生成源码共同进入 native cache identity。
- 编译器 parse/check 不依赖 setting；生成可部署原生程序时需要 setting。程序启动会再次严格解析实际 setting。
- 运行时配置可以在已打包驱动集合内修改。若改成未打包的数据库类型，启动明确失败并要求重新构建，不做动态下载或回退。

### CMS 纵向样例

```text
examples/dever/cms/
  config/setting.json
  data/
  module/
    cms.dever
    user/model/user.dever
    user/service/register.dever
    news/model/news.dever
    news/service/publish.dever
```

- `user.model.user` 保存账户及初始化管理员，`news.model.news` 通过 `author_id: user.model.user.id` 建立静态外键。
- `user.service.register` 与 `news.service.publish` 使用 transaction 函数表达真实业务边界，直接复用编译器生成的 Model 操作。
- `cms.main` 只编排可观察的纵向流程，不复制 CRUD、迁移或连接选择逻辑。
- 默认 setting 只启用 bundled SQLite，既是新项目模板也是 SQLite-only 原生产物和小内存基准的真实用例。

## Runtime 设计

### 连接注册表

`DatabaseRegistry` 在应用入口启动 HTTP/任务前完成：

1. 定位并解析 setting；
2. 创建数据目录；
3. 构造所有被 Model 引用的连接池；
4. 解析 Model -> connection slot；
5. 验证 transaction 单连接不变量；
6. 获取每个连接的 schema 锁，执行 migration 和 Seed；
7. 成功后才进入用户入口。

热路径通过整数 slot 取连接，不执行字符串查找。连接实现使用小枚举静态分派，不使用每行/每值 trait object。

### SQLite

- 使用 `deadpool-sqlite` + `rusqlite bundled`。
- pool 上限不超过 Dever 全局 blocking 容量；每次同步交互都通过既有 blocking permit 和 pool object 生命周期。
- 默认一个连接，开启 foreign keys，设置 busy timeout；WAL 是否启用由连接配置明确决定，不在 runtime 猜测。
- transaction 独占同一个 pool object 直到 commit/rollback；RowStream 不能在释放连接后继续读取。

### PostgreSQL

- 使用 `deadpool-postgres` + `tokio-postgres`；TLS 连接通过 `tokio-postgres-rustls` 复用当前 rustls/webpki roots。
- pool 获取、连接建立、查询和 row stream 都使用 Tokio，且有独立的连接上限、获取等待和 I/O 超时。
- driver future 归属连接/pool 的结构化 scope；关闭 registry 时停止接收、等待借出连接归还并排空 driver task。
- 对固定 QueryPlan 使用 statement cache；迁移 DDL 与一次性动态片段不进入无界缓存。

### 统一错误

driver 错误在所属 executor 边界映射为稳定 `DatabaseError`：

- Connection、Timeout、Cancelled；
- Constraint（含 unique/foreign-key/check/not-null 分类）；
- NotFound；
- Decode/Encode；
- Migration/SchemaConflict；
- Busy/PoolExhausted；
- Driver（保留可记录的内部 code/cause，但不直接暴露到 HTTP）。

非幂等写入不自动重试。原始 driver cause 进入诊断链，公开 error 只暴露稳定字段。

## Schema 与 Seed 执行

### 内部状态

每个数据库使用一个保留前缀的内部 history 表记录：

- Model identity；
- kind：schema/migration/seed；
- logical name/revision；
- normalized checksum；
- applied_at。

history 不是目标 schema 的替代品。每次启动仍核对数据库 catalog；history 与实际 schema 冲突时拒绝继续，不猜测修复。

### 启动流程

- PostgreSQL 使用数据库级 advisory lock；SQLite 使用写事务锁，防止多个进程同时迁移同一 schema。
- 表不存在：直接创建当前最终表、索引与约束，不回放 `from` 或历史 migrate。
- 表存在：catalog -> normalized actual schema -> diff -> validation plan -> DDL plan。
- 收紧变化先运行只读验证 SQL，任一失败都不执行 DDL。
- 命名 migrate 校验未执行/内容未改，再与 DDL/history 在一个连接事务中提交。
- Seed 在 schema 成功后执行，按唯一键 conflict-do-nothing；只忽略目标唯一冲突。
- 多连接无法拥有全局原子迁移。各连接独立加锁和提交，按连接名稳定顺序执行；后续连接失败时应用不启动，已完成连接依靠 checksum 幂等重跑。

## 查询执行数据流

```text
generated Model function
  -> evaluate typed arguments
  -> acquire connection or reuse hidden transaction
  -> dialect renders fixed QueryPlan
  -> prepare/cache statement
  -> bind concrete values
  -> execute / pull rows
  -> generated row decoder
  -> Model/List/Page/Cursor result
  -> release connection or retain for bounded RowStream
```

- to-one include 与主查询共享受控 join 计划。
- to-many include 在获得父页后用一个有界 `IN (...)` 批次查询并按父 ID 归组；超过参数上限时按固定上限分批，不按父记录逐条查询。
- list 的 count 与 data 查询按同一条件树生成；cursor 只执行 data 查询。
- stream 持有连接和 statement 生命周期，消费者停止、Task 取消或 fault 时通过 Drop/scope 清理归还。

## Transaction 执行

- 最外层 transaction 从已验证 slot 获取一个连接并 begin。
- 生成的 `DatabaseContext` 携带 `ConnectionSlot + TransactionHandle`，只沿隐藏参数传递。
- 普通 Model call 检测到上下文时验证 slot 相同并直接使用 handle；无上下文时从 pool 获取。
- 同连接嵌套 transaction 只增加逻辑作用域，不 begin/commit。
- 函数成功时最外层 commit；error/fault/取消路径统一进入 rollback guard，rollback 完成后再传播原始失败。
- rollback 自身失败追加到诊断 cause，不覆盖原始业务 error/fault。
- transaction context 不能进入 `run`、Group、Channel、Stream producer 或返回值，因此不存在逃逸连接。

## 日志与执行边界

- runtime 维护一个结构化 `DiagnosticContext`，包含 fault/error code、source span、request/task id 和 cause chain。
- lower layer 只附加 cause/context，不记录根日志；HTTP、ownerless Task、CLI 等根边界决定是否记录。
- HTTP 未映射失败在 rollback 后返回通用 500；显式 handler 可把预期 error 映射为 4xx。
- 日志 front-end 先检查 level，再构造字段。队列容量固定；debug/info 满时丢弃并累计，warn/error 等待容量。
- 文件 sink 的编码、轮转和压缩策略不在本任务冻结；先完成有界队列、stderr emergency path 和一次记录合同。

## 性能设计

- 生成代码只保留可达 Model、字段 decoder、QueryPlan 和 driver profile。
- 相同静态 SQL shape 使用每连接 statement cache；值变化不创建新 SQL 字符串或缓存项。PostgreSQL 每连接缓存暂定上限为 64；to-many 参数批次生成的动态 SQL 不进入缓存，最终上限由阶段 10 基准收敛。
- page size、to-many 子项、IN 批次、批量写入、pool、blocking、RowStream、日志全部由一个明确上限约束。
- SQLite 默认一连接，PostgreSQL 默认零预热小池；最终默认值由 64/128 MiB 定向基准确定，并固化为命名常量和文档。
- 性能基准实测 RSS/PSS/VmHWM、cgroup、线程/FD、吞吐、p50/p95/p99 和二进制体积；配置的 pool capacity 单独记录。生产 runtime 尚无低成本观测点的 pool wait、SQL 数量和数据库连接数必须标记为 `unavailable`，不能用 FD 或配置值推算冒充实测。
- 构建产物以 manifest 的 SHA-256 和字节数校验后才运行；每个 ORM case 使用独立 bundle。PostgreSQL URL 的唯一 `{case}` 必须位于数据库名，敏感 URL 只进入权限为 `0600` 的 case setting，不进入 manifest/report。
- transaction cancel 使用单连接 pool：先证明连接可用，再证明长事务占用 pool、客户端在响应前断开、连接在 handler timeout 前归还且事务行数为零，避免把 handler timeout 回滚误报为请求取消。
- 先验证稳定内存和正确回收，再优化 SQL 生成、statement 命中或 decoder；不以缓存未绑定 SQL 或扩大队列换吞吐。

## 风险与处理

- 隐式挂起和错误传播会触及编译器、官方库和全部异步 fixture，应先单独完成语义迁移并保持 Rust runtime 行为，再叠加 ORM。
- SQLite DDL 经常需要重建表；schema planner 必须以真实 catalog fixture 验证，不能只比较生成 SQL 字符串。
- 外部 setting 与驱动裁剪可能不一致；构建记录 capability，启动严格校验，错误要求重建，不加载动态插件。
- 多数据库迁移无法跨连接原子提交；稳定顺序、单连接事务和 checksum 幂等是唯一可维护恢复路径。
- PostgreSQL 测试依赖真实服务；实施阶段只运行隔离的定向 fixture，不连接开发或生产数据库。

## 回滚边界

- 每一实施阶段必须在独立语义门槛下完成；不在半迁移状态同时保留新旧源码语法。
- 自动 schema 操作在提交前不改 history；失败回滚。跨连接已有成功迁移不反向回滚，由幂等重启继续。
- 发布二进制始终保留上一个版本；危险命名 migrate 需要先有数据库备份，自动回滚程序版本不能逆转已提交的数据迁移。

# SQLite 与 PostgreSQL 类型安全 ORM

## 目标

为 Dever 增加编译期类型检查、AOT 生成具体代码且无需反射的 ORM。第一轮完整支持 SQLite 和 PostgreSQL，共用 Model、查询计划、事务、迁移和错误合同；小内存机器只运行打包后的二进制，不承担 Rust 编译成本。

## 背景与约束

- Dever 是全新语言，不保留旧 ORM、旧 `async`/`await` 或其它源码兼容层，也不增加防御性回退路径。
- 当前编译器没有用户泛型、反射、动态 `Value`、method 或 type-as-value；公开 ORM 必须保持静态、直接和可 AOT 降低。
- Rust runtime 继续使用 Tokio 和真实异步 I/O。删除 Dever 源码中的 `async`/`await` 只改变语言表面，不把 HTTP、TCP、WebSocket、SSE 或数据库驱动改成阻塞线程模型。
- SQLite 面向单机、小内存程序；PostgreSQL 面向并发后端。二者共享语义，不强行共享执行机制。
- Model、Transaction、RowStream 等数据库资源沿用现有所有权、结构化任务、背压和取消清理原则。

## Model 与字段

- `model/` 下一个源文件定义一个 Model，继续使用普通 record `type`，不增加 `model` 关键字。
- 文件只公开一个与文件名对应的 Model record；表名和列名按文件名、字段名精确派生，不做复数化猜测。
- 同一文件集中字段、索引、关联、choice、Seed、迁移和紧贴持久化的数据访问操作，不增加外部 schema 映射文件。
- 每个 Model 隐式拥有只读 `id` 和 `created_at: DateTime`。二者出现在完整查询结果中，但不出现在创建输入中；`updated_at` 不隐式生成。
- `<model-package>.id` 是数据库生成的有符号 64 位正整数，也是 Model 专属不透明类型。PostgreSQL 使用 64 位 identity，SQLite 使用 `INTEGER PRIMARY KEY`；不同 Model 的 ID 不能混用，也不能按普通 `Int` 运算。
- 关联字段直接引用目标主键，例如 `owner_id: app.model.user.id`、`parent_id: app.model.category.id?`。作为 Model 字段时自动生成外键和普通索引，但不触发隐式加载。
- 第一轮字段类型包括 `Bool`、`Int`、`Float`、`Decimal(P, S)`、`Text`、`Bytes`、Model `.id`、`Uuid`、`DateTime`、`Date`、`Time`、`Duration`、`Json`、无载荷 choice 及其可空形式。
- `List`、`Map` 和普通 record 不隐式序列化为列；结构化持久化必须显式使用 `Json`。
- 字段元数据使用可解析、可格式化、可静态检查的语法，不使用字符串 tag。

### 字符串、字节与数值

- `Text` 表示无界 UTF-8 文本，`Text(max)` 表示最多 max 个 Unicode codepoint，`Text(min, max)` 表示 min 至 max 个 Unicode codepoint；不增加语义重复的 `Char`。
- `Bytes`、`Bytes(max)`、`Bytes(min, max)` 使用相同边界形式，但长度单位是字节。
- PostgreSQL 有界 Text 使用 `VARCHAR` 与必要的 CHECK；SQLite 使用 `TEXT` 与等价 CHECK，不能依赖 SQLite 忽略 VARCHAR 长度。
- `Decimal(P, S)` 必须满足 decimal128 的精度边界；两种数据库往返后保持精确值，不静默舍入。

### UUID 与时间

- `Uuid` 是固定 128 位值，运行时不使用堆分配 Text。PostgreSQL 使用原生 UUID，SQLite 使用 16-byte BLOB；API/JSON/文本统一为小写、带连字符的 36 字符格式。
- 普通 `Uuid` 由调用方提供；`Uuid generated` 由 ORM 创建 UUIDv7 并从创建输入省略；`generated` 与可空不能同时使用。
- `Uuid` 不隐式表示关系，关系仍使用 Model `.id`。
- `DateTime` 表示 UTC 毫秒时间点；`Date` 表示公历日期；`Time` 表示日内毫秒时间；`Duration` 表示固定毫秒时长，不使用包含月份换算的数据库 interval。

### Choice

- Model 文件可声明供字段使用的无载荷 choice；只有文件名对应的 record 生成表。
- 可选项写成 `Pending = "待处理"`。数据库和 API 存储左侧稳定变体名，右侧显示名称不入库。
- 编译器生成保持声明顺序的只读 `Choice.options: Map<Choice, Text>`；一个 choice 的变体必须全部带显示名或全部不带。
- 第一轮不支持 choice payload，也不创建 PostgreSQL 专有 enum。

## 索引、Schema 与迁移

- 单字段索引写为字段后的 `index` 或 `unique`；复合索引写为 `index(a, b)` 或 `unique(a, b)`。
- 索引描述当前目标 schema，不写版本号或手工名称；编译器稳定生成数据库对象名并静态检查字段引用和重复列。
- 当前 Model schema 是唯一目标。ORM 比较数据库系统目录、内部 history 与二进制嵌入的规范化 schema，不生成源码级 `table/` 状态目录。
- 新表直接按当前最终 schema 创建；新增可空字段、有合法常量默认值的非空字段、普通索引等意图唯一的变化自动应用。
- 长度、精度、可空性、默认值和约束直接修改目标字段。放宽变化自动应用；收紧变化先验证现有数据，数据不满足时整次迁移失败并要求命名 migrate，绝不截断、舍入、猜测补值或删除数据。
- 简单字段重命名写为 `display_name: Text(64) from nickname`。已有旧表执行 rename；全新数据库忽略历史来源，只创建最终字段。
- `from` 只供 schema 规划，不进入运行时字段、CRUD 输入、查询 SQL 或最终约束。
- 开发者不维护数字 schema 版本。编译器根据规范化 schema 和变更计划生成 revision 校验值，嵌入二进制并记录到数据库内部 history。
- 复杂数据变化使用 Model 文件内稳定命名的 `migrate <name> { ... }`，包括拆分、合并、跨行转换、删除前归档和显式 drop。
- 一个命名迁移只成功执行一次；已执行迁移的规范化内容改变时拒绝启动。迁移和 history 写入在同一数据库事务内完成。
- SQLite 无法直接 ALTER 的变化由方言层在事务内重建表；PostgreSQL 使用对应 DDL。两者完成后必须得到相同逻辑 schema revision。
- 只有结构化迁移无法表达时，命名 migrate 才可包含明确方言的参数化原生 SQL；不承诺该 SQL 跨数据库复用。

## Seed

- 每个 Model 文件最多一个无版本 `seed { ... }` 块，块内直接写多个匿名创建数据块，不重复 Model 名或 `create`。
- Seed 数据按 Model 创建输入静态检查，不能填写 `id`、`created_at` 或其它 generated 字段。
- 每条 Seed 必须完整提供至少一个单字段或复合唯一键，用于稳定识别初始化记录。
- 编译器为规范化 Seed 生成校验值。内容未变化时启动不执行 Seed SQL；变化时在 schema 迁移成功后，以一个事务重新执行“缺失则插入”。
- 唯一键冲突保留数据库现值；其它非空、CHECK、外键或解码错误必须失败。Seed 不覆盖或删除线上数据，修改/删除内置数据使用命名 migrate。

## 查询与内置操作

- Model 包生成唯一一套短函数：`create`、`create_many`、`get`、`first`、`list`、`cursor`、`count`、`exists`、`stream`、`update`、`delete`、`upsert`。
- 业务代码直接调用 `user.list(...)`、`user.update(...)`，不暴露 `User.Query`、`User.List`、Active Record 实例方法或第二套 CRUD。
- 创建、更新和查询配置使用上下文推导的匿名块；底层生成具体静态输入和结果类型，不引入动态字段 Map。
- 查询只采用一次性配置块：

```dever
users = user.list({
  where = status == UserStatus.Active
    and age >= 18
    and (city == "北京" or city == "上海")
  order = [created_at.desc, id.desc]
  page = 1
  size = 20
  with = [owner]
})
```

- 不提供 `user.where(...).order(...).list()` 链式查询器；链式查询不会提升数据库性能，却会要求新增 method、泛型查询对象和所有权语义。
- 多个条件使用现有 `and`、`or`、`not` 和括号；支持比较、`in`、`between`、`contains`、`starts_with`、`ends_with`、null 与关联字段路径。
- 查询表达式在编译期降低为类型检查后的 `QueryPlan`。字段、排序、关系和 SQL 结构不能来自运行时字符串；所有值通过参数绑定。
- `list()` 默认返回第一页，默认 size 为 20，并受 `setting.json` 的统一上限约束；排序不是唯一键时自动追加 `id` 保证稳定分页。
- `list` 返回 `items/page/size/total/pages`；`cursor` 返回 `items/next/has_more` 且不执行 count；`stream` 全量遍历但保持有界内存，不提供无界 `all()`。
- 正向 to-one 由关联 ID 推导；反向 to-many 在 Model 中显式声明 `relation tasks = app.model.task.owner_id`。
- `with` 才加载关系。to-one 使用受控 JOIN 或等价单次查询；to-many 按当前父页 ID 批量加载并归组，必须有排序和数量上限；禁止 lazy loading 和 N+1。
- 多对多使用显式中间 Model，不生成隐藏表。
- 复杂查询保留参数化、类型化的原生 SQL 逃生口。调用必须声明具体 Model/record 结果和方言；不返回动态行 Map，不允许 SQL 字符串插值。

## 数据库配置与绑定

- 应用项目根采用固定布局：`module/` 是唯一 Dever 源码根，`config/` 保存 `setting.json`，`data/` 保存运行数据。编译器从项目根加载时按 `module/` 的相对路径派生逻辑 package，`module` 本身不进入 package 名；`config/` 和 `data/` 不参与源码扫描。
- 可执行文件按自身所在目录解析固定布局，不使用进程当前目录，也不提供环境变量覆盖：

```text
app/
  app
  config/
    setting.json
  data/
    db/
    upload/
    log/
    cache/
    tmp/
```

- `config/setting.json` 是唯一配置文件；缺失、无法解析或数据库配置无效时启动失败。运行时按需创建 `data/` 子目录，不生成默认 setting。
- `setting.json` 顶层使用 `app`、`http`、`database`、`log` 等对象；`database` 直接映射连接名，不增加 `connections` 包装层。
- `database.default` 必须存在；其它名称由应用定义，例如 `database.report`。
- Model 连接按以下顺序在启动时解析并缓存：
  1. Model 文件显式 `database report`；
  2. 否则按根包名查找，例如 `app.model.user` 查 `database.app`；
  3. 根包连接键不存在时使用 `database.default`。
- 只有“根包连接键不存在”允许回落 default。显式连接不存在、default 不存在或被选连接无效都直接启动失败。
- 普通 CRUD 不接收 Database 或 Transaction 参数，也不允许按运行时字符串切换连接。
- SQLite 使用 bundled `rusqlite`，同步调用进入现有有界 blocking 通道，默认小连接数并明确单写者约束。
- PostgreSQL 使用 `tokio-postgres` 与有界异步连接池；池容量、等待和超时全部受配置上限约束。TLS 必须显式选择系统根验证或禁用，不允许验证失败后静默回落明文。
- 编译器按可达数据库能力选择 runtime profile。SQLite-only 二进制不链接 PostgreSQL 驱动，PostgreSQL-only 二进制不链接 SQLite；驱动集合进入原生产物缓存身份。

## CMS 示例项目

- 在 `examples/dever/cms/` 建立可检查、可运行、可构建的完整项目，而不是只展示孤立语法片段。
- 固定目录为 `config/setting.json`、`data/`、`module/cms.dever`、`module/user/model/`、`module/user/service/`、`module/news/model/`、`module/news/service/`。
- `module/user` 和 `module/news` 分别是组件根；逻辑 package 使用 `user.model.*`、`user.service.*`、`news.model.*`、`news.service.*`，入口为 `cms.main`。
- Model 展示隐式 `id`/`created_at`、Uuid、choice、Seed、索引和跨组件关联；Service 只承载注册、发布等事务业务，不包装 Model 已生成的普通 CRUD。
- 示例默认使用 SQLite 和相对路径 `data/db/cms.db`，因此打包二进制可在无外部数据库服务的机器上直接运行；PostgreSQL 通过独立定向 fixture 验证，不让 CMS 示例依赖现有服务。

## 顺序调用、并发与事务

- Dever 源码删除 function/handler 的 `async` 标记和直接调用外层的 `await`。普通调用表示顺序等待完成，编译器根据传递挂起 effect 自动降低为 Rust coroutine。
- 只有 `run(call)` 显式开始并发，`wait(task/group)` 消耗并等待结果，`stop(task/group)` 请求取消并等待结构化清理。
- 同步入口的可达图没有挂起 effect 时不启动异步 runtime；包含网络、数据库、定时器、Channel 或异步 Stream 时只启动一个有界 Tokio runtime。
- 事务使用专用函数声明：

```dever
transaction create_order(...) (...) {
  order = order.create({...})
  inventory.update({...})
}
```

- 源码不出现 `@transaction`、Database、Transaction、begin、commit、rollback、`async` 或 `await`。
- 编译器分析完整受控调用链中的 Model 数据库 effect，静态推导唯一连接，并通过隐藏参数传递事务上下文；不使用线程局部、任务局部或全局“当前事务”。
- 最外层 transaction 函数开启物理事务；正常完成提交，error、fault 或取消先回滚再传播原始失败。
- 同连接的嵌套 transaction 函数复用外层事务，不开启 savepoint，也不提前提交。
- 无数据库 effect、跨连接、动态无法确定连接，或在 `run`/并发子任务中执行数据库 effect 的 transaction 函数在编译期拒绝。第一轮不支持分布式事务。

## Error、Fault 与日志边界

- choice 变体继续用 `error` 标记可恢复失败；编译器保留并扩展 C012，禁止丢弃、覆盖或仅用日志观察失败。
- 普通可失败调用直接返回成功值，失败自动向当前边界传播，不写 `?`：

```dever
user = user.get(id)
```

- 需要主动处理完整结果时使用一个核心操作 `result(call)`，再由现有穷尽函数分句处理成功和 error 变体。
- 自定义业务失败使用 `fail(OrderError.OutOfStock(id))`。错误集合由编译器沿调用图静态推导并显示在 API、文档和编辑器信息中，不要求每个函数重复声明错误类型。
- `error` 表示可恢复业务/外部失败；`fault` 表示程序不变量、编译器已知资源或 runtime 故障。普通业务函数第一轮不能捕获 fault。
- HTTP 未处理 error/fault 只终止当前请求，回滚事务后返回不泄露内部细节的 500，并在服务端根边界记录一次；预期 4xx 必须由 handler 显式映射。
- Task 失败由 `wait`/`result` 返回；无 owner 的任务边界记录一次。CLI 输出 stderr 并返回非零退出码。
- 日志是观察，不是错误处理；`log.error` 不满足 C012。底层驱动、事务和 Service 不重复记录同一失败。
- 正式日志 API 采用 `dever.log.debug/info/warn/error(message, fields)` 与共享结构化诊断上下文。队列有界：debug/info 满时丢弃当前事件并累计数量，warn/error 施加背压；logger 自身失败或最终 fault 直接写 stderr。

## 性能与资源边界

- ORM 只生成具体 Model 输入、行解码和静态查询计划；共享查询规划、schema diff 和方言实现，不为每个 Model 复制算法。
- 查询使用绑定参数和可复用预编译语句；结果按行拉取，有界缓冲，不默认收集全表。
- 连接池、SQLite blocking、批量写入、分页、关系子列表、stream 和日志队列全部有明确上限。
- 不静默重试非幂等写入；约束冲突、连接失败、超时、取消和解码失败保持可区分。
- 小机器验收以打包二进制的稳定 RSS、空闲连接成本、并发请求内存斜率和取消后的资源回收为准，不以构建机编译耗时或峰值内存为主要目标。

## 验收标准

- 合法 Model 在原生构建前生成确定 schema、创建输入、查询结果和 CRUD；字段、索引、Seed、关系或查询中的非法引用产生源码定位诊断。
- 未声明 `id`/`created_at` 的 Model 仍能读取这两个字段，创建时无需填写；不同 Model ID 无法误传。
- SQLite 与 PostgreSQL 对全部首批字段类型、默认值、长度、精度、choice、外键和索引产生等价逻辑约束并正确往返；PostgreSQL 系统根 TLS 校验和显式明文模式分别通过定向验证。
- 同一个查询配置块在两种数据库生成正确的参数化 SQL，支持分页、游标、stream、复合条件和显式关联加载；to-many 不产生 N+1。
- `list` 默认有界并返回 total/pages，`cursor` 不执行 count，`stream` 在慢消费者下保持有界内存。
- 从空数据库启动时直接创建最终 schema 并执行缺失 Seed；重复启动无重复数据。已存在数据库能安全自动升级，危险变化在没有有效 migrate 时保持原 schema/数据并启动失败。
- 带 `from` 的稳定二进制既能初始化新数据库，也能保留旧列数据完成 rename；自动 revision 和命名迁移 history 可重复验证且不可静默改写。
- 默认、根包和显式连接按既定优先级绑定；显式缺失连接和无效配置启动失败。事务调用链只能使用一个已解析连接。
- create/get/update/delete/list、事务提交、事务回滚和取消清理分别通过 SQLite 与 PostgreSQL 的定向真实数据库 fixture。
- 普通 Dever 网络和数据库调用无需 `async`/`await` 仍是非阻塞 I/O；只有 `run` 建立业务并发，同步程序不启动 Tokio。
- 可恢复 error 不能被丢弃或用日志代替处理；默认传播、`result(call)`、`fail(...)`、HTTP/Task/CLI 边界和事务回滚具有定向编译与运行验证。
- SQLite-only 与 PostgreSQL-only 发布二进制只包含所用驱动；目标机器不需要 Cargo、Rust、SQLite 动态库或 PostgreSQL 客户端 SDK。
- 在 64 MiB 与 128 MiB 约束下分别测量 SQLite、PostgreSQL 的空闲基线、稳定请求负载、流式读取和连接池上限；持续运行时 RSS 不无界增长，取消后连接与事务槽位回到基线。
- `deverc check examples/dever/cms`、`deverc run examples/dever/cms cms.main` 和以同一项目根执行的原生构建均使用 `module/` 作为源码根，并能由 CMS Service 完成初始化数据、注册和发布的纵向流程。

## 暂不纳入第一轮

- 旧源码兼容、第二套 CRUD、链式查询、动态 Model、反射字段访问或运行时动态 SQL 结构。
- MySQL/MariaDB、SQL Server、Oracle 驱动；只保留真实共享边界，不提前增加空适配器。
- 非默认主键、自定义主键类型、以任意 UUID 唯一键建立关系、主键策略迁移。
- choice 稳定变体改名和表/Model 重命名的通用简写；第一轮使用命名 migrate。
- lazy loading、自动 N+1、隐藏多对多表、跨数据库 join、分布式事务、savepoint、读写分离和自动故障转移。
- 数据库专有数组/enum/interval、分区表、表空间、物化视图、触发器、行级安全和在线无锁迁移。
- 通用 annotation、decorator、macro、AST 改写和用户编写的编译期函数处理器。
- 前端、管理页面、package registry、跨平台发行和编译器自举。
- 日志文件 sink 的最终格式、轮转默认值和压缩策略；该项在错误基础设施子阶段收敛，不阻塞 ORM 合同。

# ORM 架构选项

## 现有语言约束

- 一个源文件完整拥有一个 package，源码相对路径必须与 package 名匹配，因此可用 `model/` 路径承载 Model 身份，不需要再增加 `model` 声明。
- `crates/dever-core/src/native/records.rs` 为用户 record/choice 生成具体类型；生产路径没有通用运行时 Value 或反射分派。
- 系统资源由 `types.rs`、`check/symbols.rs`、`intrinsic.rs`、native emitter 和 runtime 共同定义，错误由官方 `.dever` choice 暴露。
- static handler 是编译期绑定而非运行时函数值，可复用于行映射与查询回调，但不能单独解决表名、主键、索引和关联元数据。
- 当前 native build 复用一个预编译 runtime rlib；数据库驱动选择必须进入可达性、runtime build 和缓存身份，不能只依赖链接器偶然消除代码。

## 表面模型比较

### 显式 Data Mapper（已否决）

概念形态：

```text
find(User, database, id)
insert(database, user)
update(database, user)
```

Model 是数据/schema，Database 和 Transaction 都由调用方显式传递。虽然实现直接，但业务表面重复且容易把同一事务漏传给某次操作；用户已明确否决，不作为公开或兼容 API 保留。

### Active Record（已否决）

概念形态：

```text
User.find(database, id)
user.save(database)
```

该方案会把生成类型和成员暴露给业务代码，用户已明确否决，不作为兼容表面保留。链式查询同样需要新增 method 和中间查询对象语义，且不改善生成 SQL；首版已经确定不采用。

### SQL-first typed mapper

概念形态：参数化 SQL + 静态 row decoder。它不单独承担 ORM，只作为模型包短函数之外的高级逃生口保留：参数必须绑定，调用方必须声明具体 Model/record 结果，数据库专有语句明确选择对应 dialect。

## Model 文件约定

推荐形态：

```dever
package app.model.user exposes (
  User
)

type User {
  email: Text(254) unique
  name: Text(1, 64)
  bio: Text?
}
```

- 路径 `app/model/user.dever` 对应 `package app.model.user`，文件名 `user` 精确对应公开 record `User` 和表名 `user`；不做复数化猜测。
- 文件只能公开一个 Model record。私有辅助声明是否允许由实现阶段按真实需要决定，不能形成第二个持久化类型。
- 主键 `id` 和 `created_at` 由 Model 文件约定隐式生成，不进入创建输入；查询结果仍可读取 `user.id` 与 `user.created_at`。其它 Model 通过 `app.model.user.id` 引用该强类型主键。
- 编译器可合成 `User.New` 创建输入和固定 CRUD 成员；这属于 Model 专用静态语义，不意味着加入通用嵌套 type、反射或 type-as-value。
- 简单字段属性紧邻字段；复合唯一约束、复合索引、初始化数据和有序迁移使用同文件的独立声明，避免把复杂结构压进字段行或外部配置。
- 安全且意图唯一的 schema 变化自动生成；索引和 Seed 不使用版本号。简单重命名通过新字段的 `from <old_field>` 明确，编译器自动生成 revision；删除、类型收窄和数据转换等复杂操作才使用 Model 文件底部的命名 migrate。revision/迁移校验值记录在 history 中，SQLite 表重建由 dialect 隐藏。
- Model 文件可以声明自定义持久化操作，但优先组合生成的查询/事务能力；不能覆盖通用 CRUD 的事务、错误和资源合同。

## 驱动边界

- SQLite：成熟 Rust 驱动支持 bundled SQLite，适合产生无需目标系统 SQLite 的单文件程序；同步 API 必须通过 Dever 已有 bounded blocking 资源执行。
- PostgreSQL：成熟 Tokio 驱动原生异步并支持流水线；连接 driver 必须属于 Database/Pool 的结构化作用域。
- 两者共享 QueryPlan、绑定值、行 schema、错误分类和迁移意图；SQL 编码与执行机制保持独立。

## 结论

采用 Model 包短函数作为唯一首版表面，例如 `user.list(...)`、`user.update(...)`。Model 用一文件一普通 `type` 的约定识别，不增加 `model` 关键字；创建、更新和查询块由调用上下文推导，不暴露 `User.Query` 等生成类型。复杂查询使用参数化、类型化的原生 SQL，不引入动态行对象。

跨操作事务采用 `transaction <function>` 专用函数声明。普通调用按顺序等待完成，编译器根据传递挂起 effect 自动降低为协程；Dever 源码不出现 `async`、`await`、Database、Transaction、begin、commit 或 rollback。编译器从 Model 数据库效果推导唯一连接，并把内部事务上下文沿受控调用链传递；正常完成自动提交，error、fault 或取消自动回滚。首版不允许事务中的数据库效果进入并发子任务，也不提供跨库事务、嵌套物理事务或 savepoint。

不引入 `@` 或通用函数装饰器。注解本身并不能减少事务实现，只会额外增加解析、注解解析、顺序、参数、扩展权限和源码映射合同；用户自定义处理还需要通用函数泛型、闭包捕获、async 包装与错误转发。完全自动推导事务和调用点 `transaction(handler)` 也分别存在边界隐蔽与包装遗漏问题，因此都不作为公开表面。

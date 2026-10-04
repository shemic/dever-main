# ORM 查询与内置操作方案

## 结论

公开表面只使用 Model 包短函数和一次性查询配置块：

```dever
users = user.list({
  where = status == UserStatus.Active and age >= 18
  order = [created_at.desc, id.desc]
  page = 1
  size = 20
  with = [owner]
})
```

不提供 `User.Query`、`User.List`、Active Record 实例方法或 `user.where(...).order(...).list()` 链式查询器。编译器把匿名块直接静态降低为 `QueryPlan`；源码没有 `async`/`await`，普通调用顺序等待完成。

## 为什么选择配置块

块式和链式可以生成相同 SQL，链式本身没有数据库性能优势。Dever 当前没有通用 method、lambda、用户泛型或 type-as-value，链式方案会额外引入查询对象类型、所有权、复用和执行时机合同。配置块则有以下特点：

- `create`、`update` 和 `list` 保持同一种上下文推导形式。
- 一次调用完整展示条件、排序、分页和关系加载。
- 编译器直接生成一个静态计划，不产生运行时中间查询对象。
- 公开 API 只有一套写法，不需要兼容或维护等价表面。

将来若动态 HTTP 参数确实需要逐步组合条件，应增加受限的类型化 where 值，而不是改造全部 CRUD 为链式查询。

## 基本 CRUD

```dever
users = user.list()
current = user.get(user_id)
created = user.create({
  name = "Dever"
  status = UserStatus.Active
})
updated = user.update(user_id, {
  name = "New name"
})
deleted = user.delete(user_id)
```

`create` 和 `update` 的匿名数据块由调用上下文推导字段。不能写入的 generated 字段、拼错的字段和不兼容值在编译期拒绝；底层仍是具体输入 record，不是动态 Map。

普通可失败调用成功时直接产生成功值，失败自动传播。需要主动处理完整结果时写：

```dever
result = result(user.get(user_id))
```

随后使用现有穷尽函数分句处理成功和 error 变体，不为 ORM 增加 try/catch 或 `?`。

## List、Cursor 与 Stream

`user.list()` 返回有界默认第一页。完整配置为：

```dever
users = user.list({
  where = status == UserStatus.Active and created_at >= start
  order = [created_at.desc, id.desc]
  page = 1
  size = 20
  with = [owner]
})
```

查询块处于当前 Model 上下文，所以字段直接写 `status`、`created_at`、`owner`，不重复 Model 名。它不是动态 Map，字段、运算符、值、排序和关系都由编译器检查。

- `page` 默认 1，`size` 默认 20，并受统一配置上限约束。
- 默认排序为 `id.desc`；自定义排序不是唯一键时自动追加 `id`，保证页边界稳定。
- 返回 `items`、`page`、`size`、`total`、`pages`。
- 页码查询执行数据查询和 count；不需要 total 时使用 cursor。

深翻页使用同一个配置块：

```dever
users = user.cursor({
  after = next
  size = 100
  where = status == UserStatus.Active
  order = [created_at.desc, id.desc]
})
```

`cursor` 返回 `items`、`next`、`has_more`，不执行 count。全表处理使用 `user.stream(query?)`，按行拉取并保持有界缓冲；不提供无界 `all()`。

## Where

多个条件直接组成一个布尔表达式，不增加条件数组：

```dever
users = user.list({
  where = status == UserStatus.Active
    and age >= 18
    and age < 60
    and (city == "北京" or city == "上海")
})
```

- 比较支持 `==`、`!=`、`<`、`<=`、`>`、`>=`。
- 逻辑组合使用既有 `and`、`or`、`not` 和括号。
- 集合与区间使用 `in`、`between`。
- Text 匹配使用 `contains`、`starts_with`、`ends_with`。
- 可空字段可直接与 `null` 比较。
- 关系条件使用字段路径，例如 `owner.status == UserStatus.Active`；筛选关系不会自动把关系放进结果。

ORM 将表达式规范化为静态条件树，按源码顺序收集绑定值。相同字段可重复出现，矛盾条件由数据库返回空结果；不引入运行时条件解释器。

## 关联

正向 to-one 从关联 ID 推导：

```dever
type Task {
  owner_id: app.model.user.id
}
```

查询时使用 `with = [owner]`。反向 to-many 无法可靠推断名称，因为目标 Model 可能有多个外键，因此在当前 Model 文件显式命名：

```dever
relation tasks = app.model.task.owner_id
```

- to-one 使用受控 JOIN 或等价单次查询。
- to-many 按当前父页 ID 批量加载并归组，不逐父记录查询。
- to-many 子列表必须有稳定排序和数量上限。
- 多对多使用显式中间 Model，不生成隐藏表。
- 不提供 lazy loading，读取普通字段不会在背后访问数据库。

## 内置函数

| 函数 | 用途 |
| --- | --- |
| `user.create(data)` | 创建一条并返回完整 Model |
| `user.create_many(list)` | 有界批量创建 |
| `user.get(id)` | 按主键获取一条 |
| `user.first(query?)` | 按条件获取第一条 |
| `user.list(query?)` | 自动页码分页 |
| `user.cursor(query?)` | 稳定游标分页 |
| `user.count(where?)` | 统计数量 |
| `user.exists(where?)` | 判断是否存在 |
| `user.stream(query?)` | 有界流式遍历 |
| `user.update(id_or_query, changes)` | 单条或条件批量更新 |
| `user.delete(id_or_query)` | 单条或条件批量删除 |
| `user.upsert(key, create, update)` | 按唯一键新增或更新 |

`update`/`delete` 根据首个参数静态区分主键和条件，不增加重复的 `update_many`/`delete_many` 名称。批量写入返回影响行数；业务状态流转、跨表原子编排和外部调用由 transaction 函数组合。

## 数据库绑定

普通调用不传 `Database`：

```dever
user.list({...})
user.update(user_id, {...})
```

Model 的连接在程序启动时解析并缓存：

1. Model 文件显式 `database report`；
2. 否则按根包名查找，例如 `app.model.user` 查 `database.app`；
3. 根包键不存在时使用必需的 `database.default`。

显式连接不存在、default 不存在或选中配置无效时启动失败。生成代码访问固定连接槽位，不按字符串进行每请求查找，也不允许 CRUD 临时切库。

## 自动事务

事务不属于 Model API，源码不取得或传递 Transaction：

```dever
transaction assign_task(
  user_id: app.model.user.id,
  title: Text
) (result: AssignResult) {
  user.update(user_id, {
    active = true
  })
  task.create({
    owner_id = user_id
    title = title
  })
  result = AssignResult.Assigned
}
```

编译器分析完整调用链，从 Model 数据库 effect 推导唯一连接。进入最外层函数时自动开启物理事务，直接或经普通顺序 helper 执行的操作都使用隐藏事务上下文；正常完成提交，error、fault 或取消先回滚再离开。

同连接嵌套 transaction 复用外层事务，不使用 savepoint。跨连接事务和事务中的并发数据库子任务在编译期拒绝。该设计不引入 `@`、decorator、显式 begin/commit/rollback 或动态当前事务。

## 实现与性能

- 所有 Model 复用一个编译期 `QueryPlan`、schema 引擎和方言执行层，只生成字段专属的薄代码。
- 参数绑定、预编译语句和具体行解码不经过反射或动态 Map。
- to-many 批量加载避免 N+1，也避免多个 to-many JOIN 产生笛卡尔膨胀。
- 分页、关联子列表、批量写入、行流、连接池和 SQLite blocking 全部有界。
- SQLite 和 PostgreSQL 可以用不同执行器，但必须消费同一逻辑查询计划并返回同一 Dever 类型。

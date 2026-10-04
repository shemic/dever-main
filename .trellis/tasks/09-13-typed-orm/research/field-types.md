# ORM 字段类型与隐式字段

## 默认字段

`model/` 中的每个 Model 即使源码不写，也具有：

```dever
id: <model-package>.id
created_at: DateTime
```

- `id` 是编译器合成、数据库生成的 64 位正整数主键字段；它的公开静态类型就是当前 Model 的 `<model-package>.id`，不能当作普通 Int 运算，不同 Model 之间也不能混用。
- `created_at` 由 ORM 在插入时写入当前 UTC DateTime。
- 两个字段都存在于完整 Model 和查询结果中，不存在于 `Model.New` 创建输入中。
- `updated_at` 不默认生成。Go Dever 现有 Model 中 created_at 明显比 updated_at 普遍；只有确实需要跟踪修改时间的 Model 才显式声明自动更新字段。
- `deleted_at`、version、tenant_id 和操作者字段都具有业务含义，不作为所有 Model 的隐式字段。

因此最小 Model 可以写成：

```dever
type User {
  name: Text(1, 64)
  email: Text(254) unique
}
```

读取 `user.id`、`user.created_at` 合法，但创建时只提供 name/email。

## 通用类型矩阵

| Dever 类型 | 逻辑语义 | PostgreSQL | SQLite |
| --- | --- | --- | --- |
| Bool | 布尔值 | BOOLEAN | INTEGER + CHECK |
| Int | 有符号 64 位整数 | BIGINT | INTEGER |
| Float | binary64 | DOUBLE PRECISION | REAL |
| Decimal(P, S) | 最多 34 位十进制定点数 | NUMERIC(P,S) | 精确定点编码与约束 |
| Text | 无界 UTF-8 文本 | TEXT | TEXT |
| Text(N) | 0 至 N 个 Unicode codepoint | VARCHAR(N) | TEXT + CHECK |
| Text(M, N) | M 至 N 个 Unicode codepoint | VARCHAR(N) + CHECK | TEXT + CHECK |
| Bytes | 无界字节 | BYTEA | BLOB |
| Bytes(N) / Bytes(M, N) | 有界字节 | BYTEA + CHECK | BLOB + CHECK |
| `<model-package>.id` | 具体 Model 的不透明 64 位主键/外键投影 | BIGINT identity | INTEGER PRIMARY KEY |
| Uuid | 128 位 UUID | UUID | 16-byte BLOB |
| DateTime | UTC 时间点，毫秒精度 | TIMESTAMPTZ(3) | epoch-millis INTEGER |
| Date | 公历日期 | DATE | epoch-day INTEGER |
| Time | 一天内时间，毫秒精度 | TIME(3) | millis-of-day INTEGER |
| Duration | 固定毫秒时长 | BIGINT | INTEGER |
| Json | 已验证 JSON 值 | JSONB | UTF-8 TEXT |
| 无载荷 choice | 封闭枚举 | TEXT + CHECK | TEXT + CHECK |

List、Map、普通 record 不自动变成 JSON 列。调用方必须显式选择 Json，避免字段重构在不知情时改变持久化格式。

## UUID 字段

UUID 不是每个 Model 的隐式字段。只有业务需要公开不可预测标识、跨系统交换标识或在写入数据库前生成标识时才显式声明：

```dever
type User {
  uuid: Uuid generated unique
  name: Text(1, 64)
}
```

- `uuid: Uuid` 由调用方提供，存在于创建输入中。
- `uuid: Uuid generated` 由 ORM 在创建时生成 UUIDv7，不存在于创建输入中。
- `uuid: Uuid?` 接受空值；`generated` 与可空不能同时使用。
- 运行时使用固定 16 字节值，不用堆分配的 Text 保存 UUID。
- PostgreSQL 使用原生 UUID，SQLite 使用 16-byte BLOB，索引不会保存 36 字符文本。
- API、JSON、日志及显式文本转换统一输出小写、带连字符的 36 字符形式；解析只接受语言标准库定义的 UUID 文本格式。
- Uuid 字段不会自动成为关联。关联仍写成 `app.model.user.id`，以保持外键类型检查和紧凑索引。

## 初始化数据写法

Seed 是当前 Model 的可重复初始化数据。一个块可以包含多条数据，值使用 Model 创建输入进行静态检查：

```dever
type Status {
  code: Text(32) unique
  name: Text(64)
  enabled: Bool default true
}

seed {
  {
    code = "pending"
    name = "待处理"
  }

  {
    code = "done"
    name = "已完成"
    enabled = true
  }
}
```

以后增加初始化记录时直接追加数据块，不维护 Seed 版本号。编译器按 Seed 内容校验值判断是否需要执行；执行时整体使用事务，唯一冲突保留已有记录。源码移除 Seed 也不删除数据，需要修改或删除时使用显式命名迁移。

## 可选项

固定可选项复用语言现有的无载荷 choice `type`，不另造 ORM enum：

```dever
type TaskStatus {
  Pending = "待处理"
  Running = "进行中"
  Done = "已完成"
}

type Task {
  title: Text(1, 160)
  status: TaskStatus default TaskStatus.Pending
}

seed {
  {
    title = "初始化任务"
    status = TaskStatus.Pending
  }
}
```

`Task` 是文件对应的 Model record；`TaskStatus` 是同文件的辅助 choice，不生成表。ORM 第一轮只接受不带 payload 的 choice，并将左侧变体名映射为带 CHECK 约束的 Text。右侧显示名称不存入业务列。

推荐由编译器把显示名称提供为保持声明顺序的只读 Map，不再重复定义映射：

```dever
task_status_options() (options: Map<TaskStatus, Text>) {
  options = TaskStatus.options
}
```

需要选项子集或自定义顺序时再返回 `List<TaskStatus>` 或组合 `TaskStatus.options`。choice 负责限制合法值，生成的 Map 负责名称和默认顺序。

## 索引与迁移写法

单字段索引直接跟在字段后，复合索引放在 Model 后；它们都描述当前目标 schema，不使用版本号或手工数据库名称：

```dever
type Task {
  tenant_id: app.model.tenant.id
  code: Text(32)
  status: TaskStatus index
  title: Text(1, 160)
}

unique(tenant_id, code)
index(status, created_at)
```

简单字段重命名直接写在新字段旁边：

```dever
type Task {
  summary: Text from description
}
```

编译器生成 `RENAME COLUMN`，不会把它误判成删旧列再加新列。revision 由编译器根据规范化 schema 和计划自动生成并写入数据库 history，源码不维护版本号。

拆分、合并、删除前归档等无法附着在单个新字段上的数据变化，才保留一次性命名迁移：

```dever
migrate remove_legacy_code {
  drop legacy_code
}
```

迁移名称是复杂操作的稳定身份，内容校验值用于阻止已经执行的迁移被悄悄修改。`from` 需要保留到所有可能升级的数据库都已越过该变更。

## 关联 ID

```dever
type Article {
  author_id: app.model.user.id
  category_id: app.model.category.id?
  title: Text(1, 160)
}
```

- `app.model.user.id` 与 `app.model.category.id` 是不同静态类型，不能误传。
- 点分路径的最后一段 `id` 是目标 Model 的隐式字段投影，不是新增的普通小写 type；它也能用于 function 输入、输出和局部类型位置。
- 非空和可空关联第一轮都使用 `ON DELETE RESTRICT`；SET NULL 或 CASCADE 的通用关系选项不在第一轮，必要时使用明确方言的命名 migrate。
- 每个关联 ID 自动创建普通索引和外键约束。它只表达持久化关系，不自动查询目标对象。
- 关系预加载必须显式请求并保持有界，不能形成隐藏 N+1。

## 时间 API 方向

- DateTime 值始终代表 UTC 时间点，不在值内携带可变时区数据库。
- 时区只参与解析、格式化和日历换算；数据库比较、排序和相减使用统一时间点。
- `created_at` 自动取当前 DateTime。`updated_at` 不隐式生成；第一轮需要该字段时按普通 `DateTime` 明确写入，自动更新修饰词后续单独设计。
- DateTime/Date/Time 的 parse、format、比较和加减由标准 package 提供；数据库函数通过 ORM 查询表达式或明确的原生 SQL 使用。

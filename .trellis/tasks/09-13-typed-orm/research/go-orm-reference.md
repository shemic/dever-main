# Go Dever ORM 参考结论

## 已核对实现

- `/data/project/shemic/backend/dever/orm/schema_registry.go` 通过 Go 反射读取 `dorm` 字符串 tag；Text 默认 `VARCHAR(255)`，`size:N` 生成 `VARCHAR(N)`，也允许直接写 `type:varchar(N)` 或 `type:text`。
- `ModelConfig.Index`/`Indexes` 使用额外 Go struct 表达单列或复合索引；`ModelConfig.Seeds` 使用 `[]map[string]any` 表达初始化数据。
- `/data/project/shemic/backend/dever/orm/schema_apply.go` 能建表、加字段、修改部分数据库的字段类型、删旧字段，并同步新增、变化和删除的索引。
- 当前 Seeds 只在表首次创建时插入；已有表后来新增 Seed 不会自动补入。Seed 使用动态 Map，字段名和值类型不能在编译期完整检查。
- 当前自动迁移直接对数据库执行，再可选记录 SQL；它会删除不在目标 schema 中的字段和索引。字段重命名只对大小写或下划线差异做有限推断，无法可靠判断任意重命名。
- `database.persist=true` 会把目标 schema 写入 `data/table/*.json`，`dever migrate` 再读取这些文件应用到目标数据库。该目录是 schema 传递/发布产物，不包含某个数据库实例是否成功执行的可靠状态；同一文件可以对应多个状态不同的数据库。
- Go Model wrapper 可以嵌入通用 `orm.Model[T]` 并增加 `AfterSave`、`AfterDelete` 或自定义查询方法。普通 Model 直接复用通用 CRUD，不需要 wrapper。

## 新语言应复用的设计

- 一个 Model 文件集中字段、索引、默认排序、初始化数据和少量紧贴持久化生命周期的扩展。
- 通用 CRUD、过滤、排序、分页、事务和批量操作由 ORM 提供，不为每个 Model 重写。
- 单列与复合索引都从 Model 目标 schema 自动同步。
- Model 声明生成确定的数据库 schema，SQLite/PostgreSQL 只负责方言差异。

## 不应照搬的部分

- 不使用字符串 tag、反射、`any`、动态字段 Map 或运行时猜测字段名。
- 不把无界 Text 默认截成 `VARCHAR(255)`；`Text` 表示无界文本，只有显式长度上限才产生 VARCHAR/CHECK。
- 不只在首次建表时处理初始化数据。每条 Seed 必须有稳定身份并可重复应用。
- 不把“字段从目标结构中消失”直接等同于可以删列，也不把任意新增/缺失列猜成重命名。
- 不把本地 `table/` 文件是否存在当作数据库建表或迁移完成标记。新语言将目标 schema 嵌入二进制，实际状态记录在数据库并通过系统目录核对。
- 不允许自定义方法覆盖并改变通用 CRUD 的基本事务、错误和资源合同；自定义操作组合通用查询或参数化 SQL。

## 推荐字段形态

```dever
type User {
  name: Text(1, 64)
  email: Text(254) unique
  bio: Text?
  credit: Decimal(18, 2) default 0
  status: Int >= 0 and <= 2 default 1
}
```

- `Text(N)` 表示最多 N 个 Unicode codepoint，`Text(M, N)` 表示 M 至 N 个；无参数 Text 是无界文本。
- `Bytes(N)` 表示最多 N 个字节，`Bytes(M, N)` 表示 M 至 N 个字节。
- `Decimal(P, S)` 描述数据库精度与小数位，必须满足 `1 <= P <= 34`、`0 <= S <= P`，与 Dever decimal128 的 34 位有效数字上限一致。
- `?` 已经表达可空，不再增加 `null/not null` 两套写法。
- `id` 与 `created_at` 由 Model 隐式生成；关联主键用 `app.model.user.id` 这种字段投影引用。默认值和简单 unique 靠字段元数据；复合索引、无版本 Seed 和命名迁移使用同文件的独立声明，避免把复杂结构塞进一行字段。

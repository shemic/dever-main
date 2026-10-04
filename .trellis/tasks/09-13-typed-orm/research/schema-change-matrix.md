# Model schema 变更分类

## 设计原则

Model 始终声明当前目标 schema。ORM 只保留三种演进机制：

1. 目标 schema diff：处理意图唯一且不会静默丢失数据的变化。
2. `from`：补充 diff 无法推断的旧身份，例如字段重命名。
3. 命名 `migrate`：处理需要业务决定的数据转换、修复和删除。

不为长度、默认值、可空性、索引等每类变化增加单独版本或迁移关键字。SQLite 和 PostgreSQL 的 DDL 差异由 dialect 处理。

## 变更矩阵

| 变化 | Model 写法 | 默认处理 |
| --- | --- | --- |
| 新建表 | 新增 Model 文件 | 直接创建当前最终 schema |
| 新增可空字段 | 新增 `field: Type?` | 自动添加 |
| 新增必填字段并有常量默认值 | 新增 `field: Type default value` | 自动添加并由数据库填充已有行 |
| 新增必填字段但无默认值 | 新增 `field: Type` | 空表可自动添加；有数据时要求 migrate 回填 |
| 字段重命名 | `new_field: Type from old_field` | 旧表执行 rename；新表只创建新字段 |
| 字段重命名并放宽类型约束 | `new_field: WiderType from old_field` | rename 后自动修改类型/约束 |
| 字段重命名并收紧类型约束 | `new_field: NarrowerType from old_field` | 先验证数据；不满足时要求 migrate |
| Text/Bytes 最大长度增加 | 直接修改类型参数 | 自动修改 |
| Text/Bytes 最大长度减小 | 直接修改类型参数 | 先验证现有值长度；不截断，失败时要求 migrate |
| Text/Bytes 最小长度增加 | 直接修改类型参数 | 先验证现有值；失败时要求 migrate |
| Text/Bytes 最小长度减小 | 直接修改类型参数 | 自动放宽约束 |
| Decimal 精度/小数位变化 | 直接修改类型参数 | 仅可精确往返时自动；可能溢出或舍入时要求 migrate |
| 同族无损类型扩大 | 直接修改字段类型 | 按编译器固定无损转换表自动执行 |
| 跨族或可能有损类型转换 | 直接修改字段类型并提供 migrate | 必须明确转换表达式，禁止依赖数据库隐式强转 |
| 必填改可空 | `Type` 改为 `Type?` | 自动移除 NOT NULL |
| 可空改必填 | `Type?` 改为 `Type` | 验证无 NULL；否则要求 migrate 回填 |
| 增加默认值 | 增加 `default value` | 自动修改，只影响后续写入 |
| 修改默认值 | 修改 `default value` | 自动修改，只影响后续写入 |
| 删除默认值 | 删除 `default value` | 自动修改，不改已有数据 |
| 普通索引增加/删除 | 增删 `index` 或 `index(...)` | 自动同步，不改业务数据 |
| 普通索引列或顺序变化 | 修改 `index(...)` | 删除旧索引并创建新索引 |
| 增加唯一约束 | 增加 `unique` 或 `unique(...)` | 先验证无重复；失败时要求 migrate 去重 |
| 删除唯一约束 | 删除 `unique` | 自动删除约束/索引 |
| 普通索引改唯一 | `index` 改为 `unique` | 先验证无重复 |
| 唯一索引改普通 | `unique` 改为 `index` | 自动放宽 |
| 增加外键/修改关联目标 | 改为 `<model-package>.id` | 验证无孤儿记录；失败时要求 migrate |
| 删除外键 | 改为普通字段类型或删除关联 | 自动删除约束；字段数据保留 |
| 修改外键删除行为 | 第一轮使用命名 migrate | 默认 RESTRICT；通用关系选项语法后续设计 |
| choice 增加变体 | 增加变体 | 自动放宽 CHECK |
| choice 修改显示名称 | 修改右侧文本 | 无 schema 迁移，存储值不变 |
| choice 删除稳定变体 | 删除变体 | 验证没有旧值；否则要求 migrate 映射 |
| choice 稳定变体改名 | 第一轮使用命名 migrate | 显式更新已有存储值并重建 CHECK |
| 删除字段 | 从 record 删除并写命名 migrate | 必须显式确认 drop；关联约束和索引按依赖顺序删除 |
| 删除表 | 删除 Model 并写命名 migrate | 必须显式确认，禁止仅因文件消失自动删表 |
| 表/Model 重命名 | 第一轮使用命名 migrate | 显式执行表 rename 并更新 schema identity |
| 字段拆分 | 当前字段 + 命名 migrate | migrate 负责回填多个目标字段 |
| 字段合并 | 当前字段 + 命名 migrate | migrate 负责合并与冲突规则 |
| 主键策略或主键类型变化 | 命名 migrate | 重建引用关系，首轮不自动处理 |
| generated 策略变化 | 直接修改声明并按需 migrate | 只影响新值时自动；需要改已有值时 migrate |
| Seed 新增记录 | 修改 `seed` | 按唯一键只插入缺失记录 |
| Seed 修改/删除已有记录 | 命名 migrate | Seed 不覆盖或删除线上数据 |
| 字段声明顺序变化 | 调整源码顺序 | 不改变已有数据库列顺序，不产生迁移 |
| 注释、标签、自定义方法变化 | 直接修改源码 | 不产生数据库迁移 |

## 长度修改示例

```dever
type User {
  display_name: Text(128) from nickname
}
```

如果旧字段是 `nickname: Text(64)`，规划顺序是先 rename，再把最大长度放宽到 128。全新数据库只创建 `display_name` 的最终定义。

收窄时仍只修改目标类型：

```dever
type User {
  display_name: Text(32) from nickname
}
```

ORM 先验证已有值都不超过 32 个 Unicode codepoint。全部满足就应用约束；存在超长值则整个迁移失败，不做自动截断。开发者必须通过命名 migrate 明确选择截断、拒绝、归档或其它业务规则。

## 方言执行边界

- PostgreSQL 原生支持 rename、类型变更、默认值、可空性和约束变更；类型变更可以使用 `USING` 表达式。
- SQLite 直接 ALTER 能力较少。不能原地完成的同一逻辑变化由 dialect 在事务中创建目标临时表、复制转换后的数据、替换旧表并重建索引/约束。
- 两个方言必须得到相同的规范化目标 schema 和 revision；底层是否重建表不能泄漏到 Dever Model 语法。
- Dever 不使用静默截断、自动填充猜测、名称相似度推断或数据库特有隐式类型转换。

## 不属于首轮通用 Model 的数据库管理能力

分区表、表空间、存储参数、物化视图、触发器、行级安全策略、数据库专有索引方法和在线无锁迁移不进入首轮跨数据库 Model 协议。这些能力需要 PostgreSQL 专有扩展或显式原生迁移，不能伪装成 SQLite/PostgreSQL 通用行为。

# 字段重命名与迁移版本调研

## 官方实现结论

- [Alembic Autogenerate](https://alembic.sqlalchemy.org/en/latest/autogenerate.html) 明确说明无法检测表名和列名变化；它会生成 add/drop 候选，要求开发者审查并改为 rename。
- [Prisma 自定义迁移](https://docs.prisma.io/docs/orm/prisma-migrate/workflows/customizing-migrations) 的字段重命名示例同样默认得到 drop/add，需要先生成草稿迁移，再人工改成 `RENAME COLUMN`。Prisma 用迁移目录和数据库 `_prisma_migrations` 表维护历史。
- [EF Core 管理迁移](https://learn.microsoft.com/en-us/ef/core/managing-schemas/migrations/managing) 说明框架无法判断“删除并新增”和“重命名”的真实意图；生成代码需要改为 `RenameColumn`，否则会丢失旧列数据。
- [Django 迁移操作](https://docs.djangoproject.com/en/5.2/ref/migration-operations/) 提供显式 `RenameField(old_name, new_name)`；[Django 迁移流程](https://docs.djangoproject.com/en/6.0/topics/migrations/) 把迁移文件视为数据库 schema 的版本控制，并在数据库记录应用历史。
- [Atlas 声明式重命名](https://atlasgo.io/changelog/declarative-schema-renames) 使用紧邻目标列的 `renamed_from` 显式表达意图。Atlas 早期依赖交互式询问，但非交互部署无法回答，因此把 rename 意图写回声明式 schema。
- [Atlas 声明式与版本式对比](https://atlasgo.io/concepts/declarative-vs-versioned) 表明声明式流程可以从目标 schema 规划实际数据库差异；版本式流程则保留完整迁移文件历史。两者都不能依靠一个数字版本推导 rename 意图。

## 对 Dever 的结论

- 只增加 Model 数字版本不能解决字段重命名；版本只标识状态先后，不包含旧列与新列的对应关系。
- 不采用字段名相似度自动猜测。相同类型的删一列、加一列既可能是 rename，也可能是两个独立业务变化。
- 简单 rename 采用声明式字段来源：`new_field: Type from old_field`。它比单独 migrate 块短，并把意图放在所属字段旁。
- 编译器为规范化目标 schema 与迁移计划自动产生 revision id/校验值，嵌入二进制；数据库内部 history 记录实际应用状态。开发者不写数字版本。
- `from` 必须保留到所有需要升级的数据库越过该变化；这与 Atlas 把 `renamed_from` 放进 schema 的原则一致。
- `from` 只描述已有表的升级路径。目标表不存在时直接创建当前最终 schema，只创建新字段，不要求旧字段存在，也不先创建旧结构再回放 rename。
- 拆分、合并、跨行回填、删除前归档等没有单字段对应关系的变化，仍需要命名 migrate。该复杂度来自数据转换本身，不能由版本号或启发式推断消除。

# 内容分类模型

内容分类保存标题、标识和状态。

- 包：`content.category.model`
- 公开类型：
  - `CategoryStatus`
  - `Category`
- 公开方法：无
- 使用：无

## 分类状态

状态枚举。

- 类型：`CategoryStatus`
- 分支：
  - `Active`：启用
  - `Disabled`：禁用

```dever
type CategoryStatus {
  Active = "启用"
  Disabled = "禁用"
}
```

## 分类

分类记录。

- 类型：`Category`
- 字段：
  - `name: Text(1, 80)`：名称
  - `slug: Text(1, 100)`：标识
  - `status: CategoryStatus`：状态

```dever
type Category {
  name: Text(1, 80)
  slug: Text(1, 100) unique
  status: CategoryStatus default CategoryStatus.Active index
}
```

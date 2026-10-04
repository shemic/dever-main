# 内容分类应用

分类应用负责创建和停用分类。

- 包：`content.category.app`
- 公开类型：
  - `CategoryView`
- 公开方法：
  - `create`
  - `disable`
  - `require_active`
- 使用：
  - `content.category.create(name)`
  - `content.category.disable(id)`
  - `content.category.require_active(id)`

## 分类视图

返回分类公开字段。

- 类型：`CategoryView`
- 字段：
  - `id: model.id`：记录标识
  - `name: Text`：该声明的业务值
  - `slug: Text`：规范化标识
  - `status: Text`：业务状态

```dever
type CategoryView {
  id: model.id
  name: Text
  slug: Text
  status: Text
}
```

## 创建分类

创建规范化分类。

- 函数：`create`
- 输入：
  - `name: Text`：该声明的业务值
- 输出：
  - `category: CategoryView`：该声明的业务值

```dever
create(name: Text) (category: CategoryView) {
  normalized = domain.normalize_name(name)
  domain.require_name(normalized)
  stored = model.create(
    {
      name = normalized
      slug = domain.normalize_slug(normalized)
    }
  )
  category = CategoryView {
    id = stored.id
    name = stored.name
    slug = stored.slug
    status = "active"
  }
}
```

## 停用分类

停用已有分类。

- 函数：`disable`
- 输入：
  - `id: content.category.model.id`：记录标识
- 输出：
  - `ok: Bool`：该声明的业务值

```dever
disable(id: model.id) (ok: Bool) {
  changed = model.update(
    id,
    {
      status = model.CategoryStatus.Disabled
    }
  )
  discarded = changed
  ok = true
}
```

## require_active

分类应用负责创建和停用分类。

- 函数：`require_active`
- 输入：
  - `id: content.category.model.id?`：记录标识
- 输出：无

```dever
require_active(id: model.id) () {
  stored = model.get(id)
  domain.require_active(stored.status == model.CategoryStatus.Active)
}

require_active(id: null) () {}
```

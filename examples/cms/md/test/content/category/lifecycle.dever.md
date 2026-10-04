# content.category.lifecycle

验证分类创建、规范化、活动状态读取及停用对独立分类的隔离。停用分类的关联拒绝由 HTTP 验收覆盖。

- 包：`content.category.lifecycle`
- 公开类型：无
- 公开方法：无
- 使用：无

## lifecycle

通过真实 App 操作分类，并核对独立分类的标识和可用状态。

- 函数：`lifecycle`
- 输入：无
- 输出：无

```dever
lifecycle() () {
  category = app.create("  Dever News  ")
  assert_eq(category.name, "Dever News")
  assert_eq(category.slug, "dever news")
  assert_eq(category.status, "active")
  app.require_active(category.id)
  app.require_active(null)
  assert(app.disable(category.id))
  independent = app.create("Announcements")
  assert(independent.id != category.id)
  assert_eq(independent.slug, "announcements")
  app.require_active(independent.id)
}
```

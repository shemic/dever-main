# 文章发布测试

验证文章标识和标题规范化、分页边界，以及合法发布条件。数据库发布、权限拒绝和版本冲突由 HTTP 验收覆盖。

- 包：`news.article.publish`
- 公开类型：无
- 公开方法：无
- 使用：无

## 发布

通过真实 Domain 调用核对规范化结果和合法边界。

- 函数：`publish`
- 输入：无
- 输出：无

```dever
publish() () {
  assert_eq(domain.normalize_slug(" News-Article "), "news-article")
  assert_eq(domain.normalize_title("  First article  "), "First article")
  assert_eq(domain.page_size(1), 1)
  assert_eq(domain.page_size(100), 100)
  domain.require_owner(true)
  domain.require_draft(true)
  domain.require_updated(1)
  domain.require_content("news", "Title", "Body")
  domain.require_published(true)
}
```

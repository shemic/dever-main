# user.account.validation

验证账户邮箱和显示名称规范化，包括中文名称。非法请求由 HTTP 验收覆盖。

- 包：`user.account.validation`
- 公开类型：无
- 公开方法：无
- 使用：无

## validation

核对真实 Domain 返回的规范化字段。

- 函数：`validation`
- 输入：无
- 输出：无

```dever
validation() () {
  prepared = domain.prepare(" OWNER@EXAMPLE.COM ", " Owner ")
  assert_eq(prepared.address, "owner@example.com")
  assert_eq(prepared.name, "Owner")
  unicode_name = domain.prepare("editor@example.com", " 编辑员 ")
  assert_eq(unicode_name.address, "editor@example.com")
  assert_eq(unicode_name.name, "编辑员")
}
```

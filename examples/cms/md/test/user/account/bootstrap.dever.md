# 平台初始化测试

在运行器隔离的数据库中建立租户、用户主体和私密凭据，读取所有者成员关系并核对不同租户的标识。

- 包：`user.account.bootstrap`
- 公开类型：无
- 公开方法：无
- 使用：无

## 初始化平台账户

本节定义对应的源码合同。

- 函数：`bootstrap`
- 输入：无
- 输出：无

```dever
bootstrap() () {
  created = user.account.bootstrap(
    "tenant-one",
    "Tenant One",
    " owner@example.com ",
    " Owner ",
    secret("correct horse battery staple")
  )
  user.membership.require_active(created.user_id, created.tenant_id)
  user.account.active(created.user_id)
  tenant = platform.tenant.active("tenant-one")
  assert_eq(tenant.id, created.tenant_id)
  assert_eq(tenant.key, "tenant-one")
  unrelated = platform.tenant.provision("tenant-two", "Tenant Two")
  assert(unrelated.id != created.tenant_id)
}
```

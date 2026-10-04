# 会话生命周期测试

在隔离数据库中核对会话身份字段，执行撤销和虚拟时钟清理，再验证独立新会话。旧会话拒绝由 HTTP 验收覆盖。

- 包：`user.session.lifecycle`
- 公开类型：无
- 公开方法：无
- 使用：无

## lifecycle

通过真实 Session App 核对创建、读取和清理后的独立使用。

- 函数：`lifecycle`
- 输入：无
- 输出：无

```dever
lifecycle() () {
  account = user.account.bootstrap(
    "sessions",
    "Sessions",
    "session@example.com",
    "Session user",
    secret("correct horse battery staple")
  )
  session = app.open(
    "session-subject",
    account.user_id,
    account.tenant_id,
    "sessions",
    "admin"
  )
  authenticated = app.authenticate(session.uuid, "admin")
  assert_eq(authenticated.subject, "session-subject")
  assert_eq(authenticated.user_id, account.user_id)
  assert_eq(authenticated.tenant_id, account.tenant_id)
  assert_eq(authenticated.tenant_key, "sessions")
  app.revoke(session.uuid)
  dever.test.advance_clock(3600000)
  app.cleanup()
  replacement = app.open(
    "new-subject",
    account.user_id,
    account.tenant_id,
    "sessions",
    "front"
  )
  assert(replacement.uuid != session.uuid)
  current = app.authenticate(replacement.uuid, "front")
  assert_eq(current.subject, "new-subject")
  assert_eq(current.user_id, account.user_id)
  assert_eq(current.tenant_id, account.tenant_id)
  assert_eq(current.tenant_key, "sessions")
}
```

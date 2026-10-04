# 会话应用

会话应用创建、验证和撤销站点绑定的数据库会话。

- 包：`user.session.app`
- 公开类型：
  - `SessionView`
  - `AuthSession`
- 公开方法：
  - `open`
  - `authenticate`
  - `revoke`
  - `auth_session`
  - `revoke_found`
  - `cleanup`
- 使用：
  - `user.session.open(subject, account_id, selected_tenant_id, selected_tenant_key, site_key)`
  - `user.session.authenticate(session_id, site_key)`
  - `user.session.revoke(session_id)`
  - `user.session.auth_session(stored)`
  - `user.session.revoke_found(stored)`
  - `user.session.cleanup()`

## 会话视图

本节定义对应的源码合同。

- 类型：`SessionView`
- 字段：
  - `uuid: Uuid`：会话 UUID

```dever
type SessionView {
  uuid: Uuid
}
```

## 认证会话

本节定义对应的源码合同。

- 类型：`AuthSession`
- 字段：
  - `subject: Text`：subject
  - `user_id: user.account.model.principal.id`：用户主体
  - `tenant_id: platform.tenant.model.id`：租户
  - `tenant_key: Text`：租户声明

```dever
type AuthSession {
  subject: Text
  user_id: user.account.model.principal.id
  tenant_id: platform.tenant.model.id
  tenant_key: Text
}
```

## 创建会话

本节定义对应的源码合同。

- 函数：`open`
- 输入：
  - `subject: Text`：subject
  - `account_id: user.account.model.principal.id`：用户主体
  - `selected_tenant_id: platform.tenant.model.id`：租户
  - `selected_tenant_key: Text`：租户键
  - `site_key: Text`：站点
- 输出：
  - `session: SessionView`：会话

```dever
open(
  subject: Text,
  account_id: user.account.model.principal.id,
  selected_tenant_id: platform.tenant.model.id,
  selected_tenant_key: Text,
  site_key: Text
) (session: SessionView) {
  stored = model.create(
    {
      subject = subject
      user_id = account_id
      tenant_id = selected_tenant_id
      tenant_key = selected_tenant_key
      site = site_key
      expires_at = dever.time.add(dever.time.now(), dever.time.duration(3600000))
    }
  )
  session = SessionView {
    uuid = stored.uuid
  }
}
```

## 验证会话

本节定义对应的源码合同。

- 函数：`authenticate`
- 输入：
  - `session_id: Uuid?`：会话 UUID
  - `site_key: Text`：站点
- 输出：
  - `result: AuthSession`：认证会话

```dever
authenticate(session_id: Uuid, site_key: Text) (result: AuthSession) {
  stored = model.first(
    {
      where = uuid == session_id and site == site_key and revoked == false and expires_at > dever.time.now(
      )
    }
  )
  result = auth_session(stored)
}

authenticate(session_id: null, site_key: Text) (result: AuthSession) {
  fail(dever.api.Error.Unauthorized)
}
```

## 撤销会话

本节定义对应的源码合同。

- 函数：`revoke`
- 输入：
  - `session_id: Uuid?`：会话 UUID
- 输出：无

```dever
revoke(session_id: Uuid) () {
  stored = model.first(
    {
      where = uuid == session_id and revoked == false
    }
  )
  revoke_found(stored)
}

revoke(session_id: null) () {
  fail(dever.api.Error.Unauthorized)
}
```

## 映射认证会话

本节定义对应的源码合同。

- 函数：`auth_session`
- 输入：
  - `stored: user.session.model.Session?`：查询结果
- 输出：
  - `result: AuthSession`：认证会话

```dever
auth_session(stored: model.Session) (result: AuthSession) {
  result = AuthSession {
    subject = stored.subject
    user_id = stored.user_id
    tenant_id = stored.tenant_id
    tenant_key = stored.tenant_key
  }
}

auth_session(stored: null) (result: AuthSession) {
  fail(dever.api.Error.Unauthorized)
}
```

## 更新撤销状态

本节定义对应的源码合同。

- 函数：`revoke_found`
- 输入：
  - `stored: user.session.model.Session?`：查询结果
- 输出：无

```dever
revoke_found(stored: model.Session) () {
  changed = model.update(
    stored.id,
    {
      revoked = true
    }
  )
  discarded = changed
}

revoke_found(stored: null) () {
  fail(dever.api.Error.Unauthorized)
}
```

## 清理过期会话

清理已撤销或已过期的会话记录。

- 函数：`cleanup`
- 输入：无
- 输出：无

```dever
cleanup() () {
  removed = model.delete(
    {
      where = revoked == true or expires_at <= dever.time.now()
    }
  )
}
```

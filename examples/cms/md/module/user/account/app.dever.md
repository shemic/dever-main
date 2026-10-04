# 账户应用

账户应用拥有凭据、登录、会话签发和认证身份组装。

- 包：`user.account.app`
- 公开类型：
  - `AccountView`
  - `BootstrapView`
  - `LoginView`
  - `LogoutView`
  - `Identity`
- 公开方法：
  - `bootstrap`
  - `login`
  - `establish_login`
  - `logout`
  - `verify`
  - `active`
  - `current_user`
  - `require_user`
- 使用：
  - `user.account.bootstrap(tenant_key, tenant_name, email, display_name, password)`
  - `user.account.login(credential_id, password, tenant)`
  - `user.account.establish_login(credential_id, tenant, site)`
  - `user.account.logout()`
  - `user.account.verify(claims)`
  - `user.account.active(user_id)`
  - `user.account.current_user()`
  - `user.account.require_user(value)`

## 账户视图

本节定义对应的源码合同。

- 类型：`AccountView`
- 字段：
  - `id: model.principal.id`：用户主体
  - `uuid: Uuid`：身份 UUID
  - `email: Text`：邮箱
  - `display_name: Text`：显示名

```dever
type AccountView {
  id: model.principal.id
  uuid: Uuid
  email: Text
  display_name: Text
}
```

## 初始化结果

本节定义对应的源码合同。

- 类型：`BootstrapView`
- 字段：
  - `tenant_id: platform.tenant.model.id`：租户
  - `user_id: model.principal.id`：用户主体
  - `credential_id: model.id`：凭据 ID

```dever
type BootstrapView {
  tenant_id: platform.tenant.model.id
  user_id: model.principal.id
  credential_id: model.id
}
```

## 登录结果

本节定义对应的源码合同。

- 类型：`LoginView`
- 字段：
  - `account: AccountView`：账户
  - `tenant_id: platform.tenant.model.id`：租户
  - `site: Text`：站点

```dever
type LoginView {
  account: AccountView
  tenant_id: platform.tenant.model.id
  site: Text
}
```

## 登出结果

本节定义对应的源码合同。

- 类型：`LogoutView`
- 字段：
  - `revoked: Bool`：是否撤销

```dever
type LogoutView {
  revoked: Bool
}
```

## 认证身份

本节定义对应的源码合同。

- 类型：`Identity`
- 字段：
  - `id: Text`：JWT subject
  - `user_id: model.principal.id?`：用户主体
  - `tenant_id: platform.tenant.model.id?`：租户

```dever
type Identity {
  id: Text
  user_id: model.principal.id?
  tenant_id: platform.tenant.model.id?
}
```

## 初始化平台账户

本节定义对应的源码合同。

- 函数：`bootstrap`
- 输入：
  - `tenant_key: Text`：租户键
  - `tenant_name: Text`：租户名称
  - `email: Text`：邮箱
  - `display_name: Text`：显示名
  - `password: Secret`：初始密码
- 输出：
  - `result: BootstrapView`：初始化结果

```dever
bootstrap(
  tenant_key: Text,
  tenant_name: Text,
  email: Text,
  display_name: Text,
  password: Secret
) (result: BootstrapView) {
  tenant = platform.tenant.provision(tenant_key, tenant_name)
  input = domain.prepare(email, display_name)
  hash = blocking(dever.crypto.password_hash(password))
  principal = model.principal.create(
    {
      display_name = input.name
    }
  )
  stored = model.create(
    {
      principal_id = principal.id
      email = input.address
      password_hash = hash
    }
  )
  user.membership.grant_owner(principal.id, tenant.id)
  result = BootstrapView {
    tenant_id = tenant.id
    user_id = principal.id
    credential_id = stored.id
  }
}
```

## 登录

本节定义对应的源码合同。

- 函数：`login`
- 输入：
  - `credential_id: user.account.model.id`：凭据 ID
  - `password: Secret`：密码
  - `tenant: Text`：租户键
- 输出：
  - `result: LoginView`：登录结果

```dever
login(credential_id: model.id, password: Secret, tenant: Text) (result: LoginView) {
  domain.require_active(
    model.exists(
      {
        where = id == credential_id
      }
    )
  )
  stored = model.get(credential_id)
  principal = model.principal.get(stored.principal_id)
  selected = platform.tenant.active(tenant)
  domain.require_active(stored.status == model.AccountStatus.Active)
  domain.require_active(principal.status == model.principal.PrincipalStatus.Active)
  domain.require_active(
    blocking(dever.crypto.password_verify(password, stored.password_hash))
  )
  authenticated = establish_login(credential_id, selected.key, dever.site.key())
  dever.auth.issue_cookie(
    authenticated.subject,
    uuid.to_text(authenticated.session_id),
    authenticated.tenant_key
  )
  result = authenticated.result
}
```

## 建立登录会话

密码校验在事务外完成。本函数只负责在短事务内重新检查账户、租户和成员关系，再创建会话。

- 函数：`establish_login`
- 输入：
  - `credential_id: user.account.model.id`：凭据 ID
  - `tenant: Text`：租户键
  - `site: Text`：站点键
- 输出：
  - `result: LoginView`：登录视图
  - `subject: Text`：认证主体
  - `session_id: Uuid`：会话 UUID
  - `tenant_key: Text`：租户键

```dever
transaction establish_login(credential_id: model.id, tenant: Text, site: Text) (
  result: LoginView,
  subject: Text,
  session_id: Uuid,
  tenant_key: Text
) {
  stored = model.get(credential_id)
  principal = model.principal.get(stored.principal_id)
  selected = platform.tenant.active(tenant)
  domain.require_active(stored.status == model.AccountStatus.Active)
  domain.require_active(principal.status == model.principal.PrincipalStatus.Active)
  user.membership.require_active(principal.id, selected.id)
  session = user.session.open(
    uuid.to_text(principal.uuid),
    principal.id,
    selected.id,
    selected.key,
    site
  )
  result = LoginView {
    account = AccountView {
      id = principal.id
      uuid = principal.uuid
      email = stored.email
      display_name = principal.display_name
    }
    tenant_id = selected.id
    site = site
  }
  subject = uuid.to_text(principal.uuid)
  session_id = session.uuid
  tenant_key = selected.key
}
```

## 登出

本节定义对应的源码合同。

- 函数：`logout`
- 输入：无
- 输出：
  - `result: LogoutView`：登出结果

```dever
logout() (result: LogoutView) {
  user.session.revoke(uuid.parse(dever.auth.session()))
  dever.auth.clear_cookie()
  result = LogoutView {
    revoked = true
  }
}
```

## 验证认证声明

本节定义对应的源码合同。

- 函数：`verify`
- 输入：
  - `claims: dever.auth.Claims`：已验签声明
- 输出：
  - `identity: Identity`：可信身份

```dever
verify(claims: dever.auth.Claims) (identity: Identity) {
  session = user.session.authenticate(uuid.parse(claims.session), claims.site)
  domain.require_active(session.subject == claims.subject)
  domain.require_tenant(session.tenant_key, claims.tenant)
  selected = platform.tenant.active(session.tenant_key)
  domain.require_active(selected.id == session.tenant_id)
  active(session.user_id)
  user.membership.require_active(session.user_id, session.tenant_id)
  identity = Identity {
    id = claims.subject
    user_id = session.user_id
    tenant_id = session.tenant_id
  }
}
```

## 验证用户主体

本节定义对应的源码合同。

- 函数：`active`
- 输入：
  - `user_id: user.account.model.principal.id`：用户主体
- 输出：无

```dever
active(user_id: model.principal.id) () {
  principal = model.principal.get(user_id)
  domain.require_active(principal.status == model.principal.PrincipalStatus.Active)
}
```

## 当前用户的类型化标识

读取入口已经验证的可信用户标识，保留 Model ID 类型；不重新查询全局会话数据库，也不接受请求传入的作者 ID。

- 函数：`current_user`
- 输入：无
- 输出：
  - `id: user.account.model.principal.id`：可信用户主体

```dever
current_user() (id: model.principal.id) {
  id = require_user(dever.auth.user_id())
}
```

## 要求可信用户标识

有可信用户时保留其名义类型；没有用户身份时明确拒绝。

- 函数：`require_user`
- 输入：
  - `value: user.account.model.principal.id?`：已验证的可空用户标识
- 输出：
  - `id: user.account.model.principal.id`：可信用户主体

```dever
require_user(value: model.principal.id) (id: model.principal.id) {
  id = value
}

require_user(value: null) (id: model.principal.id) {
  fail(dever.api.Error.Unauthorized)
}
```

# 成员应用

成员应用建立租户关系并验证有效成员。

- 包：`user.membership.app`
- 公开类型：无
- 公开方法：
  - `grant_owner`
  - `require_active`
  - `active`
- 使用：
  - `user.membership.grant_owner(account_id, selected_tenant_id)`
  - `user.membership.require_active(account_id, selected_tenant_id)`
  - `user.membership.active(stored)`

## 授予所有者

本节定义对应的源码合同。

- 函数：`grant_owner`
- 输入：
  - `account_id: user.account.model.principal.id`：用户主体
  - `selected_tenant_id: platform.tenant.model.id`：租户
- 输出：无

```dever
grant_owner(
  account_id: user.account.model.principal.id,
  selected_tenant_id: platform.tenant.model.id
) () {
  stored = model.create(
    {
      user_id = account_id
      tenant_id = selected_tenant_id
      role = model.MembershipRole.Owner
    }
  )
  discarded = stored
}
```

## 验证有效成员

本节定义对应的源码合同。

- 函数：`require_active`
- 输入：
  - `account_id: user.account.model.principal.id`：用户主体
  - `selected_tenant_id: platform.tenant.model.id`：租户
- 输出：无

```dever
require_active(
  account_id: user.account.model.principal.id,
  selected_tenant_id: platform.tenant.model.id
) () {
  stored = model.first(
    {
      where = user_id == account_id and tenant_id == selected_tenant_id and status == model.MembershipStatus.Active
    }
  )
  active(stored)
}
```

## 检查查询结果

本节定义对应的源码合同。

- 函数：`active`
- 输入：
  - `stored: user.membership.model.Membership?`：成员查询结果
- 输出：无

```dever
active(stored: model.Membership) () {}

active(stored: null) () {
  fail(dever.api.Error.Unauthorized)
}
```

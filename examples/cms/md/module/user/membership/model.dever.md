# 租户成员模型

控制库保存用户主体在租户中的角色。

- 包：`user.membership.model`
- 公开类型：
  - `MembershipRole`
  - `MembershipStatus`
  - `Membership`
- 公开方法：无
- 使用：无

## 成员角色

本节定义对应的源码合同。

- 类型：`MembershipRole`
- 分支：
  - `Owner`：所有者
  - `Editor`：编辑
  - `Reader`：读者

```dever
type MembershipRole {
  Owner = "所有者"
  Editor = "编辑"
  Reader = "读者"
}
```

## 成员状态

本节定义对应的源码合同。

- 类型：`MembershipStatus`
- 分支：
  - `Active`：启用
  - `Disabled`：禁用

```dever
type MembershipStatus {
  Active = "启用"
  Disabled = "禁用"
}
```

## 成员记录

本节定义对应的源码合同。

- 类型：`Membership`
- 字段：
  - `user_id: user.account.model.principal.id`：用户主体
  - `tenant_id: platform.tenant.model.id`：租户
  - `role: MembershipRole`：角色
  - `status: MembershipStatus`：状态

```dever
global type Membership {
  user_id: user.account.model.principal.id
  tenant_id: platform.tenant.model.id
  role: MembershipRole index
  status: MembershipStatus default MembershipStatus.Active index
}
```

## 成员唯一约束

本节定义对应的源码合同。

- 声明：`unique(user_id, tenant_id)`

```dever
unique(user_id, tenant_id)
```

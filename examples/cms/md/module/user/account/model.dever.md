# 账户凭据模型

控制库只在该模型保存密码哈希，公开身份由 Principal 承担。

- 包：`user.account.model`
- 公开类型：
  - `AccountStatus`
  - `Account`
- 公开方法：无
- 使用：无

## 凭据状态

本节定义对应的源码合同。

- 类型：`AccountStatus`
- 分支：
  - `Active`：启用
  - `Disabled`：禁用

```dever
type AccountStatus {
  Active = "启用"
  Disabled = "禁用"
}
```

## 账户凭据

本节定义对应的源码合同。

- 类型：`Account`
- 字段：
  - `principal_id: user.account.model.principal.id`：用户主体
  - `email: Text(254)`：登录邮箱
  - `private password_hash: Text(255)`：私密密码哈希
  - `status: AccountStatus`：凭据状态

```dever
global type Account {
  principal_id: user.account.model.principal.id unique
  email: Text(254) unique
  private password_hash: Text(255)
  status: AccountStatus default AccountStatus.Active index
}
```

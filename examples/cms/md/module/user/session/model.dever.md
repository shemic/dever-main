# 会话模型

控制库保存站点绑定、租户绑定且可撤销的登录会话。

- 包：`user.session.model`
- 公开类型：
  - `Session`
- 公开方法：无
- 使用：无

## 会话记录

本节定义对应的源码合同。

- 类型：`Session`
- 字段：
  - `uuid: Uuid`：会话标识
  - `subject: Text(1, 128)`：JWT subject
  - `user_id: user.account.model.principal.id`：用户主体
  - `tenant_id: platform.tenant.model.id`：租户
  - `tenant_key: Text(1, 64)`：JWT 租户声明
  - `site: Text(1, 32)`：站点
  - `expires_at: DateTime`：过期时间
  - `revoked: Bool`：撤销状态

```dever
global type Session {
  uuid: Uuid generated unique
  subject: Text(1, 128)
  user_id: user.account.model.principal.id
  tenant_id: platform.tenant.model.id
  tenant_key: Text(1, 64)
  site: Text(1, 32)
  expires_at: DateTime index
  revoked: Bool default false index
}
```

## 会话查询索引

本节定义对应的源码合同。

- 声明：`index(user_id, tenant_id, site)`

```dever
index(user_id, tenant_id, site)
```

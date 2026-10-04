# 租户模型

控制库保存租户目录，租户业务数据不放在本表。

- 包：`platform.tenant.model`
- 公开类型：
  - `TenantStatus`
  - `Tenant`
- 公开方法：无
- 使用：无

## 租户状态

本节定义对应的源码合同。

- 类型：`TenantStatus`
- 分支：
  - `Active`：启用
  - `Disabled`：禁用

```dever
type TenantStatus {
  Active = "启用"
  Disabled = "禁用"
}
```

## 租户记录

本节定义对应的源码合同。

- 类型：`Tenant`
- 字段：
  - `uuid: Uuid`：稳定标识
  - `key: Text(1, 64)`：租户键
  - `name: Text(1, 100)`：租户名称
  - `status: TenantStatus`：状态

```dever
global type Tenant {
  uuid: Uuid generated unique
  key: Text(1, 64) unique
  name: Text(1, 100)
  status: TenantStatus default TenantStatus.Active index
}
```

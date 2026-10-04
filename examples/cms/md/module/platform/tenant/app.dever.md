# 租户应用

提供平台初始化和登录期租户选择。

- 包：`platform.tenant.app`
- 公开类型：
  - `TenantView`
- 公开方法：
  - `provision`
  - `active`
  - `active_view`
- 使用：
  - `platform.tenant.provision(key, name)`
  - `platform.tenant.active(tenant_key)`
  - `platform.tenant.active_view(stored)`

## 租户视图

本节定义对应的源码合同。

- 类型：`TenantView`
- 字段：
  - `id: model.id`：租户 ID
  - `key: Text`：租户键
  - `name: Text`：名称

```dever
type TenantView {
  id: model.id
  key: Text
  name: Text
}
```

## 创建租户

本节定义对应的源码合同。

- 函数：`provision`
- 输入：
  - `key: Text`：租户键
  - `name: Text`：名称
- 输出：
  - `tenant: TenantView`：租户

```dever
provision(key: Text, name: Text) (tenant: TenantView) {
  stored = model.create(
    {
      key = domain.normalize_key(key)
      name = domain.normalize_name(name)
    }
  )
  tenant = TenantView {
    id = stored.id
    key = stored.key
    name = stored.name
  }
}
```

## 查找启用租户

本节定义对应的源码合同。

- 函数：`active`
- 输入：
  - `tenant_key: Text`：租户键
- 输出：
  - `tenant: TenantView`：租户

```dever
active(tenant_key: Text) (tenant: TenantView) {
  stored = model.first(
    {
      where = key == domain.normalize_key(tenant_key)
    }
  )
  tenant = active_view(stored)
}
```

## 映射启用租户

本节定义对应的源码合同。

- 函数：`active_view`
- 输入：
  - `stored: platform.tenant.model.Tenant?`：查询结果
- 输出：
  - `tenant: TenantView`：租户

```dever
active_view(stored: model.Tenant) (tenant: TenantView) {
  domain.require_active(stored.status == model.TenantStatus.Active)
  tenant = TenantView {
    id = stored.id
    key = stored.key
    name = stored.name
  }
}

active_view(stored: null) (tenant: TenantView) {
  fail(dever.api.Error.Unauthorized)
}
```

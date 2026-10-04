# 后台授权接口

后台接口管理当前租户、当前站点的角色授权。

- 包：`user.authorization.api.admin.manage`
- 公开类型：无
- 公开方法：无
- 使用：无

## 权限目录

本节声明当前站点权限目录接口。

- 声明：`get permissions`

```dever
get permissions = app.permissions
```

## 保存角色

本节声明保存当前站点角色及权限集合的接口。

- 声明：`post save`

```dever
post save = app.save
```

## 授予角色

本节声明向用户授予当前站点角色的接口。

- 声明：`post grant`

```dever
post grant = app.grant
```

## 撤销角色

本节声明从用户撤销当前站点角色的接口。

- 声明：`delete revoke`

```dever
delete revoke = app.revoke
```

## 禁用角色

本节声明禁用当前站点角色的接口。

- 声明：`delete disable`

```dever
delete disable = app.disable
```

# 授权管理应用

授权管理应用只适配 Dever 核心角色存储，不保存业务权限副本。

- 包：`user.authorization.app`
- 公开类型：无
- 公开方法：
  - `permissions`
  - `save`
  - `grant`
  - `revoke`
  - `disable`
- 使用：
  - `user.authorization.permissions()`
  - `user.authorization.save(id, name, all_permissions, permission_keys)`
  - `user.authorization.grant(user_id, role_id)`
  - `user.authorization.revoke(user_id, role_id)`
  - `user.authorization.disable(role_id)`

## 权限目录

本节定义当前站点权限目录的读取入口。

- 函数：`permissions`
- 输入：无
- 输出：
  - `values: List<dever.auth.Permission>`：当前站点权限目录

```dever
permissions() (values: List<dever.auth.Permission>) {
  values = dever.auth.permissions()
}
```

## 保存角色

本节定义当前站点角色及其权限集合的保存入口。

- 函数：`save`
- 输入：
  - `id: Text`：角色 ID
  - `name: Text`：角色名称
  - `all_permissions: Bool`：是否拥有当前站点全部权限
  - `permission_keys: List<Text>`：权限键
- 输出：
  - `ok: Bool`：保存成功

```dever
save(id: Text, name: Text, all_permissions: Bool, permission_keys: List<Text>) (
  ok: Bool
) {
  dever.auth.save_role(id, name, all_permissions, permission_keys)
  ok = true
}
```

## 授予角色

本节定义向用户授予当前站点角色的入口。

- 函数：`grant`
- 输入：
  - `user_id: Int`：用户 ID
  - `role_id: Text`：角色 ID
- 输出：
  - `ok: Bool`：授予成功

```dever
grant(user_id: Int, role_id: Text) (ok: Bool) {
  dever.auth.grant_role(user_id, role_id)
  ok = true
}
```

## 撤销角色

本节定义从用户撤销当前站点角色的入口。

- 函数：`revoke`
- 输入：
  - `user_id: Int`：用户 ID
  - `role_id: Text`：角色 ID
- 输出：
  - `ok: Bool`：撤销成功

```dever
revoke(user_id: Int, role_id: Text) (ok: Bool) {
  dever.auth.revoke_role(user_id, role_id)
  ok = true
}
```

## 禁用角色

本节定义禁用当前站点角色的入口。

- 函数：`disable`
- 输入：
  - `role_id: Text`：角色 ID
- 输出：
  - `ok: Bool`：禁用成功

```dever
disable(role_id: Text) (ok: Bool) {
  dever.auth.disable_role(role_id)
  ok = true
}
```

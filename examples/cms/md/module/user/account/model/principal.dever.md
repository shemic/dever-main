# 用户主体模型

Principal 是会话、成员关系和认证 Identity 共用的公开用户身份。

- 包：`user.account.model.principal`
- 公开类型：
  - `PrincipalStatus`
  - `Principal`
- 公开方法：无
- 使用：无

## 主体状态

本节定义对应的源码合同。

- 类型：`PrincipalStatus`
- 分支：
  - `Active`：启用
  - `Disabled`：禁用

```dever
type PrincipalStatus {
  Active = "启用"
  Disabled = "禁用"
}
```

## 用户主体

本节定义对应的源码合同。

- 类型：`Principal`
- 字段：
  - `uuid: Uuid`：身份标识
  - `display_name: Text(1, 64)`：显示名称
  - `status: PrincipalStatus`：状态

```dever
global type Principal {
  uuid: Uuid generated unique
  display_name: Text(1, 64)
  status: PrincipalStatus default PrincipalStatus.Active index
}
```

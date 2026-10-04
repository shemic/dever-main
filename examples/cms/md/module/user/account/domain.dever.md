# 账户规则

账户输入和认证断言集中在纯 Domain。

- 包：`user.account.domain`
- 公开类型：无
- 公开方法：无
- 使用：无

## 账户输入

本节定义对应的源码合同。

- 类型：`AccountInput`
- 字段：
  - `address: Text`：邮箱
  - `name: Text`：显示名

```dever
type AccountInput {
  address: Text
  name: Text
}
```

## 规范邮箱

本节定义对应的源码合同。

- 函数：`normalize_email`
- 输入：
  - `email: Text`：邮箱
- 输出：
  - `address: Text`：规范邮箱

```dever
normalize_email(email: Text) (address: Text) pure {
  address = text.lower(text.trim(email))
}
```

## 规范显示名

本节定义对应的源码合同。

- 函数：`normalize_name`
- 输入：
  - `display_name: Text`：显示名
- 输出：
  - `name: Text`：规范名称

```dever
normalize_name(display_name: Text) (name: Text) pure {
  name = text.trim(display_name)
}
```

## 准备账户输入

本节定义对应的源码合同。

- 函数：`prepare`
- 输入：
  - `email: Text`：邮箱
  - `display_name: Text`：显示名
- 输出：
  - `input: AccountInput`：规范输入

```dever
prepare(email: Text, display_name: Text) (input: AccountInput) pure {
  address = normalize_email(email)
  name = normalize_name(display_name)
  require(
    text.contains(address, "@") and not text.starts_with(address, "@") and not text.ends_with(
      address,
      "@"
    ),
    "email must contain a local part and domain"
  )
  require(not text.is_empty(name), "display name cannot be blank")
  input = AccountInput {
    address = address
    name = name
  }
}
```

## 检查输入

本节定义对应的源码合同。

- 函数：`require`
- 输入：
  - `valid: Bool`：是否有效
  - `message: Text`：内部原因
- 输出：无

```dever
require(valid: true, message: Text) () pure {}

require(valid: false, message: Text) () pure {
  fail(dever.api.Error.Invalid)
}
```

## 检查认证状态

本节定义对应的源码合同。

- 函数：`require_active`
- 输入：
  - `active: Bool`：是否有效
- 输出：无

```dever
require_active(active: true) () pure {}

require_active(active: false) () pure {
  fail(dever.api.Error.Unauthorized)
}
```

## 检查租户声明

本节定义对应的源码合同。

- 函数：`require_tenant`
- 输入：
  - `expected: Text`：可信租户键
  - `actual: Text?`：JWT 租户声明
- 输出：无

```dever
require_tenant(expected: Text, actual: Text) () pure {
  require_active(expected == actual)
}

require_tenant(expected: Text, actual: null) () pure {
  fail(dever.api.Error.Unauthorized)
}
```

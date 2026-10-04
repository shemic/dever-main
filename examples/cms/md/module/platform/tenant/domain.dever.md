# 租户规则

租户键、名称和状态规则只在本领域使用。

- 包：`platform.tenant.domain`
- 公开类型：无
- 公开方法：无
- 使用：无

## 规范租户键

本节定义对应的源码合同。

- 函数：`normalize_key`
- 输入：
  - `value: Text`：原值
- 输出：
  - `key: Text`：规范值

```dever
normalize_key(value: Text) (key: Text) pure {
  key = text.lower(text.trim(value))
}
```

## 规范租户名

本节定义对应的源码合同。

- 函数：`normalize_name`
- 输入：
  - `value: Text`：原值
- 输出：
  - `name: Text`：规范值

```dever
normalize_name(value: Text) (name: Text) pure {
  name = text.trim(value)
}
```

## 检查启用状态

本节定义对应的源码合同。

- 函数：`require_active`
- 输入：
  - `active: Bool`：是否启用
- 输出：无

```dever
require_active(active: true) () pure {}

require_active(active: false) () pure {
  fail(dever.api.Error.Unauthorized)
}
```

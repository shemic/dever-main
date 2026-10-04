# 内容分类规则

分类名称和标识在领域规则中统一规范化。

- 包：`content.category.domain`
- 公开类型：无
- 公开方法：无
- 使用：无

## 规范名称

去除分类名称两端空白。

- 函数：`normalize_name`
- 输入：
  - `value: Text`：该声明的业务值
- 输出：
  - `name: Text`：该声明的业务值

```dever
normalize_name(value: Text) (name: Text) pure {
  name = text.trim(value)
}
```

## 规范标识

统一分类标识大小写。

- 函数：`normalize_slug`
- 输入：
  - `value: Text`：该声明的业务值
- 输出：
  - `slug: Text`：规范化标识

```dever
normalize_slug(value: Text) (slug: Text) pure {
  slug = text.lower(text.trim(value))
}
```

## 检查名称

拒绝空分类名称。

- 函数：`require_name`
- 输入：
  - `value: Text`：该声明的业务值
- 输出：无

```dever
require_name(value: Text) () pure {
  check_name(value != "")
}
```

## 检查名称结果

有效名称继续执行，无效名称返回标准错误。

- 函数：`check_name`
- 输入：
  - `value: Bool`：该声明的业务值
- 输出：无

```dever
check_name(value: true) () pure {}

check_name(value: false) () pure {
  fail(dever.api.Error.Invalid)
}
```

## require_active

分类名称和标识在领域规则中统一规范化。

- 函数：`require_active`
- 输入：
  - `active: Bool`：该声明的业务值
- 输出：无

```dever
require_active(active: true) () pure {}

require_active(active: false) () pure {
  fail(dever.api.Error.Conflict)
}
```

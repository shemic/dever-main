# 文章规则

文章字段规范化、分页边界和发布状态规则集中在纯 Domain。

- 包：`news.article.domain`
- 公开类型：无
- 公开方法：无
- 使用：无

## 规范文章标识

本节定义对应的源码合同。

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

## 规范标题

本节定义对应的源码合同。

- 函数：`normalize_title`
- 输入：
  - `value: Text`：该声明的业务值
- 输出：
  - `title: Text`：文章标题

```dever
normalize_title(value: Text) (title: Text) pure {
  title = text.trim(value)
}
```

## 限制分页大小

本节定义对应的源码合同。

- 函数：`page_size`
- 输入：
  - `value: Int`：该声明的业务值
- 输出：
  - `size: Int`：受限页大小

```dever
page_size(value: Int >= 1 and <= 100) (size: Int) pure {
  size = value
}

page_size(value: other) (size: Int) pure {
  fail(dever.api.Error.Invalid)
}
```

## 检查草稿状态

本节定义对应的源码合同。

- 函数：`require_draft`
- 输入：
  - `draft: Bool`：该声明的业务值
- 输出：无

```dever
require_draft(draft: true) () pure {}

require_draft(draft: false) () pure {
  fail(dever.api.Error.Conflict)
}
```

## 检查文章所有权

发布操作只允许文章作者执行。

- 函数：`require_owner`
- 输入：
  - `owned: Bool`：该声明的业务值
- 输出：无

```dever
require_owner(owned: true) () pure {}

require_owner(owned: false) () pure {
  fail(dever.api.Error.Forbidden)
}
```

## require_published

文章字段规范化、分页边界和发布状态规则集中在纯 Domain。

- 函数：`require_published`
- 输入：
  - `published: Bool`：该声明的业务值
- 输出：无

```dever
require_published(published: true) () pure {}

require_published(published: false) () pure {
  fail(dever.api.Error.NotFound)
}
```

## require_updated

文章字段规范化、分页边界和发布状态规则集中在纯 Domain。

- 函数：`require_updated`
- 输入：
  - `changed: Int`：条件更新影响行数
- 输出：无

```dever
require_updated(changed: 1) () pure {}

require_updated(changed: other) () pure {
  fail(dever.api.Error.Conflict)
}
```

## require_content

文章字段规范化、分页边界和发布状态规则集中在纯 Domain。

- 函数：`require_content`
- 输入：
  - `slug: Text`：规范化标识
  - `title: Text`：文章标题
  - `body: Text`：正文
- 输出：无

```dever
require_content(slug: Text, title: Text, body: Text) () pure {
  require_valid(slug != "" and title != "" and text.trim(body) != "")
}
```

## require_valid

文章字段规范化、分页边界和发布状态规则集中在纯 Domain。

- 函数：`require_valid`
- 输入：
  - `valid: Bool`：该声明的业务值
- 输出：无

```dever
require_valid(valid: true) () pure {}

require_valid(valid: false) () pure {
  fail(dever.api.Error.Invalid)
}
```

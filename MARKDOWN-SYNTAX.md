# Dever Markdown 源码规范

安全数据合同与 `.dever` 完全一致：Model 私有字段在字段说明和代码中均写 `private password_hash: Text(255)`；App 通过显式 View 隐藏完整 Model。`Secret` 使用同一个内置标量和静态非观察检查，不能通过 Markdown 注释或文档元信息获得额外权限。Test source 的 `secret(Text)` 同样由测试源身份授权。

`.dever.md` 用 Markdown 保存可读说明，用顶层 `dever` 或 `typescript dever` 围栏保存 Dever 代码。代码仍由 Dever 编译器检查和执行；`typescript dever` 只用于请求编辑器采用 TypeScript 语法高亮。

完整项目示例见 [CMS](examples/cms/md/module/news/article/api/admin/manage.dever.md)；旧的[购物车结算](examples/old/markdown/module/main.dever.md)仍保留供语言回归使用。

## 源码说明

文件必须有且只有一个一级标题。标题使用自然语言名称，标题下依次写源码用途、路径推导的源码身份、公开类型、公开方法和调用方式：

````markdown
# 账户接口

向调用方返回一条问候语。

- 包：`user.account.api`
- 公开类型：无
- 公开方法：无
- 使用：无
````

源码身份由文件相对源码根的路径确定：`module/user/account/api.dever.md` 是 `user.account.api`，`module/user/account/app.dever.md` 是 `user.account.app`，`test/user/account/ensure_editor.dever.md` 是 `user.account.ensure_editor`。App 中的类型和方法自动列入公开清单，调用方式隐藏 role，写作 `user.account.function(input_name, ...)`；`app/<topic>.dever.md` 的 topic 也不进入调用名。Model 主记录和 choice 自动列入公开类型。Domain、Adapter、API 和测试没有公开方法。没有内容时必须写 `无`。同名函数有多个参数数量时，每个签名各写一条调用方式。

Markdown 应用测试固定使用 `test/<component>/<domain>/<topic>.dever.md`。文件必须用一个函数章节记录与 `<topic>` 同名的零输入、零输出普通函数；可在其他章节记录只供本文件使用的私有辅助声明。代码块内可使用测试专用的 `assert(...)` 和 `assert_eq(actual, expected)`，文档结构、函数输入输出和公开清单仍按本规范校验，不存在测试专用的简化格式。

Markdown 与普通源码使用同一目录约束：Model/API 主文件可与其 topic 目录共存；其他 role 二选一且单 topic 应并回主文件。非 API topic 目录只有一层，只有 `api/` 可递归。应用代码块不写 `package`、`exposes` 或 `internal`；`public` 只能修饰需要匿名访问的单条 HTTP API 声明。

一级标题下不能放 Dever 代码块；每个 Dever 声明放在自己的二级标题下。

API 章节用 `- 声明：` 标出 `[public] get <action>`、`[public] post <action>`、`[public] put <action>`、`[public] delete <action>`、`cmd <action>` 或 `rest [model.topic]`，代码块中的声明与说明须一致。`public` 不适用于 CMD 或 REST。API 只绑定本领域 App 或声明自动 REST，不写处理函数体。

## 类型说明

每个 type 使用一个二级标题。标题使用自然语言名称，随后写用途、技术名称、字段或分支，以及唯一的 type 代码块：

````markdown
## 用户

保存用户的显示名称和年龄。

- 类型：`User`
- 字段：
  - `name: Text`：显示名称
  - `age: Int >= 0`：年龄

```typescript dever
type User {
  name: Text
  age: Int >= 0
}
```
````

record 使用 `字段`；choice 使用 `分支`，例如：

```markdown
- 分支：
  - `Found(user: User)`：找到用户
  - `error Failed(message: Text)`：查询失败
```

空 record 写 `- 字段：无`。字段或分支的名称、类型、约束、可见性、顺序和数量必须与代码一致。每项在全角冒号 `：` 后写非空的自然语言说明。

## 函数说明

每个逻辑函数使用一个二级标题。标题使用自然语言名称，随后写用途、技术名称、输入、输出，以及唯一的函数代码块：

````markdown
## 生成问候语

根据姓名生成问候语。

- 函数：`greet`
- 输入：
  - `name: Text`：需要问候的人
- 输出：
  - `message: Text`：生成的问候语

```typescript dever
greet(name: Text) (message: Text) {
  message = "你好，" + name
}
```
````

没有输入或输出时必须写 `- 输入：无` 或 `- 输出：无`。输入和输出使用编译器解析后的统一函数签名，名称、类型、顺序和数量必须一致。条件分句中的字面量和分支模式仍记录为统一类型，例如 `true` 和 `false` 分句的输入都写 `value: Bool`。

同一函数、同一参数数量的所有分句必须放在同一个二级标题和同一个代码块内，并且各分句的输入参数名必须一致。不同函数、不同参数数量的重载或 type 必须分别使用二级标题。

## Model 声明

`module/<component>/<domain>/model.dever.md` 定义简单领域 Model；复杂领域使用 `model/<topic>.dever.md`，记录名分别与 domain 或 topic 对应。类型字段说明保留存储参数，例如 `Text(1, 64)`、`Decimal(12, 2)`；`global`、默认值、索引以及 REST 的 `owner/create/replace/search` 修饰符由代码检查，不在字段清单中重复。

database、index、unique、relation、seed、migrate、sql 每个声明使用独立二级标题、用途说明和 `- 声明：` 元数据。例如：

````markdown
## 初始化管理员

按唯一邮箱创建初始用户。

- 声明：`seed`

```dever
seed {
  { email = "admin@example.com" }
}
```
````

声明标识分别为 `database primary`、`index(status, created_at)`、`unique(email, name)`、`relation articles = news.article.model.author_id`、`seed`、`migrate remove_legacy`、`sql lookup`。SQL 声明还需要按函数说明的格式列出输入、输出。编译器核对声明标识、参数顺序与类型；说明不能替代代码的存储校验。

## Job 声明

`job.dever.md` 或 `job/<topic>.dever.md` 使用同一 Job 语法，公开类型和方法清单均为空。Job 段仍写 `- 函数：`、输入和输出元信息；代码中的 `job`、`retry(n)`、`timeout(ms)` 由语言检查，不在文档中重复配置。payload 类型属于同领域 App。

`database` 与 `schedule` 各占一个二级标题，声明标识分别为 `database default`、`schedule cleanup`：

````markdown
## 定时清理

每个 UTC 整点调度一次。

- 声明：`schedule cleanup`

```dever
schedule cleanup = "0 * * * *"
```
````

Job 的字段、调用权限、持久化和测试控制均与 `.dever` 一致；Markdown 不扩大公开性或 effect 权限。

## 通用规则

Port 的函数段在输出之后增加 `- 允许失败：\`app.DeliveryError\``，必须与 bodyless 签名的 `fails` 类型一致。Adapter/fake 函数名完整写为 `port.send` 或 `port.mail.send`；公开方法清单仍为空。Adapter 的 `setting { ... }` 使用普通类型段格式：`- 类型：\`Setting\`` 和完整字段清单；编译器按该 Adapter 私有记录核对字段。

- 源码和每个声明的用途说明不能为空。输入、输出、字段和分支的逐项说明也不能为空。
- 一级标题不能有顶层 Dever 代码块，每个二级标题恰好对应一个；声明不能跨代码块，也不能重复出现在多个标题下。
- `dever` 与 `typescript dever` 的含义相同。围栏必须符合 CommonMark、显式闭合，并且被 CommonMark 解析为顶层代码块。
- 三级及更深标题、表格、普通段落和其它语言代码块可用于补充说明，不参与程序执行。补充内容放在所属 Dever 代码块之后。
- `.dever` 文件继续使用原语法，不要求 Markdown 说明。
- `fmt` 只改 Dever 代码块内部；`check`、`api`、`run` 和 `build` 同时检查代码及文档契约。

结构错误使用 M004–M005；源码身份、公开清单或声明名不一致使用 M006；函数输入不一致使用 M007；函数输出、类型字段或分支不一致使用 M008。签名错误会同时标出 Markdown 条目和对应 Dever 声明。

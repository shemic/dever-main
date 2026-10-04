# Dever 生产 API 边界

## 2026-09-28 自动权限与 database-only 租户修订

本节替代下文所有关于手写 `auth.require`、verify 返回权限列表、框架不提供 RBAC、`tenant.mode`、`tenant.control_database`、table/field 租户模式和 Job capability 列表的旧合同。

- `public` 是唯一匿名入口。所有非 public HTTP API 默认同时要求可信身份和当前接口权限；源码不得手写权限字符串或调用 `dever.auth.require`。
- 编译器根据 `<component>/<domain>/api/<site>/**` 和 API 声明自动生成权限目录。权限 key 为 `component.domain.site.action`；HTTP method 只保存为元数据，不进入 key。`rest` 自动生成 `read/create/replace/delete`，自定义 API 使用声明 action。public API 不生成权限项。
- 权限目录由构建产物携带，并在应用初始化时幂等同步到控制数据库 `_dever_permission`；移除的 API 标记 inactive，不直接删除授权历史。源码、组件和业务代码不注册权限。
- 官方 auth runtime 提供租户内 `_dever_auth_role`、`_dever_auth_role_permission`、`_dever_auth_user_role`。用户只通过角色取得权限，多角色取并集；不支持直接用户授权、deny、角色继承或权限优先级。`all_permissions` 仅覆盖同一站点的 active 权限。
- verify 只建立 `id/user_id/tenant_id` 可信身份，不返回权限列表。Runtime 针对当前 permission key 做索引化 EXISTS 查询，不把大量权限写入 JWT、Session 或请求上下文。`dever.auth` 只公开身份读取及签发/清理凭据能力。
- API 权限只决定能否调用动作。资源归属、状态流转和其它数据权限仍由 App 读取可信身份后调用纯 Domain 规则；Domain 不读取隐式身份。未认证返回 401，接口权限或业务授权失败返回 403。
- 多租户只保留 database 隔离。配置改为 `tenant.database`，删除 `tenant.mode` 和 `tenant.control_database`，不保留兼容分支。
- 编译器自动识别组件：拥有 tenant Model 或 tenant Job 的组件属于租户组件，其余为平台组件；混合组件按租户组件处理。租户组件默认启用，控制库 `_dever_tenant_component` 只保存禁用例外。入口携带静态可达组件集合，Runtime 在 API/CMD/Job 入口统一检查。
- User Job 保存可重新验证的身份引用和一个发起操作 permission key，不保存权限集合；每次执行前重新 verify，并按当前角色关系复核该 permission。旧 capability schema 的活跃 User Job 在迁移时阻断，不能降级为 System。
- `_dever_permission` 和 `_dever_tenant_component` 是 Runtime 私有控制表；角色表由官方 auth runtime 统一拥有。项目仍拥有 User、Session、Tenant 业务 Model。

## 2026-09-26 已批准修订

用户已批准在备份后实施完整站点、可信身份、多用户权限和多租户方案。本节替代下文涉及身份、租户、入口配置的旧限制；尚未完成的字段绑定、Cookie、标准错误和上传仍属任务，不因新增范围而消失。

- `sites.<key>.path` 是每个组件/领域 `api/` 下的相对源码目录，递归包含子目录；不是 URL 前缀。`admin` 只匹配 `api/admin/**`，不能匹配同名 `api/admin.dever`。URL 保持现有组件/领域/API topic 生成规则，不增加源码 `site` 声明。站点匹配遗漏、歧义和路由冲突在服务开始前拒绝。
- `public` 只修饰 HTTP API 声明，普通接口默认认证。站点选择配置中的认证 Provider，Host 可进一步约束入口；客户端提供的站点/用户/租户值不能自行成为可信身份。
- Provider 验证签名和站点凭据后，调用静态检查的业务 App verify，检查账号、会话、成员关系并建立只读 `site.key`、`auth.id/user_id/tenant_id`。优先复用已有账号领域，不要求新建 `auth` 领域或 `admin` 组件。
- 身份、接口权限和数据范围分别检查。接口权限由 Runtime 自动校验，数据范围由业务 App/Domain 处理，所有入口不能绕过对应边界。
- 开启多租户后，普通持久化 Model 属于当前租户，`global` Model 使用平台库；只支持 database 分库。不影响普通 DTO，不因缺租户而回退平台库。CRUD、REST、统计、关联和批量路径共用隔离合同。
- 租户连接池有界且可回收；初始化/迁移属于受控租户生命周期。跨库引用保留类型但不能隐式建立物理外键/Join；自动事务只覆盖一个物理数据库。隔离模式切换必须经过数据迁移。
- 租户功能开通和规则配置属于业务数据；实现差异复用已编译 Port/Adapter。禁止租户 ID 分支、复制整套项目或执行数据库内代码。
- Job 原子入队并保存受控租户/执行身份，执行时恢复并复核资格；CMD 不能通过自报用户 ID 伪造身份。日志关联站点/身份/租户而不泄漏凭据。
- 所有部署配置仅来自 `config/setting.json`，无环境变量。保留并应用 log，runtime 可选并按能力选择服务，移除无用途 app 配置及迁移其工具消费者。
- 同步 Dever/Markdown CMS，验证站点目录、跨站凭据、两个租户相同记录 ID、不同角色、匿名边界、Job 上下文、事务回滚和平台共享数据。

本轮已完成并逐项比对的修改前备份：`/data/project/dever-backups/pre-auth-tenancy-20260926-rm6kL6/dever.tar.gz`。SHA-256：`e4f0666d054ca241484e8333c16270f039a26c3c616b7ddd0f0bea50476ca9b7`。

## Goal

在保持固定 JSON envelope 的前提下，以 Model + `rest` 覆盖受约束的普通 CRUD，以显式 API 动词绑定 App 业务动作；同时让 API 安全承载结构化请求、会话 Cookie、显式业务错误、隐藏请求上下文和受限文件上传。本任务正在实施，以下是完整目标合同；已实现的子集以 implement.md 为准。

## Dependencies

- `secure-data-contracts`：Secret Cookie/token 和私密输出拒绝。
- `time-typed-codec`：结构化请求/响应和完整标量 codec。
- `port-adapter`：上传后的对象存储由业务 App 调用 storage Port。

## Requirements

- API 使用 `get`、`post`、`put`、`delete`、`cmd` 和 `rest` 声明；移除 `get_`/`post_`/`delete_` 前缀语法，不加入 `patch`。同领域唯一主 Model 可由无参数 `rest` 推导，自动路径为 `/<component>/<domain>`；自定义动作以 `post publish = app.publish` 等形式绑定 App 签名。
- 纯 CRUD 无需 App；编译器生成的 `rest` 才可直接降低到同领域 Model 操作。手写 API 不直接调用 Model；需要业务处理时调用同领域 App。App 中所有函数都是领域公开能力，私有辅助逻辑归 Domain，不增加 `expose` 或 `crud ... on model` 声明。
- Model 普通字段按名称推导 CRUD 输入输出；只有例外才声明 `create`、`replace`、`search` 或 `owner` 字段绑定。`owner <field> = <context-read expression>` 为自动 REST 创建赋值及 GET/PUT/DELETE 行范围，客户端不能覆盖。字段绑定表达式只能使用静态类型可检查的纯计算或只读上下文能力，禁止数据库、网络和写 Effect；它们不隐式作用于手写 App 的 Model 调用。
- `get` 调用链禁止数据库写 Effect；`post`、`put`、`delete` 写调用链默认同一事务。`cmd`、Job 的数据库写入也按 Effect 建立事务，不依赖 HTTP 动词。无需在 App 源码重复声明 `transaction`。
- 源码身份固定为 `<component>/<domain>/<role>.dever[.md]`，role 只接受 `app/domain/model/port/adapter/api/job`。一个领域优先每个角色一个文件；按主题拆分时非 API 主题只允许一层。App、Domain、Port、Adapter、Job 的根文件与主题目录互斥；主 `model.dever` 可与附属 `model/<topic>.dever` 共存，基础 `api.dever` 可与路径分组 `api/**` 共存。主题必须表达业务含义，不按函数机械拆文件；搬动 Model 文件可能改变持久化身份，不能作为无迁移的整理。
- `deverc run/build` 的目标入口由编译器生成，不要求应用编写 `main.dever`。无命令参数时按 `config/setting.json` 启动 API/worker；指定 CMD 时命令行 JSON 由该 CMD 的静态签名解码。没有可运行入口时报错；部署选择继续只读 `config/setting.json`，不用环境变量覆盖。
- App 函数只表达有生产用途的公开业务能力；私有纯业务规则属于同领域 Domain。静态拒绝可证明无生产调用的私有函数、没有增加契约的纯转发以及只透传 Model CRUD 的 App 包装；应用构建还应拒绝不可达的 App 能力，测试调用不算生产用途。结构相似但业务含义可能不同的实现先保留警告，不用函数数量或行数配额。
- 保留 JSON envelope：成功 HTTP 200、`code=0`、`message="ok"`；错误使用 HTTP status 对应数值 code，`data=null`。
- GET/DELETE 接受 wire-safe scalar 和 nullable query 参数；POST/PUT JSON 使用静态类型请求体，不接受动态字段名或任意查询 Map。
- POST multipart 接受一个包含受限 scalar/Upload 字段的 static record；JSON 和 multipart 模式由签名静态决定且互斥。
- Runtime 隐式传递请求上下文，不在业务函数签名中声明 `dever.api.Context` 参数，也不允许存储或输出。API 的受控能力提供 request id、method/path、受信任边界内的 client address、受限 Header 读取、Text/Secret Cookie 读取，以及受控 response Header/Secure Cookie 写入；身份解析仍由业务实现。
- Header name/value严格校验；禁止应用设置Content-Length、Transfer-Encoding、Connection和原始Set-Cookie。Cookie属性使用typed API并限制Domain/Path/SameSite/Secure/HttpOnly/Max-Age。
- 自定义 API handler 必须把业务失败显式映射为标准 `dever.api.Error`：Invalid/Unauthorized/Forbidden/NotFound/Conflict/TooManyRequests。生成的 REST 使用固定输入、归属和数据库错误映射；未捕获失败返回通用 500 并只写脱敏结构化日志。
- API response继续直接返回静态业务值，不暴露 `dever.http.Response`，不允许自定义绕过envelope的JSON。
- Upload采用流式读取到runtime-owned临时文件或等价有界spool，不将大文件整体复制到内存；限制总字节、单文件、part数、字段数、header和filename长度。
- Upload filename不作为路径，MIME只作声明且需业务/Adapter复核；拒绝路径穿越、重复字段和不完整multipart。
- Upload handle为affine资源：必须由批准storage操作消费或显式close；正常、失败、timeout、取消和进程退出均清理未接管临时文件。
- 登录、Session 和资源级授权由项目 App/Domain 实现；接口 RBAC 由官方 auth runtime 自动执行，不增加业务 middleware。

## Acceptance Criteria

- [x] `rest` 自动生成 GET 列表/详情、POST、PUT、DELETE；显式动词/CMD 和固定 envelope 在 Dever 与 Markdown 源码中一致，DateTime 等标量按 shared codec 格式。
- [x] 路径、角色、Model 自动识别、字段绑定 Effect、`owner` 的所有 REST 读写范围和无歧义规则均有正反例；手写 App Model 调用不受 REST 字段绑定的隐藏影响。
- [x] GET 写入被静态拒绝，POST/PUT/DELETE 及 CMD/Job 写调用链的提交、错误回滚和取消行为有定向验证。
- [x] POST/PUT record 和 GET/DELETE query/path 严格验证 duplicate/unknown/missing/type/size；非法输入返回 400 固定 envelope。
- [x] 请求上下文由 runtime 隐式传递，客户端无法伪造；源码不得声明、存储或返回原始 Context。
- [x] Cookie parse/write覆盖Secure/HttpOnly/SameSite/Path/Max-Age，header injection和受禁header拒绝。
- [x] Secret session token不能日志/JSON/string泄漏，但可通过typed secret cookie sink发送。
- [x] 标准API Error映射到400/401/403/404/409/429；未映射App failure返回脱敏500。
- [x] multipart正常上传不全量驻留内存；越界、畸形、取消和handler失败后无临时文件残留。
- [x] 手写 API 只绑定同领域 App；自动 REST 仅通过编译器生成的 Model 路径。认证示例由业务解析会话并提供只读身份能力。
- [x] HTTP/1和HTTP/2已有deadline、request id、connection drain回归不退化。
- [x] Model/API 根文件和主题目录共存、其他 role 冲突、无 main 的 run/build/CMD JSON、缺少入口以及无意义私有/App 包装都有正反测试；合法单用途业务规则和库检查不被误拒绝。

## Out Of Scope

- GraphQL、gRPC、WebDAV和任意动态路由。
- 外部 IAM、OAuth Provider、直接用户权限、deny、角色继承和复杂策略语言。
- front CORS策略和浏览器页面。
- 断点续传、分片合并、病毒扫描和对象存储协议；由后续业务Port/Adapter扩展。

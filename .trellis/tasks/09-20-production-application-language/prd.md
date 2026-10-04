# Dever 生产级大型应用语言支持

## Goal

在不引入 front、编译器分发或任意动态插件系统的前提下，补齐 Dever 编写生产级大型后端所需的安全数据、时间与静态编解码、Port/Adapter、持久化 Job、生产 API 和大型 CMS 验收能力。最终应用保持路径驱动、静态类型、显式副作用和 `config/setting.json` 单一部署配置来源，AI 生成代码不能通过新增随意目录、直接调用实现层或脱离作用域的后台任务绕过架构边界。

## Background

- 当前应用架构已经支持 `main/app/domain/model/port/adapter/api` 路径角色，但 `port` 仍拒绝所有声明，尚无 `job` 角色。
- 当前 App 可以直接调用同领域 Adapter，Domain/Model/Port/Adapter 之间的调用范围也过宽，不能形成真实可替换边界。
- 当前 Model 拒绝私密字段；CMS 的密码哈希仍是普通公开 Model 字段。
- 当前 API 只接收 Text/Int/Bool 参数，无法安全承载登录、Cookie、结构化输入和上传。
- `config/setting.json` 的 `app`、`log` 对象当前没有运行时合同，应用仍能调用环境变量读取函数。
- DateTime 等类型已有数据库表示，但缺少 UTC 构造、解析、格式化和运算；API 输出仍使用内部整数表示。
- HTTP 底层已有 listener close 和连接排空；本任务只补进程信号、API/Job 组合生命周期，不重写 HTTP 引擎。
- 应用测试体系已经具备根 `test/`、类型安全断言、独立进程、隔离 SQLite fixture 和单套件编译缓存，可承接新能力的长期回归。

## Requirements

### R1. Stable Application Architecture

- 应用源码角色最终限定为 `app`、`domain`、`model`、`port`、`adapter`、`api`、`job`；不增加 service、repository、controller、dto、middleware、common、shared、internal 或 manager 目录。
- 手写 API/Job 只调用同领域 App；自动 REST 由编译器生成同领域 Model 操作；App 调用同领域 Domain/Model/Port 和其他领域 App；Domain 只调用同领域 Domain；Adapter 只实现 Port 并调用标准库或自身私有辅助函数。`deverc run/build` 的应用入口由编译器生成，不要求源码 `main.dever`。
- Model 不声明业务函数；Port 不含实现；API 和 Job 不承载业务规则；跨领域只能调用 App。
- 单文件与同名角色目录继续互斥；除 API 可按 URL 嵌套外，角色主题目录保持平铺且单主题文件不得机械拆分。

### R2. Secure Data And Configuration

- Model 支持领域私密字段；只有拥有领域的 App 和编译器生成的 Model 代码可以读写，其他领域、API、Job、测试和公开 App 合同不能观察该字段。
- App/API/Job 输出和日志不能包含私密字段或 Secret；静态检查必须在生成本地代码前拒绝。
- 提供不渲染、不可比较、不可序列化到普通 JSON 的 Secret 值，以及安全随机 token、Argon2id 密码 hash/verify、SHA-256/HMAC 和常量时间验证能力。
- Secret 只能进入编译器认可的安全 sink；不得通过字符串拼接、日志、普通 HTTP Header 构造或错误信息隐式解密。
- 部署配置只来自可执行文件相邻的 `config/setting.json`。移除应用层环境变量读取；命令行参数保留为显式程序输入，但不能覆盖应用配置。
- Adapter 和运行时配置具有静态 schema、未知字段拒绝、Secret 字段保护和启动前验证；App/Domain 不直接读取部署配置，业务可变数据继续放 Model。

### R3. Time And Shared Typed Codec

- 后端时间统一使用 UTC；提供 DateTime 当前时间、RFC3339 解析/格式化、Date/Time 解析/格式化和 DateTime/Duration 安全运算。
- 数据库存储继续使用整数毫秒，不改变既有 SQLite/PostgreSQL 列表示。
- 编译器建立一套静态类型值 schema 和具体代码生成路径，供 Adapter setting、Job payload、API 输入/输出共同使用；不得增加运行时反射 Value 解释器。
- JSON 解码拒绝重复键、未知字段、越界数值、错误日期和过深/过大的输入；编码拒绝私密字段、Secret、资源、Task、Stream 和非有限 Float。
- DateTime 的外部 JSON 表示为 UTC RFC3339 字符串，Decimal/Id/Uuid 保持无损文本表示，Duration 明确使用整数毫秒。

### R4. Port And Adapter

- Port 源只声明类型、签名和允许失败，不包含函数体；App 通过 Port 调用外部边界。
- Adapter 显式实现 Port，签名必须完全匹配，实际失败必须落在 Port 声明的集合内；不依赖运行时 trait object 或动态代码加载。
- 单实现自动选择；多实现由 `config/setting.json` 按稳定 Port 身份选择。缺失、未知或不完整绑定在启动前失败。
- 每个 Adapter 可声明自身类型化 setting；未选择 Adapter 的配置不得强制存在。
- 应用测试可为可达 Port 提供用例私有 fake；fake 不进入生产源码、部署配置或其他测试用例。

### R5. Durable Job

- 新增 `job.dever` 或平铺 `job/*.dever`；Job 声明为非源码可调用的后台入口，只能调用同领域 App，输入必须是可持久化静态类型，输出必须为空。
- 支持立即任务、指定 UTC 时间任务和编译期校验的 UTC 周期计划。
- Job 采用数据库持久化、租约领取、并发上限、执行超时、有界退避重试、dead-letter 和崩溃后租约恢复；语义明确为至少一次执行。
- Job payload 保存稳定任务身份和 schema 指纹。部署后遇到不兼容 payload 时进入明确失败状态，不静默误解码。
- App 事务内入队必须复用同一逻辑数据库事务；Job 与业务事务连接不一致时编译失败，避免业务提交成功但任务丢失。
- 任务表、租约和调度状态属于运行时私有 schema，不要求业务创建 Job Model。
- 同一产物支持 `api`、`worker`、`all` 三种运行模式，仅由 `config/setting.json` 选择；编译器生成对应的 API/Job 启动与排空入口，不要求应用手写 `main()` 或 `serve()` 调用。
- SIGTERM/Ctrl-C 停止接收新请求和领取新任务，在配置的期限内排空 HTTP 与在途 Job，再执行数据库和日志关闭。

### R6. Production API Boundary

- 使用 `get/post/put/delete/cmd/rest` 声明，保留固定 `{code,message,data}` JSON envelope，不返回低层 `dever.http.Response`。唯一主 Model 的 `rest` 自动生成受约束的 CRUD，手写 API 绑定同领域 App。
- POST/PUT 支持静态类型请求体；GET/DELETE 支持完整可表示标量的 query/path 输入；复用共享 typed codec 和严格未知字段检查，不暴露动态查询 Map。
- Model 可为自动 REST 声明受 Effect 约束的字段来源和 `owner` 行范围，普通字段由编译器推导；这些绑定不隐式改变手写 App 的 Model 操作。
- 请求 Context 在 runtime 隐式传递；受控 API 能力读取 Header/Cookie/request ID/client address，并设置 Cookie/响应 Header。原始 Context 不出现在源函数签名，也不得存储或输出。
- GET 调用链禁止数据库写入；POST/PUT/DELETE 和 CMD/Job 的写调用链自动建立事务，业务源码无需重复标记 `transaction`。
- API 失败必须显式映射到标准 API Error，未映射业务失败继续视为 500 并脱敏记录。
- 上传使用有总量、单文件、字段数、MIME 和文件名限制的流式/临时文件边界；拒绝路径穿越，临时资源在成功、失败和取消后均清理。
- 鉴权、会话和授权属于业务 App/Domain；不增加 middleware 业务层或框架内置 RBAC。

### R7. Large CMS Acceptance

- 同步维护 `examples/cms/dever` 与 `examples/cms/md`，两者具有等价声明、Model schema、API、Job、Port binding 和测试合同。
- CMS 至少覆盖 user/account、user/session、user/access、content/article、content/revision、content/category、media/asset、publishing/schedule 和 audit/operation。
- 验证密码 hash、Secret token、私密字段、会话 Cookie、权限判断、媒体存储 Port、fake Adapter、事务内发布任务、定时发布、失败重试和审计记录。
- CMS 不引入 front；普通 CRUD 只使用编译器生成的 `rest`，不写透传 App/API 函数。登录、命令、上传等业务入口仍通过手写 API 绑定 App。

## Acceptance Criteria

- [ ] 非法角色路径、App 直调 Adapter、Domain 访问 Model/Port/Adapter、API/Job 跨域调用等在 `deverc check` 阶段稳定拒绝。
- [ ] Model 私密字段可由所属 App 持久化和验证，但无法进入 App/API/Job 输出、日志、其他领域或测试观察面。
- [ ] Secret、密码和安全随机能力有成功、错误和泄漏拒绝回归；应用源码无法读取环境变量配置。
- [ ] UTC 时间和静态 typed codec 对 `.dever`/`.dever.md` 结果一致，API、setting 和 Job 不维护平行编解码逻辑。
- [ ] Port 单实现、多实现配置选择、缺失绑定、签名/失败不匹配和测试 fake 均有正反测试。
- [ ] SQLite Job 可验证原子入队、租约恢复、延迟/周期调度、重试、dead-letter、payload 不兼容和优雅停止；PostgreSQL 代码路径通过定向编译，真实 PostgreSQL 只在隔离配置存在时验收。
- [ ] `api`、`worker`、`all` 三种模式只读取 `config/setting.json`，无环境变量或命令行配置覆盖。
- [ ] 生产 API 验证结构化输入、标准错误、Cookie/Header、请求上下文和受限上传清理，仍保持固定 JSON envelope。
- [ ] Dever 与 Markdown CMS 的 check、test 和行为合同一致；最终定向运行验证不依赖 front 或外部服务。
- [ ] 每个子任务只运行对应最小离线检查；不运行 workspace 全量测试、默认性能压测、真实外部服务或未说明的持久服务。

## Out Of Scope

- front、Page JSON、React 插件和浏览器界面。
- 私有编译器分发、交叉编译、自托管、包注册表和动态插件加载。
- 内置用户/RBAC/租户业务模型；这些由 CMS App/Domain 实现。
- Exactly-once 外部副作用保证；Job 明确采用至少一次语义，业务动作负责幂等。
- 任意 cron 时区和 DST 规则；周期计划首版固定 UTC。
- Redis、Kafka、RabbitMQ 或外部队列依赖；首版复用 SQLite/PostgreSQL。
- 全量测试、默认真实 PostgreSQL、64/128 MiB 压测和 front 构建。

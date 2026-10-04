# Dever 生产 API 边界设计

## 2026-09-28 自动权限设计

### 静态权限身份

权限是 checked HIR 的入口元数据，不是业务 Text 常量。每个非 public API 入口拥有一个 `Permission`：`key/component/domain/site/action/method`。site 由现有目录映射得到，custom action 来自 API 声明，REST 展开为 read/create/replace/delete。key 不包含 HTTP method；同一 component/domain/site 下重复自定义 action 必须静态拒绝。public 入口没有 Permission。

权限清单由 native application 内嵌。应用数据库初始化后、服务开始前，Runtime 在控制数据库事务中创建并同步 `_dever_permission`：当前清单 upsert 为 active，清单外旧行标记 inactive。build/check 不连接数据库，同步失败时服务不得启动。

### 标准角色存储与请求授权

官方 auth runtime 在授权数据库维护 `_dever_auth_role`、`_dever_auth_role_permission`、`_dever_auth_user_role`，并以 `_dever_auth_version` 管理私有 schema。关系使用稳定 permission key，不使用环境相关自增权限 ID。Role 以 `(site, id)` 为身份，user-role 和 role-permission 同样携带 site 复合边界；不同站点可以复用同一角色 ID，且不能互相覆盖或授权。旧单列角色 ID schema 在一个事务中按 role.site 回填关系并替换三表，未知或不完整 schema 拒绝启动。授权只做 allow 并集，`all_permissions` 受 site 和 enabled 限制。

verify 输出收缩为 `id: Text, user_id: ModelId?, tenant_id: ModelId?`。Runtime 完成身份 scope 后，使用当前 user、site 和 permission key 对角色关系执行单次 EXISTS 查询；没有用户或授权记录返回 Forbidden。权限不进入 token、Session claims、Identity 或源码能力。租户应用在 tenant scope 内查询本租户角色库；平台入口查询控制数据库。

认证、组件可用性、接口授权和业务执行顺序固定为：路由/站点与凭据校验 -> verify -> 建立 tenant scope -> 检查可达租户组件 -> 检查 permission key -> 解码请求与调用 App。public 入口跳过 verify 与角色检查，并且其完整调用图必须静态证明不触达租户组件。

角色写入由 Runtime 的窄接口统一完成：列出 active 权限、创建/更新角色、事务替换角色权限、分配/撤销用户角色。写入时验证 site、active permission、角色状态和当前租户；这些能力只能从受保护的写 API 到达，业务组件不直接写私有表。租户 provision 时创建 site-scoped owner role并分配首个用户，避免授权死锁；保留 Owner 不能通过普通 save/grant/revoke/disable 修改。

### Job 与租户组件

User Job 只保存一个入口 permission key 和既有 provider/site/subject/session/tenant 身份引用。执行顺序固定为重新 verify、确认当前编译权限、检查租户组件、查询当前角色；失败转 blocked 且不进入业务 dispatch。v4 capability 列表迁移到新 schema 时，旧活跃 User Job 统一阻断为 `authorization_contract_changed`，物理删除旧 capability 列，历史终态记录保留。

编译器以 tenant Model 和 tenant Job 的静态归属识别租户组件，并为 API/CMD/Job 入口计算可达租户组件集合。混合组件整体视为租户组件，它拥有的全部 Job 都绑定租户队列，即使单个 Job 只访问 global 数据。控制库 `_dever_tenant_component(tenant_id, component, disabled_at)` 只存禁用例外。禁用优先于角色授权，不删除租户数据或改变 schema。

### 配置收缩

租户配置只有 `database/max_pools/idle_timeout_ms`。`database` 选择控制连接；物理租户数据库仍由现有 database 配置的 tenant target 派生。旧 `mode`、`control_database` 及 table/field 值明确失败，不保留双读或迁移别名。

## 2026-09-26 身份、站点与租户修订

此节是用户批准后的当前合同，替代旧 Auth Boundary 中的无身份运行时限制。实现仍复用既有 AST/HIR、wire、静态路由、隐藏请求上下文、ORM、Job 和 Port，不增加 Go 应用框架层。

### 配置与路由

`sites.<key> = { path, auth, hosts? }`。path 相对每个领域 api 目录，只匹配物理父目录（SourceLayout topics 排除文件名），空 path 用于单站根 API。保留完整 topics 生成 URL，站点不改写路径。每条 HTTP API 必须得到无歧义站点归属，Host 是进一步限制，不是认证结果。CMD 不从 HTTP 目录映射取得可信身份。

API 仅有 rest 和动词到 App 的绑定，增加单个 HTTP 声明的 public 修饰；禁止源码 site 声明、隐式全站共享接口及强制按站点复制 App/Model。注册后的路由元信息是认证 Provider 选择的唯一依据。

### 认证与权限

`auth.providers.<key>` 保存凭据验证配置和业务 App verify 引用。配置引用必须经过静态签名/Effect 校验并进入生产调用图、native emission、数据库初始化和 API 契约。密码/角色/成员业务复用账号领域，认证代码并不要求名为 auth 的目录。

运行时校验凭据后由受控桥接调用 verify；普通源 record 不能写入当前身份。verify 在租户业务自动事务之前完成。只读上下文包含身份来源、站点和可选用户/租户，不携带权限集合。App 可读 auth，Domain 保持纯规则。租户一经建立在当前调用链内不变，切换接口签发新凭据。

操作权限由 API/REST 静态入口自动生成并由 Runtime 查询标准角色关系；源码不登记或检查权限字符串。自动 REST 与 App 共享权限失败及标准 JSON Error 映射。身份对象不包含权限集合或可渲染原始 Token/Cookie。

### 数据边界

Model 的静态结构与物理存储分离：global 使用平台连接，其他持久化 Model 在 tenant 开启时要求可信租户。持久化模式只支持 database。数据库名只由内部稳定租户标识导出，不提供 table/field 分支。

普通 ORM 与自动 REST 使用共同存储/范围合同；不可证明符合隔离的原生 SQL 拒绝。连接由显式可回收拥有者管理，不永久泄漏每租户的 static handle。租户初始化有受控状态与 schema 版本，业务请求不能自行建库。跨平台库引用为逻辑引用，不能产生跨库 FK/Join；写入口只拥有单物理库事务。

### 后台执行与差异实现

Job 的租户身份由入口系统生成并与入队原子持久化，不信任 payload 内自报身份。User Job 只保存 Provider、站点、subject、session、租户声明和一个入口 permission key，不保存 token 或权限集合；执行时重新调用 verify，并查询最新角色关系复核该权限。受控 System 与 User 身份明确区分，缺少可信身份或旧 schema 不能降级。Worker 使用有界调度，租户任务不得回退平台队列。CMD 使用 System 身份，不能读取 HTTP Header/Cookie 或自报用户。

租户功能、额度及配置是业务数据。Port 候选实现静态编译、共享同一签名/失败/Effect 合同，选取依据可信租户配置。log 配置使用现有 logger 并关联调用元数据；runtime 缺省按 API/Job 能力选择模式，显式 CMD 不改变部署配置。

### 验证边界

先静态反例和 runtime in-process 验证，再生成代码/AOT 及双源码 CMS 定向用例。网络/数据库用例只使用 test 自有临时目录、随机 loopback 端口或明确配置的隔离数据库，运行前说明，不启动现有服务，不运行全量测试，不使用环境变量配置。

## Source And Generated CRUD

本节记录目标方向。声明式 API/CMD、编译器生成入口、基础 REST 和部分隐藏请求上下文能力已实现；Model 字段绑定、可信 `owner`、完整 Cookie 策略、标准 API Error 与 Upload 仍是待实施合同，不能把下述示例全部视为现有语法。

```dever
# module/user/profile/model.dever
type Profile {
  owner user_id: user.account.model.id = user.auth.id()
  name: Text(1, 64) index
  bio: Text?
}

# module/user/profile/api.dever
rest
```

路径是 `/user/profile`，不重复目录名。仅当同领域主 Model 唯一时允许无参数 `rest`；多 Model 必须显式指定，歧义时报编译错误。生成 REST 覆盖 GET 列表与 `/{id}` 详情、POST、PUT 全量替换、DELETE，默认只传输本表公开字段和外键 ID，不自动展开或嵌套写关联。分页有界，外部筛选仅允许显式 `search` 或已索引的静态字段，不接受任意 QueryPlan。

Model 字段不重复列读/写名单：普通字段由编译器按字段名推导；`create`、`replace`、`search` 表达式仅描述不同的输入来源或纯转换。`owner field = context_read()` 对生成 REST 的创建赋值及所有读写施加同一行范围；GET/PUT/DELETE 的身份条件必须进入同一数据库查询或写语句，客户端不得设置或修改 owner。字段表达式仅在生成的 REST 入口执行，不改变普通 App `model.create/get/...` 的行为。只读上下文能力和纯函数可作为静态调用目标；数据库、网络、任务和写 Effect 不允许出现在字段绑定中。多个接口需要不同权限策略时使用明确的 App 操作。

```dever
# 自定义动作才需要 App；App 签名就是 API 输入输出合同。
post publish = app.publish
cmd rebuild = app.rebuild
```

`get`、`post`、`put`、`delete` 是 API 声明而非函数名前缀；`cmd` 独立于 HTTP。GET 调用链禁止数据库写 Effect；POST/PUT/DELETE 和 CMD/Job 的数据库写入在入口调用链自动建立事务，失败或取消回滚。`rest` 与同路径同方法的显式声明冲突时报错，不暗中覆盖。无需 App 透传 CRUD、`crud ... on model`、`expose`、`package` 或手写启动 `main.dever`。

应用业务源码由 `<component>/<domain>/<role>` 唯一确定身份；固定 role 为 app/domain/model/port/adapter/api/job。App 函数均为领域公开能力，Domain 保留私有纯业务规则；topic 文件名不进入 App 调用名。非 API 主题目录只允许一层。App/Domain/Port/Adapter/Job 的根文件与主题目录互斥；主 `model.dever` 可以和附属 `model/<topic>.dever` 共存，`api.dever` 可以和 `api/**` 共存。`user/` 仅为 component，`user/profile/` 为 domain。主 Model 的逻辑身份与表名保持 domain 名，附属 Model 的身份与表名取 topic；移动文件不是纯重构。

生成的应用入口收集 API、Job 和 CMD；没有命令参数时按 setting 的 runtime mode 启动服务，CMD 由稳定的 `<component>.<domain>.<name>` 名称和一个 JSON 输入显式选择。CMD 选择不能改写部署配置；没有可运行入口或 CMD 名称歧义时失败。基础 REST 和自定义 HTTP/CMD 共享类型化 wire 与固定 JSON envelope，不能维护第二套动态解码器。

函数约束使用已检查 HIR 和生产入口调用图，不按长度或函数数量猜测质量。未被生产入口或其他真实函数引用的私有实现、完全不增加类型/范围/失败/效果合同的私有转发、只有原样 Model CRUD 的公开 App 包装是硬错误。应用 build/run 对不可达 App 能力报错；库式 check 不推测外部调用方，保留提示。W003 相似实现仍是提示，不自动合并不同业务含义。所有分句保持同文件连续声明，合法单用途复杂规则不受影响。

## Custom Handler Shapes

GET/DELETE 从类型化 query/path 输入读取，POST/PUT 从静态 JSON 请求体读取；multipart 入口接收一个包含受限 scalar/Upload 字段的 record。API 自动继承绑定的 App 签名，不再机械重写参数和唯一 `response` 输出。请求值和业务失败继续使用共享静态 codec 与固定 JSON envelope。

## Context And Response Metadata

Runtime 为每个 request 创建仅在调用链内有效的 opaque Context handle，不作为源函数参数传递。受控 API 能力返回复制的 Text 或 Secret 值，并修改 Context-owned response metadata。Native route 在 handler 成功/标准失败后统一合并 headers/cookies 并构造 envelope；handler 不能直接获得底层 Response。

Cookie使用具体record/choice表示属性。Secret cookie sink接受Secret并直接编码header，不产生Text中间值。普通response header仅接受Text且经过token/CRLF和forbidden-name检查。

## Error Mapping

官方 `dever.api.Error` 是唯一可离开API handler并被协议映射的error choice。API通常通过 `result(app.call(...))` 捕获业务错误，再用小的API-local clause映射。Native route区分：

- success -> 200/0/ok/data；
- api.Error ->对应status/code/message/null；
- InputError -> 400；
- other inferred failure/fault -> log request metadata, return generic500。

错误message有长度上限且不得包含Secret/private value。业务层不依赖HTTP status。

## Upload Lifecycle

HTTP request reader为multipart路径提供bounded streaming/spool模式。Parser逐part验证headers和limits，将文件写入 `data/tmp` 下create-new随机名，业务只看到opaque Upload、原始filename展示值、declared MIME和size。Scalar fields经过shared decoder。

Upload资源owner跟踪临时文件。Storage Adapter通过批准runtime file/stream操作消费并显式commit ownership；未commit drop/close删除。API response完成前检查所有Upload已消费/关闭，防止隐式长期文件。

普通JSON路径保留现有bounded full body，因为全局body limit明确；multipart不复用会全量聚合的`Request.body`。

## Auth Boundary

登录、Session 和资源级授权仍由业务实现。业务 verify 向隐藏调用上下文提供只读身份，供 `owner` 绑定或 App 中显式 `dever.auth` 能力使用；接口角色关系由官方 auth runtime 读取。CMD 使用受控 System 身份；认证请求入队的 User Job 持久化最小身份来源和入口 permission key，并在执行时重跑 verify 与当前授权查询。`owner` 只约束自动 REST，手写 App 的 Model 操作必须明确检查其数据范围。

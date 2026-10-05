# Dever

Dever 语言编译器。基础类型、表达式、命名输出、条件分句、集合、Stream 和值/资源语义已经接通语义检查与原生执行。提供源码格式化、基础标准库、JSON、HTTP/1.1 连接池、HTTP/2 多路复用、流式传输、SSE、WebSocket、TLS、可配置的有界并发，以及 SQLite/PostgreSQL 类型安全 ORM。Linux 机器安装、共享工具链与 ARM64 应用交叉构建已有实现；公开二进制发行资产、其它平台发行和自举仍需单独交付。

面向 Dever 程序开发者的完整语法、标准包和示例说明见 [Dever 语言开发指南](LANGUAGE.md)；编译器的长期架构和语义实现见 [Dever 语言实现原理](IMPLEMENTATION.md)。

应用开发从 [安装指引](skills/references/install.md) 和独立 [dever-language skill](skills/SKILL.md) 开始。Skill 仓库为 `shemic/dever-main-skills`，本仓库的 `skills/` submodule 固定配套提交；源码维护者使用 `git clone --recurse-submodules git@github.com:shemic/dever-main.git`。

```sh
dever new my_app
dever check my_app
dever test my_app
dever run my_app -- hello.greeting.greet '{"name":"Dever"}'
dever build my_app --output my_app/program
```

`new my_app --markdown` 生成等价 Markdown 项目，包含配置、AI 指引和应用测试。机器版 `dever update` 显式从官方 Releases 更新核心与配套 skill；`dever skill path [project-root]` 读取对应版本，`dever skill install <new-directory>` 安装持续跟随机器版本的 AI 入口。地址不写入业务 `config/setting.json`，普通 run/build 不联网。多个项目共享 `/opt/dever`，无需在每个项目复制核心；`crates/` 与 `library/` 是编译器和标准库维护目录。

[示例索引](examples/README.md) 以 CMS 为当前业务示例：[Dever 实现](examples/cms/dever/module/news/article/api/admin/manage.dever)和 [Markdown 实现](examples/cms/md/module/news/article/api/admin/manage.dever.md)同步维护。旧示例及编译拒绝反例归档在 `examples/old/`，正式回归测试保留在根目录 `test/`。

源码同时支持 `.dever` 和 `.dever.md`。应用固定使用 `module/<component>/<domain>/<role>` 结构，role 为 App、Domain、Model、Port、Adapter、API 或 Job；源码不写 package 或 import，`public` 只允许修饰单条匿名 HTTP 声明。跨领域只能调用 `<component>.<domain>.<function>` 形式的 App 能力，其他 role 保持领域私有。编译器从 API、CMD 和 Job 声明生成入口，不再使用 `module/main.dever`。应用测试放在 `test/<component>/<domain>/<topic>`，由文件名选择同名零输入、零输出函数。Markdown 合同见 [Markdown 源码规范](MARKDOWN-SYNTAX.md)。CMS 展示账号、成员、会话、admin/front 站点以及平台库与租户库分离。

例如 `module/news/article/api/admin/manage.dever` 只声明入口，业务组合留在 App；`examples/old/` 保存旧合同回归材料，不是当前应用目录示例。

```dever
rest model
post publish = app.publish
```

## 运行

这是编译器开发仓库，构建编译器自身需要 Rust 1.95 或以上、锁定的 Cargo 依赖和显式准备的 LLVM 18/LLD 作者 SDK。私有 `dever run/build/test` 默认将 checked HIR 编译为 LLVM object，再通过内置 LLD 链接；不调用 Rust/Cargo/cc，不通过环境变量选择后端，也不隐式下载依赖。CLI 参数是项目根，源码固定放在项目根的 `module/` 中，运行配置固定为 `config/setting.json`。

编译器同级必须有 `runtime/<platform>/manifest.json` 声明的 runtime、CRT 和静态库。作者侧使用独立的 base、SQLite、PostgreSQL、双数据库优化 archive，通过 [原生发行包制作流程](sdk/native-release.md) 生成带签名清单的新目录；输入固定写在作者根下的 `config/setting.json`。

```bash
cargo build --offline --locked -p dever-cli --bin dever
cargo build --offline --locked -p dever-cli --example native-release
target/debug/examples/native-release <author-root> --output <new-release-directory>
```

制作器离线工作，不覆盖已有目录；它复用安装端的格式/摘要校验，复制完成后签名，私钥不进入产物。私有调试可按制作说明将该 `runtime/`、`lib/` 放到匹配的 `dever` 同级。六目标正式发行仍需各自资产和平台验收。缺失或损坏的 pack 会明确报错；`check/fmt` 和零用例 `test` 不需要 runtime pack。

现成 Linux 二进制可通过 Port 的 `external command "bin/tool" {}` 直接调用，传参数数组及可选 Bytes 输入，得到退出码与两路 Bytes 输出；不用实现 Worker 协议或编写包装脚本。`run/build` 自动嵌入程序和显式随附的动态库，限额与文件授权仍在 `config/setting.json`，不使用 shell、环境变量或 PATH。单独的 command 不需要 Lib 锁；完整写法见 [语言说明](LANGUAGE.md)。

外部 Lib 通过显式命令解析，项目声明写入 `config/setting.json` 的根 `lib` 数组，例如 `"lib": ["pip:fixture@1.0.0"]`，锁定结果写入 `dever.lock`；`run/build` 只消费已有锁，不联网、不读取环境变量或系统 PATH。旧 `config/lib.json` 不再接受：

```bash
dever lib add <project-root> pip:fixture@1.0.0
dever lib list <project-root>
dever lib install <project-root>
dever lib doctor <project-root>
dever lib update <project-root>
dever lib remove <project-root> pip:fixture@1.0.0
```

当前仓库已接入 PyPI、npm registry 和 Go proxy 解析、校验下载及机器共享缓存。真实 provider 只从已验证的机器资源读取签名 runtime 描述和 pack；缺少资源明确报错，不使用宿主语言环境。v6 锁绑定 Worker 合同、Python extras、npm 嵌套实例、Go sumdb 证据、准确源文件与完整构建输入。`lib install` 按已有锁恢复，配置和锁文件字节不变；第三方构建无法重现锁定摘要时明确失败。不同 Worker 独立解析依赖。显式 Lib 准备可使用签名工具包构建 PEP 517 sdist、执行 npm 安装钩子及原生 addon 编译；`run/build` 只复用已锁定输出。SDK 自动绑定 Port 同名操作并校验输入、结果与错误，见 `sdk/README.md` 和 `LANGUAGE.md`。Go Worker 使用锁定工具离线编译/链接，支持目标文件选择与 `go:embed`，启动不需要系统 Go。

发行包分为基础包和可选扩展。基础包保留 Dever 编译器、本机四种数据库运行库和配套 skill；Python/Node/Go 由 `lib add/update/install` 按实际需要准备，源码构建路径再准备构建工具。`dever target add linux-aarch64` 准备 ARM 应用目标。同版本资源在机器上共享，普通 `run/build` 不下载；`dever update` 更新核心、skill 和当前已安装的扩展，全部准备成功后才切换活动版本。基础与扩展均采用 Zstandard，首装无需系统 zstd。

原生制作器已支持 Python/Node/Go runtime/build pack 的确定性打包与统一签名；Linux x86_64 三生态的签名安装、受管 `check/run/build` 和移除源码/机器目录后的无系统语言独立执行已通过，包含 Python/Node 原生依赖。Python sdist/extras/原生扩展、npm 高级依赖/安装钩子/native addon 和 Go sumdb 均已有实现及真实第三方依赖定向证据。Linux 签名首装、升级保护和两个项目/真实用户共享核心也已通过自有完整验收。Linux Worker 使用 OS 沙箱；当前 root 验收不代表受限 AppArmor 下非 root 部署通过。输入仍是私有作者资产和临时签名，尚未交付公开首次安装包或其他平台发行。具体流程见 [原生发行包制作流程](sdk/native-release.md)。

Dever Package 用 `config/setting.json.package` 声明，例如 `"package": {"registry": "https://packages.example.test", "use": ["catalog@^1.0.0"]}`。`dever package add/update/remove/list/doctor <project-root>` 共用机器摘要缓存；版本和传递依赖、Package 摘要及其 Lib 闭包统一写入 `dever.lock`。`check/test/run/build` 加载锁定的 Package 源码，不隐式解析或下载，不允许 Package 覆盖本地同名组件；离线移除保留剩余 Package 的锁定版本。这里的 registry 地址只是配置格式示例，不是已发布服务。

编译器底层已有 LLVM 18/LLD 连接桥和强类型 HIR→LLVM 转换，覆盖数值（含 Decimal）、Text/Id、Bytes、集合、嵌套记录/选择型/可空值、静态 handler、遍历、Result 和原始错误调用链。文件/流、真实协程、Task/Group/Channel、blocking/parallel、TCP/HTTP/TLS/SSE/WebSocket 及系统能力复用原 runtime 和同一 typed ABI。受管值通过具体生命周期回调转移，业务不经过解释器或动态 Value。简单内核已验证六目标链接，复杂内核已验证六目标对象及 Linux x86_64 实际执行；Windows 复杂链接仍需正式 runtime 支持。Rust unsafe 例外仅限桥接 crate 的私有 `ffi.rs`，不是语言 FFI。

独立 LLVM 应用入口已接通 CMD→App→配置选中的 Dever/external Port/Adapter、SQLite/PostgreSQL Model/事务，以及 HTTP/REST、可信身份、自动权限、database-only 租户/组件状态、Cookie/Header、Upload 存储和日志配置。持久 Job 复用现有队列、重试、调度、权限重验和事务；应用 Test 保持整套一次编译、稳定用例编号、每用例独立进程及 fake/数据库/时钟隔离。external 调用沿用统一 Worker 协议，支持具名类型化输入/输出、声明的业务错误和校验后提取的内嵌资源；只在边界编码 JSON。Worker 属于本次运行，退出时先关闭 Worker，再排空子任务和关闭其他资源；配置仍只读取 `config/setting.json`。私有 CLI 和 Linux 可信 daemon 编译已接 LLVM，正式目标包和跨平台发行仍未交付；本机定向验收不等于性能或正式发行验收。

```bash
cargo run --offline --quiet -p dever-cli --bin dever -- check examples/cms/dever
cargo run --offline --quiet -p dever-cli --bin dever -- test examples/cms/dever
cargo run --offline --quiet -p dever-cli --bin dever -- fmt examples/cms/dever --check
cargo run --offline --quiet -p dever-cli --bin dever -- run examples/cms/dever -- user.account.bootstrap '{"tenant_key":"tenant-one","tenant_name":"Tenant One","email":"owner@example.com","display_name":"Owner","password":"change-me-now"}'
cargo run --offline --quiet -p dever-cli --bin dever -- tenant migrate examples/cms/dever 1
cargo run --offline --quiet -p dever-cli --bin dever -- tenant owner examples/cms/dever 1 admin 1
cargo run --offline --quiet -p dever-cli --bin dever -- tenant owner examples/cms/dever 1 front 1
cargo run --offline --quiet -p dever-cli --bin dever -- check examples/cms/md
cargo run --offline --quiet -p dever-cli --bin dever -- test examples/cms/md
cargo run --offline --quiet -p dever-cli --bin dever -- clean examples/cms/dever
```

`tenant migrate` 和两条 `tenant owner` 中的租户 `1` 应替换为 bootstrap 响应中的 `tenant_id`，Owner 命令最后的用户 `1` 应替换为响应中的 `user_id`。迁移先建立租户物理库和私有 Job/授权 schema，再分别为 admin/front 站点建立核心 RBAC Owner，完成后才能启动受保护 API/Worker。bootstrap 中的 Owner membership 是业务成员身份，不代替核心接口授权。两套 CMS 源码具有相同的 Model 与 App 合同。编译器开发入口是 `target/debug/dever`，也是 `cargo run -p dever-cli -- ...` 的默认目标。机器版启动器的内部构建名是 `dever-launcher`，发行时安装为 `dever`，版本核心仍安装为 `dever-core`。Go 框架使用独立的 `dever-go`；本仓库构建不会安装或替换全局命令。

正式工具链只公开一个安装在机器级 `bin/` 下的 `dever`。它从签名版本仓库选择版本：项目可在 `config/setting.json` 写精确的 `{"dever":{"version":"0.1.0"}}`，未写时使用机器活动版本。`install/update/use/uninstall/version/cache status/cache clean` 属于机器管理命令。`dever clean <project-root>` 只回收当前用户超过 24 小时且所属进程已经退出的 Dever 临时构建目录和旧 `.dever-run-*` 文件，不删除 build 输出、源码、配置、data、Lib 或 Cargo 缓存。Linux `deverd` 已用内核 peer credential 接受其他机器用户的状态查询和按 SHA-256 传输的共享 Lib 资产，非服务所有者不能清缓存或读取管理 token；普通查询返回 `verified:false` 的元数据摘要，完整校验与清理由管理员执行。服务不可用明确失败，不会本地回退。开发 fixture 可用 `deverd --root <machine-root>` 启动服务。编译器贡献者清理本仓库 `target/` 时使用 Cargo 自己的 `cargo clean`。Linux daemon 编译提交规范源码和无秘密配置绑定，签名核心重新检查并编译；完整相同输入可跨项目、跨用户复用产物。App/Test 仍由调用者运行。macOS/Windows 跨用户认证、系统服务注册和正式原生发行仍未交付，开发态 socket 不等于完整跨平台安装。

保留一个可独立运行的程序：

```bash
cargo run --offline --quiet -p dever-cli --bin dever -- build examples/cms/dever --output examples/cms/dever/cms-app
./examples/cms/dever/cms-app user.account.bootstrap '{"tenant_key":"tenant-one","tenant_name":"Tenant One","email":"owner@example.com","display_name":"Owner","password":"change-me-now"}'
```

`build` 的输出路径必须是新文件，父目录必须存在；已有文件不会被覆盖。独立程序从可执行文件同级的 `config/setting.json` 加载配置，因此正式产物放在项目根或保持相同的部署目录结构。编译和运行使用同一 LLVM 路径；`run` 直接执行编译器持有的临时程序，临时 object、链接输入快照和程序在执行后清理。私有作者缓存位于编译器同级 `cache/native`，绑定编译器、pack/profile 和完整 IR；命中仍检查 pack 和程序完整性。独立程序无需 LLVM、Cargo、Rust 工具链或 `.dever` 源码即可运行。机器管理的 `dever-core` 通过可信 daemon 编译；服务缺失明确失败，不降级到私有缓存。签名版本必须覆盖 native manifest、全部链接输入和编译器动态库，命中缓存也重新校验。

## 已支持

Linux x86_64 开发机可用 `dever build <项目目录> --target linux-aarch64 --output <新文件>` 生成 ARM64 应用；部署端不需要 Dever 编译器、Rust 或 LLVM。工具链须已包含对应 ARM 运行库，第三方 Lib 用 `dever lib update <项目目录> --target linux-aarch64` 显式准备。省略目标继续本机构建，`run/test` 也保持本机执行。构建不隐式联网、不读取环境变量选择工具；作者输入及 Go 宿主工具与目标标准库的区别见 [发行制作说明](sdk/native-release.md)。

- 应用源码使用路径推导的 component/domain/role；App 自动成为跨领域能力，其他 role 仅在所属领域内可见。Model/API 主文件可与同 role 主题目录共存；其余 role 的主文件与目录互斥，单个非 Model/API 主题应并回主文件。
- Bool、Int、Decimal、Float、Text、Id；字段型、选择型、可空值、List、Map、MapEntry、Bytes 和 `Stream<Item>`。
- 构造、字段读写、算术、比较、布尔短路、单个及多个命名输出；初始化检查、稳定类型和值语义。
- Int/Decimal 字段与输出范围、`private` 字段、`error` 失败义务、默认错误传播、`result(call)` 主动捕获、`fail(...)`、显式 `recover("reason")` 和传递式 `pure` 检查；API 基线约束公开契约变化，保守建议提示重复判断和包装。用法见 [指南第 18 节](LANGUAGE.md#18-编译器约束与公开接口)及 `examples/old/dever/contracts/`。
- 分句是唯一条件分派方式，检查重叠、遗漏、不可达和多输入组合；`other` 是所在输入位置全部明确模式的补集，不依赖书写顺序。
- `each`、`reduce`、`reduce_until`、`filter`、`find`、`sum`、`append`、`first`、`get`、`put`、`remove`、`entries`、`length`。List、Bytes 和 Stream 共用静态 handler 检查，handler 参数可跨函数转发但不是运行时值。Map 保留插入顺序，可空返回不产生嵌套可空类型。
- `parallel_each(handler, sequence, workers[, context])` 接受 List、Bytes、Stream 或 AsyncStream，并根据序列与 handler 的挂起 effect 选择作用域线程、共享 blocking worker 或有界 Group。并发数始终有上限，返回前清理已启动工作；副作用完成顺序不保证。
- 普通调用自动等待其完整调用链；源码不声明 `async`，也不写 `await`。`run`、`wait`、`stop`、`timeout`、`race`、有界 `Group`、`parallel`、`blocking` 和有界 `Channel` 构成结构化并发基础。Task/Group 必须在父作用域消费，超时和竞争都会清理剩余任务。
- `run/build` 的应用入口由 API、CMD 和 Job 声明生成；CMD 以稳定名称和一个 JSON 对象选中。HTTP API 按 `config/setting.json` 的 `sites.<key>.path` 归属站点，浏览器 Cookie 使用配置的外部 `sites.<key>.origin` 做精确同源校验；默认需要认证，只有单条 `public get/post/put/delete` 可匿名。函数、用户类型和 package 依赖不能形成循环。
- `dever test <project-root>` 按名称串行执行 `test/` 中的应用测试。`assert(Bool)` 与同类型可比较值的 `assert_eq(actual, expected)` 只在测试源码可用；每个用例使用独立进程，涉及 Model 时使用运行器生成的临时 SQLite 配置，不读取部署数据库配置，也不从环境变量接收数据库覆盖。
- `library/` 中的核心和官方 package 都是正常检查的 `.dever` 源码。Text 提供 Unicode 标量索引/切片、字符编码转换、查找、切分、替换与连接；Int/Decimal/Float 提供解析与文本转换。解析失败或索引越界返回 `null`，不截断或默认为零。
- Bytes 与文件/TCP 资源原语；可恢复错误使用选择型结果，数值故障和标准输出失败定位到 `.dever` 源码。
- Model role 提供隐式 ID/创建时间、静态 CRUD、分页/游标/Stream、关联、迁移、Seed、事务和参数化 typed native SQL；Model 操作只能由所属领域 App 调用，SQLite 与 PostgreSQL 共享 Model、QueryPlan 和 schema 语义。
- `dever.auth` 通过配置引用的只读 App verify 建立三字段隐藏身份；所有非 `public` HTTP API 自动按 `component.domain.site.action` 查询核心 RBAC，源码不写权限字符串。权限目录、角色、角色权限和用户角色由 Dever 私有表维护；用户 Job 只保存入口权限键，执行前重新验证身份、组件状态和当前角色。多租户只支持 `tenant.database` 选择控制连接；`global type` 留在平台库，其他 Model 使用独立租户数据库，租户组件从 Model/Job 自动推导并可由受控 CLI 禁用或恢复。未迁移、未就绪、组件禁用或缺少租户上下文都直接失败，不回退平台库。
- 自动 REST 支持 `owner` 行范围及字段级 `create/replace/search` 纯绑定；上传使用有界 multipart spool 和 affine `Upload`，只允许 POST App 直接接收，并经 Port/Adapter 调用 `dever.storage.put` 或显式关闭。

## 格式化与官方库

`dever fmt <project-root>` 统一规范化 `module/` 和存在的 `test/` 中的两空格缩进、换行和空白，保留注释、表达式分组及 record 字段的求值顺序。`--check` 只列出需格式化的文件并返回非零。格式化不要求语义检查通过；整批源码先通过语法检查和写入准备，才逐文件原子替换，不能把操作系统中途故障视为跨文件事务。

`.dever.md` 先检查标题、说明列表和代码块的一对一结构，再只格式化 Dever 代码块内部；说明文字、标题、围栏和其它代码块保持原样。语义检查还会拒绝与代码名称、类型、数量或顺序不一致的说明。

- `dever.process.arguments()` 读取应用参数（不含可执行文件路径）；应用配置只读 `config/setting.json`。`dever.time` 提供 Unix 毫秒、单调纳秒和显式等待，均返回源码定义的结果类型。
- `dever.task.sleep(milliseconds)` 提供非阻塞等待，`ticks(milliseconds)` 提供不积压的周期流；调用链是否挂起由编译器推断。同步 `dever.time.sleep` 保持阻塞语义。
- `dever.json.parse(text)` / `stringify(document)` 使用普通分句、reduce 和 Map 完成 JSON 解析与编码。`dever.json.value` 定义公共结果、文档和节点类型，节点以整数链接表示嵌套；数字保留原文。重复键、非法转义、未配对代理项和多余输入明确失败。深度最多 256 层；编码最多 16,777,216 个 Unicode 标量，并验证手工构造的文档引用。
- `dever.http` 复用 Hyper 的 HTTP/1 与 HTTP/2 引擎，支持持久连接、逐块响应和 SSE；HTTP/1 支持流水线、chunked 和 WebSocket 升级。`client/request/open/open_stream` 提供固定目标的有界连接池和流式上传/下载，支持 DNS。设置 `limits.http2 = dever.http.default_http2_limits()` 开启 HTTP/2 多路复用，独立限制连接数、流数和接收窗口。TLS 复用 rustls/tokio-rustls，校验证书并为 HTTP/2 协商 ALPN h2；关闭 Listener 后通过 GOAWAY 排空。静态 handler 可携带类型化 Context。WebSocket-over-HTTP/2、CONNECT、推送与其他协议扩展仍未交付。

JSON 算法和官方接口位于 `library/dever/`；HTTP 的协议状态机复用运行时中的 Hyper，业务 handler 直接编译执行。精确限制见 [LANGUAGE.md](LANGUAGE.md)。

`examples/old/` 保留旧语言合同、网络和运行时回归材料；它们不代表当前应用目录结构。[CMS 示例](examples/cms/dever/module/news/article/api/admin/manage.dever) 展示 App/Domain/Model 边界、站点认证、平台/租户数据分离和发布事务。

## 资源边界

普通值复制后独立修改；资源句柄和 Stream 别名共享资源状态。关闭文件或连接任一别名后，其他别名的操作会得到 `Failed`；关闭 Stream 任一别名后，全部别名结束，重复关闭无副作用。`dever.io.create` 只创建新文件，不覆盖已有文件；`open` 只读打开。`read` 和 `chunks` 接受正数大小上限，允许短读；Stream 在自然 EOF 后结束，终止错误只产生一个 `Failed` 项。

TCP 的连接、监听、接收、读写会被推断为挂起并复用一个 Tokio runtime；`connect_timeout` 要求数值 IP。连接流和字节流为 `AsyncStream`，按需拉取；`parallel_each` 使用有界任务组，先等待容量再拉取。Socket 支持同时读写，关闭唤醒等待方，取消部分写入会关闭连接。HTTP、SSE、WebSocket 共用现有任务作用域和连接限额；停止服务会排空升级后的会话。文件 I/O 仍需通过 `blocking(...)` 与调度线程隔离。

HTTP 池的连接上限包含空闲连接；响应体读完才归还连接，提前关闭则丢弃该连接。流式传输按块限制并复用 Bytes 存储，不收集整条长连接。TLS 配置共享信任材料，并使用小容量的有界会话缓存。线程、任务、连接和块大小上限共同约束运行资源，应用保留的数据和操作系统缓冲仍需单独计入内存。

这是全新语言，接口直接迭代，仓库源码同步迁移，不提供旧源码兼容别名或回退后端。TCP 用法见 [异步 TCP 示例](examples/old/dever/async_tcp/module/main.dever)；流的关闭、限流和取消契约见 [LANGUAGE.md](LANGUAGE.md)。

编译器通过最后使用分析移动普通值；含文件、Socket、Listener 或 Stream 的值保留到源码作用域结束，避免优化提前关闭外部资源。基准解释器仅通过 `reference` feature 启用，用于与原生结果和故障位置进行差分验证；CLI 的默认 `build/run` 不使用解释器。

记录字段更新、独占列表消费、直接字符遍历和可证明安全的集合融合由编译器自动优化。HTTP 复用 Hyper 增量协议处理，Bytes 共享消息缓冲区；List/Bytes 并行动作按有限批次分发。私有 LLVM CLI 复用编译器同级 `cache/native` 的原生产物，命中时仍检查源码、API 基线、pack 和程序完整性；Lib 资产和 Linux 编译产物通过 `deverd` 共享；编译缓存按完整请求复用，不能仅凭产物摘要读取。大型 Worker 资源随原生产物内嵌；启动校验按块读取。前端检查结果仅在同进程内缓存。实现与边界见 [IMPLEMENTATION.md](IMPLEMENTATION.md)。

## 定向验证

按当前修改选择定向测试，不默认运行全部测试。原生测试会执行本地小程序；资源测试只使用自身临时目录、临时 SQLite 数据库及 `127.0.0.1` 随机端口。真实 PostgreSQL 测试默认忽略，只有仓库根 `config/setting.json` 明确配置 `database.postgres_test` 后才手动运行，不接受环境变量或命令行连接覆盖。可复制 `config/setting.example.json` 后按本机 PostgreSQL 凭据修改；测试 URL 的数据库名必须保留 `{case}` 占位符。

```bash
cargo test --offline -p dever-tests --test core_semantics
cargo test --offline -p dever-tests --test core_native
cargo test --offline -p dever-tests --test runtime_foundations
cargo test --offline -p dever-tests --test hello_native
cargo test --offline -p dever-tests --test structured_concurrency
cargo test --offline -p dever-tests --test async_runtime
cargo test --offline -p dever-tests --test async_native
cargo test --offline -p dever-tests --test async_composition
cargo test --offline -p dever-tests --test pooled_network
cargo test --offline -p dever-tests --test pooled_library
```

新增工具和库按需选择 `formatter`、`standard_library`、`native_ownership`、`concurrency`、`json_library`、`http_library`、`http_engine`；差分验证使用 `cargo test --offline -p dever-tests --features reference --test differential`。

`performance` 默认忽略，只在明确进行有界性能验证时运行：`cargo test --offline -p dever-tests --test performance -- --ignored --nocapture`。它比较 Int、Float、List、Decimal、Map 五个本地计算内核，排除构建、文件读取和进程启动，构建耗时单独记录。双方交错运行五个独立进程，每个进程预热后取九次测量，以各自全部样本的中位数比较，目标不超过对应基线的 1.15 倍。结果只代表该环境和这些内核，不代表 HTTP 吞吐或整个语言的性能。

同一目标还包含 `allocation_sensitive_source_scenarios`，运行 `test/dever-tests/fixtures/performance` 的记录文本、列表、JSON 和 16,384 个不同键用例。`concurrency` 的 `parallel_task_granularity_measurement` 也默认忽略，用于有限任务调度测量，不启动服务。耗时、内存与验证范围见任务实施记录。

独立二进制的常驻内存、异步调度和 HTTP/HTTPS 基准位于 [test/performance](test/performance/README.md)。构建和运行分开，支持专属 cgroup 预算及 Rust runtime/Hyper 对照，保存原始计数、延迟与资源采样。文档包含首轮 1 worker/128 MiB 短时结果及 64 MiB 空闲验证；这些结果不代表吞吐上限或长期稳定性。基准必须显式运行，不进入默认测试。

# Dever 语言开发指南

本文描述当前已经实现的 Dever 语言。它面向使用 Dever 编写程序的人，不涉及编译器内部实现。

编译、检查、静态特化和运行时边界的设计见 [Dever 语言实现原理](IMPLEMENTATION.md)。

## 1. 快速开始

先按 [安装指引](skills/references/install.md) 安装机器版 Dever。AI 开发使用独立 [dever-language skill](skills/SKILL.md)。已有命令时直接生成项目；目标目录必须不存在，父目录必须已存在：

```bash
dever new my_app
# Markdown 写法：dever new my_app --markdown
```

命令接收项目根。Dever 源码固定放在 `module/`，部署配置固定放在 `config/setting.json`，运行数据放在 `data/`。模板生成：

```text
my_app/
  config/
    setting.json
  AGENTS.md
  README.md
  module/
    hello/
      greeting/
        app.dever
        api.dever
  test/hello/greeting/greet.dever
```

`my_app/module/hello/greeting/app.dever`：

```dever
greet(name: Text) (message: Text) {
  message = "Hello, " + name
}
```

`my_app/module/hello/greeting/api.dever`：

```dever
cmd greet = app.greet
```

`my_app/config/setting.json`：

```json
{
  "log": { "level": "info" }
}
```

模板不需要数据库或 HTTP 配置。添加 HTTP 时再完整配置站点与身份，不能只添加 get 而遗漏 sites/auth。配置不会编译进二进制。`run` 使用项目目录中的配置；独立二进制启动时使用可执行文件同级的 `config/setting.json`，相对数据路径也以该目录为基准。

语言在开发和安装后的命令名均为 `dever`，Go 框架使用 `dever-go`。开发仓库生成 `target/debug/dever`；机器版启动器根据 `config/setting.json` 中可选的精确 `dever.version` 选择已安装版本，未锁定项目使用机器活动版本，再把项目命令和退出码原样分派。启动器的内部构建名为 `dever-launcher`，安装后仍叫 `dever`，版本核心为 `dever-core`。Linux 安装制作与机器共享已实现；公开发行资产尚未发布。源码构建不会替换机器上已有的全局命令。

以下命令在已经提供 `dever` 的开发环境中执行：

```bash
dever fmt my_app --check
dever check my_app
dever test my_app
dever run my_app -- hello.greeting.greet '{"name":"Dever"}'
dever build my_app --output my_app/app
```

Linux 开发环境运行构建产物：

```bash
./my_app/app hello.greeting.greet '{"name":"Dever"}'
```

- `new`：从随版本验证的模板原子创建新项目，不覆盖已有目录；`--markdown` 生成等价 Markdown 源码。
- 机器版 `update`：从官方 GitHub Releases 显式下载并校验，统一更新活动核心与对应 skill；不修改业务配置。`skill path [project-root]` 返回经验证的配套指引，`skill install <new-directory>` 安装稳定的 AI 加载入口。项目可锁定 dever.version，普通项目命令不会触发更新。
- `check`：加载全部源码并完成语法、名称、类型和分句检查。
- `fmt --check`：只检查格式，不修改文件。
- `fmt`：统一格式化项目 `module/` 和存在的 `test/`；需要修复格式时单独执行 `dever fmt my_app`。
- `test`：检查生产源码与应用测试，再按名称串行运行每个隔离用例；没有 `test/` 时报告 0 个测试并成功。
- `run`：编译器从 API、CMD、Job 声明生成入口；无参数时按 `setting.json` 的 runtime mode 启动服务，`-- <component>.<domain>.<cmd> '<json-object>'` 只执行指定命令。
- `build`：默认生成本机独立可执行程序；Linux x86_64 可加 `--target linux-aarch64`，使用预先准备的 ARM runtime 和依赖生成 ARM 程序。输出路径必须尚不存在，已有文件不会被覆盖。发布时把二进制放在项目根，使它能从同级 `config/setting.json` 加载部署配置。Windows 和 macOS 发行流程尚未交付。

`check`、`fmt`、`run` 和 `build` 默认只使用本地源码及随编译器提供的 package，不会隐式联网下载依赖。

## 2. 源文件与领域职责

Dever 源文件使用 `.dever` 或 `.dever.md` 后缀和 UTF-8 编码。两种格式共用语言规则；Markdown 写法见下一小节。标识符使用 ASCII；注释和 Text 可以包含任意 UTF-8 字符。

```dever
# 单行注释
type Greeting { message: Text }
greet(name: Text) (response: Greeting) {
  response = Greeting { message = "你好，" + name }
}
```

规则如下：

- `#` 开始单行注释，没有块注释。
- 不写分号，声明和语句由换行或结构边界分隔。
- 应用 `run/build` 不使用手写 `main.dever`；入口由 API、CMD 与 Job 声明生成。
- 业务源码固定为 `<component>/<domain>/<role>.dever`，或在确实需要按主题拆分时使用 `<component>/<domain>/<role>/<topic>.dever`。
- role 只能是 `app`、`domain`、`model`、`port`、`adapter`、`api`、`job`。应用源码不写 `package`、`exposes`、`public` 或 `internal`。
- `app` 中的全部函数自动成为领域能力，调用名固定为 `<component>.<domain>.<function>`；同组件可省略 component，同领域内部可写 `app.<function>`。`app/<topic>.dever` 的 topic 不进入调用名。
- App 能力的输入、输出和向外传播的错误都属于公开契约，不能引用 Domain、Adapter 等私有类型。业务错误类型应声明在 App，Domain 可通过 `app.ErrorType.Variant` 使用它。
- `domain`、`model`、`port`、`adapter`、`api` 和 `job` 都是领域私有实现。跨领域、跨组件只能调用目标 App；API 和 Job 入口不是普通源码调用目标。
- App 可调用本领域 Domain、Port 及其他领域 App；API 只能调用同领域 App。Domain 只能调用同领域 Domain。Adapter 只能调用同文件 helper 与标准能力，不能反调 App、Domain、Model、Port 或其他 Adapter。Model 操作仍只能由所属领域 App 使用静态 DSL 执行。
- 同一 role 的单文件与同名目录二选一。非 API 的 topic 目录保持一层；只有 `api/` 可递归。role 目录只有一个源码文件时必须合回 role 文件，避免无意义拆分。
- 一个文件可放一组内聚类型和函数，不按函数机械拆文件。禁止用 `common`、`shared`、`utils`、`helper`、`base`、`service` 作为业务源码桶。
- `port` 只声明类型和无函数体的操作合同，必须写 `fails app.ErrorChoice`；错误 choice 至少包含一个显式 error 分支，也可引用公开标准错误 choice。输入必须是无约束的明确值类型，输出同样不加范围约束。当前 Port 不接受 handler 参数。App/Domain 的外部文件、网络 I/O 必须经过 Port；日志、时间、随机数等已有能力保持原约束。
- 没有 `import`、`use`、通配导入或别名。
- 不同领域之间不能循环依赖；同领域职责文件共享一个依赖边界，函数递归仍被拒绝。

Port 示例：App 声明 `type DeliveryError { error Unavailable(message: Text) }`；同领域 `port.dever` 声明 `send(message: Text) () fails app.DeliveryError`；`adapter.dever` 实现 `port.send(message: Text) () { ... }`。Port topic 使用 `port.mail.send`。qualified 实现不能被普通函数调用。Port/Adapter 只能引用其合同实际使用的 App 类型，不能调用 App。Port 自有 DTO 保持私有，不能泄漏到 App 公开签名。

一个 Adapter 文件完整实现一个 Port；单实现自动绑定，多实现由 `config/setting.json` 的 `adapter.<component.domain[.topic]>.use` 选择，根 adapter 名为 `default`，topic 名为实现名。编译产物包含全部候选，运行时选择不改变 failure 全集、effect 或 suspension 合同。每个实现的实际 failure 必须是声明全集的子集。

Adapter 可声明 `setting { endpoint: Text token: Secret }`，通过只读 `setting.endpoint` 访问。配置形如 `{"adapter":{"notification.mail":{"use":"smtp","setting":{"endpoint":"...","token":"..."}}}}`。仅选中实现要求 setting；未知 Port/实现/字段、重复键、缺失或错误类型在入口前失败。无 setting 的单实现允许省略配置文件。Secret 不可观察，配置错误不回显值；不使用环境变量。

外部程序同样只能实现 Port。需要调用现成 Linux 二进制时用 `external command`；需要类型化多操作 Worker 协议时用 `external exec` 或生态 Adapter。

普通二进制不必实现 Dever 协议，也不必另写 Python/Go 包装。例如将程序放在 `module/media/tool/bin/tool`：

```dever
# module/media/tool/port.dever
execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) fails dever.process.Error
```

```dever
# module/media/tool/adapter.dever
external command "bin/tool" {}
```

```dever
# module/media/tool/app.dever
version() (text: Text) {
  output = port.execute(["--version"], null)
  text = dever.bytes.to_text(output.stdout)
}
```

```dever
# module/media/tool/api.dever
cmd version = app.version
```

执行 `dever run . -- media.tool.version '{}'`，或先 `dever build . --output program` 再执行 `./program media.tool.version '{}'`。

command Port 只有一个操作，操作名由业务决定；输入固定为 `args: List<Text>`，可选第二项 `stdin: Bytes?`，输出固定为 `output: dever.process.Output`，错误为 `dever.process.Error`。只需 argv 时可省略整项 stdin 参数。Output 有 `code: Int`、`stdout: Bytes`、`stderr: Bytes`；二进制输出不做隐式文本转换，正常非零退出码也是结果。每次调用启动一次进程，不重试，不使用 shell；空格、引号、`$()` 等按参数原样传递。未提供 stdin 时立即发送 EOF。启动失败、超时和输出超限产生源位置 runtime fault。

调用限额只从 `config/setting.json` 读取：`{"adapter":{"media.tool":{"command":{"timeout_ms":30000,"output_limit":1048576}}}}`。示例值就是默认值；timeout_ms 最大 3600000，output_limit 为 stdout/stderr 合计字节数、最大 8388608，均必须为正数。argv 含终止符总计最多 65536 字节且不含 NUL，stdin 最多 8388608 字节。超时、取消和退出会回收本次沙箱进程树。需要文件、网络或子进程时仍在声明中写 `allow file/network/process` 并使用已有部署授权；command 不接受 `setting` 或 `lib` 声明。

`run/build` 离线嵌入 ELF 和相邻 `bin/lib/` 下显式随附的普通动态库文件；静态程序可直接使用，动态程序使用签名发行包的 GNU loader 和 OS 库。不扫描宿主库，不接受符号链接或 lib 子目录；缺库、错架构、SONAME/版本不匹配、越界搜索路径在构建期失败。Linux x86_64 与 ARM64 必须提供各自架构的二进制和库，Dever 不转换第三方二进制。单独使用 command 不需要 `dever lib` 或 `dever.lock`；资源可随 Dever Package 分发，Package 自身仍须锁定。测试继续用 Port fake，不执行真实命令。

在 Adapter 文件中写一个 `external exec` 声明，编译器会用对应 Port 的全部操作、输入、输出、业务错误和可选 `setting` 生成唯一协议合同，不需要再逐个重复函数：

```dever
setting {
  endpoint: Text
  token: Secret
}

external exec "worker/mail" {
  allow network
}
```

entry 是所属领域目录内的可移植相对路径；上例构建时收集 `module/notification/mail/worker/mail`，将全部候选 exec Worker 的字节、SHA-256 和可执行位嵌入程序。入口原子提取到私有 `data/cache/lib/<bundle>`，每次启动 Worker 重新验证精确资源清单、摘要和 Unix 所有权/权限；不扫描其他 bundle，不依赖部署目录保留源码 Worker。路径不接受绝对形式、`.`、`..`、反斜杠或盘符，也不通过 shell、`PATH`、环境变量或外部命令字符串查找。capability 名称为 `network`、`file`、`process`、`gpu`；重复值和未知值在检查期拒绝。Worker 握手必须精确回显已编译的 Port、Adapter、schema、操作和 capability 集合。Linux 的文件、网络、进程权限还由统一 OS 沙箱执行；文件授权仅来自 `config/setting.json` 的 Adapter 配置。GPU 与其他 OS 沙箱尚不支持，明确失败。宿主必须允许所需 namespace 和 seccomp；不会自动放宽 AppArmor 等主机策略。

`external` 只在 Adapter role 中有意义，且一个文件只能包含该声明和至多一个 `setting`。它完整实现且只能实现同领域一个 Port identity；不能同时写 Dever Adapter 函数或 helper。应用测试仍必须提供 case-local Dever Port fake，不启动生产 Worker，也不读取生产 Adapter setting。reference 后端明确不执行外部程序，正式执行只走 native 后端。

生态 Adapter 复用同一 Port 合同，声明中的 Lib 不带重复的生态前缀：

```dever
external pip "worker/mail.py" {
  lib "httpx@0.28.1"
  allow network
}
```

`pip/npm/go` 与 `exec` 使用相同的生命周期和 typed wire schema；各 Worker 的依赖与 runtime 独立锁在 `dever.lock`。Lib 解析是显式 `dever lib` 操作；`run/build` 只读取已锁定的资产。Python/JavaScript 打包器生成 SDK、合同和 runner，自动绑定入口中与 Port 操作同名的函数，不要求业务再写 handlers 注册表。运行树只从锁定 pack、依赖和当前 Adapter 源码生成；Python 固定使用 `-I -S -B`，Node 支持 ESM 和 CommonJS。启动解释器、argv 和工作目录必须匹配编译嵌入的资源清单，既不寻找系统语言环境，也不从部署配置接受任意命令。

Go Worker 使用锁定的目标专用 build pack 内的 compiler、linker 和标准库离线编译。入口源码声明 `package main`，按操作名导出 `func(context.Context, any, any) (any, error)`：`send` 对应 `Send`，`text.render` 对应 `TextRender`；生成入口静态绑定函数并由 Go 编译器检查签名，不要求业务手写 `main` 或注册表。构建按目标 build tags 选文件，支持 `go:embed`，导入只能来自标准库、当前 Adapter 源码、SDK 或锁定的 Go module 闭包。构建产物只携带 Worker 可执行文件与 checked 合同，启动不寻找系统 Go。参与解析的 `go.mod` 和最终 module ZIP 均验证官方 sumdb 签名、包含证明与 Go `h1`；项目现有锁保存 checkpoint，一致性和反回滚以该锁为锚，主动删除锁会重新建立锚。

`dever-lock-v6` 保存每个 Worker 的精确生态依赖，以及原始归档的准确来源和文件名。Python extras 使用规范化、排序后的 `pip:name[a,b]@version`，不同 Worker 不共享隐式 extras；实际选中 wheel 的 METADATA 决定依赖，原生 wheel 还必须符合签名解释器的 tags、扩展 ABI 和封闭动态库搜索规则。npm 保留每个安装实例的嵌套路径、peer 上下文、optional 省略原因和 bundled 归属，不能把不同依赖上下文压成一份全局版本表。

`dever lib install <root>` 按已有锁恢复 Package、Lib 和必要运行环境，不重新选择版本，不改项目配置和锁文件。缺少构建产物时使用收据中的完整固定输入重放，并核对输出摘要；第三方构建不能重现相同字节时明确失败。运行环境和构建工具通过机器共享签名扩展按需准备，`run/build` 仍不联网。ARM 应用资源先用 `dever target add linux-aarch64` 准备。

显式 `dever lib add/update` 可用签名 build pack 构建 PEP 517 sdist、执行 npm registry 包的 `preinstall/install/postinstall` 和原生 addon 编译。build pack 必须提供目标运行时、头文件、编译器、sysroot、固定 shell 和工具闭包；缺少的工具或库明确报错，不寻找宿主环境。Python 的静态与动态 build requirements 使用独立精确锁，console scripts 安装到隔离构建 prefix；metadata 与 wheel hook 分进程执行，最终 wheel 必须匹配父进程保存的 metadata。npm publishing hooks 不作为 registry 安装钩子执行，可选依赖构建失败会裁剪其不可用闭包，必需依赖失败则整个准备失败。

源码构建收据绑定源码摘要、运行时、工具包、固定 frontend、构建依赖与输出摘要；npm 还绑定完整安装图。`run/build` 离线消费已锁定输出，不再次执行上述安装钩子或源码构建。Node addon 在仅有目标 Node 和运行库的独立沙箱中逐个验证；所有 `.node` 文件必须有成功或明确拒绝记录，运行树只安装成功项，其他平台/ABI 的变体保留在原锁定归档中。本次源码生成的 addon 验证失败会使准备失败，不能改用纯 JavaScript 回退并声称原生构建成功。Linux 构建使用只读工具与输入、独占可写工作区并禁止网络。构建期限为五分钟，stdout/stderr 各限 16 MiB；工作区每 250 ms 检查 512 MiB/65,536 项预算并在超限时回收子进程，这不是内核磁盘 quota，检查间隔内可暂时超限。最终安装树仍限 256 MiB/65,536 项。普通 Worker 的能力由同一 OS 沙箱执行，其他平台不能借宿主工具绕过缺失的发行支持。

Dever Package 的请求放在 `config/setting.json.package`：`registry` 是显式 HTTPS registry origin，`use` 是 `name@version-range` 数组。`dever package add/update` 解析版本和传递依赖；Package 与 Lib 共用 `dever.lock` 和机器摘要缓存。Package 的源码归属于 `module/<name>/`，Worker 资源归属于同组件路径或 `worker/<name>/`；其他组件路径不能混入归档，也不能与项目本地组件重名。`check/test/run/build` 自动加载锁定源码，无须增加 import 或注册方法；缺失的锁定 Worker 必须失败，不能使用项目同名文件替代。`remove` 离线裁剪已验证锁，不重新选择剩余依赖版本。

`test/<component>/<domain>/<topic>.dever` 可在本文件写 `port.send(...) (...) { ... }` fake；每个可达 Port 必须完整提供 fake，不读取生产 Adapter 配置或回退生产实现。同一套件只编译一次，每个 case 在独立进程绑定自己的 fake。编译检查与代码生成也按 case 独立推断 effect、failure 和挂起行为，不合并不同测试的 fake；生产合同独立检查。

持久任务使用同领域 `job.dever`，复杂时按主题拆成平铺 `job/*.dever`。Job 只适配同领域 App，不直接调用 Model、Domain、Port、Adapter 或并发原语：

```dever
database default
job publish(input: app.Publish) () retry(5) timeout(30000) {
  app.apply(input)
}
job cleanup() () retry(3) timeout(1000) {
  app.clean()
}
schedule cleanup = "0 * * * *"
```

Job 必须声明数据库、`retry` 和 `timeout`。输入为零个或一个同领域 App record，输出为零；payload 使用共享 wire codec，允许保留 Model 身份的 ModelId，不允许 Secret、private 字段、资源或 Choice。`retry(1..100)` 包含首次执行，`timeout(1..3600000)` 单位为毫秒。入口不能作为普通函数或 handler 传递。

App 使用 `dever.job.enqueue(job.publish, input, key)`；无 payload 时省略 input。`enqueue_at` 在 key 后增加 DateTime。返回 Id；key 为 1..1024 字节的业务幂等键。同物理租户、执行租户、Job identity 与 key 的 pending/running 任务返回同一 Id，终结后允许重新使用 key。transaction App 内业务写入与入队共享物理事务；跨连接事务被拒绝。入队不会同步调用 handler 或继承其 effect/failure。

入队必须处于编译器建立的可信执行作用域。受保护 HTTP 入口在自动权限校验成功后保存 User 身份的 Provider、站点、subject、session、租户声明和该入口唯一的 permission key；不保存 JWT、Cookie、角色或权限集合。Worker 每次执行都重新调用配置绑定的 verify，确认该 key 仍在当前编译产物的权限目录，并查询当前角色；session、租户、组件或权限已失效时任务进入 blocked，业务 handler 不执行。public HTTP 调用链不能入队；CMD、schedule 和测试入口使用明确的 System 身份。Job payload 内的用户或租户字段始终只是业务数据，不能成为可信执行身份。

`schedule` 仅绑定本文件零输入 Job。cron 固定为 UTC 五字段（分、时、日、月、星期），支持数字、`*`、列表、范围、步长，星期为 0..6；日和星期均受限时按 OR 匹配。首次启用从当前分钟建立 cursor，不回放历史；后续每轮最多扫描并推进 256 个分钟槽，仅匹配 cron 的槽生成任务。cursor、occurrence 和推进同事务，多 scheduler 与已完成任务都不会重建同一窗口。修改已持久化的 cron fingerprint 会失败，需要明确迁移。

应用入口由编译器根据 Job 与 API 声明生成；两者同时存在时，共用运行时并由唯一配置文件 `config/setting.json` 选择启动模式：

```json
{
  "runtime": { "mode": "all", "shutdown_ms": 30000 },
  "job": { "workers": 2, "poll_ms": 250, "lease_ms": 60000, "retry_base_ms": 1000, "retry_max_ms": 300000 }
}
```

这里还需配置实际 `database`，API 模式需配置 `http`。`mode` 为 `api`、`worker` 或 `all`；省略 runtime 时编译产物按实际 API/Job 能力推导 api、worker 或 all，关闭期限默认 30000 ms。显式 mode 仍必须与源码能力和所需配置一致。worker 连接池至少比 worker 数量多一个连接，留给 claim/heartbeat。SQLite 的写事务会阻塞续租，因此还要求 `lease_ms` 大于每个 Job 的 `timeout`；PostgreSQL 支持较短租约并续租。未知字段、无效范围、所选 mode 缺少源码能力或设置均在监听/claim 前失败，不支持环境变量或命令行覆盖。

任务是至少一次执行；外部副作用仍需业务幂等。每次 claim 增加 attempt 并生成独立 token，续租和结果提交要求 token 与有效 lease 匹配。崩溃、取消后的任务保留 lease，到期才能重领，达到次数上限进入 dead。失败按有界指数退避；未知 target、schema 不兼容、坏 payload、无可信执行身份、身份/能力复核失败，或不满足当前租约约束的历史任务策略进入 blocked，错误摘要不保存业务明文。私有队列 schema 升级时，无法证明执行身份或能力的历史活跃 User 任务也会阻断，不会降级为 System。SIGTERM/Ctrl-C 停止接收新请求和 claim，排空到共同期限后取消在途工作，汇合后关闭数据库并 flush 日志。

应用测试可用 `dever.test.advance_clock(ms)` 推进 case 独立时钟，`dever.test.drain_jobs(limit)` 执行到期任务并返回处理数（limit 为 1..10000）。测试复用生产队列状态机、case 的 SQLite 与 Port fake，不启动服务或读取部署配置；没有可达 Job 时 drain 返回 0，也不因此要求数据库。

`job.lease_ms` 范围为 100..7200000 毫秒，Job timeout 上限仍为 3600000 毫秒。测试使用最大租约，保证所有合法 timeout 都能在隔离 SQLite 中运行。

路径示例：

```text
my_app/module/user/account/api.dever
my_app/module/user/account/app.dever
my_app/module/user/account/domain.dever
my_app/module/user/account/model.dever
my_app/module/news/article/app/publish.dever
my_app/module/news/article/app/query.dever
```

最后两项表示文章 App 已有至少两个内聚主题；对外仍调用 `news.article.publish(...)` 和 `news.article.count()`。只有一个主题文件时应改回 `news/article/app.dever`。

命名规则：

| 对象 | 格式 | 示例 |
| --- | --- | --- |
| component、domain、role、topic、function、字段和局部值 | `lower_snake_case` | `user.account`、`can_sign_in` |
| type、选择项 | `UpperCamelCase` | `User`、`Created` |
| 缩写 | 按普通单词处理 | `Id`、`HttpClient`、`user_id` |

### 2.1 Markdown 程序源码

使用 `.dever.md` 可以把程序和面向人的用途说明放在一起。Dever 代码仍写在顶层 `dever` 或 `typescript dever` 围栏中；`typescript dever` 让编辑器尝试使用 TypeScript 高亮，第二个标记用于确认其中仍是 Dever 源码。普通 `.md` 文档不参与编译。

Markdown 结构也是编译契约：一个一级标题说明包，一个二级标题说明一个 type 或一个逻辑函数。标题使用自然语言，技术名称、公开清单、输入输出和类型结构使用固定列表，并由编译器与代码核对。

例如 `user/account/api.dever.md`：

````markdown
# 问候接口

向调用方返回一条问候语。

- 包：`user.account.api`
- 公开类型：无
- 公开方法：无
- 使用：无

## 获取问候

绑定本领域的问候能力。

- 声明：`get greeting`

```typescript dever
get greeting = app.greet
```
````

规则如下：

- 文件必须有且只有一个一级标题。一级标题下先写非空包说明，再依次写 `包`、`公开类型`、`公开方法`、`使用`；一级标题下不能有 Dever 代码块。
- 每个 type、逻辑函数或 API 声明必须分别使用一个二级标题、非空用途说明和一个 Dever 代码块。一个逻辑函数包括同一名称、同一参数数量的全部条件分句；不同声明不能共用说明，分句也不能拆到多个说明中。
- type 说明使用 `类型` 以及 `字段` 或 `分支`。函数说明使用 `函数`、`输入`、`输出`；API 说明使用 `声明`。空清单显式写 `无`；非空输入、输出、字段和分支均须带自然语言说明。
- 源码身份必须与路径一致。App 的公开名称和调用方式由 role 自动得到；Model 的记录、choice 和 ID 类型由 Model role 自动提供；API 声明不列入公开方法。字段、分支、函数输入和输出的名称、类型、数量及顺序必须与代码一致；同一函数各分句的输入参数名必须相同。函数模式按解析后的统一类型记录，例如 `true`/`false` 分句写 `Bool`。
- 路径规则与普通源码一致；`user/account/app.dever.md` 的源码身份是 `user.account.app`，其中 `register` 的调用方式是 `user.account.register(...)`。
- 两种源码格式可以在一个项目中混用；Model/API 主文件可与 topic 目录并存，其他 role 二选一。
- 程序围栏使用至少三个反引号或波浪号，允许 0–3 个空格缩进，信息字符串须是字面的 `dever` 或 `typescript dever`；关闭围栏使用相同字符且长度不短于开始围栏，尾部只允许空格，不允许 Tab。程序围栏必须闭合，不能依靠文件结束隐式关闭。
- 三级及更深标题、表格、普通补充说明、其它语言及无语言标记的代码块不执行。引用、列表、HTML 或展示代码块内的围栏不作为程序。补充内容放在所属 Dever 代码块之后。
- 函数输入输出、条件分句、类型、失败义务、`pure` 和依赖边界都按现有 Dever 规则检查，说明文字不能代替正式契约。
- `check/api/run/build` 命令不变，并强制检查 Markdown 契约；错误显示 `.dever.md` 原文中的行列并关联对应代码。`fmt` 检查结构并仅更新程序块内部，保留说明、围栏和展示代码块。

固定列表的完整格式见 [Dever Markdown 源码规范](MARKDOWN-SYNTAX.md)，完整接口示例见 [Markdown CMS](examples/cms/md/module/news/article/api/admin/manage.dever.md)。`typescript dever` 只复用现有高亮规则；精确的 Dever 高亮、补全和语言服务仍需编辑器扩展。

### 2.2 应用测试

应用测试位于项目根的 `test/<component>/<domain>/<topic>.dever` 或 `.dever.md`。文件名 `<topic>` 选择同名的普通 `topic() ()` 作为唯一入口；入口不能有参数或输出，也不能声明为 transaction。文件中的其他函数和类型只供本文件使用，不会自动执行或向生产源码、其他测试公开。路径只允许 component、domain、topic 三层，不建立 `common`、`shared` 或全局 fixture 目录。

测试是所属领域的受控友元：可调用同领域 App 和 Domain，跨领域只能调用目标 App。测试可以读取 App 返回的 Model 值，但不能直接执行 Model CRUD，也不能调用 Port、Adapter、API 或其他测试。生产源码同样不能反向依赖测试。测试不能启动 `dever.api.serve()`。

首版断言只有：

```dever
register() () {
  account = user.account.register("New@Example.com", "New", "hash")
  assert(account.email == "new@example.com")
  assert_eq(account.display_name, "New")
}
```

`assert` 只接受 Bool；`assert_eq` 要求两侧类型完全相同且可比较，不做隐式数值转换。失败报告测试身份、原始源码位置和实际/期望值。业务失败继续用 `result(call)` 与 choice 分句验证，不增加异常断言语法。

`dever test <project-root>` 在执行任何用例前检查全部生产和测试源码，并把所有测试入口编译成一个可缓存的原生测试程序；每个测试仍在独立原生进程与临时项目中运行。不使用 Model 的测试不读取数据库配置或初始化数据库；使用 Model 的测试走正式迁移、Seed、事务和关闭流程，但配置由运行器在临时目录生成且只使用 SQLite。测试命令不会读取项目 `config/setting.json` 或项目 `data/`，也不从环境变量接收数据库覆盖；成功和失败都会清理临时文件。首版不提供共享 setup、筛选、并行、重试或自动超时。

## 3. 类型与值

### 3.1 基础类型

| 类型 | 含义 |
| --- | --- |
| `Bool` | `true` 或 `false` |
| `Int` | 有符号 64 位整数，溢出不会环绕 |
| `Decimal` | decimal128，最多 34 位有效数字，银行家舍入 |
| `Float` | IEEE 754 binary64 近似数 |
| `Text` | UTF-8 文本，不做隐式 Unicode 规范化 |
| `Id` | 不透明稳定标识 |
| `Bytes` | 二进制字节序列 |
| `Secret` | 不透明敏感值；仅受控密码学及输入边界使用 |

literal 规则：

```dever
enabled = true
count = 42
price = 19.90
ratio = 1.25e6
name = "中文和 Unicode 都可以"
missing = null
```

- 整数 literal 默认是 `Int`。
- 带小数点的 literal 默认是 `Decimal`；明确的 Float 上下文可将其作为 `Float`。
- 科学计数 literal 是 `Float`。
- Text 支持 `\\`、`\"`、`\n`、`\r`、`\t` 和 `\u{HEX}`。
- `Decimal` 与 `Float` 不隐式混合；Int 转 Float 使用 `float.from_int`。

### 3.2 字段型 type（record）

字段型 type 是固定结构的数据：

```dever
type User {
  id: Id
  name: Text
  phone: Text?
}

create_user(id: Id, name: Text) (user: User) {
  user = User {
    id = id
    name = name
    phone = null
  }
}

rename(user: User, name: Text) (result: User) {
  result = user
  result.name = name
}
```

构造时必须一次写全全部字段。未知、重复或遗漏字段都是编译错误。字段只能在完整值上读取或修改，使用 `value.field = expression` 语法；不能靠逐字段赋值创建一个半初始化值。

### 3.3 选择型 type（choice）

选择型 type 表示一个值当前只能处于其中一种状态：

```dever
type CreateResult {
  Created(user: User)
  InvalidName(reason: Text)
  EmailAlreadyUsed(email: Text)
}

result = CreateResult.InvalidName("name is required")
```

选择项必须通过所属 type 限定。选择项可以不携带数据，也可以声明一个或多个有名称、有类型的携带值。携带值的名称属于 type 契约；构造和匹配仍按声明位置书写，不是命名参数调用。

### 3.4 可空、List 与 Map

`Value?` 表示 `Value` 或 `null`。没有 `Value??`、隐式默认值、`?.` 或强制解包。

```dever
names = ["Ada", "Lin"]
labels = {
  "env" = "production"
  "region" = "china"
}
```

- List 的元素必须具有相同静态类型。
- Map 的类型是 `Map<Key, Value>`，保留插入顺序。
- Map key 可以是 Text、Int、Bool、Id 或不携带数据的选择型 type。
- Decimal、Float、可空值、字段型值、List 和 Map 不能作为 Map key。
- 空 `[]` 和 `{}` 必须能从参数或输出等上下文推断类型。
- `get`、`first` 和 `find` 返回可空值；可空化是幂等的。
- `entries(map)` 返回保持插入顺序的 `List<MapEntry<Key, Value>>`；每个 `MapEntry` 有 `key` 和 `value` 两个字段。修改局部 MapEntry 仍遵守值语义，不会回写原 Map。

### 3.5 Stream 与资源

`Stream<Item>` 是按需拉取、顺序、一次消费的流。文件、Socket、Listener 和 Stream 是资源值；复制它们会建立指向同一资源状态的别名。Stream 的所有别名共享一个读取位置，任何别名消费或关闭后，其他别名都会看到相同状态。

`AsyncStream<Item>` 是异步拉取的资源流，等待下一项时挂起当前协程。它的别名共享读取位置和关闭状态；元素可跨线程传递时，流也可跨线程传递。它不与同步 Stream 隐式转换，不预取或自动收集为 List。

普通字段型值、选择型值、List 和 Map 使用值语义：修改一个局部名称不会改变调用方或其他名称持有的值。

## 4. function 与命名输出

Dever 的所有行为都由 function 定义：

```dever
line_total(price: Decimal, quantity: Int) (total: Decimal) {
  total = price * quantity
}
```

- 第一组括号是输入，第二组括号是命名输出。
- 没有输入或输出时仍写 `()`。
- 不使用 `function` 或 `return` 关键字。
- 调用按位置传参，不支持命名参数调用。
- 零输出调用是一条动作语句。
- 单输出调用直接得到值。
- 多输出调用得到只能按输出名称访问的结果。
- 用户 function 不能直接或间接递归。
- function 签名由 package、名称和输入数量确定；同名但输入数量不同的 function 是不同签名。

多输出示例：

```dever
name_parts(value: Text) (original: Text, normalized: Text) {
  original = text.trim(value)
  normalized = text.lower(original)
}

display(value: Text) (result: Text) {
  parts = name_parts(value)
  result = parts.original + ":" + parts.normalized
}
```

普通应用没有手写 `main()`；命名输出保留在 App 函数中，API/CMD 绑定一个可序列化输出并封装为 JSON。

## 5. 分句是唯一条件机制

Dever 没有 `if`、`else`、`switch`、`match` 或条件表达式。所有业务分支都使用同名 function 分句：

```dever
type OrderStatus {
  Draft
  Paid
  Rejected(reason: Text)
}

status_text(status: OrderStatus.Paid) (result: Text) {
  result = "paid"
}

status_text(status: OrderStatus.Rejected(reason)) (result: Text) {
  result = reason
}

status_text(status: other) (result: Text) {
  result = "pending"
}
```

可空值也通过分句处理：

```dever
phone_text(phone: Text) (result: Text) {
  result = phone
}

phone_text(phone: null) (result: Text) {
  result = "not provided"
}
```

数值范围可以直接写在输入模式中：

```dever
grade(score: Int < 60) (result: Text) {
  result = "fail"
}

grade(score: Int >= 60 and < 90) (result: Text) {
  result = "pass"
}

grade(score: Int >= 90) (result: Text) {
  result = "excellent"
}
```

同一签名的全部分句必须紧邻书写。中间插入 type 声明、其他 function 或另一输入数量的同名 function 后，再次书写原签名会直接报错，不会形成第二组规则。

分句输入支持：完整类型、选择项、`null`、Bool/Text/数值常量、Int/Decimal 范围和 `other`。选择项必须为每个携带值提供一个位置绑定；不需要使用的绑定可以写 `_`。编译器把一组分句作为规则检查：

- 输入和输出契约一致；
- 分句互不重叠；
- 所有可能值都被覆盖；
- 没有不可达分句。

分句没有源码顺序优先级。多输入分句按所有输入组合检查覆盖和重叠；`other` 是该输入位置在整组明确模式之外的补集，只能用于分句输入。

## 6. 线性 body 与运算

function body 只包含顺序赋值和 function 调用：

```dever
final_price(price: Decimal, rate: Decimal) (result: Decimal) {
  discounted = price * (1 - rate)
  result = decimal.round(discounted, 2)
}
```

`=` 用于初始化，也用于修改当前 function 内的 input、局部值或 output。编译器保证读取前已初始化、字段宿主完整、局部类型稳定，并要求退出前给所有输出赋值。

运算规则：

| 运算 | 适用范围 |
| --- | --- |
| `+ - *` | 同类 Int、Decimal、Float；Int 可无损提升为 Decimal |
| `/` | Int/Int 与 Decimal 运算返回 Decimal；Float/Float 返回 Float |
| `//`、`%` | 仅 Int，整数除法向零截断 |
| `+` | 也用于两个 Text 的连接 |
| `== !=` | 相同的可比较类型 |
| `< <= > >=` | Int、Decimal、Float |
| `and or not` | Bool，`and` 和 `or` 短路 |

相等比较也适用于成员本身可比较的字段型、选择型、可空值、List、Map 和 MapEntry。List 按位置比较；Map 按插入顺序比较，所以键值相同但插入顺序不同的两个 Map 不相等。Float 遵守 IEEE 754 规则，`NaN == NaN` 为 `false`，包括它嵌套在集合或字段中时。

优先级从高到低为：调用和字段访问、一元 `-`/`not`、`* / // %`、`+ -`、比较、`and`、`or`。同级二元运算从左到右，括号可以改变顺序。

Int 溢出、Decimal 除零等静态可知故障在检查阶段报错；运行时才知道的数值故障会终止当前执行并报告 `.dever` 文件、行和列。

## 7. 集合与受控重复

Dever 没有 `for`、`while` 或用户递归。重复处理使用具名 handler 和标准组合器：

```dever
views = each(to_view, users)
active = filter(is_active, users)
user = find(has_id, users, expected_id)
total = sum(prices)
total = sum(line_total, lines, tax_rate)
state = reduce(update_state, values, initial_state)
state = reduce_until(update_until_done, stream, initial_state)
parallel_each(send_one, jobs, 4, context)
```

| 操作 | 可处理序列 | handler / 结果 |
| --- | --- | --- |
| `each` | List、Bytes；Stream、AsyncStream 仅限零输出 handler | 零输出时执行动作；单输出时生成 List |
| `filter` | 仅 List | handler 输出 Bool，返回保留原顺序的 List |
| `find` | 仅 List | handler 输出 Bool，返回第一个匹配项或 `null` |
| `sum` | 仅数值 List | 直接求和，或用单输出数值 handler 投影后求和 |
| `reduce` | List、Bytes、Stream、AsyncStream | handler 为 `(Item, State) -> State`，返回最终状态 |
| `reduce_until` | List、Bytes、Stream、AsyncStream | handler 输出下一状态和 `stop: Bool`，处理当前元素后可停止 |
| `parallel_each` | List、Bytes、Stream | 非挂起零输出 handler；第三个参数是请求并发数，范围为 1–256 |
| `parallel_each` | List、Bytes、AsyncStream | 挂起零输出 handler；第三个参数是任务组容量，范围为 1–65536，同时受全局任务容量约束 |

Bytes 的元素类型是 Int，值域为 0–255。List、Bytes 可使用非挂起或挂起 handler；同步 Stream 只使用非挂起 handler。AsyncStream 的 `each`、`reduce`、`reduce_until` 可使用两类 handler，具体类别由编译器推断。非挂起 handler 不能在挂起执行路径中隐藏阻塞或未限定的 handler effect。`each`、`filter`、`find` 和投影式 `sum` 最多可以带一个末尾 context，handler 的输入顺序是元素、context。`parallel_each` 的可选第四个参数也是 context。需要多项 context 时，先组合成一个字段型值。`reduce` 和 `reduce_until` 不接收 context；共享数据应放入 State。

空 List 的 `sum` 返回相应数值类型的零；空序列的 `reduce` 和 `reduce_until` 原样返回初始状态。`reduce_until` 在处理当前元素并得到 `stop = true` 后停止；如果输入是 Stream，下一个元素保持未消费，可以通过任一 Stream 别名继续读取。

AsyncStream 消费会使当前函数被推断为挂起。组合器本身顺序等待拉取和 handler 完成，不需要额外包装。消费返回、提前停止、故障或取消都会关闭这个异步流，其他别名不能继续消费。`parallel_each` 先等待空位再拉取，并在等待下一项时观察子任务故障；返回前清理已启动的工作。`close(stream)` 本身不挂起，会唤醒等待中的拉取。

`reduce` 示例：

```dever
add(value: Int, total: Int) (next: Int) {
  next = total + value
}

total(values: List<Int>) (result: Int) {
  result = reduce(add, values, 0)
}
```

`reduce_until` 的 handler 必须输出下一状态和名为 `stop` 的 Bool：

```dever
take_until_limit(value: Int, total: Int) (next: Int, stop: Bool) {
  next = total + value
  stop = next >= 100
}
```

除 `parallel_each` 外，List 和 Bytes 按位置顺序处理，Stream 按拉取顺序处理，挂起 handler 逐项等待完成。完全非挂起的 `parallel_each` 创建不超过请求并发数的作用域线程。处于挂起调用链时，命名的非挂起 handler 复用 runtime 的 blocking worker，实际并发受该 worker 上限约束，且不能再次触发并发；挂起 handler 则复用有界 Group，先等待任务容量再取下一项。`parallel_each` 的 Stream 仍由单一生产端顺序拉取，不会先把整个流收集到内存。handler 的业务错误保留原类型并由调用方继续传播，不转换成普通字符串。handler 副作用的完成顺序不保证，调用返回前会等待所有已启动工作结束。handler 输入和 context 不能包含 Stream。

## 8. 静态 handler

function 可以接收受限的静态 handler：

```dever
transform_all(
  transform: handler(value: Int) (result: Text),
  values: List<Int>
) (results: List<Text>) {
  results = each(transform, values)
}
```

handler 只能是签名匹配的具名 function，或者当前 function 收到的 handler 参数。它不是普通运行时值，不能存入局部值、字段、List、Map、Stream，不能比较，也不能作为输出返回。

## 9. 挂起推断与结构化并发

所有 function 使用同一种声明语法。普通调用保持顺序完成；如果调用链包含网络、非阻塞定时器、AsyncStream 或其他挂起操作，编译器会沿静态调用图推断挂起 effect，并在生成的 Rust 内部插入 `.await`。Dever 源码不声明 `async`，也不写 `await`：

```dever
sleep_ok(result: dever.time.SleepResult.Done) () recover("this operation may continue after timer failure") {}

sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("this operation may continue after timer failure") {}

delayed_value(value: Int) (result: Int) {
  sleep_ok(dever.task.sleep(10))
  result = value
}

main() (result: Int) {
  task = run(delayed_value(42))
  result = wait(task)
}
```

直接调用始终等待结果；只有 `run(call(...))` 显式并发启动。`run` 同时支持挂起函数和普通函数，普通函数会进入受限 blocking worker，不会堵塞调度线程。它返回局部仿射 `Task`：Task 不能复制、嵌套进其它值、跨线程传递或作为输出返回，并且必须恰好由一次 `wait(task)`、`stop(task)`、`timeout` 或 `race` 消费。`stop` 仅接受零输出 Task，语义是请求取消并等待任务停止；取消不会回滚已经发生的外部副作用。Task 离开异常路径时也会请求取消，不会成为脱离父作用域的后台任务。

动态数量的零输出任务使用 `Group`：

```dever
workers = group(8)
run(workers, write_one(message))
wait(workers)
```

`group(limit)` 的 limit 范围是 1–65536。达到 limit 后，新的 `run(group, call(...))` 会等待已有任务完成，不会无限排队。Group 也是局部仿射值，最终必须由 `wait(group)` 或 `stop(group)` 消费；第一个子任务失败时，其余子任务会被取消并等待结束。

CPU 计算和阻塞调用必须显式分类：`parallel(pure_call(...))` 只接受 pure 非挂起 function；`blocking(blocking_call(...))` 只接受 effect 分析已识别为阻塞系统边界且不含未知 handler effect 的非挂起 function。文件、同步睡眠和标准输出属于阻塞边界；TCP 调用会被推断为挂起。两种边界的参数都在提交前按源码顺序求值，工作由有界线程池承担。挂起函数不能原地执行阻塞调用、隐藏并发的集合 handler 或 effect 未知的非挂起 handler 参数；必须使用 `blocking` 或调整调用结构。

有界 Channel 用于任务通信：

```dever
messages = channel(Text, 64)
send(messages, "ready")
message = receive(messages)
close(messages)
```

Channel 容量范围是 1–65536。`send`、`receive` 会使当前调用链被推断为挂起；队列满时发送方等待，队列空时接收方等待。`receive` 返回 `T?`：收到值时返回该值，Channel 关闭且缓存值耗尽后返回 `null`。Dever 不产生嵌套可空类型，因此 `Channel<T?>` 的接收结果仍是 `T?`，发送的 `null` 与关闭后的空结果都表现为 `null`。`close(channel)` 幂等，并唤醒等待中的发送方和接收方；关闭前已经缓存的值仍可读完。`Channel<T>` 和 `AsyncStream<T>` 的 T 必须可跨线程传递。

跨挂起点仍存活或进入 `run`、Channel、`parallel`、`blocking` 的普通值必须可跨线程传递。Task 和 Group 是受编译器跟踪的局部仿射值，不能嵌套、传参或逃逸。完全非挂起的入口不初始化 Tokio；推断为挂起的入口创建唯一的有界 Tokio 多线程运行时。默认调度线程数按 CPU 数量确定且最多 64，CPU/阻塞 worker 默认最多 8，同时存活的 `run`/Group 子任务默认最多 4096，硬上限为 65536。

`dever.task.sleep(milliseconds)` 是非阻塞定时等待，要求非负毫秒数，成功无输出，失败默认传播；显式捕获使用 `dever.time.SleepResult`。已有 `dever.time.sleep` 保持同步阻塞语义；挂起调用链需要同步睡眠时必须使用 `blocking(...)` 隔离。运行时由生成入口统一启动，业务源码不手工创建或嵌套 runtime。

`timeout(task, millis, fallback)` 等待任务；millis 必须为正。到期先停止并排空任务，再执行零输入的静态 fallback。fallback 必须与任务的命名输出签名完全相同，支持零个、一个或多个输出。程序故障继续传播，不触发 fallback。`race(first_task, second_task, ...)` 至少接收两个输出签名相同的 Task，等待第一个完成，再停止并排空其余任务；它也不吞掉程序故障。两者会使调用链挂起，并消费传入的全部 Task。

```dever
unavailable() (value: Text) {
  value = "请求超时"
}

# lookup 的输出声明为 (value: Text)。
pending = run(lookup())
value = timeout(pending, 1000, unavailable)

first_task = run(lookup_primary())
second_task = run(lookup_secondary())
value = race(first_task, second_task)
```

`stream(channel)`、`stream(list)`、`stream(bytes)` 创建按需拉取的 AsyncStream；元素必须可传递，Bytes 产生 Int。消费流时由编译器推断挂起。Channel 流先读完关闭前的缓存再结束，发送的 null 仍是一个实际元素。关闭流释放其 Channel 别名，不关闭调用方另持有的 Channel。

`dever.task.ticks(millis)` 创建 `AsyncStream<Int>`，周期必须为正；第一个值在一个周期后产生，序号从 1 开始。消费过慢时跳过错过的周期，不积压 tick 或预先创建任务。用 `each`/`reduce_until` 消费，关闭流释放定时器。

## 10. 失败处理

可恢复失败使用 choice 中显式标记的 `error` 变体。编译器不根据 `Failed` 等名称猜测错误：

```dever
type MessageResult {
  Ready(text: Text)
  error Failed(message: Text)
}

read_message(bytes: Bytes) (message: Text) {
  message = dever.bytes.to_text(bytes)
}

required_port(value: Int < 1) (port: Int) {
  fail(MessageResult.Failed("port must be positive"))
}

required_port(value: Int >= 1) (port: Int) {
  port = value
}
```

普通调用默认传播失败，因此 `message = dever.bytes.to_text(bytes)` 不需要 `?`、`try` 或 `await`。调用链的错误集合由编译器推导；未处理的失败到达 HTTP、Task 或 CLI 根边界时，由运行时统一回滚事务、记录一次并终止对应执行边界。

需要主动检查成功和全部错误变体时，使用 `result(call)` 捕获，然后交给穷尽分句：

```dever
decode(result: dever.bytes.DecodeResult.Decoded(text)) (message: Text) recover("invalid input is shown to the user") {
  message = text
}

decode(result: dever.bytes.DecodeResult.Failed(reason)) (message: Text) recover("invalid input is shown to the user") {
  message = reason
}

main() (message: Text) {
  message = decode(result(dever.bytes.to_text(dever.bytes.from_text("hello"))))
}
```

`fail(error_variant)` 主动产生一个领域失败并立即结束当前调用；只能传入带 `error` 标记的变体。`result(call)` 只关闭该次调用的默认传播，捕获结果仍必须被穷尽处理、继续返回或显式恢复。Dever 不提供 exception、`throw` 或 `catch`。

错误必须传播、传给同样受检查的处理函数，或转换成明确错误。读取消息、打印日志、覆盖变量或交给空函数都不算处理。主动降级需要声明 `recover("具体原因")`，同一函数所有分句必须声明相同理由；例如可选本地配置缺失时采用固定默认值。理由必须非空，编译器不判断这项业务策略是否合理。详见第 18 节。

编程错误或不可恢复运行时故障会终止当前执行，并带源码位置；普通 function 不能捕获这类故障。

## 11. 常用核心操作

无需 package 前缀的核心名称：

```text
Bool Int Decimal Float Text Id Bytes Secret List Map MapEntry Stream AsyncStream Channel
true false null other
each parallel_each reduce reduce_until filter find sum
append first get put remove entries length close
run wait stop group parallel blocking channel send receive timeout race stream
```

List 与 Map 操作：

```dever
extended = append(values, value)
first_value = first(values)
label = get(labels, "env")
labels = put(labels, "env", "development")
labels = remove(labels, "env")
pairs = entries(labels)
size = length(values)
```

`append`、`put` 和 `remove` 返回新值，不修改原集合。`first(List)` 和 `get(Map, key)` 返回可空值。`entries(Map)` 返回按插入顺序排列的 MapEntry List。`length` 接受 List、Map、Text 或 Bytes；Text 按 Unicode codepoint 计数。核心 `close` 接受 Stream、AsyncStream 或 Channel，且没有输出。

## 12. 标准 package

标准 package 本身也使用 Dever 编写。下表链接到当前公开源码；其中的 `public` 是编译器随附标准库的内部边界，不是应用源码语法范本：

| package | 已提供的主要接口 |
| --- | --- |
| [`text`](library/text.dever) | `trim`、`lower`、`upper`、`at`、`slice`、`index_of`、`contains`、`starts_with`、`ends_with`、`split`、`replace`、`join`、`is_empty`、Unicode codepoint 转换 |
| [`int`](library/int.dever) | `parse`、`to_text` |
| [`decimal`](library/decimal.dever) | `parse`、`to_text`、`from_int`、`round` |
| [`float`](library/float.dever) | `parse`、`to_text`、`from_int` |
| [`math`](library/math.dever) | `sqrt`、`sin`、`cos`、`log`、`pow`、Float 分类 |
| [`dever.id`](library/dever/id.dever) | `from_text`、`to_text` |
| [`dever.bytes`](library/dever/bytes.dever) | Text/Bytes 转换、`from_ints`、`length`、`at`、`slice`、`concat` |
| [`dever.process`](library/dever/process.dever) | `arguments` |
| [`dever.crypto`](library/dever/crypto.dever) | `token`、`password_hash`、`password_verify`、`sha256`、`hmac_sha256`、`constant_time_eq` |
| [`dever.time`](library/dever/time.dever) | `now`、`parse_datetime`/`format_datetime`、`parse_date`/`format_date`、`parse_time`/`format_time`、`duration`、`add`、`subtract`、`difference`、底层时钟与 `sleep` |
| [`dever.task`](library/dever/task.dever) | 非阻塞 `sleep` 和周期 `ticks` |
| [`dever.io`](library/dever/io.dever) | 标准输出、文件创建/打开/读写/关闭、分块 Stream |
| [`dever.net`](library/dever/net.dever) | TCP 连接、监听、读写、超时、连接与数据 Stream |
| [`dever.json`](library/dever/json.dever) | JSON `parse`、`stringify`、节点/属性/数组元素访问 |
| [`dever.http`](library/dever/http.dever) | Hyper HTTP/1、HTTP/2 多路复用、流式客户端和静态 handler |

数值或索引解析失败通常返回可空值；文件、网络、Bytes、JSON、HTTP 和 ORM 的可恢复错误默认沿调用链传播。普通调用返回成功值，`result(call)` 才产生显式选择型结果；具体以 function 输出类型为准。

### UTC 时间和类型化 wire

`DateTime` 内部是 Unix epoch 有符号整数毫秒；`Date` 是 UTC 午夜对应的 epoch 毫秒，`Time` 是午夜起 `[0, 86400000)` 毫秒，`Duration` 是有符号毫秒。数据库表示、排序和 Model fingerprint 不随外部格式变化。

`dever.time.now()` 返回 UTC `DateTime`。`parse_datetime(Text)` 接受 RFC3339 及显式 offset，`format_datetime(DateTime)` 输出 UTC `YYYY-MM-DDTHH:MM:SS[.sss]Z`；零毫秒省略小数，非零固定三位。输入最多三位小数，拒绝闰秒、子毫秒精度和 UTC 转换后超出四位年份的日期。`parse_date`/`format_date` 使用 `YYYY-MM-DD`；`parse_time`/`format_time` 使用 `HH:MM:SS[.sss]`。日历无效或格式化值不在可表示范围内会失败。

`duration(millis: Int)` 构造 Duration；`add(DateTime, Duration)`、`subtract(DateTime, Duration)` 和 `difference(later: DateTime, earlier: DateTime)` 使用 checked i64 算术，最后一个返回 Duration。算术溢出显式失败；合法 i64 结果仍可能超出四位日历格式化范围。确定性转换与算术是 `pure`，读取当前时间有 time effect。普通调用传播失败，显式 `result(call)` 使用 `DateTimeResult`、`DateResult`、`TimeResult`、`DurationResult` 或 `TextResult`，成功分支均为 `Read(value)`，错误分支为 `Failed(message)`。

共享 wire schema 从已检查类型生成具体编解码器。Bool/Int/Float 使用 JSON 标量；Decimal/Id/Uuid 使用字符串，不经过 Float；ModelId 使用 i64 整数且 schema 保留 Model 的 nominal identity；DateTime/Date/Time 使用上述字符串格式，Duration 使用整数毫秒。支持 List、nullable 和没有 private 字段或字段范围约束的 record。缺失 nullable 字段得到 null，缺失必填字段、未知字段、重复键、错误标量和非有限 Float 均失败；Int 只接受 i64 范围内的整数字面量。Choice、Map、Related 和资源不属于当前 wire 合同。

边界 JSON 最多 16 MiB、64 层容器、65536 个值（含根和容器）。Json 保留验证后的原始 JSON 和数字拼写，不经过动态浮点转换。Secret 仅允许 API/setting 输入策略生成，普通输出和 Job payload 禁止；不同策略具有不同 schema identity。业务函数接收具体类型，边界节点不会传入业务代码。API response、Adapter setting 和 Job payload 复用此 codec；API 输入的后续扩展也必须复用这个 owner。

### JSON

JSON 解析使用扁平、带类型的 `Document`，不产生动态对象。`dever.json.parse` 直接返回 `Document`；`result(dever.json.parse(...))` 使用 `ParseResult` 捕获失败。这些公开 type 位于 `dever.json.value`，契约是：

```dever
type Value {
  Null
  Boolean(value: Bool)
  Number(raw: Text)
  String(text: Text)
  Array(children: List<Int>)
  Object(members: Map<Text, Int>)
}

type Document {
  root: Int
  nodes: Map<Int, Value>
}

type ParseResult {
  Parsed(document: Document)
  error Failed(position: Int, message: Text)
}

type WriteResult {
  Written(text: Text)
  error Failed(node: Int, message: Text)
}
```

调用签名：

```text
dever.json.parse(Text) -> dever.json.value.Document
dever.json.stringify(dever.json.value.Document) -> Text
dever.json.node(dever.json.value.Document, node_id: Int) -> dever.json.value.Value?
dever.json.property(dever.json.value.Document, object_id: Int, name: Text) -> Int?
dever.json.element(dever.json.value.Document, array_id: Int, index: Int) -> Int?
```

`Document` 用整数 ID 保存一棵 JSON 树：`root` 是根节点 ID；`nodes` 的 ID 从 0 连续递增；子节点 ID 小于父节点 ID。Object 将属性名映射到子节点 ID，Array 保存有序子节点 ID。`property` 和 `element` 返回节点 ID，再通过 `node` 取得 Value；缺失、类型不符或越界时返回 `null`。JSON 的 `null` 则是实际存在的 `Value.Null`，两者不同。

数字保留原始 JSON 文本，避免先丢失精度。解析失败位置是从 0 开始的 Unicode codepoint 偏移，包括输入末尾位置。重复键、非法转义、未配对代理项、多余输入和超过 256 层的嵌套会传播 `ParseResult.Failed`。手工构造 Document 时，`stringify` 会检查根节点、连续 ID、子节点引用和输出规模，失败时传播 `WriteResult.Failed`；两者都可用 `result` 显式捕获。完整的解析和写回分句见 [标准库示例](examples/old/dever/library/module/main.dever)。

### HTTP

`dever.http` 使用 Hyper HTTP/1 和 HTTP/2 引擎，业务 handler 由 Dever 编译为静态 Rust async 调用。HTTP/1.1 支持持久连接、流水线、Content-Length 和 chunked；HTTP/2 在同一连接上并发处理多个请求流。

```text
dever.http.default_limits() -> Limits
dever.http.default_http2_limits() -> Http2Limits
dever.http.header(name: Text, value: Text) -> Header
dever.http.header_value(List<Header>, name: Text) -> Bytes?
dever.http.serve(route, dever.system.Listener, Limits) -> ()
dever.http.send(address: Text, port: Int, Request, Limits) -> Response
```

`Request` 包含 `method: Text`、`target: Text`、`headers: List<Header>`、`body: Bytes`；`Response` 包含 `status: Int`、`headers`、`body`。`Header` 包含 `name: Text` 和 `value: Bytes`，因此重复头和非 UTF-8 头值可以保留。接收到的名称为小写，列表保留同名头的值顺序，不承诺不同名称之间的原始排列。`header` 构造文本头；`header_value` 忽略名称大小写并返回第一项，找不到返回 null。

| Limits 字段 | 默认值 | 约束 |
| --- | --- | --- |
| header_bytes | 8192 | 至少 8192；协议解析缓冲区和头部预算 |
| body_bytes | 1048576 | 非负；单条请求/响应正文的上限 |
| timeout_ms | 5000 | 正数；分阶段服务端期限，客户端单次请求总期限 |
| connections | 32 | 1–65536；服务端并发连接数或每个客户端池的连接上限，另受任务资源约束 |
| http2 | null | null 使用 HTTP/1；Http2Limits 使用 HTTP/2 |

开启 HTTP/2 沿用原来的服务和客户端函数：

```dever
http_limits() (limits: dever.http.Limits) {
  limits = dever.http.default_limits()
  limits.connections = 2
  limits.http2 = dever.http.default_http2_limits()
}
```

| Http2Limits 字段 | 默认值 | 约束 |
| --- | --- | --- |
| streams | 16 | 每连接并发流上限；connections × streams 不超过 65536，另受运行时任务容量约束 |
| stream_window_bytes | 65535 | 单流接收窗口，1–2147483647 |
| connection_window_bytes | 262144 | 连接总接收窗口，65535–2147483647 |

HTTPS 的双方配置必须协商 ALPN `h2`；明文双方使用 HTTP/2 prior knowledge，不发送 h2c Upgrade，不自动降级协议。窗口固定，不自动扩大；帧和引擎发送缓冲设置为 16 KiB。窗口限制协议接收流量，不是整个进程内存上限：已收集的请求正文、每流待发块、TLS/系统缓冲和应用持有的值还需要内存。小机器应一起设置 connections、streams、body_bytes 和 chunk_bytes。

HTTP/2 的 `Request.target` 仍为 `/path?query` 或 OPTIONS `*`。引擎负责伪头；入站 authority 在没有 Host 时规范化为 Host，已有 Host 必须与 authority 一致。应用不能传入 Connection、Keep-Alive、Proxy-Connection 等逐连接头字段；请求 TE 只允许 trailers，响应不允许 TE。单流中止不关闭相邻流；连接级故障影响该连接的全部流。

`serve` 和 `send` 的正文在限额内完整缓冲；流式服务使用下节的 `serve_live`。单帧正文共享字节存储，多帧逐步合并，不为每个微小帧积累元数据队列。HTTP 引擎负责报文定界；应用不得传入 Content-Length、Transfer-Encoding 或 Upgrade。响应状态为 200–599，204/205/304 的正文必须为空；HEAD 由引擎抑制响应正文。HTTP/1 接收头数量沿用 Hyper 默认上限 100，HTTP/2 使用解压后的头列表字节预算；发送头预算为引擎生成的头预留 80 字节。

route 的签名为 `handler(request: dever.http.Request) (response: dever.http.Response)`；是否挂起由 route 的调用图推断：

```dever
route(request: dever.http.Request) (response: dever.http.Response) {
  response = dever.http.Response {
    status = 200
    headers = [dever.http.header("Content-Type", "text/plain; charset=utf-8")]
    body = dever.bytes.from_text(request.target)
  }
}

start(listener: dever.system.Listener) () {
  dever.http.serve(route, listener, dever.http.default_limits())
}
```

先用 `dever.net.listen(...)` 创建 Listener，再调用 serve；编译器会自动等待网络结果。完整启动分句见 [HTTP 示例](examples/old/dever/http/module/main.dever)。serve 持续接收连接，可作为 `server = run(dever.http.serve(...))` 启动，再用 `stop(server)` 取消并等待子任务清理。调用者持有的 Listener 别名仍由调用者关闭。

- 服务端先预留任务容量，再接受连接；没有无界应用队列或每连接独占线程。每条 HTTP/1 连接按顺序执行 handler。
- 握手超时或底层 I/O 错误关闭该连接；正文超时返回 408，handler 超时返回 504，正文超限返回 413。HTTP/1 随后关闭连接，HTTP/2 结束对应流。写入停滞超过期限关闭连接。
- 期限和取消在异步挂起处生效；已开始的 blocking 操作仍遵守既有“等待完成”的契约，需要操作自身的超时。
- 对端协议错误影响对应流或连接。无效服务配置、accept 故障、handler 程序故障及无效响应沿 serve 的任务作用域传播，并保留 Dever 故障位置。
- send 为单次请求，驱动 future 随请求一起完成或取消；需要复用连接时使用下文的 client/request。成功返回 Response，网络/协议/输入失败默认传播；通过 `result` 捕获时为 `ResponseResult.Read(response)` 或 `Failed(message)`，不自动重试。
- send 的 address 是数值 IPv4/IPv6，port 为 0–65535；请求必须有一个 Host 头。target 使用 `/path?query`，或 OPTIONS 的 `*`，不接受代理 absolute-form 或 CONNECT。
- 普通 `serve/send` 不接受升级；WebSocket 使用下节的 HTTP/1 专用入口。HTTPS 使用下文的 TLS 服务和客户端；当前不支持 CONNECT、非空 trailer、服务器推送或 WebSocket-over-HTTP/2。

### 流式 HTTP 与 SSE

`serve_live` 的 handler 是 `handler(request: Request, reply: dever.system.HttpReply) ()`。它可以发送普通响应、流式正文和 SSE；HTTP/1 还可升级为 WebSocket。handler 内部是否挂起由编译器推断。HTTP/1 每连接最多一个活跃 handler；HTTP/2 每请求流独立拥有 handler 和清理范围，并受 streams 限额约束。

```text
dever.http.default_live_limits() -> LiveLimits
dever.http.serve_live(route, Listener, Limits, LiveLimits) -> ()
dever.http.respond(HttpReply, Response) -> ()
dever.http.start(HttpReply, status: Int, List<Header>) -> ()
dever.http.write(HttpReply, Bytes) -> ()
dever.http.finish(HttpReply) -> ()
dever.sse.start(HttpReply, List<Header>) -> ()
dever.sse.send(HttpReply, Event) -> ()
```

`LiveLimits` 默认 `chunk_bytes = 65536`、`idle_ms = 60000`、`heartbeat_ms = 15000`；块大小必须为正，`0 < heartbeat_ms < idle_ms`。`Limits.body_bytes` 仍约束请求正文和 `respond` 的完整正文，流式输出按块限制，不累计整条长连接的字节数。响应通道仅容纳 1 块；还需计入当前写入、协议引擎、操作系统及应用自身的缓冲，不代表每条连接总共只用一块内存。

响应只能开始一次。`respond` 提交完整响应；`start` 提交流式响应头，随后 `write` 按调用顺序发送，`finish` 结束正文。204/205/304 使用 `respond`。重复开始、超大块、关闭后写入、向 SSE 通道直接写普通正文均传播 `Failed`，可用 `result` 捕获。写入等待受 `Limits.timeout_ms` 限制；对端停止消费时不会无限排队。

handler 返回会结束正文；正文被消费完或丢弃、HEAD 抑制正文、连接断开或 `stop(server)` 会取消相应 handler 并等待其子任务清理。已经交给通道的块不因取消而自动重发。`HttpReply` 是线程安全资源别名，不能在所属会话结束后继续使用。handler 没有提交响应就返回属于程序故障；提交响应头之后的程序故障会截断正文并沿服务任务传播。

首个响应头沿用普通请求期限；提交流式头之后不再使用整段 handler 的 5 秒期限。普通流在 `idle_ms` 内没有输出会关闭；SSE 在空闲 `heartbeat_ms` 后输出注释心跳。心跳不占用额外任务；客户端不读取时仍受 HTTP 停滞写入期限约束。

`dever.sse.Event` 字段为 `event: Text`、`data: Text`、`id: Text?`、`retry_ms: Int?`。空 event 使用浏览器默认事件名；data 按 CR/LF/CRLF 分行且保留尾部换行；空 id 会重置浏览器的事件编号。event 禁止换行，id 禁止换行和 NUL，retry_ms 非负。编码后的完整事件必须放入一块，心跳要求块上限至少 3 字节。`sse.start` 持有 Content-Type，并在应用未指定时添加 Cache-Control: no-cache。应用从请求 `Last-Event-ID` 头读取重连位置，负责历史保存和重放；库不缓存历史事件。

### WebSocket

`dever.websocket` 复用 tokio-tungstenite 的 RFC 6455 握手、掩码、分片重组、UTF-8 校验和关闭协议，不使用自写帧解析器。

```text
dever.websocket.default_limits() -> Limits
dever.websocket.accept(HttpReply, Limits) -> dever.system.WebSocket
dever.websocket.connect(address: Text, port: Int, target: Text, Limits) -> dever.system.WebSocket
dever.websocket.open(url: Text, ClientTls, Limits) -> dever.system.WebSocket
dever.websocket.send(WebSocket, Message) -> ()
dever.websocket.receive(WebSocket) -> ReceiveState
dever.websocket.messages(WebSocket) -> AsyncStream<MessageEvent>
dever.websocket.close(WebSocket, code: Int, reason: Text) -> ()
```

连接成功直接得到 WebSocket；用 `result` 捕获时使用 `ConnectResult`，分支为 `Connected(socket)` 和 `Failed(message)`。Message 包含 `Text(value: Text)`、`Binary(value: Bytes)`、`Ping(value: Bytes)` 和 `Pong(value: Bytes)`。receive 直接返回 `ReceiveState.Read(message)` 或正常关闭的 `End`，错误默认传播，捕获时使用 `ReceiveResult`。messages 在正常关闭时结束，故障产生最后一个 `MessageEvent.Failed`。持续接收可使用 `each` 或 `reduce_until`，无需循环或递归语法。

WebSocket Limits 默认 `message_bytes = 1048576`、`idle_ms = 60000`、`write_ms = 5000`，均为正数。message_bytes 限制完整数据消息及分片累计；控制帧最多 125 字节。每连接读取缓冲区为 4 KiB，协议写缓冲有上限。方向锁允许同时读写；idle_ms 是一次 receive（含等待方向锁）的期限，write_ms 约束一次写入、客户端握手或关闭握手。超时读取会关闭资源；已开始写帧时取消会关闭连接，尚在等待写锁的取消不影响正在写入的另一个任务。

客户端 connect 的 address 是数字 IP，target 为 `/path?query`。open 接受完整的 `ws://` 或 `wss://` URL，支持 DNS，WSS 使用传入的 ClientTls 校验证书和目标名称；不接受 URL 用户信息或 fragment。握手期限包含域名解析、TCP 和 TLS，不自动重试。服务端 accept 从当前 HTTP 请求升级，失败时应用仍可返回普通错误响应。升级后连接仍占服务端名额，并由原 handler 作用域拥有；handler 结束或父任务停止都会关闭它，即使其他地方还持有别名。

应用需持续调用 receive/messages 才能读取协议帧；收到 Ping 时引擎先发送 Pong，再交给应用。库不建立额外的定时 Ping 任务，可通过 send(Ping(...)) 实现应用保活。close 校验合法状态码和最多 123 UTF-8 字节的原因，等待对端关闭握手；关闭会唤醒等待中的读写。当前不提供关闭帧元数据、子协议协商或压缩扩展。

完整示例见 [live_http](examples/old/dever/live_http/module/main.dever)：`/stream` 分段输出，`/events` 推送三条 SSE 事件，`/ws` 回显文本和二进制消息。

### HTTP 连接池与流式客户端

```text
dever.http.default_pool_limits() -> PoolLimits
dever.http.client(origin: Text, ClientTls, Limits, PoolLimits) -> dever.system.HttpClient
dever.http.request(HttpClient, Request) -> Response
dever.http.open(HttpClient, Request) -> StreamResponse
dever.http.open_stream(HttpClient, StreamRequest) -> StreamResponse
dever.http.close_client(HttpClient) -> ()
```

client 返回固定 origin 的共享连接池；需要捕获时使用 `ClientResult`，成功项为 `Ready(client)`。origin 如 `https://example.com:443`，不能含用户信息、业务路径、query 或 fragment；支持 DNS。请求 target 仍用 `/path?query`，没有 Host 时由 origin 补上。每个目标建立自己的池，不保留不断增长的全局主机表；不自动重放已发送请求或跟随重定向。HTTP/2 连接停止接收新流时，协议引擎确认尚未发送的请求会在原期限内重新排队。

| PoolLimits 字段 | 默认值 | 约束 |
| --- | --- | --- |
| idle_ms | 10000 | 正数；归还的连接闲置多久后关闭，即使没有下一个请求也会回收 |
| chunk_bytes | 65536 | 正数；流式上传块上限及每次下载返回块上限 |
| read_ms | 60000 | 正数；一次上传取块或下载拉取的等待期限 |

`Limits.connections` 限制物理连接数，空闲和正在排空的连接也计入上限。HTTP/1 的请求占用上限等于 connections；HTTP/2 的请求上限为 connections × streams，每连接另限 streams。每池一个作用域拥有驱动和有界队列；HTTP/2 的协议子任务也归连接作用域。池属于创建它的任务作用域，作用域结束后，即使外部还持有别名，也不能继续使用。直接调用 `create_client(...)` 会在当前任务内完成；不要通过一个立即结束的 `run` 子任务创建长期共享的池。

request 完整收集响应，正文受 `body_bytes` 限制，排队、建连、发送、接收合计受 `timeout_ms` 限制。open 发送普通 Request 并返回响应头与流；open_stream 的 StreamRequest 把 body 改为 `AsyncStream<dever.io.ReadEvent>`，其余字段与 Request 相同。成功均为 `Read(response: StreamResponse)`，其中 body 也是该异步流；`Chunk(bytes)` 代表数据，终止错误最多产生一次 `Failed(message)`，正常 EOF 直接结束。

流式客户端只按需读取，不收集整段上传或下载正文；总长度不限于 `body_bytes`，单块及等待时间受 PoolLimits 限制。等待请求槽位、DNS/TCP/TLS 和响应头受 `timeout_ms` 限制，响应流随后按 `read_ms` 读取，适用于 SSE 等长连接。HTTP 出站长度和分块编码仍由引擎拥有，转发入站响应时须移除 Content-Length、Transfer-Encoding 等受保留的头。

HTTP/1 响应体完全读完才归还连接，提前 `close(response.body)`、故障或取消会关闭该连接。HTTP/2 的连接可同时服务多个请求；完成或关闭正文释放该流，不中止邻流。仅持有未消费的正文会继续占用请求名额，活跃连接不会被空闲回收。close_client 关闭所有池别名、唤醒等待者并等待驱动清理；下载块共享字节存储，应用保留的块仍计入应用自身内存。

### TLS、共享上下文与优雅停机

```text
dever.tls.system() -> dever.system.ClientTls
dever.tls.client(ca_pem: Bytes) -> dever.system.ClientTls
dever.tls.server(cert_pem: Bytes, key_pem: Bytes) -> dever.system.ServerTls
dever.http.serve_tls(route, Listener, Limits, ServerTls) -> ()
dever.http.serve_live_tls(route, Listener, Limits, LiveLimits, ServerTls) -> ()
```

system 使用随二进制提供的公共 CA；client 仅信任指定 PEM CA，返回 Ready(config) 或 Failed(message)。server 读取应用提供的证书链和私钥 PEM，返回同样形状的结果。TLS 复用 rustls/tokio-rustls，校验证书链、有效期和目标名称；客户端和服务端配置可跨任务共享，并使用有界会话缓存。TLS 服务沿用 HTTP 的握手期限和连接上限；serve_live_tls 提供 HTTPS 流式响应和 SSE，HTTP/1 模式还支持 WSS 升级。HTTP 单独派生 ALPN 配置，不改变其他 TLS 使用者。

需要共享应用状态时，声明具体 Context，并传入系统 serve 原语的可选末尾参数。编译器要求 handler 的最后输入与 Context 类型一致且可跨任务传递；Context 按普通值语义复制，Channel、HttpClient 等资源字段共享同一资源。

```dever
type Context {
  prefix: Text
  requests: Channel<Int>
}

serve(listener: dever.system.Listener, context: Context) () {
  dever.system.http_serve(route, listener, dever.http.default_limits(), context)
}

route(request: dever.http.Request, context: Context) (response: dever.http.Response) {
  send(context.requests, 1)
  response = dever.http.Response {
    status = 200
    headers = []
    body = dever.bytes.from_text(context.prefix + request.target)
  }
}
```

http_serve_live、http_serve_tls 和 http_serve_live_tls 也接受这个末尾参数；live handler 的输入顺序为 request、reply、context，TLS 配置位于 context 之前。这里直接绑定静态 handler，不引入闭包或动态路由解释器。

优雅停机先调用 `dever.net.close_listener(listener)` 并处理结果，再等待 server Task。HTTP 停止接收新连接，HTTP/2 同时发送 GOAWAY，已开始的响应继续完成；WebSocket/SSE 会话仍由现有 handler 拥有。客户端收到 GOAWAY 后不再向该连接派发新请求，也不自动重放已发送请求。使用 `timeout(server, grace_ms, expired)` 给排空设置上限，expired 是零输入、零输出的静态 handler；到期先停止剩余任务并等待清理。需要立即停止时使用 `stop(server)`。完整可运行用法见 [backend 示例](examples/old/dever/backend/module/main.dever)。

### 模块 API

普通应用由编译器从 API、CMD 和 Job 声明生成入口。没有命令参数时，`run` 和编译产物按 `config/setting.json` 的 runtime mode 启动可用服务；CMD 必须显式传名称和一个 JSON 对象，不能借 CMD 参数改变部署模式。`run/build` 拒绝手写 `module/main.dever`。

`module/user/account/app.dever`：

```dever
type Greeting { message: Text }

greet(name: Text) (greeting: Greeting) {
  greeting = Greeting { message = "你好，" + name }
}
```

`module/user/account/api.dever`：

```dever
get greeting = app.greet
public post greeting = app.greet
cmd greeting = app.greet
```

前两项对应 `GET`、`POST /user/account/greeting`；CMD 名称为 `user.account.greeting`，输入是单个 JSON 对象，输出同样使用 `{"code":0,"message":"ok","data":...}`。HTTP 声明默认要求认证，只有明确的登录、健康检查等单条 HTTP 声明可加 `public`；`public cmd` 和 `public rest` 非法。public 入口忽略浏览器自动携带的旧 Cookie，使过期会话不会阻止重新登录；客户端显式提交的无效 Authorization 仍返回 401，不能降级成匿名请求。`get/post/put/delete <action> = app.<function>` 只绑定本领域无歧义的 App 函数，继承其输入输出签名，输出名不必叫 `response`。GET 的完整调用链不能写数据库或入队 Job；普通写 HTTP/CMD/Job 调用链在入口建立事务。含 password hash/verify 的 HTTP 调用链不建立覆盖密码计算的外层事务，其写入必须全部进入显式短 `transaction`，且事务内禁止密码计算；短事务必须重查写入依赖的账户、租户或成员状态。API role 不写函数体或旧 `get_` 前缀。GET/DELETE 从 query 读取标量输入，POST/PUT 从 `Content-Type: application/json` 的对象正文读取；命令也读取严格 JSON 对象。缺少必填值、额外或重复字段均拒绝。输入输出使用同一静态 wire codec，当前不支持 Choice、Bytes 等类型。

简单领域使用 `api.dever`；复杂领域可同时使用 `api.dever` 与 `api/<scope>/<resource>.dever`，topic 依次进入 URL，例如 `module/user/account/api/admin/session.dever` 中的 `get status = app.status` 对应 `/user/account/admin/session/status`。`sites.<key>.path` 匹配这里的 API 父目录：`admin` 匹配 `api/admin/**`，不匹配同级文件 `api/admin.dever`，也不改写 URL。每条 HTTP API 必须且只能归属一个站点；目录遗漏、path 前缀重叠和同一路径同方法重复都会在运行前报错。`rest` 可为同领域唯一 Model 生成 GET 列表/详情、POST、PUT、DELETE；多 Model 时写 `rest model` 或 `rest model.<topic>` 消歧。列表只接受有界 `page`、`size` 和至多一个显式 `search` 条件，不开放任意查询。自动 REST 只序列化公开字段；`private owner user_id: ... = dever.auth.user_id()` 在创建时写入可信身份，并把同一 owner 条件放入全部读写 SQL。客户端不能提交 owner 或无 `input` 的服务端字段。需要不同资源策略或跨 Model 编排时仍使用 App，不把业务规则塞进 REST 声明。

HTTP 绑定的 App 调用链可用 `dever.api.request_id()`、`method()`、`path()`、`client_address()`、`header(name)`、`cookie(name)`、`secret_cookie(name)` 读取请求元数据，并用 `set_header(name, value)`、`set_cookie(name, value, options)`、`set_secret_cookie(name, secret, options)` 设置响应元数据；无需传递 Context 参数。`client_address()` 只返回直接 TCP peer 的地址，不信任转发 Header。`secure_cookie_options()` 提供 `Path=/`、`SameSite=Lax`、`Secure`、`HttpOnly` 的起点，业务可构造 `CookieOptions` 调整 Domain 和 Max-Age。Secret Cookie 使用 `v1.` 加无填充 base64url 存储原始 Secret 字节，读写不会把 Secret 转成业务 Text；非此格式的值读作缺席。敏感请求头不能经 `header` 读为 Text，Secret Cookie 必须使用 Secure 与 HttpOnly；响应 Header 不允许覆盖 Set-Cookie、Content-Type、Content-Length 等运行时字段或注入换行。Header/Cookie 先暂存在请求作用域，只有 handler 成功、响应序列化成功且数据库提交成功后才发布；失败和回滚会丢弃它们。Cookie 写请求和使用 Cookie 认证的非安全方法必须携带 Origin，并要求 Origin 与 Host 都精确匹配 `sites.<key>.origin` 配置的外部 HTTP/HTTPS scheme、规范化主机和有效端口；运行时不从内部明文监听协议猜外部 HTTPS，也不信任转发 Header。CMD/Job 调用链不能使用 HTTP 请求能力，脱离请求的并发任务也不能调用这些能力。

POST App 可以直接接收一个顶层 `Upload` 及普通标量，此时请求必须是 `multipart/form-data`。Runtime 按 `http.upload` 的总大小、单文件、字段、part、header 和 filename 上限流式写入 `data/tmp`；filename 仅作展示值，不能成为路径。`Upload` 是 affine 请求资源，不能嵌套、复制、作为输出或进入 GET/CMD/helper；App 可读取 metadata 后显式 `close_upload`，或一次性传给同领域 Port，再由 Adapter 调用 `dever.storage.put`。未消费资源及解析失败、handler 失败和取消路径会清理临时文件。

监听、站点和认证 Provider 都只在 `config/setting.json` 配置：

```json
{
  "http": { "host": "127.0.0.1", "port": 8181 },
  "auth": {
    "providers": {
      "session": {
        "verify": "user.account.verify",
        "jwtSecret": "replace-with-a-deployment-secret-of-at-least-32-bytes",
        "ttlSeconds": 3600,
        "cookie": "cms_session"
      }
    }
  },
  "sites": {
    "admin": {
      "path": "admin",
      "auth": "session",
      "hosts": ["admin.example.com"],
      "origin": "https://admin.example.com"
    },
    "front": {
      "path": "front",
      "auth": "session",
      "hosts": ["www.example.com"],
      "origin": "https://www.example.com"
    }
  },
  "log": { "level": "info" }
}
```

Provider 使用固定 HS256 token，校验算法、签名、有效期、Provider、站点、subject 和 session；密钥只放部署配置，不进入源码。Bearer 和配置指定的 Secure Cookie 二选一，不能同时提交；使用 Cookie 认证的非安全方法以及任何 Cookie 写入都必须通过上述精确同源检查。需要浏览器 Cookie 的站点必须配置 `origin`；它是部署入口的外部 origin，不是内部监听地址。Provider 的 `verify` 必须引用一个公开普通 App 函数：输入为 `dever.auth.Claims`，输出为该 App 自己定义的公开身份 record，字段严格且只能是 `id: Text`、`user_id: <ModelId>?`、`tenant_id: <ModelId>?`。verify 只负责验证账号、session、成员和租户关系；它只能读 global Model，不能返回权限列表、读取原始 HTTP 请求、再次读取身份、写数据库或入队 Job。

身份建立后，App 可读取 `dever.auth.id()`、`session()`、`user_id()`、`tenant_id()` 与 `dever.site.key()`；`dever.auth.owns_user(model_id)` 用于显式资源所有权比较。所有非 `public` HTTP API 都在读取业务输入和调用 App 前自动校验精确权限 `component.domain.site.action`；REST 固定生成 `read/create/replace/delete`，HTTP method 只是目录元数据，同一权限键不能重复声明。源码没有 `auth.require`，也不能自报权限。登录使用 `dever.auth.issue_cookie(subject, session, tenant)`，登出使用 `dever.auth.clear_cookie()`；两者自动采用当前 Provider 在 `setting.json` 中配置的 Cookie 名和 TTL。

`user_id()`、`tenant_id()` 默认返回 `Int?`；当赋值或函数参数明确期待某个 `model.id?` 时，返回对应的可空名义 ID，不需要再次查询会话表。编译器逐条检查受保护 API 的调用链和自动 REST 字段绑定：该 ID 必须与当前站点 `verify` 身份 record 的对应字段类型一致，`owns_user` 的参数也必须匹配其 `user_id`。这不是通用的 Int 到 ModelId 转换，非空 ID 仍须通过 `null` 分支处理取得。只有源码、没有部署配置的检查延后 Provider 匹配；配置检查和构建必须完成匹配。public、CMD、Job 和普通应用测试不会因此获得身份读取能力。

核心在控制库维护全局 `_dever_permission` 目录，服务启动时同步当前编译产物并把已删除权限标为 inactive。角色、角色权限和用户角色存放在当前身份对应的授权库：租户用户使用租户库，平台用户使用控制库。App 可通过 `dever.auth.permissions()`、`save_role(...)`、`grant_role(...)`、`revoke_role(...)`、`disable_role(...)` 实现很薄的后台管理适配；当前站点和租户由隐藏上下文确定。这些管理能力只能由受保护的写 API 到达，`public` 和 GET 调用链会被静态拒绝。新租户迁移完成后使用 `dever tenant owner <root> <tenant-id> <site> <user-id>` 建立保留的全权限 Owner，普通 save/grant/revoke/disable 均不能修改该保留角色。底层 `issue` 与通用 Cookie API 保留给非浏览器协议组合。身份、租户和权限都不接受客户端参数或 Header 自报，Domain 保持纯业务规则。`dever.api.Error` 的 `Invalid`、`Unauthorized`、`Forbidden`、`NotFound`、`Conflict`、`TooManyRequests` 使用固定安全消息映射到 400/401/403/404/409/429；其它失败只记录脱敏日志并返回通用 500。

只有 CMD、没有 HTTP 声明的应用无需 `http`、`sites` 或 `auth` 配置。正常响应使用 HTTP 200，正文为 `{"code":0,"message":"ok","data":...}`；错误时 `code` 与 HTTP 状态一致且 `data` 为 `null`。多租户只有 database 隔离，不存在 mode 开关：`tenant.database` 选择控制连接，`global type` 使用平台库，其他持久化 Model 和 Job 使用可信身份或显式 `dever tenant migrate` 选择的租户物理库。缺少租户、租户未迁移、schema fingerprint 过期都会失败，绝不回退平台库。租户组件从 tenant Model 和 tenant Job 自动推导；混合组件整体按租户组件处理，它拥有的全部 Job 使用租户队列。public API 不能触达租户组件，API/CMD/Job 执行前统一检查组件状态。运维通过 `dever tenant component disable|enable <root> <tenant-id> <component>` 控制租户功能，平台组件或未知组件不能切换。

## 13. Model、ORM 与数据库

### 13.1 Model 文件与字段

Model 同时支持 `.dever` 和 `.dever.md`，复合扩展名不影响类型名或表结构。Markdown 中的 Seed、索引、关系、迁移和 SQL 声明按 [Markdown 规范](MARKDOWN-SYNTAX.md)分别说明；CMS 的两套实现位于 `examples/cms/dever` 与 `examples/cms/md`。

简单领域在 `<component>/<domain>/model.dever` 中定义一个与 domain 同名的 Model；复杂领域可使用 `model/<topic>.dever`，记录名与 topic 对应。文件继续使用普通 record `type`，不增加 `model` 或 `public` 关键字；主记录、choice 和生成的 ID 类型由 Model role 自动提供。以下 `user/account/model.dever` 对应 `user.account.model.Account`：

```dever
type AccountStatus {
  Active = "启用"
  Disabled = "禁用"
}

type Account {
  uuid: Uuid generated unique
  email: Text(254) unique
  display_name: Text(1, 64)
  status: AccountStatus default AccountStatus.Active index
  organization_id: organization.team.model.id?
}

index(status, created_at)
```

每个 Model 自动增加只读 `id` 和 `created_at: DateTime`。`id` 的类型是该 Model 专属的 `<model-role>.id`，例如 `user.account.model.id`；不同 Model 的 ID 不能混用。`created_at` 和 generated 字段不出现在创建输入中。

Model 字段支持 `Bool`、`Int`、`Float`、`Decimal(P, S)`、`Text`、`Bytes`、`Uuid`、`DateTime`、`Date`、`Time`、`Duration`、`Json`、无载荷 choice、Model ID，以及它们的可空形式。`Text(max)`/`Bytes(max)` 指定最大长度，`Text(min, max)`/`Bytes(min, max)` 同时指定最小和最大长度。结构化值不会自动序列化为列，需要时显式使用 `Json`。

默认值和 Seed 共用编译期存储校验：Int 必须是 i64 范围内的整数，Float 必须有限，Decimal 精度和小数位不能溢出，Text 长度按 Unicode 标量计数。非法常量不会等到生成 Rust 或访问数据库后才报错。

- `generated` 当前用于由 ORM 生成 UUIDv7 的非空 `Uuid`。
- `default` 接受与字段类型匹配的静态常量或无载荷 choice 变体。
- `index` 和 `unique` 定义单字段索引；顶层 `index(a, b)`、`unique(a, b)` 定义复合索引。
- `target.model.name.id` 字段自动成为外键并自动建立普通索引，但不会自动加载目标记录。
- choice 的右侧 Text 是显示名称；数据库保存左侧稳定变体名，`Choice.options` 按声明顺序提供选项 Map。

自动 REST 的字段来源直接写在 Model 字段同行，不改变 App 中的普通 ORM：`create = expr` 只处理 POST，`replace = expr` 只处理 PUT，`search = expr` 处理可选 GET 列表筛选。表达式引用 `input` 时读取客户端同名值并做纯转换；不引用 `input` 时由服务端上下文提供，客户端同名字段会被拒绝。表达式只能调用同领域 `pure Domain`、无 effect 标准函数或精确白名单的只读 request/auth/site 能力，不能访问 Model、Job、时间、响应修改或并发。`search` 最多一个，必须是公开、非空、可索引 wire 标量；owner 与 search 同时存在时自动生成复合索引。

### 13.2 CRUD、条件和分页

编译器为 Model 生成唯一一套短函数，不需要手写 Repository 或第二套 CRUD：

```dever
created = model.create({
  email = "reader@example.com"
  display_name = "Reader"
})

page = model.list({
  where = status == model.AccountStatus.Active
    and (display_name == "Reader" or display_name == "Admin")
  order = [created_at.desc, id.desc]
  page = 1
  size = 20
})

changed = model.update(created.id, {
  status = model.AccountStatus.Disabled
})
```

可用操作为 `create`、`create_many`、`get`、`first`、`list`、`cursor`、`count`、`exists`、`stream`、`update`、`delete` 和 `upsert`。这些操作只能在所属领域 App 内以 `model.<operation>`（多 Model 时为 `model.<topic>.<operation>`）调用；main、API 和其他领域必须调用该领域 App。创建、更新和查询块只能直接出现在对应调用位置，由目标 Model 提供字段命名空间；它们不是运行时 Map 或可保存的查询对象。

`where` 支持 `and`、`or`、`not`、括号、比较、null，以及 `in`、`between`、`contains`、`starts_with`、`ends_with`。查询结构、字段和排序全部在编译期确定，运行值始终通过数据库参数绑定。

`list` 默认第一页、默认 20 条，返回包含 `items/page/size/total/pages` 的 Model `Page`；`cursor` 返回 `items/next/has_more` 且不执行 count；`stream` 按行拉取并保持有界内存。配置中的 `max_page_size` 同时限制分页、关系子列表和原生 SQL 多行结果，不提供无界 `all()`。

### 13.3 关联

外键字段会合成同名去掉 `_id` 的 to-one 关系。反向 to-many 在目标 Model 文件中显式声明：

```dever
relation articles = news.article.model.author_id
```

查询只有写入 `with` 才加载关系：

```dever
accounts = model.list({
  order = [id.asc]
  with = [organization, articles]
})
```

关系字段使用 `Related<T>` 的 `Unloaded`/`Loaded(value)` 两态，业务代码通过分句穷尽处理。to-one 使用受控 JOIN；to-many 按当前父结果批量查询并归组，不逐条查询。`with` 只接受一层静态关系名，并只用于 `first`、`list` 和 `cursor`；`stream` 不加载关系。多对多关系使用显式中间 Model。

### 13.4 事务和数据库绑定

事务是专用 function 声明，不向业务代码暴露 Database、Transaction、begin、commit 或 rollback：

```dever
transaction register(email: Text, display_name: Text) (account: model.Account) {
  account = model.create({
    email = email
    display_name = display_name
  })
}
```

编译器沿完整调用图推导数据库 effect。最外层 transaction 成功时提交；error、fault 或取消时先回滚再传播原失败。同一连接的嵌套 transaction 复用外层事务。无数据库操作、跨连接，或在事务中通过 `run`、`parallel`、流式游标、Channel 启动可逃逸工作都会在构建前被拒绝。`blocking(call())` 可以在事务中使用，但仍须满足其目标非挂起、无数据库和无并发的既有检查，调用者会等待它完成。

数据库错误默认传播，保留 `dever.database.Error` 的类别和消息：Pool、PoolExhausted、Connection、Timeout、Cancelled、Database、Constraint、NotFound、InvalidData、Migration。要主动捕获一个返回 Account 的 App 调用，可以声明下面的结果类型；`result(load_account(...))` 的成功载荷和完整错误集合必须与调用一致。

```dever
type ReadResult {
  Found(value: user.account.model.Account)
  error Failed(error: dever.database.Error)
}
```

`Failed` 包装原始错误 choice，事务回滚和任务等待不会把它改成普通 fault。捕获后仍须按语言规则传播、返回或通过明确的 recover 策略处理；错误不会因读取或记录日志而被视为已处理。启动阶段的配置与迁移失败发生在业务入口之前，仍会阻止进程启动。

Model 默认按 component 名选择连接，例如 `user.account.model` 优先使用 `database.user`；该键不存在时使用 `database.default`。Model 文件需要固定连接时，在类型声明前写：

```dever
database report
```

显式连接不存在会直接失败，不回退 default。普通 CRUD 不接收数据库句柄，也不能按运行时字符串切换连接。

配置顶层存在 `tenant` 时，普通 Model 默认为租户 Model；平台账号、租户目录等共享数据在声明前写 `global type Account { ... }`。`tenant` 只接受 `database`、`max_pools` 和 `idle_timeout_ms`；`database` 指向保存租户就绪状态、组件开关和全局权限目录的控制连接。SQLite 连接为租户 Model 配置 `tenant_directory`，PostgreSQL 配置最多 43 个 lower_snake_case 字节的 `tenant_database_prefix`。业务请求不会自动建库，必须先执行 `dever tenant migrate <project-root> <positive-id>`。PostgreSQL migration 会通过该连接的配置 URL 幂等执行 `CREATE DATABASE <prefix>_<tenant-id>`，部署角色必须具有建库权限；随后才连接租户库并迁移，失败不会写 ready marker。一个事务不能混用 global 与 tenant Model。

### 13.5 Schema、迁移和 Seed

当前 Model 是目标 schema。启动时 ORM 比较数据库 catalog、内部 history 和二进制中的规范化 schema：空库直接建立最终表；安全且意图唯一的变化自动应用；收紧长度、精度或约束前先检查已有数据。失败不会截断、舍入或删除数据。

简单字段重命名使用 `from`：

```dever
type User {
  display_name: Text(64) from nickname
}
```

已有数据库把 `nickname` 改名并保留数据；全新数据库只创建 `display_name`。复杂或破坏性变化使用稳定命名的 `migrate`，例如：

```dever
migrate remove_legacy_code {
  drop legacy_code
}
```

命名迁移成功后只执行一次，已执行内容被修改时拒绝启动。SQLite 需要重建表的变化和 PostgreSQL DDL 都由同一逻辑 schema 计划产生。

需要在结构变化前清理旧数据，或在结构变化后回填数据时，在命名迁移中使用 `before`、`after`：

```dever
migrate normalize_display_name {
  before {
    sqlite = "UPDATE user SET display_name = ?1 WHERE display_name = ?2"
    postgres = "UPDATE user SET display_name = $1 WHERE display_name = $2"
    parameters = ["Editor", "Legacy editor"]
  }
  after {
    sqlite = "UPDATE user SET display_name = ?1 WHERE display_name = ?2"
    postgres = "UPDATE user SET display_name = $1 WHERE display_name = $2"
    parameters = ["Administrator", "Admin"]
  }
}
```

每块必须声明两个方言和参数列表，只接受一条 `INSERT`、`UPDATE` 或 `DELETE`，不接受多语句、DDL、事务控制和保留的内部表名。值通过参数绑定；参数支持 Bool、Int、Text、null 和指数形式 Float，无类型上下文的小数不隐式转换成 Float。参数序号必须连续且覆盖参数列表。

已有表先验证 catalog 和历史，再按源码顺序执行未应用迁移的全部 `before`、自动结构变更、全部 `after`，最后执行 Seed 并记录历史。任一步失败都会回滚该 Model 的数据、结构和历史。全新数据库直接建立最终结构，不执行历史数据变换。已应用迁移的内容和顺序不能改变；变换只能引用当时已存在的表，不保证跨 Model 原子升级。

初始化数据直接写在 Model 文件的无版本 `seed` 块中：

```dever
seed {
  {
    email = "admin@example.com"
    display_name = "Administrator"
    status = UserStatus.Active
  }
}
```

每条 Seed 必须完整提供一个唯一键。ORM 只插入缺失记录，不覆盖或删除线上数据；修改既有初始化数据使用命名迁移。schema、迁移和 Seed 在同一连接的事务与启动锁内执行。

### 13.6 Typed native SQL

内置查询无法清楚表达的复杂读取可以在 Model 文件中声明静态、参数化的原生 SQL：

```dever
type UserSummary {
  status: UserStatus
  total: Int
}

sql summary(status: UserStatus) (summary: UserSummary?) {
  sqlite = "SELECT status, COUNT(*) FROM user WHERE status = ?1 GROUP BY status"
  postgres = "SELECT status, COUNT(*) FROM user WHERE status = $1 GROUP BY status"
}
```

两种方言都必须是 Text 字面量。参数按声明顺序绑定，禁止字符串插值、动态列名和动态行 Map。结果可以是具体 Model、同文件中仅供 SQL 使用的私有 record，或同领域 App 定义的 View，并支持可空值和 List。App View 用 `app.ArticleView` 引用，字段必须公开且为支持的存储标量；不能包含 Secret、嵌套 Model，也不能引用其他领域的 View。SQL 结果普通 record 不接受字段范围约束，数据库解码不能证明这些约束。调用形式与内置操作一致，例如 `model.summary(status)`，并复用当前事务、连接池、statement cache 和具体行 decoder。

GET 允许两种方言均被证明只读的简单自有表投影：`SELECT 字段 FROM 自有表`，可带字段与编号参数的等值 `WHERE`（以 `AND` 连接）、自有字段 `ORDER BY` 和编号参数 `LIMIT`。函数、CTE、子查询、其他表或未识别的 SQL 仍按可能写入处理，不能绑定 GET；不会仅凭 `SELECT` 前缀放行。

### 13.7 `setting.json`

数据库配置直接放在 `database` 对象中：

```json
{
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/app.db",
      "max_connections": 1,
      "max_page_size": 100
    },
    "report": {
      "type": "postgres",
      "url": "postgres://user:password@db.example.com/report",
      "tls": "system",
      "min_connections": 0,
      "max_connections": 4,
      "max_page_size": 100,
      "wait_timeout_ms": 5000,
      "io_timeout_ms": 30000
    }
  }
}
```

`database.default` 必须存在。PostgreSQL 的 `tls` 必须明确为 `system` 或 `disabled`，证书校验失败不会自动降级明文；`wait_timeout_ms` 限制连接池等待，`io_timeout_ms` 独立限制 SQL、事务和迁移 I/O。两者省略时分别为 5000 和 30000 毫秒，可配置范围都是 1–300000。启动时创建 `data/db`、`data/upload`、`data/log`、`data/cache` 和 `data/tmp`；配置缺失或无效时直接失败，不生成默认配置。

## 14. 文件、TCP 与 Stream

文件和 TCP 操作成功时返回资源或数据状态，错误默认传播。`dever.io.open/create` 返回 File；`read` 返回 `ReadState.Read(bytes)` 或 `End`；write 和资源 close 成功无输出。用 `result(call)` 捕获时，分别使用 OpenResult、ReadResult、WriteResult、CloseResult。读取可能短读；文件 `create` 只创建新文件，不覆盖已有文件。

```dever
type CountResult {
  Counted(total: Int)
  error Failed(message: Text)
}

sum_byte(value: Int, total: Int) (next: Int) {
  next = total + value
}

consume(event: dever.io.ReadEvent.Chunk(bytes), total: CountResult) (next: CountResult) {
  next = add_chunk(total, bytes)
}

consume(event: dever.io.ReadEvent.Failed(message), total: CountResult) (next: CountResult) {
  next = CountResult.Failed(message)
}

add_chunk(total: CountResult.Counted(value), bytes: Bytes) (next: CountResult) {
  next = CountResult.Counted(reduce(sum_byte, bytes, value))
}

add_chunk(total: CountResult.Failed(message), bytes: Bytes) (next: CountResult) {
  next = CountResult.Failed(message)
}

streaming(result: dever.io.ReadStreamResult.Streaming(stream)) (total: CountResult) {
  total = reduce(consume, stream, CountResult.Counted(0))
  close(stream)
}

streaming(result: dever.io.ReadStreamResult.Failed(message)) (total: CountResult) {
  total = CountResult.Failed(message)
}

opened(result: dever.io.OpenResult.Opened(file)) (total: CountResult) {
  total = streaming(dever.io.chunks(file, 4096))
}

opened(result: dever.io.OpenResult.Failed(message)) (total: CountResult) {
  total = CountResult.Failed(message)
}
```

`dever.io.chunks(file, limit)` 返回同步 Stream；`dever.net.chunks(socket, limit)` 返回 AsyncStream。显式捕获创建失败时分别使用 `dever.io.ReadStreamResult`、`dever.net.ReadStreamResult`。两种流都产生 `dever.io.ReadEvent.Chunk`；创建时 limit 非正数或源已关闭会直接失败。`dever.net.connections(listener)` 直接返回 `AsyncStream<ConnectResult>`。

关闭 File、Socket 或 Listener 的任一别名后，其他别名上的操作会返回失败。`close(stream)` 是幂等的零输出核心操作：它关闭该 Stream 的全部别名并释放 Stream 持有的源别名，但不会关闭调用方另外持有的 File 或 Socket。Stream 正常结束后所有别名都只会继续得到结束；终止性读取错误最多产生一个 `ReadEvent.Failed`，然后结束。资源最后一个别名离开作用域时会被释放；需要确定关闭结果时仍应调用对应 package 的 close 并处理它的选择型结果。

TCP 的 `connect`、`connect_timeout`、`listen`、`accept`、`read`、`write` 都会被推断为挂起；直接调用时顺序等待，需要并发时使用 `run(...)`。`port`、`timeout`、`close`、`close_listener`、`chunks`、`connections` 不挂起。所有网络等待复用程序的 Tokio runtime；Socket 支持同时读写，每个方向按操作串行，关闭会唤醒等待方。

`connect` 支持主机名，DNS 使用共享有界 blocking 通道，没有连接超时承诺。`connect_timeout` 要求数值 IPv4/IPv6 和正毫秒数。`timeout(socket, milliseconds)` 设置随后开始的每次读写操作的期限，包括等待同方向锁及全部写入；读超时不消费后续数据。一次写入已经写出部分字节后，超时、故障或取消会关闭 Socket，绝不自动重发整条消息。尚未写出字节的取消不关闭 Socket。文件操作仍是同步边界，在挂起调用链中使用 `blocking` 隔离。

关闭 AsyncStream 会结束所有流别名并释放生产者。它不会关闭调用方另外持有的 Socket 或 Listener；需要同时结束这些别名时显式调用 `dever.net.close` 或 `close_listener`。停止持有唯一资源别名的消费任务后，该资源随任务清理释放。

## 15. 完整可运行示例

[示例索引](examples/README.md) 列出可运行示例及编译拒绝反例，提供每个项目的入口、预期结果和运行边界。每个示例目录都是独立项目根，源码位于其 `module/`。

当前完整示例位于 [examples/old/dever/library/module/main.dever](examples/old/dever/library/module/main.dever)。它把以下能力放在同一个程序中：

- `package` 与公开零输入入口；
- 命名输出；
- `List<Int>` 和 `reduce`；
- JSON 解析、选择型失败分句与重新编码；
- Text、Int 和 Unicode。

运行：

```bash
dever check examples/old/dever/library
dever fmt examples/old/dever/library --check
dever run examples/old/dever/library
```

输出：

```text
json = {"message":"你好 Dever","total":6}
```

其他针对性示例：

- [examples/old/dever/core/module/main.dever](examples/old/dever/core/module/main.dever)：用户 type、选择型、可空值、数值范围分句、List、Map、值语义和数值规则。
- [examples/old/dever/stream/module/main.dever](examples/old/dever/stream/module/main.dever)：文件分块读取、Stream 和 `reduce`。
- [examples/old/dever/contracts/module/main.dever](examples/old/dever/contracts/module/main.dever)：受约束的端口、明确失败、主动恢复、纯函数和包依赖。
- [examples/old/dever/http/module/main.dever](examples/old/dever/http/module/main.dever)：Hyper HTTP/1 服务及静态 route。
- [examples/old/dever/async/module/main.dever](examples/old/dever/async/module/main.dever)：Task、Group、Channel、非阻塞定时、`parallel` 和 `blocking`。
- [examples/old/dever/backend/module/main.dever](examples/old/dever/backend/module/main.dever)：源码运行参数、共享 Context、HTTP 连接池和优雅停机；请求一次后自动退出。
- [examples/cms/dever/module/news/article/api/admin/manage.dever](examples/cms/dever/module/news/article/api/admin/manage.dever)：生成的 admin REST/业务入口、站点认证和租户文章事务。

## 16. 当前未实现

以下能力不能写进当前 Dever 程序：

- `if`/`else`、`switch`、`match` 和条件表达式；条件永久使用 function 分句。
- `for`、`while`、`break`、`continue` 和用户递归；重复使用集合/Stream 组合器。
- class、继承、interface、method、annotation、macro、unsafe 和 FFI。
- lambda、闭包、一般化一等 function、用户泛型 type/function。
- exception、`throw` 和 `catch`；可恢复错误使用推导式默认传播、`result(call)` 和 `fail(...)`。
- 通用动态 Json 值、Set 和 BigInt；持久化 JSON 使用 Model 的 `Json` 字段，普通 JSON 处理使用 `dever.json.Document`。
- CONNECT、非空 trailer、服务器推送、WebSocket-over-HTTP/2，以及 WebSocket 子协议、压缩扩展和关闭帧元数据。
- 正式签名 runtime/build packs、共享 daemon 编译及跨平台发行验收。现有 LLVM 内核覆盖受管值、静态 handler、Result、文件/流、真实协程与并发组合、TCP/HTTP/TLS/SSE/WebSocket 及系统能力；独立应用入口已接通 CMD→App→Dever/external Port/Adapter、SQLite/PostgreSQL Model/事务，以及 HTTP/REST、可信身份、自动权限、database-only 租户与组件状态、Cookie/Header、Upload 存储和日志配置。持久 Job 和应用 Test 复用现有语义，Test 保持一次编译及每用例进程/fake/数据库/时钟隔离。external 复用统一 Worker 协议、类型化数据与声明业务错误，以及校验后提取的内嵌资源。Worker 与其他资源归属每次入口：退出先关闭 Worker，再排空子任务并关闭数据库等资源。私有 `dever run/build/test` 默认使用 LLVM 与显式 runtime pack，不读取后端环境变量或调用宿主 Rust/Cargo/cc；managed `dever-core` 仍等待可信 daemon 编译入口，不能把本机定向验收当作正式或全平台发行。
- table/field 多租户隔离模式，以及按租户业务配置动态选择已编译 Adapter；当前只支持 database 隔离和进程级 Adapter 选择。
- 编译器自举。

编译器会拒绝尚未支持的语法，不会把它们静默转换成默认行为。

## 17. 开发建议

1. 先把领域状态建模为字段型和选择型 type。
2. 用小型具名 function 组合业务逻辑；把每个分支写成穷尽、互斥的分句。
3. 需要重复时选择 `each`、`filter`、`find`、`sum`、`reduce` 或 `reduce_until`；只有副作用可并行且顺序不重要时使用 `parallel_each`。
4. 用可空值表达简单缺失；需要区分多种失败原因时定义选择型结果。
5. 在系统边界保留 Bytes、资源和显式失败，不隐式吞掉 I/O、JSON 或 HTTP 错误。
6. 每次修改后先执行 `fmt --check` 和 `check`，再运行或构建入口。

## 18. 编译器约束与公开接口

普通字段型 type 可以直接构造，无需为每个数据类型增加工厂。需要数值不变量时，把范围写在字段上：

```dever
type Port {
  value: Int >= 1 and <= 65535
}

default_port() (port: Port) pure {
  port = Port { value = 8080 }
}
```

`Port { value = 0 }` 编译失败。未知 `Int` 也不能直接放入该字段，先用已有范围分句验证；验证成功分句中可以直接构造。范围同样适用于 choice 载荷、命名输出和 handler 签名中的输入/输出。仅支持 Int、Decimal 的数值范围，不支持任意用户谓词。赋值、字段修改和返回必须继续满足范围。编译器保留复制前的事实，修改原值不会改变副本的事实；遇到不能证明的运算不会猜测安全或自动插入默认值。

真正需要隐藏表示时，使用 `private` 字段：

```dever
type Session {
  private key: Text
  label: Text
}
```

当 Session 声明在 App 中时，其他领域可以使用它，但不能读取、修改 key，也不能构造含私有字段的 Session；普通 record 的 private 仍属于声明 package。`private` 不适用于 choice 载荷。不要给普通数据全部加 private，否则会人为增加构造和读取包装函数。它是源码访问边界，不是敏感数据加密机制。

Model 可以声明 `private password_hash: Text(255)`。只有同 component/domain 的 App 能读写私有字段，生成 CRUD 正常持久化；增加 private 本身不改变数据库列或迁移 fingerprint。含 private 字段的完整 Model、Page、Cursor、Related 及其嵌套容器不能穿过 App 参数/输出、API JSON、错误载荷或入口渲染。App 返回显式 View；循环 Model 关联不会绕过检查。普通 record 的 private 语义保持不变。

`Secret` 是独立内置标量，可以传参、赋值和存入 record/容器，但不可比较、拼接、渲染、普通 JSON 输出、作为 Map key、Model 列或错误载荷。App 可接收 Secret，不能输出它。检查器拒绝嵌套泄漏；原生运行时 Debug 固定显示脱敏占位，持有的字节在释放时清零。不提供 Secret-to-Text/Bytes 通用转换。

`dever.crypto.token(bytes)` 从操作系统生成 16–1024 字节的 Secret；`password_hash(Secret)` 返回 Argon2id v19 PHC Text，固定内存 19456 KiB、2 轮、1 lane、32 字节结果；`password_verify(Secret, Text)` 对错误密码返回 false，对损坏/超限 PHC 返回类型化失败。验证先检查资源上限，再执行哈希。token/hash/verify 失败分别使用 TokenResult/HashResult/VerifyResult，成功分支 Ready，失败分支 Failed(message: Text)，错误不携带输入。哈希和验证是阻塞操作，挂起调用链使用已有 `blocking(...)` 边界。

`dever.crypto.sha256(Bytes)` 与 `hmac_sha256(Secret, Bytes)` 返回 Bytes；`constant_time_eq(Bytes, Bytes)` 对相同长度内容进行常量时间比较，长度不是秘密。仅 compiler-owned Test source 提供 `secret(Text)` 固定输入构造，生产源码没有该入口。所有部署配置仍只来自 `config/setting.json`。

`pure` 写在函数输出声明后，禁止文件、网络、时钟、进程参数读取、安全随机源、标准输出、Stream 操作和并发等效果。检查沿函数调用和具名 handler 传递；调用具体纯 handler 可以通过，声明为 pure 的泛化函数不能调用未限定效果的 handler。纯函数仍可能产生算术 fault，`pure` 不承诺不会失败。

领域之间不声明依赖清单。源码通过 App 名称引用跨领域类型、函数和 handler；编译器根据这些真实引用自动建立依赖图并检查循环。新增或删除引用只修改实际使用位置，不需要同步维护另一份声明。

主动恢复的例子：

```dever
configured_or_default(result: domain.PortResult.Ready(port)) (value: domain.Port) pure recover("optional local configuration uses port 8080") {
  value = port
}

configured_or_default(result: domain.PortResult.Failed(message)) (value: domain.Port) pure recover("optional local configuration uses port 8080") {
  value = domain.Port { value = 8080 }
}
```

完整的 domain 定义见 [contracts 示例](examples/old/dever/contracts/module/domain.dever)。`recover` 允许这个函数结束时消费失败义务，因此应该放在实际决定恢复策略的小函数上，不能为通过编译给全部函数加同一个声明。

失败义务会跟随 record、choice 载荷和集合。未知集合的局部查找不能证明剩余元素已处理；可能覆盖旧值的 Map 更新也不能静默丢掉旧失败。可用完整遍历处理元素、继续返回原集合，或在真实恢复边界声明策略。空遍历不能消费没有传给 handler 的错误上下文，短路跳过的调用也不能算处理。分析是保守的，未实现任意程序的语义等价证明。

应用接口快照自动收录 App 能力、App 合同类型以及 Model 合同；topic 文件名和其他角色的私有实现不会进入快照。通过 API 基线约束后续变化：

```bash
dever api my_app --output my_app/dever.api
dever check my_app
```

首次生成需要新路径；已存在的文件不会覆盖。`check`、`run`、`build` 自动检查项目根已有的 `dever.api`，变化时打印增删项并失败。快照记录公开名称、类型、可见字段、构造边界、约束、效果和恢复策略。正常修改私有实现不扩大公开接口。更新基线需要单独生成新文件并审查差异，不会自动“修复”基线。基线必须由项目审查或权限保护；如果允许 AI 同时任意修改源码和基线，它就不是授权边界。未提供基线时不会限制接口变化。

编译器还会建议清理有证据的重复范围判断（W002）、相同私有实现（W003）、可复用的纯调用（W004）和没有增加契约的私有转发（W005）。未使用赋值沿用 W001，不叠加第二套规则。指定应用入口时，W007 提醒没有外部使用需求的公开函数；普通库检查不推测未知调用方。建议不改变执行，不作为硬错误，也不按行数或函数个数强迫抽象。官方源码执行同样的契约检查，CLI 默认只展示当前用户源码的建议；官方硬错误不会隐藏。

契约分析有明确的工作上限（当前每轮相应证明最多 16,384 个工作单元，名义类型嵌套最多 128 层）。超大嵌套失败结构或事实证明超过上限时报 C015，不会跳过检查后放行。共享结构使用缓存与写时复制，普通重复引用不会直接展开成指数数量的副本。

这些约束能拒绝确定的违规，并提示部分冗余，不能证明一个功能有业务价值，也不能完全杜绝过度设计。

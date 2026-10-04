# Dever 外部 Lib 生态

## Goal

让 Dever Package 通过现有 Port/Adapter 边界安全复用 PyPI、npm、Go Modules 及后续其它语言生态。用户统一使用 `dever lib` 管理依赖，`dever run/build` 自动管理 Worker；业务源码不直接调用外语函数，也不接触语言工具链、环境变量或进程协议。

## Definitions

- **Package**：Dever 组件的发行单位，包含 `.dever` 业务源码并接受完整静态检查。
- **Lib**：PyPI、npm、Go Modules 等外部生态依赖。
- **Adapter**：Port 与 Lib 之间唯一合法的桥接层。
- **Worker**：运行外部 Adapter 的隔离进程，不是用户可调用的业务层。

## Requirements

### Language Boundary

- 外部 Adapter 继续属于 `module/<component>/<domain>/adapter`，完整实现同领域一个 Port。
- App 只调用 Port；Domain、Model、API、Job 不直接调用 Lib、Worker 或外部 SDK。
- `port.dever` 仍是输入、输出、failure、effect 和 schema 的唯一来源；不得从 Python annotation、TypeScript 类型或 Go 接口反向定义业务合同。
- 外部 Adapter 使用严格的 `external <ecosystem> <entry>` 声明，可包含 Lib 与 capability 声明；禁止任意 shell/build 命令。
- 普通 Dever Adapter、测试 Port fake 与现有运行时选择合同保持不变。

### Protocol And Lifecycle

- 所有语言共用一个版本化 Dever Component Protocol，而不是在编译器中增加 `python_call`、`go_call`、`js_call` 特例。
- 协议支持 handshake、schema identity、typed setting、call、result/error、cancel、health 和 shutdown。
- Worker 长驻并由结构化生命周期拥有；启动失败、崩溃、超时、取消和关闭均有界，不产生 detached 进程。
- 业务错误只能返回 Port 声明的 error variant；协议破坏、进程崩溃和非法输出成为安全 runtime fault，不自动重试。

### Dependency Management

- 用户命令为 `dever lib add/list/update/remove/doctor`；生态使用 `pip:`、`npm:`、`go:` 等明确前缀。
- 解析结果写入编译器生成的项目根 `dever.lock`，记录精确版本、传递依赖、内容摘要、runtime、目标平台和 Port schema identity。
- `dever.lock` 是可提交、不可手改的构建输入，不替代只负责部署配置和 Secret 的 `config/setting.json`。
- 只有显式 `lib add/update` 可以联网；`check/test/run/build` 默认离线且不得从用户 PATH 或环境变量选择语言 runtime。
- 首批正式生态是 `exec`、`pip`、`npm` 和 `go`；Maven、NuGet、Composer、Gem 通过同一扩展合同后续增加，不修改语言核心和协议。

### Security And Capabilities

- 外部源码不受 Dever 内部静态检查，因此默认无网络、项目外文件、子进程、设备和 GPU 权限。
- Adapter 必须显式声明所需 capability；声明参与 Port effect 的保守分析，并由 Worker sandbox 执行。
- Worker 只接收当前 Adapter 的 typed setting，不读取完整 `setting.json`；Secret 不进入日志、协议诊断或缓存身份明文。
- 安装脚本、路径、符号链接、归档解包和原生产物必须经过边界、摘要和所有权校验。

### Run, Build And Package

- `dever run` 从机器级 Dever 缓存准备并监督选中的 Worker，不要求用户手动启动服务。
- `dever build` 把所有可运行时选择的外部 Adapter、runtime 与锁定依赖封装进自包含应用；目标机器不需要 Python、Node、Go 或包管理器。
- 嵌入资源以内容摘要验证后释放到应用 `data/cache/lib`，不覆盖项目源码、配置或用户文件。
- Dever Package 可以携带外部 Adapter 声明；普通使用者只执行 `dever package add`，项目解析时把传递 Lib 统一锁定。
- 不同 Adapter 的依赖环境隔离，允许同一外部包的不同版本共存；机器缓存按内容去重。

## Acceptance Criteria

- [ ] 同一个 Port 可以分别由 Dever、`exec`、Python、JavaScript 和 Go Adapter 实现，App 调用代码不变。
- [ ] 协议 schema、operation、输入、输出或 error 不匹配在用户业务代码执行前失败。
- [ ] Worker 崩溃、取消、超时和主程序关闭不会遗留进程或把半条响应当成功结果。
- [ ] `lib add/update` 产生确定性 lock；断网 `check/run/build` 使用相同 lock 和机器缓存成功，缺失依赖明确失败且不隐式联网。
- [ ] Python、Node 和 Go 组件在目标机器没有对应系统 runtime/toolchain 时仍可由构建产物运行。
- [ ] 未声明 capability 的网络、文件、进程和设备访问被拒绝；Secret 与其他项目路径不出现在诊断或状态输出。
- [ ] 一个 Dever Package 可以封装外部 Lib，另一个项目只安装该 Package 即可构建和运行。
- [ ] 新增第四种生态只实现 resolver/runtime pack/SDK，不修改 Port、Component Protocol 或 App 调用规则。

## Out Of Scope

- FFI、动态库加载和同进程外语解释器。
- 自动把任意第三方函数、类或对象模型暴露成 Dever API；普通生态库仍需要薄 Adapter bridge。
- 云端编译、源码上传或运行时联网安装。
- 外部 Adapter 热切换、请求级实现选择和无约束自动重试。
- 修改现有 Dever Package/Component/Domain 的业务命名和调用规则。

## Child Deliverables

1. `09-29-external-lib-protocol`：语言合同、统一协议、通用 `exec` Worker 和生命周期。
2. `09-29-external-lib-ecosystems`：`dever lib`、`dever.lock`、共享缓存及 pip/npm/go resolver 和 SDK。
3. `09-29-external-lib-packaging`：`run/build` 自包含打包与 Dever Package 传递依赖。

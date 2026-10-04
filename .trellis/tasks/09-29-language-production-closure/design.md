# Dever 语言生产化收口设计

## 2026-10-02 Linux 两项收口

当前缺口是完整质量门/持续负载证据，以及 AppArmor 限制下非 root 的 namespace 创建入口。CI 复用根 test 与 performance owners，通过显式设置记录工具路径、作者输入、阶段结果及源码摘要；ignored 内部探针由父测试调用，缺配置不算通过。实际持续测试覆盖双 CMS、HTTP/2 和连接回收的 64/128 MiB 预算，每个关键场景至少 30 分钟。总负载使用自有 cgroup 限制为 2 CPU/4 GiB，压力客户端与服务分别计量。格式门历史差异采用备份后的机械格式化，不混入语言规则变更。

AppArmor 审计确认当前私有 loader 才是首个 exec，bwrap 路径规则不会附着。将已签名 bwrap 资产改为静态 ELF 并直接 exec；受限主机只用固定 root-owned 内容地址 helper，精确匹配嵌入摘要并验证无符号链接、不可组/其他用户写、无 setuid/file capability。复用现有签名安装/事务 owner 安装 helper 与专用 ABI 4.0 profile，不新增任意执行代理或信任源。profile 按上游 bwrap 能力分离结构限制其子进程，必须验证 NNP、直接 aa-exec 与 namespace/capability 拒绝。宿主加载操作独立审批，不能关闭系统限制或用 root 通过替代非 root 验收。

修改归属：test/ci 与 test/performance 管验收；dever-sandbox 管入口/资产检查；CLI toolchain packaging/bootstrap 管可信资产和固定安装目标；SDK 作者输入及根 test 随合同同步。其他平台、公开发布、语言/API/CMS 业务重构不在当前范围。

## 总体边界

收口工作分为四个相互独立、最终统一验收的边界：应用运行边界、外部生态边界、工具链/发行边界和验收证据边界。已有编译器和运行时不做平行重写；每个缺口只在实际拥有该行为的层修复。

## 1. 应用运行边界

API 只负责请求适配、参数解码、上下文读取和响应编码；核心业务继续由现有 Model、领域规则、Port/Adapter 和 Job 承载。真实 PostgreSQL 验收使用项目配置文件提供连接信息，不新增环境变量兼容路径。权限和租户上下文从可信运行时来源进入请求链路，数据库模式租户选择在数据访问边界完成。

CMS Dever 与 Markdown 实现共享行为合同和验收矩阵，但不共享会掩盖实现差异的业务源码。每一套实现都必须经过同样的输入、状态和输出断言。

## 2. External Lib 边界

Dever Package 只声明和组合 Port/Adapter；Lib resolver 负责解析锁定、缓存和运行时准备；Worker 负责统一进程协议。解析、下载、准备和启动是显式阶段，`run/build` 只消费已经锁定并验证的结果。不同生态的差异封装在 resolver/runtime provider 中，不进入 Dever 语言层或业务 API。

所有 Worker 通过统一协议传递版本、capability、请求、响应、错误、取消和超时。构建时可嵌入运行时和依赖，运行时只启动锁定选择的 Worker，并验证内容完整性。

Go Worker 复用同一个由签名发行物和 `dever.lock` 精确摘要绑定的 `runtime.pack`。包内 `dever-runtime.json` 的 Go 专用 `build` 段固定 compiler、linker、源码分析器与目标标准库 importcfg；工具和标准库仅在构建阶段解包到私有临时目录，使用绝对路径和清空的子进程环境。编译按目标 build tags 选择源码，沿已锁定的 Go module 闭包解析导入，并以包内标准库和本次生成的依赖 archive 形成 importcfg。Go SDK 与生成的 `main` 同编译；生成入口把已检查的 operation 名静态绑定到同名导出的 Go 函数，Go 编译器校验统一签名，业务源码不手写注册表或 `main`。产物只嵌入编译好的 Worker 和 checked manifest，运行时继续复用既有 Worker 启动与协议校验。缺少构建资源、导入、目标或不支持的 cgo/汇编能力均明确失败；私有固定目标 pack maker 只用于自有 fixture，不代表正式签名发行或 Go sumdb 验收。

## 3. 工具链与发行边界

本地开发仍可使用 Rust 后端，但公开发行物必须将编译/运行所需的私有实现封装在受保护的工具链和 `deverd` 服务边界内。共享缓存不再由普通 CLI 直接读写；服务负责认证、路径解析、租约、发布和清理。平台实现通过适配层隔离 Linux/macOS/Windows 的权限、服务注册和安装差异。

## 4. 验收与回滚

- 每个阶段先运行最小定向检查，再运行该阶段的集成检查。
- 真实数据库、跨平台构建和外部网络解析不可用时，不修改测试使其“通过”；记录阻塞原因和重新运行命令。
- 新实现必须保留 fixture/fake 路径，普通单元测试不强制启动真实外部 Lib。
- 发行边界先通过离线 fixture 和干净目录检查，再接入真实平台/注册表。
- 若某阶段失败，回滚到该阶段前的任务边界，不回退已完成的编译器、缓存和协议基础。

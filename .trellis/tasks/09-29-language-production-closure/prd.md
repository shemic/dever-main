# Dever 语言生产化收口需求

## 2026-10-02 当前执行范围

用户最新确认先完成Linux的完整CI/长时间压力测试，以及受限AppArmor下非root第三方Worker；其他平台和公开发行暂不推进。下方保留完整产品目标，本次交付不以这些延期项阻塞已授权的Linux工作。

- 完整质量门覆盖当前workspace静态检查、默认测试、必要feature组合、Python运行器合同及有明确前提的Linux运行级验收；内部probe、作者准备与不可用条件必须逐项分类，禁止把跳过算通过。
- 长压测复用现有CMS/HTTP2/连接恢复运行器，每个关键持续场景至少30分钟，覆盖64/128MiB预算、业务响应、OOM、RSS/FD趋势和退出回收；记录实际负载和限制，不把计划时长或短测说成完成。
- 非root修复必须保留文件/网络/子进程隔离、签名资产和UID边界。不得关闭全局AppArmor限制、引入setuid绕过、依赖环境变量或系统PATH碰巧存在的语言/沙箱程序。若需要安装或加载宿主专用profile，先完成可审查的产品改动及策略，再单独取得操作确认。
- 用户此次明确授权完整CI和上述长压测；仍只使用自有临时目录、loopback服务/数据库和受限资源，不改动现有业务服务。

## 目标

把 Dever 从“语言核心和部分生产能力已实现”收口到“可以用来开发、验证、打包并交付大型项目”的状态。收口必须建立在现有实现上，不重复实现已经完成的编译器、运行时、API、Job、Port/Adapter、测试运行器和缓存治理能力。

## 背景与已确认事实

- Dever 的语言核心、Native 编译链、主要运行时、Model/ORM、Context、API 基础、权限基础、Job、Port/Adapter、应用测试运行器和构建缓存已有定向实现。
- 生产 API 源码大部分已经实现，但真实 PostgreSQL、真实 HTTP 组合链路和最终任务状态尚未收口。
- CMS 目前是基础示例和部分能力验证，尚未完成大型项目级双源码验收。
- External Lib 已有统一协议和 Worker 基础，但 pip/npm/Go 真实生态解析、运行时打包、Package 传递依赖和独立运行尚未完成。
- 当前发行路径仍主要依赖本地 Rust/Cargo 开发工具链，正式跨平台发行、机器级共享工具链和认证缓存服务尚未完成。
- 项目配置必须使用 `config/setting.json`；不恢复环境变量配置方案。
- 当前租户隔离只承诺 database 模式；FFI、front 和表/字段隔离不纳入本轮收口。

## 范围

### A. 生产 API 与数据库验收

1. 完成生产 API 的最终合同检查，清理已被后续实现覆盖的旧任务项。
2. 使用 `config/setting.json` 提供隔离的真实 PostgreSQL 配置和可重复验收入口。
3. 验证权限目录、角色、跨站点同名角色、多角色、撤销权限和租户数据库隔离。
4. 验证请求解析、响应错误映射、Cookie/Header/Context、上传、取消、事务和 HTTP/2 组合行为。

### B. 大型 CMS 双源码验收

1. 完成 Dever 和 Markdown 两套 CMS 的 Model、业务规则、权限、会话、媒体、发布、审计和 Job。
2. 两套源码使用同一份行为合同，测试结果和关键输出保持一致。
3. 验证 `check/test/run/build` 及 `api/worker/all` 配置模式。
4. 删除或隔离旧 demo-only 流程、旧种子和不符合当前目录约束的代码。

### C. External Lib 生态

1. 完成统一 external Adapter/Worker 协议的生产错误、取消、超时、capability 和生命周期行为。
2. 完成 pip、npm、Go Modules 的真实解析、锁定、缓存和运行时准备。
3. 完成 Python、JavaScript、Go SDK 和 `Adapter -> Lib` 依赖映射。
4. 完成 Dever Package 的传递 Lib 依赖、run/build 集成和无系统运行时执行。
5. `run/build` 不隐式联网；所有解析和下载均由显式命令及锁文件驱动。

### D. 发行版与共享工具链

1. 完成 `deverd` 机器级共享工具链、认证 IPC、并发发布、租约、完整性校验和安全缓存边界。
2. 支持在无 cargo/rustc 的干净机器上安装、运行和构建公开项目。
3. 完成 Linux、Windows、macOS 的发行物、安装/升级/卸载边界和平台安全检查。

### E. 最终质量验收

1. 完成相关定向测试、全量 workspace 测试和真实 PostgreSQL 验收。
2. 完成 CMS、外部 Lib、发行版的运行级检查。
3. 完成二进制体积、启动时间、并发、RSS、64/128 MiB 内存压力和缓存占用报告。
4. 在工具可安装时启用 rustfmt、clippy 和 CI 检查。

## 不在本轮范围

- FFI。
- front 产品能力。
- table/field 租户隔离；只实现 database 模式。
- 通过环境变量配置 PostgreSQL 或其它运行参数。
- 重新设计已稳定的语言语法和 API 协议。

## 验收标准

- 真实 PostgreSQL 下 API、权限、角色、租户和事务链路通过；无配置时不会连接外部数据库。
- Dever/Markdown CMS 的同一验收矩阵全部通过，并能完成 check、test、run、build。
- pip/npm/Go Lib 可锁定、准备、打包并通过 Worker 调用；运行不依赖宿主机对应语言环境。
- 干净机器安装后不需要 cargo/rustc 才能执行公开 Dever 项目。
- Linux、Windows、macOS 的发行边界和未支持项有可执行检查，不泄漏私有 Rust/Trellis 源码。
- 全量、定向、性能和安全检查的状态均有明确记录：通过、失败、跳过或阻塞，不以计划代替证据。

## 既有任务关系

本任务是收口总任务，不复制已有实现，直接复用并收口以下任务：

- `09-20-production-api`
- `09-20-large-cms-acceptance`
- `09-29-external-libs` 及其三个子任务
- `09-06-private-compiler-distribution` 与 `09-28-shared-toolchain`
- `09-19-application-testing`
- `09-29-build-cache-storage`

## 当前无阻塞产品决策

本轮按以上范围执行，不再等待新的产品语法或权限设计决定。若平台供应商或真实 PostgreSQL 不可用，只能将对应验收标记为 blocked/skipped，并保留可重复入口，不得伪造通过。

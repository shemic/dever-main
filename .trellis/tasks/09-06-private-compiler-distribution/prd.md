# Dever 私有编译器与公开发行边界

## Goal

让用户只接触 Dever 语言、工具链和发行物，不接触或依赖 Rust/Cargo/rustc；Rust 仅作为私有仓库中的首代引导实现，不进入公开源码、安装流程、命令输出或用户运行环境。

## Background

- 当前 `/data/project/dever` 没有配置 Git 远端，Cargo workspace、Rust 源码、测试和 README 尚未提交，因此没有证据表明实现已经对外发布。
- 当前仓库及 Git 历史包含 Rust 实现信息，不能直接转换为公开仓库。
- 当前 `deverc run/build` 调用用户环境中的 `rustc`，并在临时目录生成 Rust 源码；这不符合对外隐藏实现和免 Rust 工具链的要求。
- 二进制实现无法可靠抵抗专业逆向识别。本任务隐藏正常产品表面和源码实现，不承诺不可逆向。
- 当前 Rust 编译器及计划使用的 LLVM 后端都有必须合规审查的第三方许可证；不能通过删除或伪造法定声明隐藏实现。

## Requirements

### Repository Boundary

- 当前编译器仓库保持私有，不向公开仓库推送现有 Git 历史、Cargo 文件、Rust 源码或内部设计资料。
- 对外发行使用一个没有当前仓库历史的新仓库或等价的独立公开渠道。
- 公开内容只包含 Dever 用户需要的文档、`.dever` 示例、安装入口和构建后的发行物，不包含编译器实现源码。

### Product Surface

- 用户只使用正式的 `dever` 命令和 `.dever` 源码，不需要了解 Cargo、Rust、rustc 或内部生成语言。
- 公开文档、安装器、帮助文本、普通诊断、日志和临时文件不得暴露 Rust/Cargo/rustc 实现细节。
- `dever check/run/build` 不要求用户安装 Rust 工具链，也不得调用用户 PATH 中的 `rustc` 或 `cargo`。
- `dever build` 生成的程序可独立运行，不依赖 Cargo、Rust 工具链或私有编译器源码。
- 更换公开发行边界和原生后端不得改变已经确认的 `.dever` 表面语法、诊断位置或当前可执行语义。

### Native Backend

- Rust 可以继续用于私有编译器本身，但用户侧原生编译路径必须由随 `dever` 交付的内置后端完成。
- 不采用“把 rustc 改名或静默捆绑”来满足隐藏要求；底层工具链不能作为可观察的用户侧子进程或安装前置条件。
- 编译必须在用户本机离线完成，不上传 `.dever` 源码或中间表示，也不依赖远程构建节点。
- 不采用云端编译来回避本地后端，因为这会额外引入源码上传、网络依赖和服务可用性边界。

### Initial Platform Boundary

- 首版支持 Linux、Windows 和 macOS；所有编译均在用户本机离线完成。
- Linux、Windows 和 macOS 均提供 x86_64 与 ARM64 版 `dever`。
- 任一受支持宿主上的 `dever build` 都必须能够离线生成 Linux、Windows 和 macOS 的 x86_64/ARM64 程序，形成 6 个宿主发行物到 6 个目标的完全对称矩阵。
- 生成跨平台程序不能依赖远程构建节点或上传源码及中间表示。
- 目标后端、链接能力和 Dever 平台运行时随发行物提供，不能要求用户另行安装目标平台编译器或 SDK。
- `dever build` 为 macOS 目标生成平台运行所需的离线 ad-hoc 链接签名；它不代表开发者身份，也不能通过 Gatekeeper 分发验证。
- Developer ID 身份签名和 Apple 公证是独立的可选发布阶段，不属于 `dever build`；Developer ID 安全时间戳和 Apple 公证均允许并必然需要联网。

## Acceptance Criteria

- [ ] 从一台没有 `cargo` 和 `rustc` 的目标机器安装公开发行物后，可以执行当前里程碑对应的 `dever check/run/build`。
- [ ] 安装、帮助、正常运行、普通错误和构建临时目录中均不出现 Rust、Cargo、rustc 或 `.rs` 源码。
- [ ] `dever build` 的输出是可独立运行的目标平台程序，并保持当前 `.dever` 示例的可观察行为。
- [ ] 公开仓库或公开发行包不包含当前私有仓库的 Git 历史、Cargo metadata、Rust 源码或私有 Trellis 资料。
- [ ] 自动检查能够证明公开构建路径没有启动 `cargo` 或 `rustc`，并覆盖没有 Rust 工具链的干净环境。
- [ ] 断网且未安装 Windows 编译工具链的 Linux 环境可以生成 Windows 程序，并在真实 Windows 环境验证其行为。
- [ ] 断网且未安装 Xcode 或 Apple SDK 的受支持环境可以生成 macOS 程序，并在真实 Intel Mac 和 Apple Silicon Mac 上验证其行为。
- [ ] 六个宿主发行物分别能够生成六个目标程序，36 种宿主到目标组合使用同一命令合同并通过确定性产物检查；每个目标程序均在对应真实系统与架构上执行验证。
- [ ] macOS ARM64 产物包含可离线生成的有效 ad-hoc 链接签名；编译不访问开发者身份或公证服务。
- [ ] 单独的发布流程明确区分 ad-hoc 链接签名、需要联网安全时间戳的 Developer ID 身份签名和需要联网的 Apple 公证。
- [ ] 私有仓库仍可维护和验证 Rust 引导编译器，公开边界不迫使内部源码改名或制造重复实现。

## Out Of Scope

- 阻止专业人员通过反汇编、二进制指纹或运行时特征推断实现语言。
- 为隐藏实现而混淆、加壳或伪造二进制来源。
- 在本任务中实现 HTTP、JSON、数据库、CRUD 或完整 Dever 语言路线。
- 公开编译器源代码或建立外部贡献者的编译器开发环境。
- 在 `dever build` 中管理用户的 Developer ID 凭据、执行身份签名或隐式提交 Apple 公证。

## Open Question

- 是否允许依法必须提供的第三方许可证清单提及 Rust；如果不允许，则 Rust 引导编译器只能作为私有 seed，Dever 自举编译器必须成为首次公开发行的前置条件？

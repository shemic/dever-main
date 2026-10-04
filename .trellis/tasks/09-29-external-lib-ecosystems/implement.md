# Implementation Plan

- [x] 定义 requested/locked/artifact/runtime-pack 内部模型和 canonical lock codec。
- [x] 实现共享 resolver orchestration、SHA-256、归档安全、atomic lock/cache publish 和 doctor。
- [x] 增加 `dever lib add/list/update/remove/doctor` 并复用统一项目根/诊断处理。
- [x] 提供 `ArtifactStore` 适配边界和 test-owned 内容寻址 fixture；正式 `deverd` IPC 仍由共享工具链任务提供。
- [x] 实现 PyPI wheel/PEP 508/440、npm packument/semver/SRI 和 Go proxy/MVS resolver，接入已验证安装版本及签名 runtime 描述。
- [ ] 发行正式 CPython/Node/Go runtime/build pack 并运行无宿主验收。
- [x] 完成锁定 host-target build pack 的受管 Go compile/link、build tags、module ZIP、go:embed 与同名操作绑定；源码移除和清空环境独立 Worker 执行通过。
- [x] 完成 Python extras/wheel/sdist/PEP517、npm 高级依赖图/安装钩子/native addon 与 Go sumdb；2026-10-02 已运行官方第三方依赖隔离构建和离线 Worker 验收，证据见父生产化任务。
- [x] Python、JavaScript、Go SDK 协议库、checked manifest typed 校验及显式宿主验收；Python/JS 自动同名 entry 绑定已接入打包器。
- [x] 使用本地 fixture registry 覆盖 deterministic lock、摘要、归档路径、冲突、offline 和双 target cache 隔离。
- [x] ArtifactStore 提供 verified `get`，并拒绝摘要不匹配；npm scoped name 和 remove 依赖关系有明确诊断。
- [x] 增加 `LibResolver`/`ProviderManifest` 边界和真实 provider 接线；仅显式 Lib 命令解析/下载，不读取 PATH 或环境变量。
- [x] `doctor` 按各生态校验精确依赖图、可达闭包和构建收据；npm 允许合法依赖环。ArtifactStore 在 Worker 启动前验证目标和摘要。
- [x] 编译器/CLI 接通 external Adapter 的精确 Lib 请求：支持 `lib "pip:name@version"`、`lib "npm:name@version"`、`lib "go:name@version"`，请求进入 schema identity 并参与 lock 校验。
- [x] 更新 LANGUAGE、README、IMPLEMENTATION 与 compiler/toolchain specs，保留尚未实现和缺发行资产的边界。
- [x] 运行 CLI/lib resolver/SDK 定向测试；只使用注入 transport、自有 loopback 镜像和显式宿主 fixture，不访问公网 registry、不运行全量。

## Current implementation boundary

2026-10-02：当前锁格式为 v5，Python extras/原生 wheel/sdist 构建、npm 嵌套/peer/optional/bundled 图、安装钩子与 native 分类裁剪、Go sumdb 已实现。最终默认解析 **71/71**，Worker **7/7**，严格 CLI Clippy 通过；simplejson3.20.1、bufferutil4.0.9 和 google/uuid1.6.0 的显式真实构建与离线 Worker 均通过。命令、作者资产与运行日志统一记录于 `../09-29-language-production-closure/implement.md`，不复制历史进度为当前缺口。正式公开及其他平台资产尚未交付，任务暂不归档。以下为历史基线。

本阶段已完成 compiler-owned lock/resolver boundary、真实 registry resolver、机器共享资产缓存及三 SDK typed manifest。显式宿主协议验收不是正式受管 pack/无宿主发行验收；未安装签名 runtime 描述的开发入口仍明确失败，不使用系统包管理器或宿主语言环境。

2026-09-30：Lib 声明统一到 `config/setting.json.lib`，拒绝旧独立文件；canonical v2 lock 绑定 checked Worker contract 与 runtime（零 Lib managed Worker 也锁 runtime）。每个 Worker 独立 resolve/doctor，跨 Worker 版本可以不同。嵌入精确比对完整 provider 元数据。CLI `external_libs` 最终 27/27 通过，真实 provider 测试注入协议 transport，Go ZIP 的受限跳转使用自有 loopback HTTP 镜像；不使用 FixtureRegistry 冒充产品解析，也不声称已访问公网/发行 pack。Linux 不同 UID 的 artifact put/get、token 隔离和 owner-only clean 通过。CLI Clippy `-D warnings` 与所属文件私有 rustfmt check 通过。

## Review Gates

- 不让各 resolver 重复 lock/cache/path/HTTP 验证。
- Go ZIP 重定向仅允许固定 GCS bucket、最多两跳、总 30 秒；URL 规范化、userinfo/fragment/端口/路径检查与循环/Location 检查都在后续请求前，诊断不回显签名 URL。
- 不在 run/build 中解析版本或联网。
- 不允许项目 registry URL、runtime path 或 token 来自环境变量。
- 不把生态依赖模型暴露成 Dever 业务类型。

## Verification

- `cargo check --offline -p dever-cli`：通过。
- `cargo test --offline -p dever-cli --lib libs::tests`：4/4 通过。
- `cargo test --offline -p dever-cli --test external_libs`：3/3 通过。
- 手工 `deverc lib add/list/doctor`：通过。
- 未运行公网 registry 下载、正式签名 runtime 或全量测试；真实 resolver 的定向证据来自注入 transport 的协议元数据，不能等同于发行验收。

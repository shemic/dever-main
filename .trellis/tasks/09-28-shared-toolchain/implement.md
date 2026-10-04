# Implementation Plan

## Phase 1: Contracts

- [x] 定义唯一的机器目录布局、发行 manifest、版本语法、签名输入、active-version 状态和平台权限合同。
- [x] 在 `config/setting.json` 增加严格的可选 `dever.version`，并覆盖未知字段、非法版本和独立产物读取行为。
- [x] 固定用户命令：`install/update/use/uninstall/version/cache status/cache clean`，现有项目命令不改变。

## Phase 2: Global Launcher And Version Store

- [x] 把稳定 launcher 与版本核心分开；所有项目命令通过同一选择函数分派。
- [x] 实现版本安装、签名/摘要验证、staging、自检、原子发布和 active-version 可恢复原子切换。
- [x] 实现更新回滚、当前版本卸载保护、缺失锁定版本诊断和显式降级拒绝。
- [ ] 实现 Linux、macOS、Windows 平台标准提权适配。
- [ ] 为 Linux、macOS、Windows 分别实现机器级入口、目录权限和服务注册，平台差异只留在 adapter。

## Phase 3: Cross-user Build Cache

- [ ] 将版本核心原生编译接入受保护的 daemon build service；已完成的 Status/Clean/Lib artifact IPC 不代替编译请求。
- [ ] 使用密码学内容摘要统一编译器、runtime、目标、生成内容与选项身份；SHA-256 与 store 已实现，但版本核心尚未通过受保护服务使用它。
- [x] 实现认证 IPC、并发请求合并、不可变原子发布、租约、完整性复核和安全结果交付。（IPC 客户端只接受 token-authenticated `deverd` 响应，未恢复同进程 fallback。）
- [x] 实现 `cache status/clean` 与更新/卸载后的损坏、staging 和无引用条目清理。
- [x] Linux 采用内核 peer credential 接受不同 UID 的 content-addressed Lib artifact put/get 和 metadata-only status；拒绝非 owner clean/管理 token 读取，限制每 UID/总连接、单资产/总容量/条目数。

## Phase 4: Integration And Documentation

- [x] 将现有 `check/api/fmt/test/run/build/tenant` 命令接入统一 launcher，验证参数和退出码透传。
- [x] 更新 `README.md`、`LANGUAGE.md`、`IMPLEMENTATION.md` 和 backend toolchain spec，明确用户只有一个 `dever`。
- [x] 使用签名本地 fixture 验证双版本、双项目、并发缓存发布、升级故障与缓存清理。
- [x] 使用 root 与自有 uid=65534 fixture 客户端验证同一 launcher、不同 UID 的 status/artifact IPC、token/clean/admission 隔离；不创建系统账号或安装真实全局服务。
- [x] 运行受影响 crate 检查和定向测试；不默认运行全量测试或替换当前机器的真实全局命令。
- [ ] 最终安全审查安装提权、路径穿越、符号链接、TOCTOU、缓存投毒、降级与跨用户信息泄露。

## Rollback

安装和更新始终保留上一活动版本，active-version 只在新版本自检通过后切换。launcher 或服务升级失败时恢复上一套文件和服务定义；缓存是可再生数据，回滚不迁移或修改任何项目目录。

## Implementation Record (2026-09-29)

- 已完成稳定 `dever` 二进制、严格项目版本读取、签名/摘要/平台验证、双版本安装与切换、活动版本追加校验日志、失败回滚、缺失锁定版本诊断、参数与退出码透传。
- 已完成开发态原生缓存 SHA-256 化，以及未来 `deverd` 内部 store 的不可变发布、完整性复核、读/写租约、状态与清理算法；该 store 明确不构成 IPC 或跨用户权限边界。
- 复审后，launcher 的 `cache status/clean` 在认证 IPC 未完成前统一明确失败，不再直接构造 store；安装复制固定为机器共享只读权限，launcher 在执行前复核状态、公钥、版本目录和发行文件的属主与可写位，并拒绝 `update` 通过 `latest` 隐式降级。
- 本地签名 fixture 已覆盖双版本、双项目、并发缓存发布、篡改拒绝、健康检查失败、部分 active 记录恢复和清理租约。没有创建真实系统用户、安装全局命令或变更系统服务。
- 真实 Linux/macOS/Windows 提权与服务注册 adapter、认证 IPC、版本核心到 `deverd` 的构建请求、签名发行目录与下载（当前 `latest` 只是未签名本地 fixture 指针）、内置后端发行包和六宿主发行验证仍由父任务 `09-06-private-compiler-distribution` 阻塞；这些项保持未勾选，不能用同进程 store 代替。
- 最终定向复验：受影响的 `dever-cli`、`dever-runtime`、`dever-core` 离线 `cargo check` 通过；`shared_toolchain` 6/6、`application_config` 6/6 通过。`rustfmt`、`clippy` 未安装；未运行全量、性能、真实系统安装或跨平台测试。
- 本轮补充 `deverd --root <machine-root>` 本地服务和长度分帧 IPC：token 文件与 socket 均限制为 owner-only，`cache status/clean` 通过服务访问 CacheStore；服务不可用时客户端失败，不构造本地 store。跨平台 peer credential、系统服务注册、提权和真实双用户验证仍未完成。

# Implementation Plan

- [x] 扩展 syntax/parser/formatter/Markdown，仅在 Adapter role 接受 external 声明并保持精确诊断。
- [x] 扩展 HIR/Port 注册、role matrix、dependencies/effects/failures/specialization，复用现有 Adapter owner。
- [x] 从 Port/setting 类型生成 Component Protocol schema hash 和 native codecs。
- [x] 在 runtime 增加有界 frame codec、消息状态机和严格请求 ID 管理。
- [x] 实现结构化 Worker supervisor、stdin/stdout/stderr、timeout/cancel/shutdown 和 crash cleanup。
- [x] 将 external Port dispatch 接入 native emitter；reference 明确拒绝。
- [x] 在仓库根 `test/` 增加 parser/contract/protocol/lifecycle/Port 回归，fixture 只使用测试自有可执行程序。
- [x] 更新 LANGUAGE、IMPLEMENTATION、compiler/toolchain/directory/error specs。
- [x] 运行受影响 crate check 及 external protocol、port_adapter、application_testing 定向测试；不运行全量或外部服务。

## Review Gates

2026-10-02补充：以下09-29记录为历史切片。当前Rust/LLVM入口共用Scope-owned Worker与Linux namespace/seccomp；kernel能力/资源/后代清理、协议及LLVM定向已通过。GPU/其他平台拒绝，受限AppArmor非root入口尚未收口。准确日志与最终组合状态见`09-29-language-production-closure/implement.md`。

- 不新增第二套 JSON parser、effect graph、Adapter registry 或进程作用域。
- 不允许外语 entry、Lib 或 capability 成为 App 可见类型。
- 不使用 shell command string、PATH lookup、环境变量配置、detached process 或自动 retry。

## Implementation Record (2026-09-29)

- Adapter-only `external exec` syntax、format 和 Markdown declaration contract 已接通；entry 只接受领域内的可移植相对路径，capability 固定为 `network/file/process/gpu`。
- checker 从同领域唯一 Port 合成完整实现，复用原有 Adapter 选择、setting、wire、effect、failure、dependency 和 specialization owner；schema checksum 绑定 setting、操作、顶层 wire 字段名、具体类型和业务错误 payload。
- native 生成具体输入/输出/error codec，正式调用统一进入 runtime Component Protocol；reference 对可达外部实现明确报不支持。
- runtime Worker 使用显式 canonical entry、清空环境和固定参数启动，具有有界队列、启动频率、frame/JSON 限额、严格消息字段/request id、health、timeout/cancel drain、crash restart 和 bounded shutdown。关闭时先封闭 admission，再取消在途调用并丢弃排队调用；所有协议写入有期限，fault 和 shutdown 都在 supervisor 内显式 kill/wait。协议 fault 先关闭旧 receiver，再允许下一次调用重建 Worker，不继续复用已失配流。
- 生成应用入口在 success、business error 和 runtime fault 后统一调用 `component::shutdown()`，随后才关闭数据库；主错误不被 cleanup error 覆盖。
- capability 当前保证为编译期固定集合/effect 汇总以及 `ready` 精确回显校验；没有实现或宣称 OS 级 syscall/network/filesystem/GPU sandbox。
- 未实现 resolver、`dever.lock`、机器级共享缓存、最终 build 资源嵌入、shell/PATH/env fallback、detached process 或 FFI。

## Verification Record (2026-09-29)

- 初次 native 复验因根盘仅余约 156 MiB 无法暂存 runtime；仅删除仓库内可重建的 `target/debug/incremental` 后释放约 8.7 GiB，随后完成下列复验。未删除 Cargo 依赖缓存、native runtime 缓存、源码或用户数据。
- PASS: `cargo check --offline -p dever-runtime --features wire`。
- PASS: `cargo check --offline -p dever-core`。
- PASS: `external_component` 1/1；覆盖 setting handshake、health、两次调用、业务 error、主动 cancel、timeout、crash/restart、fault cleanup、shutdown、80 个并发/排队调用的 admission closure、重复字段、乱序 ID、未知 kind、超大和截断 frame。
- PASS: 完整 `port_adapter` target 14/14；覆盖 Adapter-only parser/check/format/Markdown、setting 与顶层 wire 名称的 schema 漂移、effect/failure、test fake、native Worker 端到端 dispatch 和 cleanup。
- PASS: `application_testing` 8/8；application-test 发现、隔离、错误和 native suite 回归通过。
- UNAVAILABLE: `cargo fmt --all -- --check` 与 Clippy；stable toolchain 未安装 rustfmt/cargo-fmt 和 cargo-clippy component。
- NOT RUN: 全量、外部服务、数据库和性能测试；遵守本任务定向验证范围。

# Implementation Plan

- [x] 完成并验收 `external-lib-protocol`，冻结语法、wire 和生命周期合同。
- [x] 完成全局工具链共享缓存的依赖接口，不在 Lib 任务复制机器安装/版本管理。
- [x] 完成并验收 `external-lib-ecosystems` 的离线确定性边界，以本地 fixture registry 覆盖 lock/doctor/artifact 校验。
- [x] 完成并验收 `external-lib-packaging` 的 fixture 资源打包、摘要复核和私有 extraction 边界。
- [x] 跨子任务复核 Port effects/failures、setting/Secret、测试 fake、缓存身份和 `run/build` 行为。
- [x] 更新 LANGUAGE、README、IMPLEMENTATION 和 backend specs，删除已被正式合同替代的占位说明。
- [ ] 只运行各子任务列出的定向检查；不默认运行全量、真实外部 registry、服务或性能测试。

## 当前收口补充（2026-09-29）

- [x] 已增加 `ProviderManifest`/`LibResolver` 边界，fixture provider 与真实 pip/npm/Go provider 明确分离。
- [x] `dever.lock` doctor 现在校验传递依赖闭包、环依赖和缺失依赖；ArtifactStore 在 Worker 使用前可验证目标、摘要和尺寸。
- [x] 真实三生态 registry resolver、checked manifest 三 SDK、Python/JS 自动 Worker、Linux 不同 UID 的共享 artifact IPC 已接线并定向验收；当前证据见下方和生产化收口任务。
- [x] Go managed compile/link 和 Dever Package 安装/传递 Lib 已实现并有定向 fixture 证据；具体完整链路与当前计数见生产化收口任务。
- [x] 高级生态安装/Go sumdb已在2026-10-02完成Linux实现及真实第三方包定向验收。
- [ ] 正式公开签名语言packs与其他平台发行尚未交付。缺少时明确失败，不回退到PATH、环境变量或宿主包管理器。当前Linux增强签名程序复验见生产化任务，历史fixture不替代新证据。

## 2026-09-29 收口复验

- `cargo check --offline --locked --workspace`、`external_libs` 6/6、CLI Lib 单测 4/4、Port Adapter Lib 请求定向 1/1 通过。
- 本阶段已完成仓库内可安全验证的协议、lock、fixture resolver、artifact 校验、embedded resource 和 run/build preparation；真实 pip/npm/Go registry、受管 CPython/Node/Go runtime、正式三语言 SDK、Package 发布元数据和无宿主运行仍为发行资产阻塞。

## Integration Gate

2026-09-30 当前边界：Port/Adapter 16/16、协议 lifecycle 1/1、typed 三 SDK 宿主协议、Python/JS 默认 Worker 6/6、资源 9/9、Linux cache 14/14 与实际不同 UID 1/1 已通过。managed Node 已由 checked App CMD 编进独立 Native 程序；受管 Go Worker 已按锁编译并完成源码移除、清空环境的独立执行；Package 已实际完成安装→check/test/run/build→停止 daemon→独立运行。自有工具包不等于正式签名或无宿主发行；高级生态安装、daemon 编译和正式后端仍有未实现代码，不仅是“发行资产阻塞”。

三个子任务完成后，使用同一 Port 的 Dever/exec/Python/Node/Go 实现进行定向端到端验证，并证明目标环境没有系统 Python、Node、Go 时构建产物仍可运行。

## Rollback

外部 Adapter 是新语法且没有旧兼容面。若某子任务未通过，不启用对应生态或打包入口；现有纯 Dever Port/Adapter、run/build 与配置合同保持可用。

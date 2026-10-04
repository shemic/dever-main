# Dever Lib 构建与 Package 集成

## Goal

把已锁定的 external Adapter 接入 `dever run/build`，生成无需目标机器安装第三方语言环境的自包含应用，并允许 Dever Package 声明和传递 Lib 依赖。

## Dependencies

- `external-lib-protocol` 与 `external-lib-ecosystems` 已完成。
- 私有编译器发行任务提供正式目标 runtime/worker packs；没有正式 pack 的目标不能标记支持。

## Requirements

- `run` 在用户代码前验证 external declarations、lock、cache、runtime pack 和 schema，启动选中 Adapter Worker，并在应用生命周期结束时完整清理。
- `build` 为目标平台准备所有运行时可选 external Adapter，不按当前 `setting.json` 偷偷裁剪闭合实现集合。
- Worker、语言 runtime、依赖和 manifest 作为摘要保护的只读资源嵌入现有独立应用输出，不把源码、下载 cache 或构建工具链带入产物。
- 运行时只释放到应用目录的 `data/cache/lib/<digest>`，使用私有 staging、完整性验证和原子发布；不执行已存在但摘要不匹配的文件。
- Dever Package 中的 external declaration 是传递 Lib 请求来源；项目解析统一 lock，Package 使用者不需要直接运行 `lib add` 才能理解依赖，但缺 cache 时仍需显式联网命令。
- Package 更新导致 Lib/schema 变化时必须更新 lock；旧 lock 不回退、猜测或运行。
- 同一项目的不同 Adapter 可以携带不同 runtime/版本；构建报告分项显示体积，不隐藏大 runtime 成本。

## Acceptance Criteria

- [ ] `run` 对 exec/Python/Node/Go fixture 自动启动、调用和关闭 Worker，现有纯 Dever 项目行为不变。
- [ ] `build` 输出在没有 Python、Node、Go 和 Dever 编译器的隔离目标环境运行成功。
- [ ] 资源截断、替换、权限错误、staging 中断和并发首次启动不会执行未验证或半成品 Worker。
- [ ] 切换 `adapter.<port>.use` 不重新构建即可选择已打包候选；未打包/未知选择启动失败。
- [ ] 安装一个携带 Lib 的 Dever Package 后，项目 lock 可确定性解析并离线重建。
- [ ] 构建报告列出每个 runtime/Worker/依赖的压缩与释放体积，重复内容只嵌入一次。
- [ ] application tests 继续使用 case fake，不释放或启动生产 Lib。

## Out Of Scope

- 动态从生产网络下载缺失 Worker。
- 把不同应用的运行时释放目录合并为全局可写执行目录。
- FFI、共享库加载和外语源码调试器集成。

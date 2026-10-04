# Dever Lib 生态解析

## Goal

在统一 external Adapter/Worker 协议之上，实现确定性的 `dever lib`、`dever.lock`、机器级内容缓存以及首批 pip/npm/go resolver 和官方 SDK。

## Dependencies

- `external-lib-protocol` 已完成并冻结协议版本和 external Adapter HIR。
- `shared-toolchain` 提供机器级版本目录、权限和共享 cache API；本任务不复制该实现。

## Requirements

- 命令固定为 `lib add/list/update/remove/doctor <project-root> ...`，保持 CLI 的显式项目根合同。
- spec 使用 `pip:name@version`、`npm:name@version`、`go:module@version`；禁止无版本请求进入 lock。
- `add/update` 解析完整传递依赖、平台 artifact、runtime pack 与摘要，写 canonical `dever.lock`；写入采用 prepared + atomic replace。
- `list/doctor/remove` 默认不联网；`doctor` 校验声明、lock、缓存和 Adapter schema identity 一致。
- 共享 cache 按 verified bytes 和 target identity 去重；项目之间不共享可写 dependency state。
- pip/npm/go 各自使用固定 resolver adapter，不执行项目提供的 shell；需要构建的生态 hook 必须在声明 capability 的隔离构建 sandbox 内运行。
- Python、JavaScript、Go SDK 实现同一个协议和生成的类型合同；SDK 不定义第二套业务 schema。
- 所有项目命令离线消费 lock；缺失 artifact 给出需要执行的精确 `dever lib` 命令，不隐式下载。

## Acceptance Criteria

- [ ] 三个生态的本地 fixture registry 解析出确定性 lock；重复解析字节一致。
- [ ] 版本冲突、摘要错误、篡改 cache、非法归档路径、缺平台 artifact 和 schema 漂移均明确失败。
- [ ] 两个项目共享相同 artifact bytes，但 lock、Adapter setting 和项目文件保持隔离。
- [ ] Python/Node/Go SDK fixture 完成 handshake、success、declared error、cancel 和 shutdown。
- [ ] PATH 中放置伪造 python/node/go 不影响选择；不读取语言相关环境变量。
- [ ] 断网且 cache 完整时项目命令前置验证成功；cache 缺失时不联网。
- [ ] 新 resolver 可通过固定内部接口加入，不修改 CLI 命令、lock 核心 schema 或 Worker 协议。

## Out Of Scope

- Maven、NuGet、Composer、Gem 的正式 resolver。
- 公共 registry 服务建设和真实互联网端到端测试。
- 最终应用资源嵌入与目标机器运行。

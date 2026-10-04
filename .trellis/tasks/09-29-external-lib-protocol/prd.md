# Dever Lib 协议与 Worker

## Goal

在不引入任何具体语言包管理器的前提下，完成外部 Adapter 的语言合同、统一进程协议、通用 `exec` Worker 和安全生命周期，为所有后续生态提供唯一运行边界。

## Requirements

- 仅 Adapter role 接受一个 contextual `external exec "relative/entry"` 声明；它可与一个现有 `setting` record 共存，不可与 Dever 实现函数混合。
- 外部 Adapter 自动完整实现其对应 Port；缺 Port、多个 Port identity、非法 entry、Lib/capability 字段或不完整 Worker handshake 均在入口前失败。
- 初始 capability 固定为 `network`、`file`、`process`、`gpu`；未知、重复或与平台不兼容的 capability 拒绝。
- 复用 Port 的类型/failure/effect/setting schema，不向 App 暴露 Worker/JSON/进程类型。
- 实现版本化、长度分帧、预算受限的 Component Protocol，并拒绝重复字段、未知消息、重复/未知请求 ID、错误 schema 和非法 error variant。
- Worker 由应用 Scope 拥有，使用显式可执行路径和清理后的环境，不通过 shell 或 PATH 查找。
- Test 继续要求 case-local Port fake，不启动生产 Worker，不读取 Adapter setting。
- reference backend 对 external Adapter 返回明确不支持；native backend 完成正式执行，不增加模拟 fallback。

## Acceptance Criteria

- [ ] 合法 external Adapter 通过 check/format/Markdown 合同，并参与现有 Port effect/failure/specialization。
- [ ] 通用 fixture Worker 完成 setting handshake、两次调用、业务 error、取消、健康检查和正常关闭。
- [ ] schema/Port/Adapter/protocol 不匹配、超大/截断/重复/乱序消息均在有界时间失败。
- [ ] Worker 启动失败、崩溃、超时、调用取消和主程序退出不遗留子进程或悬挂任务。
- [ ] stdout 只能承载协议，stderr 日志不会破坏响应；Secret 和输入值不进入协议错误。
- [ ] 未声明 capability 的受控操作被 supervisor 拒绝；声明 capability 进入 Port 保守 effect。
- [ ] 现有 Dever Adapter 选择和 application-test fake 回归保持通过。

## Out Of Scope

- pip/npm/go resolver、SDK、联网下载和项目 lock。
- 把 Worker 资源嵌入最终 build 产物。
- FFI、动态库、任意 shell 和远程 Worker。

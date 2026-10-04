# Design

## Compiler Model

HIR `Adapter` 增加 closed implementation kind：`Dever` 或 `External`. `External` 保存 ecosystem、entry、capabilities 和 compiler-generated Port wire schema。Port 仍是唯一 Function ID；dependency/effect/failure owners 看到外部实现的显式摘要，不解析外语源码。

Parser 仅在 Adapter role 把 `external`、`lib` 和 `allow` 识别为 contextual syntax。首个子任务只接受 `external exec`，但 AST/HIR ecosystem 使用封闭 enum，后续 pip/npm/go 不复制 parser 或 dispatch。

## Wire

每帧为 `u32 big-endian length + UTF-8 JSON`，最大正文复用 runtime wire 16 MiB 限制。消息包含固定 `kind` 与按 kind 严格字段；payload 使用 compiler-generated concrete codec，不把动态 Node 交给业务。

启动序列：supervisor 启动进程 -> `hello` -> Worker `ready` -> admission。关闭序列：停止 admission -> cancel pending -> `shutdown` -> join -> bounded kill。协议 EOF、非法 frame 或进程退出关闭全部 pending request，并保留第一个 runtime fault。

## Process Ownership

新增 runtime `component` owner，封装 Child、stdin writer、stdout reader、stderr bounded logger、pending request table 和 Scope guard。所有 task 注册到现有结构化作用域；Drop 只能触发已有 supervisor 清理，不能 detached kill/reap。

不使用 shell、PATH 或应用环境变量。entry 由构建器规范化为当前项目/产物拥有的文件；运行器传入专用管道和固定协议参数。初期 sandbox adapter 提供统一拒绝/允许合同，平台强化放在后续生态/打包子任务，不假装纯 Rust 检查等于 OS sandbox。

## Failure Semantics

Worker 返回的业务 error 必须匹配 Port declared error identity。启动、协议、timeout、crash 和 sandbox 失败转为 source-located runtime fault，不伪造业务 variant，不自动重放请求。崩溃后允许下一次调用创建新 Worker，但有固定启动频率上限。

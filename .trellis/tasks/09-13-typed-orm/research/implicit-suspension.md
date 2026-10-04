# 隐式挂起的顺序调用

## 决定

Dever 源码删除 function/handler 的 `async` 标记和直接异步调用外层的 `await(...)`。普通调用始终表示调用完成后才执行下一条语句；如果调用链包含数据库、网络、定时器、Channel 或异步 Stream 等挂起效果，编译器自动把对应函数降低为协程。

```dever
route(request: dever.http.Request) (response: dever.http.Response) {
  user = user.get(request.user_id)
  response = render(user)
}
```

等待 I/O 时只挂起当前协程，Tokio 工作线程可以执行其它连接和任务。语法简化不意味着阻塞线程，也不改变 HTTP/TCP/WS/SSE runtime、连接池、背压、超时与取消清理。

## 并发边界

普通调用严格顺序；只有 `run` 显式启动并发：

```dever
first = run(load_user(first_id))
second = run(load_user(second_id))
first_user = wait(first)
second_user = wait(second)
```

- `run(call(...))` 创建受限 Task。
- `wait(task_or_group)` 消耗任务或任务组并取得结果/等待排空。
- `stop(task_or_group)` 请求取消并等待结构化清理。
- CPU 同步计算继续使用 `parallel`，明确阻塞调用继续使用 `blocking`。
- 普通调用不产生 Task，也不允许把可挂起调用结果当 Future 保存。

## 编译器边界

- 系统 intrinsic 和官方库入口提供基础挂起效果；调用图、静态 handler 专门化和 collection handler 传播效果。
- checker 在效果推导后区分顺序调用、`run` 和 Task `wait`；HIR/native 仍可保留内部 coroutine/await 节点，但不把它们暴露到源码。
- 同步入口的完整可达图没有挂起效果时沿用直接原生入口，不初始化 Tokio。
- handler 是否允许挂起由其使用位置和实际效果静态检查，不再由源码 `async handler` 标记。
- API snapshot、Markdown 合同和格式化输出的是新语法；旧 `async`/`await` 不保留兼容分支。

## 迁移范围

- lexer、parser、syntax AST、checker、effect/cycle/failure 分析、HIR 与 native emitter。
- `dever.task`、`dever.net`、`dever.http`、`dever.websocket`、`dever.sse` 的官方 Dever 声明。
- HTTP/1、HTTP/2、TCP、TLS、WebSocket、SSE、AsyncStream、Channel 和结构化并发测试中的嵌入源码。
- backend/http/live/async 示例、性能 fixtures、LANGUAGE 与 Trellis backend 规范。

Rust runtime 内的 Rust `async/.await` 不在迁移范围；删除它们会把高并发 I/O 退化为阻塞或线程堆叠，与目标相反。

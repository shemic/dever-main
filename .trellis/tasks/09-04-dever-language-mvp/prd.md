# Dever Language Definition

## N5：HTTP/2（已完成）

用户在 P2 性能验证结束后批准继续接入 HTTP/2。使用 Hyper 实现 buffered/live 服务端和 single-use/pooled 客户端；复用既有请求/响应、TLS、AsyncStream 与 SSE 接口。Limits.http2 显式配置协议和每连接流数、接收窗口，TLS 必须协商 h2，明文使用 prior knowledge。每流取消隔离，Hyper 内部任务保留 Dever Scope，GOAWAY 排空不自动重试已发请求。详细验收以 `../09-12-http2/prd.md` 为准。

N5 已由 88 项定向测试、原生执行和独立审查验收。后续 HTTP/2 性能矩阵与有界稳态/恢复压测已由子任务完成；小时级稳定性、编译器发行/跨平台、自举及其余路线图仍未完成。

仅定向协议、生命周期与原生桥接验证；不重跑 P1/P2 性能矩阵，不改根任务调度，不接 RFC 8441、h2c Upgrade、推送、trailer、数据库或发行工具。下文各轮排除范围仅约束相应历史阶段。

## 已完成阶段 N4：并发组合与可复用网络客户端（2026-09-12）

用户批准完成上一轮清单的 1 和 2。交付源码运行参数、静态 HTTP handler 的类型化上下文、任务超时/多路竞争等待、周期与 Channel 流、异步内存序列遍历；有界 HTTP 连接池、域名、流式请求/响应、HTTPS/WSS 客户端和 TLS 服务端。保留 N3 SSE/WS 会话、取消和背压契约。HTTP/2、数据库、协议扩展和分发工具不属于本轮。

任务、连接及正文队列有界；取消和关闭回收所属工作，已开始的阻塞工作必须排空。复用 Tokio、Hyper、tungstenite 和 Rustls，不维护兼容接口或自写协议栈。只运行定向编译/测试及自有 loopback 随机端口，不测旧性能基线，不启动现有服务，不提交。

以上范围已接通 checked source、原生编译和运行时。83 项定向用例、实际 CLI check/fmt/build 与无 Rust 工具链环境中的独立后端示例通过；具体证据见 implement.md 的 N4 Evidence，公开用法以 LANGUAGE.md 为准。

## 已完成阶段 N3：流式响应、SSE 与 WebSocket（2026-09-12）

用户已授权“做下一阶段”，沿用新语言直接演进和低内存运行目标。保留普通 HTTP 的直接 handler 路径，增加 `serve_live` 和 HttpReply：同端口处理完整响应、逐块正文、SSE 和 WebSocket。HTTP 请求仍有界缓冲；响应通道只有 1 个槽位，长连接配置与普通请求期限分开。

WebSocket 服务端/客户端复用 tokio-tungstenite 0.30.0（handshake feature），提供 Text/Binary/Ping/Pong、AsyncStream 消息消费和关闭握手。SSE 提供字段编码、Last-Event-ID 访问和空闲注释心跳，历史重放归应用所有。HttpReply/WebSocket 进入编译器资源、类型、effects、API、Markdown 和 native 契约；不引入新语法、旧 API 兼容或自写 WebSocket 协议栈。

连接、handler 及升级后会话共用 Scope/Group 上限与取消排空；等待写锁时取消不能破坏另一任务的帧。仅执行根 test/ 下的定向编译器/运行时验证和自有回环端口，不测试旧性能基线、常驻示例或已有服务。TLS/WSS、HTTP/2、客户端池、子协议/压缩扩展、关闭帧元数据和数据库不属于 N3。

## 前一阶段 N2：HTTP 引擎（2026-09-12，已完成）

用户在 N1 完成后批准继续。采用 Hyper 1.x HTTP/1 引擎、hyper-util TokioIo/Timer 和共享 bytes 缓冲区；不额外引入路由框架。此阶段取代历史“HTTP 全部由 Dever 自写”的实现决定。官方类型与静态业务 handler 仍由 Dever 定义，不保留旧协议 API 或备用解析器。

- `serve(async handler, Listener, Limits) -> ()`：持久连接、流水线、chunked 输入，复用现有有界任务组、背压、超时和取消清理。accept/配置/程序故障沿任务传播，对端协议错误只结束对应连接。
- `send(address, port, Request, Limits) -> ResponseResult`：单次请求，无客户端池和重试，driver 与请求同生命周期。
- Header 列表保留重复头和字节值；消息体在显式上限内缓冲，单帧共享，多帧增量合并。
- 直接迁移示例、检查器、native bridge 和测试。只用离线定向检查、自有 loopback 随机端口，不测旧基线、不跑全量或现有服务。
- TLS、HTTP/2、流式响应、客户端池、非空 trailer、WebSocket/SSE 留待后续；N3 优先处理长连接协议。

## 前一阶段 N1：异步 Stream 与 TCP（2026-09-12，已完成）

用户批准继续，并明确这是全新的语言：无需旧源码兼容，不增加兼容别名、回退实现或推测性的防御分支。仓库调用方直接迁移。此决定取代下文历史阶段中的“网络暂停”和“兼容基线”要求。

- 增加 `AsyncStream<Item>`，支持在 async function 中按需 `each`、`reduce`、`reduce_until`，以及有界 `parallel_each`；handler 可为 async，关闭/提前结束释放生产端。
- `dever.net` 直接采用 Tokio 异步 TCP；connect、listen、accept、read、write 为 async。资源名称仍为 Socket、Listener；无 sync/aio 两套 API。
- 连接流和字节流不预取；并发消费先等待容量再拉取。复用现有结构化 Task/Group，停止后等待子任务清理，连接支持同时读写。
- 保留协议必须的输入约束、失败输出、超时、关闭和取消语义；写入中断不得自动重发已写字节。
- 现有 HTTP 传输调用链随 TCP 改为 async；成熟 HTTP 引擎接入属于 N2，WebSocket/SSE 属于 N3，本轮不实施。
- 只做根 test/ 下定向检查及自己创建的 loopback 随机端口验证；不测旧性能基线，不运行全量测试、业务服务、全量 build 或全局安装。

## 前一阶段：并发基础（2026-09-12，已完成）

前一阶段先完成同步、异步和多线程（含协程/任务）基础，网络当时暂停；当前 N1 恢复异步 Stream/TCP。下述并发契约已经进入编译器前端、HIR、原生后端和 runtime；最终状态以本任务的定向验证和 `LANGUAGE.md` 公开契约为准。不测旧实现性能基线。

### 暂存的网络方案

- 后续需要 TCP、HTTP、WebSocket、SSE 长连接，复用成熟 Rust 库。
- 候选栈：Tokio 统一网络调度，Hyper/hyper-util 处理 HTTP，Axum 复用服务路由、升级和 SSE，WebSocket 复用其 tungstenite 生态，bytes 管理共享缓冲区。实施前核对版本、工具链和 feature 兼容性。
- 共用连接限额、有界队列、背压、超时、取消与关闭。长连接保活独立于普通请求超时；SSE 事件历史保存/重放属于应用责任。
- 不在每次 I/O 上 block_on，也不把同步阻塞 handler 直接放入异步调度线程。HTTPS/WSS、HTTP/2 的首轮范围尚未确定。
- 取消旧性能基线工作，不取消新能力必要的定向正确性验证；不启动业务服务或压测。

### 并发语言契约（已批准实施）

#### Goal

增加可明确推理的同步、异步协程和多线程能力，为 TCP、HTTP、WebSocket 与 SSE 提供统一运行基础。小机器只运行打包后的二进制；空闲任务不占用一个系统线程，任务、线程和队列都有明确上限。契约可以直接演进，维护值/资源语义的清晰性，不承担旧源码兼容义务。

#### In Scope

- 普通 function 保持同步；`async` 声明异步 function，`await` 等待异步调用或已启动任务。
- `run` 启动异步调用并返回受限 Task；`stop` 采用“请求协作取消并等待清理完成”的安全语义，而不是仅发送信号。
- 每个 async function 是默认结构化任务域；动态数量的零输出任务可由有界 `group` 统一拥有、等待和停止。
- `parallel` 将 pure 同步计算交给有界多线程执行；`blocking` 供明确的阻塞边界使用。`parallel_each` 按序列类型复用相应的有界执行器。
- 有界 `channel` 以及 `send`、`receive`、`close` 用于任务通信；禁止无界队列。
- async 静态 handler、跨线程可传递性、任务消费、effects/failures、API 快照、Markdown 契约、格式化和原生代码生成均由编译器检查。
- 运行时复用 Tokio 的协程与多线程调度；整个程序最多建立一个异步运行时，CPU/阻塞任务使用受限执行通道。

#### Out of Scope

- 本阶段不实现 TCP、HTTP、WebSocket、SSE、TLS 或数据库；网络方案按上节暂存。
- 不加入裸线程、共享可变全局对象、用户锁、分离后无人管理的后台任务、无界 channel、强杀线程或副作用回滚。
- 不同时加入 lambda、闭包、一等 function、用户泛型、用户递归、循环块或异常机制。
- 同步 `Stream<Item>` 保持当前阻塞拉取契约；异步流在网络恢复前单独设计，不做隐式转换。

#### Acceptance Criteria

1. 完全同步的入口不启动 Tokio runtime；接口发生变化时直接迁移仓库源码。
2. async function 和 async 静态 handler 的调用都必须直接位于 `await(...)` 或 `run(...)` 上下文；同步 function 中的异步调用在原生构建前拒绝。
3. `await(call(...))` 顺序执行并保持原函数的命名输出；`task = run(call(...))` 先启动，`await(task)` 恰好消费一次任务并取得相同输出。
4. Task 不能复制、存入 record/List/Map/Stream、作为普通参数/输出逃逸、重复等待或在 function 返回时仍未消费；必须 `await` 或 `stop`。
5. `stop(...)` 第一阶段只接受零输出 Task 或 Group，协作取消并等待清理；已发生的副作用不回滚，长时间同步计算不承诺立即停止。
6. `group(limit)` 的 limit 必须为正且有实现上限；只拥有零输出异步任务。容量已满时 `run(group, call(...))` 施加背压，不无限排队；`await(group)` 或 `stop(group)` 消费该 Group。
7. `parallel(call(...))` 只接受 pure 同步 function，参数和输出必须可跨线程传递；`blocking(call(...))` 只接受编译器传递 effect 分析确认包含阻塞系统边界的同步调用。二者第一阶段只能用于 async function，并使用明确的有界容量；同步代码继续使用现有 `parallel_each`。
8. `channel(Type, capacity)` 容量必须为正且有实现上限；send 在满时等待，receive 在空时等待并返回 `Item?`（值表示 Item，`null` 表示 Closed），close 唤醒等待方；调度器或 Channel 内部故障作为带源码位置的 fatal error 传播。
9. async handler 的同步/异步属性是签名的一部分；所有同签名分句必须一致，专门化和递归检查不能因异步边界失效。
10. 任务启动、等待、停止、channel、时间等待和 blocking 均进入传递 effect 分析；pure function 不能隐藏这些效果，失败义务不能因任务边界被吞掉。
11. 异步入口只创建一次运行时；多个挂起任务不按任务创建线程。工作线程、阻塞线程、group、channel 与待处理任务均有保守默认上限及显式配置边界。
12. 仅运行根 `test/` 下针对新增语法、检查、原生执行、取消/关闭和有界调度的离线定向测试；不跑全量测试、服务测试、旧实现基线或网络压测。

#### Key Naming Decisions

- 保留 `async`、`await`、`parallel`、`blocking`、`channel`、`send`、`receive`、`close`。
- 使用 `run` 代替 spawn，使用 `stop` 代替 cancel，使用 `group` 表示动态任务所有者；不再增加 join，统一由 `await` 等待。任务操作沿用函数式调用外观，不提供第二套前缀语法。
- 协程是 async Task 的运行时实现，不增加 coroutine 关键字。

## Markdown Source Follow-through (2026-09-09)

用户批准保留 `.dever`，新增 `.dever.md` 源码容器，并进一步要求编译器强制说明与代码一致。顶层 `dever` 或 `typescript dever` 围栏内写正常 Dever 声明，共用原有语法、契约、API 和原生路径。一个 H1 说明 package；每个 H2 只说明一个 type 或一个 `(函数名, 参数数量)` 及其全部分句。编译器核对 package 名、公开类型/方法/使用方式、类型字段/分支、函数输入输出的名称、类型、数量和顺序，空项显式写 `无`。`typescript dever` 的首个标记供编辑器尝试 TypeScript 语法高亮。跨格式同包重复拒绝；格式化保留说明，诊断定位原文并关联代码。此决定替代下方历史阶段“只加载 `.dever` 文件”及“标题说明无约束”的范围限制，不开放其它语言或 HIR/JSON 作为源码。正式写法见 `LANGUAGE.md` 的 Markdown 小节。

## Approved Performance Follow-through (2026-09-08)

用户批准实施本轮性能方案。保持现有语法、值/资源语义、错误位置、数值顺序和编译器契约。交付类型属性缓存、依赖驱动契约分析、记录更新复制消除、独占列表消费、直接消费字符列表的遍历优化、HTTP 增量扫描、标准库及原生产物缓存。循环融合必须证明副作用和失败顺序等价；并发分批以有限测量为依据，不要求线程池。补编译/运行/内存定向基准并记录优化前后证据。只运行离线定向测试、自有临时文件/进程和内存协议输入，不运行全量或服务压测，不提交或安装全局命令。

## Current Follow-through (2026-09-07)

用户在剩余项盘点后要求“实施剩下的”。继续完成已确定语言的工具、标准库和验证工作，并推进后续运行能力；既有语法、分句、静态 handler、值/资源语义保持兼容。下方历史里程碑的暂缓项不阻止本次明确要求的工作。

- 提供 `.dever` 的规范格式化及 CLI `fmt`/只读检查，保留注释、表达式含义和求值顺序；任一输入语法错误时不改写文件。
- 补常用 Text、数值解析/格式化、进程参数/环境和时钟接口。公开接口与组合逻辑仍由普通 `.dever` package 定义。
- 实现仅用于验证的 HIR 基准解释器，覆盖已实现核心语义并与原生结果、故障位置对照；生产 build/run 始终使用原生编译。
- 实现保持值语义的最后使用优化，提供可复现、有限工作量的同语义性能基线，不对现有服务压测。
- 继续推进 JSON、HTTP 与受控并发等官方包所需能力；协议与应用逻辑使用 Dever 基础语法，系统边界保持最小。
- 数据库引擎此前未定，已向用户异步询问；这一选择不阻塞其余工作。
- 编译器发行、跨平台沿用此前暂缓安排；自举仍是尚未交付的长期项。本次不安装或替换全局命令，不提交，不访问既有服务。

验收按每项真实完成的行为和定向证据记录，不将已完成的基础语法测试当作新增工具和库的验收。

## Goal

当前目标是实现已经确定的基础语言，使官方 package 与应用能够用同一套 `.dever` 语法编写。2026-09-07 用户批准实现基础类型、表达式、函数输出、分句、赋值和值语义、集合与处理函数、统一 package 编译及最小系统接口。原生 Hello World 里程碑已于 2026-09-06 完成，作为回归基线。

长期目标仍是改善 AI 开发中的硬编码、重复实现、无依据兜底和大项目上下文问题。表面语法保留 package、type、function、分句和值语义；用户所说的“用户类”用已有 `type User` 表达，不引入 class 或继承。Rust 等引导技术不进入用户源码或语言兼容契约。

基础阶段曾将 HTTP、JSON 和性能验证后移；当前 Follow-through 已推进这些能力。编译器发行与跨平台仍暂缓，数据库引擎未定；用户 CRUD 示例不是基础语言的额外交付。保留原生执行方向和既有表面语法，不增加契约 DSL。

## Approved Core Implementation

- 沿用既定条件分句、线性 body 和集合函数；不再把循环语法作为待确认项，不增加条件块、循环块或用户递归。
- 每项语言能力贯通源码检查、typed HIR 与原生执行；不能以 AST 已解析代替支持。
- 官方 `.dever` package 与本地 package 共用语言规则。官方命名空间由编译器随附源码提供，本地源码不能冒充官方来源。
- 系统接口提供 Bytes、opaque 文件/TCP 资源、读写和显式关闭；普通值保持值语义，资源别名标识同一外部资源，关闭后操作得到显式失败。
- 可恢复系统错误以官方源码声明的选择型结果返回；程序故障保持源码定位。系统原语不包含 HTTP 路由或应用业务。
- 验收覆盖完整语言样例、负例诊断、分句顺序变化、集合顺序与值语义，以及隔离临时文件和本地回环连接。使用必要的最小定向检查，不运行全量测试、既有服务或压测。

## Approved Foundation Extension

Status: implemented and verified on 2026-09-07.

### Goal And Scope

补齐官方 `.dever` package 编写协议解析、持续 I/O 组合和宿主回调所需的最小表达能力，同时保留分句条件、线性 body、无循环 block 和无用户递归的长期规则。本阶段一次完成 `reduce`、`reduce_until`、Bytes 遍历、`Stream<Item>`、受限静态 handler 参数，以及文件/TCP 的流式包装；不在 runtime 中加入 HTTP 或业务特例。

### Sequence Operations

- 带状态遍历对外名称固定为 `reduce`；此前讨论中的 `fold` 和 `accumulate` 名称作废。
- `reduce(handler, sequence, initial_state)` 固定从左到右执行，始终显式提供初始状态；空序列直接返回初始状态。
- handler 依次接收当前元素和上一步状态，必须输出一个与状态类型相同的下一步状态：

```dever
add(value: Int, total: Int) (next_total: Int) {
  next_total = total + value
}

total(values: List<Int>) (result: Int) {
  result = reduce(add, values, 0)
}
```

- `reduce_until(handler, sequence, initial_state)` 使用相同顺序，但 handler 必须有两个输出：第一项是下一步状态，第二项必须命名为 `stop` 且类型为 Bool。处理完当前元素后，`stop` 为 true 就立即返回，不再请求下一个元素。
- `reduce` 和 `reduce_until` 不提供额外 context 参数；共享配置应组合进状态，避免同一调用同时维护两套跨元素数据。
- sequence 只接受 `List<Item>`、Bytes 或 `Stream<Item>`。Bytes 的元素静态类型为 Int，运行时值保证在 0 至 255，不先复制为 `List<Int>`。Text 如需按 UTF-8 遍历，必须显式使用 `dever.bytes.from_text`。
- `each(List/Bytes)` 保留零输出动作和单输出 List 映射；`each(Stream)` 只接受零输出 handler，不把未知长度或无限输出隐式收集到内存。Stream 的最终状态由 `reduce`/`reduce_until` 返回。
- `filter`、`find`、`sum` 本阶段仍只处理 List；不增加 Stream 到 Stream 的惰性 map/filter 管道。

### Static Handler Inputs

function 输入可以声明受限 handler 签名：

```dever
serve(
  route: handler(request: Request) (response: Response),
  config: Config
) (result: ServeResult) {
  result = serve_with(route, config)
}
```

- handler 类型只允许出现在 function 输入中；输入按位置匹配类型，命名输出同时匹配名称、顺序和类型。
- 调用方只能传入签名匹配的具名 function，或转发当前 function 的 handler 参数。
- handler 参数可以直接调用，也可以传给 `each`、`reduce`、`reduce_until` 或另一个 handler 输入；不能参与分句匹配，不能赋给局部值、存入 record/List/Map/Stream、比较或作为输出返回。
- handler 不是运行时值。typed HIR 保留静态绑定，AOT 以 function 和实际 handler 绑定组成专门化实例，生成直接调用；不产生函数指针、闭包、虚调用或反射注册表。
- 递归检查覆盖 handler 调用与转发后的专门化调用图，不能通过 handler 绕过无递归规则。

### Stream And Resource Contract

- `Stream<Item>` 是编译器提供的唯一新增标准泛型类型，用户仍不能声明泛型 type 或泛型 function。
- Stream 是拉取式、顺序、一次消费的 opaque 资源；复制 Stream 只产生同一资源的别名，所有别名共享读取位置、结束状态和显式关闭状态。Stream 不可比较，也不能成为 Map key。
- `reduce_until` 提前停止时 Stream 保持可用，调用方可从下一个未消费元素继续处理。正常耗尽后继续消费只得到结束，不会重新执行生产者。
- 正常 EOF 使用 Stream 自然结束表示，不作为普通元素。可恢复 I/O 错误必须作为官方 choice 的 `Failed(message: Text)` 元素交给 handler；终止性错误产生一个 Failed 后结束 Stream，不能转换成默认值或程序故障。
- 流式读取使用 `dever.io.ReadEvent`，其分支固定为 `Chunk(bytes: Bytes)` 和 `Failed(message: Text)`；现有单次 `read` 继续返回 `ReadResult.Read/End/Failed`，两种接口不混用结束语义。
- 流创建结果固定为 `dever.io.ReadStreamResult`，其分支为 `Streaming(stream: Stream<ReadEvent>)` 和 `Failed(message: Text)`。`dever.io.chunks(file, limit)` 与 `dever.net.chunks(socket, limit)` 都返回该类型；非正数、无法表示的 limit 或创建时已经关闭的源立即返回 Failed。
- `dever.net.connections(listener)` 直接产生 `Stream<ConnectResult>`；创建后关闭 listener 或终止性 accept 错误产生一个 `ConnectResult.Failed` 后结束。
- 核心操作 `close(stream)` 没有输出并且幂等。它停止该 Stream 及其别名并释放 Stream 持有的底层资源引用，但不主动关闭调用方仍单独持有的 File、Socket 或 Listener；关闭后的 Stream 与正常耗尽一样不再产生元素。调用方先关闭底层资源时，依赖它的未关闭 Stream 产生一个 Failed 后结束；最后一个资源别名离开作用域时仍由原生资源析构兜底。
- 首阶段只有阻塞、顺序执行；Socket 延续显式 timeout。没有预取、后台线程、async、await、并发消费或共享可变业务全局状态。

### Acceptance Criteria

1. List 和 Bytes 的 `reduce` 按源顺序传递状态，空序列返回初始状态；Bytes 不经过中间 List。
2. `reduce_until` 在 stop 元素之后不调用 handler 或拉取 Stream；同一 Stream 随后从未消费位置继续。
3. `each(Stream)` 接受零输出 handler，拒绝单输出或多输出 handler；List/Bytes 的既有映射和动作语义不回归。
4. handler 输入支持具名 function、跨 package function 和多层静态转发；签名不匹配、存储、返回、比较和 handler 间接递归均在原生构建前拒绝。
5. 生成程序中的 handler 是专门化直接调用；不引入通用 runtime Value、运行时函数指针或逐元素虚拟 handler 分派。
6. Stream 别名共享游标、结束和关闭状态；自然 EOF、提前停止、幂等 `close`、底层关闭与终止性错误均有确定测试。
7. 官方 `dever.io`/`dever.net` 流式接口作为普通 `.dever` 源码经过同一 parser、resolver、checker 和 HIR；runtime 只拥有系统读取和 Stream 拉取原语。
8. 文件验证只使用测试自有临时目录，TCP 验证只使用 `127.0.0.1` 随机端口、有限数据和超时；不连接、启动或修改现有服务。
9. 实际 `check`、`run`、`build` 与独立原生程序覆盖一个分块状态解析样例，证明结果来自 `.dever` handler 和状态，而非编译器特判。

### Out Of Scope

- HTTP、JSON、CRUD、数据库、并发服务器和异步语法；本阶段交付的是这些 package 可复用的底层表达能力。
- 一般化用户泛型、lambda、闭包、一等 function、Stream 惰性转换、循环 block、用户递归和异常机制。
- formatter、语义基准解释器、性能门槛、自举、package 分发、跨平台构建和全局命令迁移。

### Known Risks And Controls

- 静态 handler 转发可能产生重复原生实例；专门化键按 function 与完整 handler 绑定去重，并且只生成入口可达实例，不退化为运行时分派。
- Stream 的内部生产者调用有固定边界成本；文件和 Socket 以 Bytes chunk 为单位拉取，不按单字节跨越动态边界，有限 List/Bytes 热路径仍是直接循环。
- 首阶段的阻塞 Stream 不能承担并发服务器；这是明确范围限制，不能在 HTTP package 中用隐藏线程或全局队列规避。
- 把 I/O 失败作为最后一个元素要求 handler 穷尽处理官方 choice；类型检查和终止状态测试共同防止错误被静默丢弃或无限重复。

## Completed Hello Milestone

- 示例位于 `examples/dever/hello/app/user.dever`，与 `package app.user` 路径一致。
- 支持 Text 字面量、Text 参数、局部赋值、普通零输出函数调用和公开零输入入口。
- 内置标准签名为 `dever.io.println(text: Text) ()`，输出 UTF-8 文本和一个换行；写入失败报告 Dever 调用位置并以非零状态退出。
- 修改文本、调用其他自定义函数、跨包调用公开函数都必须由实际源码驱动；不能特判示例名称或内容。
- 名称、参数、可见性、包路径和重复定义检查在原生编译前完成；递归及包依赖循环按既有契约拒绝。
- 未实现的类型、表达式、分句和输出签名明确报不支持，不静默生成近似代码。
- 提供本地 `check <source-root>`、`run <source-root> <package.function>`、`build <source-root> <package.function> --output <path>`；不替换全局命令。
- 验收真实 stdout、错误定位、原生可执行文件和输出失败；不启动任何服务或执行全量测试。

## Deferred User API Proposal

以下保留之前的 API 讨论作为后续参考；其中持久化、并发与性能要求尚未重新确认，不属于当前验收或实施前置条件。

### Confirmed Requirements

- 应用业务使用 Dever 源码（`.dever` 或 `.dever.md` 的程序块）；不能手写 Rust 用户 CRUD 再把 Dever 当作装饰。
- 用户模型只有 ID、姓名、手机号，CRUD 和 HTTP JSON 交互必须实际可用。
- 保留已经讨论的表面语法；只增加实际应用所需的标准 package 能力和必要语义。
- 编译器与通用运行时不能特判 `User`、`name`、`phone` 或 `/users`；这些信息来自应用源码。修改业务类型和路由不能要求修改编译器。
- 数据需要真实持久化；重启后仍可查询，不能用进程内 Map 冒充实际存储。
- 性能验证覆盖真实读写、序列化和持久化，不用空接口成绩替代 CRUD 性能。

业务模型沿用现有语法：

```dever
type User {
  id: Id
  name: Text
  phone: Text
}
```

### Proposed API Acceptance

以下为待最终收敛的最小 API 行为，不增加登录、管理页面或其它业务模块：

| Request | Behavior |
| --- | --- |
| `POST /users` | 提交姓名和手机号；服务端生成 ID，返回创建的用户 |
| `GET /users/{id}` | 查询单个用户；不存在时返回明确的未找到结果 |
| `GET /users` | 按稳定顺序、有上限地分页查询 |
| `PUT /users/{id}` | 完整更新姓名和手机号，ID 不可变 |
| `DELETE /users/{id}` | 删除指定用户，删除后无法查询到 |

- `name`、`phone` 使用 Text；不把手机号当数值，也不擅自添加特定国家的手机号规则或手机号唯一约束。
- 缺少字段、非法 JSON、无效 ID、未知用户和存储失败需要明确响应；不能静默填值或伪装成功。
- 并发访问不能产生重复 ID、半写入或未说明的丢失更新；具体并发写入契约随存储设计确定。
- 吞吐、p50/p95/p99 延迟、错误率和资源使用需要在固定数据量、读写比例、并发和持久化配置下报告，并与同环境、同功能的原生基线比较。数值性能门槛仍待收敛，现有计算内核的 1.15 倍预算不能直接替代 API 指标。

### Open Decisions

- 后续 API 先允许内存测试数据；需要持久化时再讨论数据库、并发更新和性能预算。

### Deferred From This Milestone

- 契约 DSL、AI 生成器、全面重复语义检测和完整的大项目上下文工具。
- 登录/RBAC、前端页面、多租户、分布式部署及其它没有被请求的用户业务。
- 与用户 API 无关的完整数值标准库、全部集合组合、自举和组件分发系统；既有语言契约作为后续路线保留。

## Language Roadmap

后续章节保留原 Minimal Core 的语言契约，作为本轮定义讨论的基础。已确认的长期规则与首版能力限制分别标明；讨论中的功能不代表已经实现，编译器仍须明确拒绝尚未支持的源码。

## Repository Evidence

- 仓库当前没有已发布的 `.dever` parser、formatter 或兼容格式。
- 2026-09-05 开工检查确认：当前仓库只有规划资料，没有 Cargo workspace、Rust 源码或测试。HEAD 也只包含两份旧规划文档。
- 新语言没有兼容旧语义图、旧 CLI、旧生成结果或旧测试的义务。旧规划由 Git 文档历史保留，不在新实现中增加迁移层。
- 从零建立 Rust workspace；复用源码位置诊断思路和仓库根目录 `test/` 约束。旧领域模型和旧后端契约不能决定新语法。

## Core Language Roadmap Scope

1. `.dever` 源文件、UTF-8 文本、注释、literal 和唯一 formatter。
2. package 路径、公开名称、完整限定名和源码根目录内的自动依赖解析。
3. 被动 type、字段型值、选择型值、可空值、List、Map、Bytes 和 opaque Stream 资源。
4. 显式 function 边界、线性 body、局部类型推断和值语义修改。
5. 通过同名 function 分句表达的穷尽、互斥条件规则。
6. 算术、比较、布尔和文本连接表达式。
7. `each`、`reduce`、`reduce_until`、`filter`、`find`、`sum` 以及最小 List/Map 标准函数。
8. Int、Decimal、Float 的确定数值语义和最小 `math`/`decimal` 标准 package。
9. parser、formatter、resolver、type checker、clause analyzer、语义基准解释器、原生 AOT 编译和 CLI。
10. Rust 编写的首代引导编译器，以及不依赖 Rust 表面语法、可被未来 Dever 自举编译器复用的 typed HIR 和运行语义。

## Language Contract

### 1. Source And Package

- 源文件后缀固定为 `.dever`，编码为 UTF-8。
- 代码标识符只使用 ASCII 英文；注释、字符串和文档可以使用任意 UTF-8。
- `#` 开始单行注释；Core 0.1 不提供块注释。`//` 只表示整数除法，不具有第二种词法含义。
- 语句和声明按换行或结构边界分隔，不写分号。
- 一个文件完整拥有一个 package；同一 package 不能拆到多个文件。
- 源文件相对 `src/` 的路径必须与 package 点分名称一致。

```text
src/account/user.dever  ->  package account.user
```

文件第一项必须是 package 声明：

```dever
package account.user exposes (
  User
  create
)
```

`exposes` 只能列出当前 package 已声明的 type 或 function。未列出的名称只在当前 package 可见；公开 function 名称会公开该名称的全部签名和分句。

同 package 引用使用短名称，跨 package 引用必须使用完整限定名：

```dever
normalized = normalize_email(email)
user = account.user.create(id, name, email)
```

Core 0.1 不提供 `import`、`use`、通配导入或别名。编译器从完整限定引用建立依赖图，并拒绝 package 循环依赖。`app`、`domain`、`feature`、`flow`、`policy`、`view` 和 `scenario` 都不是关键字，需要时只是普通 package 路径段。

#### Package Origins And Distributed Components

语言层只有 package，不新增 `component` 关键字。package 是源码和 API 命名空间；“组件”只是 registry 中用于安装、签名和版本管理的发布单元，可以包含一个根 package 及若干子 package。组件不能绕过各 package 的 `exposes` 边界。

package 来源固定分为三类：

| 来源 | 名称规则 | 示例 | 获取方式 |
| --- | --- | --- | --- |
| 核心标准 package | 编译器保留的单段名称 | `text`、`math`、`decimal` | 随编译器提供 |
| Dever 官方组件 | `dever.<component>` 根路径 | `dever.http`、`dever.sql` | 显式安装 |
| 公共第三方组件 | `<publisher>.<component>` 根路径 | `stripe.payment`、`shemic.robot` | 显式安装 |

本地业务 package 继续使用项目内路径，例如 `account.user`、`order.checkout`。本地源码不能声明核心标准名称或 `dever.*` 官方命名空间。同一个构建中如果本地源码、标准库或未来已安装组件提供了相同 package 名称，编译器直接报告重复定义，不进行覆盖或静默择优。

调用形式对所有来源一致，版本不会进入业务源码：

```dever
user = account.user.create(input)
name = text.trim(raw_name)
response = dever.http.send(request)
payment = stripe.payment.create(order)
```

从 0.2 起，组件的稳定身份与下载来源相互独立。`shemic.contact` 是进入 package 名称和类型身份的 component id；GitHub、其他 Git 服务或 Dever registry 只是可以替换的 source。仓库迁移不会迫使业务源码改名。

公共组件必须通过命令显式加入，不能因为遇到未知名称就在 `check` 或 `build` 时自动联网：

```text
dever add dever.http@1
dever add stripe.payment@3
dever add shemic.contact --git https://github.com/shemic/contact.git --tag v1.2.0
dever add shemic.contact --git https://github.com/shemic/contact.git --rev 8f34c2a
dever update stripe.payment
dever fetch
```

Git source 必须同时给出 tag 或 revision，不接受未固定的默认分支 HEAD。tag 在加入时解析为完整 commit；`dever.lock` 同时记录 source URL、完整 commit 和规范化源码内容的 SHA-256，后续 tag 被移动也不会改变构建。组件内全部 package 必须位于 component id 根路径下，例如 `shemic.contact` 可以提供 `shemic.contact.phone`，不能同时冒充 `account.user`。

0.2 的项目根目录增加两个由 CLI 维护、需要提交的元数据文件：

```text
dever.mod   # component id、直接依赖及其 registry/git/path 来源要求
dever.lock  # 完整依赖图、精确版本/commit、内容摘要和实际提供的 package
```

开发者仍只手写 `.dever`；`dever add`、`dever update` 和未来的本地 link 命令负责规范化更新元数据。分开保存意图和精确结果，使直接依赖保持可读，同时保证所有机器解析出同一份源码。

下载源码不复制到每个项目。Dever 使用平台级、可由 `DEVER_HOME` 改址的全局只读内容寻址仓库：

```text
$DEVER_HOME/
  git/<source-url-hash>/             # Git bare mirror，只负责取对象
  store/sha256/<content-hash>/       # 已校验、不可修改的组件源码
  build/<target>/<artifact-hash>/    # 可删除的编译缓存
```

项目目录只保留 `src/`、`dever.mod`、`dever.lock` 和可删除的 `target/dever/` 构建结果。多个项目依赖相同 commit 时共享同一份 store 内容。`dever add`、`dever update`、`dever fetch` 是仅有的默认联网入口；普通 `check`、`build` 和 `run` 只读取本地源码、内置标准 package、lock 与 store，缺失时明确要求执行 `dever fetch`，不隐式下载。

Core 0.1 只实现本地 package 与随编译器提供的核心标准 package，同时锁定上述命名和解析边界。registry、`dever add`、签名验证和外部组件缓存确定延后到 0.2，不影响现有 `.dever` 调用形式。

### 2. Canonical Naming

| 对象 | 唯一格式 | 示例 |
| --- | --- | --- |
| package 路径段 | `lower_snake_case`，优先单数领域名 | `account.user`、`sales_order` |
| 文件名 | `lower_snake_case` | `user.dever` |
| type | `UpperCamelCase` | `User`、`CreateResult` |
| 选择项 | `UpperCamelCase` | `Pending`、`Created` |
| function | `lower_snake_case` | `create`、`can_sign_in` |
| 输入、输出、局部值、字段 | `lower_snake_case` | `user_id`、`created_at` |
| 标准泛型参数 | 有含义的 `UpperCamelCase` | `Item`、`Key`、`Value` |

大小写不控制公开性。缩写按普通单词处理：`Id`、`Url`、`HttpClient`、`user_id`、`callback_url`。编译器强制可机械判断的大小写、ASCII 和路径格式；“是否是好名称”只产生规范诊断，不依赖英语词典阻止编译。

### 3. Types And Literals

Core 0.1 的基础标量只有：

| 类型 | 语义 |
| --- | --- |
| `Bool` | `true` 或 `false` |
| `Int` | 有符号 64 位整数，范围 `-9223372036854775808` 至 `9223372036854775807` |
| `Decimal` | decimal128，最多 34 位有效十进制数字，round half to even |
| `Float` | IEEE 754 binary64 近似数 |
| `Text` | UTF-8 文本；相等按实际字符序列判断，不做隐式 Unicode 规范化 |
| `Id` | 不透明稳定标识，只支持相等比较、传递和序列化 |

Core 0.1 不提供 `UInt`、`BigInt`、`Percent`、字符类型或隐式动态 `Json` 类型。

- 整数 literal 默认是 Int。
- 带小数点的 literal 默认是 Decimal；在明确要求 Float 的静态上下文中直接成为 Float。
- 科学计数 literal，例如 `1.25e6`，是 Float。
- Decimal literal 必须能被 34 位 decimal128 精确表示；源码常量不会被静默舍入，超出精度或范围时由编译器拒绝。运行时算术仍按已确定的银行家舍入产生结果。
- Text 使用双引号，支持 `\\`、`\"`、`\n`、`\r`、`\t` 和 `\u{HEX}` Unicode 转义。
- `true`、`false` 和 `null` 是保留 literal。
- 源码不提供 NaN 或 Infinity literal；Float 运算可以产生这些值。

用户定义 type 只有两种形状，不能混写：

```dever
type User {
  id: Id
  name: Text
  phone: Text?
}

type CreateResult {
  Created(user: User)
  InvalidPhone(reason: Text)
  EmailAlreadyUsed(email: Text)
}
```

字段型 type 是固定字段数据；选择型 type 表示同一时间只能是其中一种选择。type 没有方法、构造器、hook、继承或行为，也不能递归包含自身。

选择项必须通过所属 type 限定：

```dever
result = CreateResult.Created(user)
```

选择项携带的数据在声明中必须有名称和类型，构造时按声明位置传入。字段型值必须一次完整创建：

```dever
user = User {
  id = id
  name = name
  phone = null
}
```

未知、重复或遗漏字段都是编译错误，包括值为 `null` 的可空字段。formatter 按 type 声明顺序输出字段。点号只访问或修改已经完整存在的字段型值，不能逐字段创建半初始化对象。

`Value?` 表示 `Value` 或 `null`。普通类型永远不能为 `null`，`Value??` 非法，也没有 `?.`、`??`、强制解包或隐式默认值。可空值必须交给穷尽的 function 分句处理。

标准泛型函数产生可空返回值时，可空化是幂等的：`first(List<Int?>)`、`find` 的同类调用及 `get(Map<Text, Int?>, key)` 都返回 `Int?`，不产生嵌套可空类型。返回 `null` 不区分“没有元素/键”和“已有值为 null”；需要区分时，业务使用带显式选择项的 Value 类型。源码仍禁止显式书写 `Int??`。

### 4. List And Map Values

List 使用方括号创建：

```dever
users = [first_user, second_user]
empty_users() (users: List<User>) {
  users = []
}
```

List 元素必须是同一静态类型。空 List 必须能从 output、参数或其他静态上下文推断元素类型，否则编译错误。

Map 是 `Map<Key, Value>`，使用裸大括号创建：

```dever
default_labels() (labels: Map<Text, Text>) {
  labels = {
    "env" = "production"
    "region" = "china"
  }
}
```

同一 Map literal 的 key 类型一致、value 类型一致，重复 key 是编译错误；空 Map 也必须能推断完整类型。`Type { ... }` 是固定字段型值，裸 `{ ... }` 是动态 Map，二者不会合并成万能对象。

Map key 只允许 Text、Int、Bool、Id 和不携带数据的选择型 type。Decimal、Float、可空值、字段型值、List 和 Map 不能成为 key。

Map 保证插入顺序。literal 保留源码顺序；`put` 新 key 时追加，覆盖已有 key 不移动位置；`remove` 后重新写入视为新的末尾项。`entries`、显示和未来的 JSON 边界都必须遵守这一顺序。

### 5. Function Definitions And Calls

所有行为只有一种定义形式：

```dever
line_total(line: Line) (total: Decimal) {
  total = line.price * line.quantity
}
```

- 第一组括号是输入，第二组是命名输出。
- 普通输入和输出必须写成 `name: Type`；body 内局部值可以推断。
- 没有输入或输出时仍写空括号。
- 不使用 `function` 或 `return` 关键字。
- 输入名称是当前分句的局部绑定；输出名称和输入输出类型属于稳定签名。
- 调用只按位置传参，不提供命名参数调用。
- 用户 function 以“package、名称、输入数量”确定签名；同名同参数数量的全部分句必须具有相同的基础输入类型和输出契约。不同参数数量可以形成独立签名。
- 同一签名的全部分句必须在同一文件中连续书写。
- Core 0.1 禁止直接或间接递归；重复执行只能通过 `each`、`reduce` 和 `reduce_until` 这些受检查的具名 handler 组合器进入。

选择项会提供所属选择型 type，普通 type 会提供自身类型；`null` 和 `other` 则从同组其他分句推断基础输入类型。只写 `null`/`other` 而没有任何可推断类型的分句组是编译错误。`Text` 与 `null` 两个分句共同形成 `Text?` 输入，而不是两个运行时重载。

单输出调用直接得到该值，零输出调用作为一条动作语句，多输出调用得到只能按输出名称访问的结果：

```dever
allowed = can_sign_in(user.status)
parts = split_name(name)
display = parts.first + parts.last
```

不提供按位置拆包。命名 function 只能在普通调用位置、作为标准序列/集合操作的 handler，或传给受限静态 handler 输入；Core 0.1 不提供 lambda、闭包，也不能把 function 存入局部值、字段、List、Map 或 Stream。

### 6. Function Clauses Replace Conditional Blocks

function 分句是 Dever 唯一的条件分派机制。这是 2026-09-07 用户确认的长期语言规则；条件 block 和条件表达式不属于后续待增加能力。Bool 表达式及其短路规则仍按第 8 节定义，业务分支通过调用具名 function 的分句选择。

示例：

```dever
can_pay(status: OrderStatus.Draft) (allowed: Bool) {
  allowed = true
}

can_pay(status: other) (allowed: Bool) {
  allowed = false
}
```

冒号左侧始终是当前分句的局部输入名称；不同分句可以使用不同名称。右侧允许：

- 普通类型，表示该类型的任意值；
- `true`、`false`、`null`、Text 或数值常量；
- 完整限定的选择项及其数据绑定，例如 `CreateResult.Created(user)`；
- 选择项中的 `_`，表示忽略该携带值；
- Int 或 Decimal 的静态常量比较；
- 上述数值比较的一个上下界组合；
- 上下文关键词 `other`。

数值范围直接放在输入中：

```dever
absolute(value: Int < 0) (result: Int) {
  result = -value
}

absolute(value: Int >= 0) (result: Int) {
  result = value
}

grade(score: Int >= 60 and < 90) (name: Text) {
  name = "pass"
}

grade(score: Int < 60) (name: Text) {
  name = "fail"
}

grade(score: Int >= 90) (name: Text) {
  name = "excellent"
}
```

Float 不进入静态范围分句，因为 NaN 不具有普通全序；先通过 `math.is_nan` 等 function 得到 Bool，再对 Bool 写分句。

分句没有源码顺序优先级。编译器把同一签名的全部分句作为一组规则，拒绝类型不一致、重叠、遗漏和不可达情况。多输入 function 中，每个 `other` 是该输入位置全部明确模式的补集，编译器再检查各位置组合是否覆盖完整输入空间。`other` 只在分句输入冒号右侧具有该含义，不能作为普通值存储、传递或组合。

分句输入中不能调用任意 function 或读取运行时状态。更深一层的业务选择必须调用另一个有名称的 function，源码不提供 `if`、`else`、`switch`、`match`、`when` 或 `by` block。

### 7. Linear Body And Value Semantics

body 只包含顺序赋值和 function 调用：

```dever
final_price(price: Decimal, rate: Decimal) (result: Decimal) {
  discounted = price * (1 - rate)
  result = decimal.round(discounted, 2)
}
```

表达式由 literal、名称、字段访问、值构造、function 调用、一元运算和二元运算组成。没有条件表达式、循环 block、匿名 function、复合赋值或隐藏控制流。

`=` 既用于初始化，也用于修改当前 function 的 input、局部值或 output。编译器强制：

- 读取前已经完整初始化；
- 字段修改的宿主值已经完整初始化；
- function 结束时全部命名输出都已赋值；
- 赋值类型稳定，不通过再次赋值改变局部类型；
- 无意义覆盖和写入后未读取至少产生诊断。

普通值的赋值、传参和输出采用值语义。修改只属于当前局部名称，调用方或其他名称不会观察到共享别名变化；变化只能通过命名 output 或未来的显式 effect function 离开当前调用。实现可以用唯一所有权或写时复制消除真实复制，但源码不暴露借用、生命周期或 `clone`。File、Socket、Listener 和 Stream 是显式 opaque 资源例外：复制只建立同一资源状态的别名，不复制外部资源。

### 8. Expressions And Operators

普通算术直接使用符号：

```dever
subtotal = price * quantity
ratio = 5 / 2
page = 5 // 2
remainder = 5 % 2
eligible = user.active and user.age >= 18
```

| 运算 | 规则 |
| --- | --- |
| `+ - *` | Int、Decimal 或 Float 的同类运算；Int 与 Decimal 混合时 Int 无损提升为 Decimal |
| `/` | Int/Int 返回 Decimal；Decimal 与 Int 返回 Decimal；Float/Float 返回 Float |
| `//` | 只接受 Int，结果向零截断 |
| `%` | 只接受 Int，余数符号跟随被除数，并满足 `a = (a // b) * b + (a % b)` |
| `+` | 也可连接两个 Text |
| `== !=` | 相同可比较类型 |
| `< <= > >=` | Int、Decimal 或 Float 的同类比较；Int 可提升为 Decimal |
| `and or not` | 只接受 Bool，并具有短路语义 |

可比较类型包括上述标量，以及成员递归可比较的字段型、选择型、可空、List、Map 和 MapEntry。字段型按声明字段逐项比较；选择型比较选择项和携带值；List 按位置比较；Map 按插入顺序比较 key/value 对，因此顺序不同的 Map 不相等；可空值中只有两个 null 相等，两个非 null 值继续比较其内容。Float 保持下文的 NaN 规则，包括嵌套在集合或字段中时。命名多输出结果只能访问输出字段，不能直接比较。相等运算不做 Int/Decimal 提升。

Decimal 与 Float 不隐式混合，实际 Int 值转为 Float 也必须显式调用 `float.from_int`。Core 0.1 不提供 Float 到 Decimal 的隐式或有损转换。

运算优先级从高到低固定为：调用和字段访问、一元 `-`/`not`、`* / // %`、`+ -`、比较、`and`、`or`。同级二元运算从左到右；括号可以显式改变顺序。formatter 不依赖空白表达优先级。

### 9. Collection And Sequence Functions

Core 0.1 只保留以下集合行为，不增加循环语法：

```dever
views = each(to_view, users)
views = each(to_view, users, context)

active = filter(is_active, users)
active = filter(is_active, users, context)

user = find(is_target, users)
user = find(is_target, users, context)

total = sum(prices)
total = sum(line_total, lines)
total = sum(line_total, lines, context)

state = reduce(update_state, values, initial_state)
state = reduce_until(update_until_done, values, initial_state)

parallel_each(consume, values, 4)
parallel_each(consume, values, 4, context)
```

- `each`、`filter`、`find` 和投影式 `sum` 最多有一个末尾 context；需要多项共享数据时先组合成字段型值，其 handler 先接收元素、再接收 context。
- `reduce` 和 `reduce_until` 的第三项是必需的初始状态，不是可选 context；handler 先接收元素、再接收当前状态，且不接受第四项参数。
- `each` 的处理 function 有一个输出时按输入顺序产生 `List<Output>`；没有输出时只按顺序执行动作；多个输出不合法。
- `filter` 的处理 function 必须输出 Bool，并保留原列表顺序。
- `find` 找到第一个 Bool 为 true 的元素后停止，返回 `Item?`；没有结果返回 `null`。
- `sum(List<Number>)` 直接求和；投影形式要求处理 function 输出 Int、Decimal 或 Float。
- 空 List 的 `sum` 返回相应数值类型的零。
- 除显式 `parallel_each` 外，List 操作按列表顺序执行，Bytes 按字节位置执行，Stream 按拉取顺序执行；Float 求和不得为了优化而重排。
- `reduce` 和 `reduce_until` 的完整 List/Bytes/Stream、状态和停止契约由 Approved Foundation Extension 定义；它们不开放循环 block 或用户递归。
- `parallel_each` 接受 List、Bytes 或 Stream、1–256 个工作线程和可选 context；handler 必须零输出，输入依次是元素和 context。复用既有静态 handler 与专门化规则，不产生用户可存储的 function 值。
- 生产端顺序拉取且队列有界，工作端的副作用顺序不保证。handler 输入与 context 不能递归包含 Stream，源 Stream 只在生产端拉取；文件与 Socket 等别名仍遵守共享资源规则。
- `parallel_each` 返回前等待全部已启动动作；程序故障在后续拉取边界停止分派，保留第一个已记录故障并传播源码位置。已经阻塞的拉取或系统调用由显式系统超时约束，不提供强制取消或后台遗留任务。非法线程数在拉取前产生程序故障。

其他最小标准函数：

```dever
users = append(users, user)
user = first(users)

value = get(labels, "env")
labels = put(labels, "env", "development")
labels = remove(labels, "env")
pairs = entries(labels)
```

`append` 返回新 List；`first(List<Item>)` 返回 `Item?`。`get` 返回 `Value?`；`put` 和 `remove` 返回新 Map；`entries` 返回按插入顺序排列的 `List<MapEntry<Key, Value>>`。没有 `list.add`、数字点号索引、`map.key` 或 `map[key]`。

`any`、`all`、`min`、`max` 不进入首版；真实重复需求出现后再增加普通标准 function。

### 10. Recoverable Failures And Runtime Faults

可恢复业务失败使用选择型输出：

```dever
create_message(result: CreateResult.Created(user)) (message: Text) {
  message = user.name
}

create_message(result: CreateResult.InvalidPhone(reason)) (message: Text) {
  message = reason
}

create_message(result: CreateResult.EmailAlreadyUsed(email)) (message: Text) {
  message = email + " already exists"
}
```

这同时替代 `throw` 和 `catch`。调用方接收完整选择值，再通过命名分句显式转换。Core 0.1 不提供 exception、自动错误传播、隐藏提前退出或类似 Rust `?` 的语法。

编译器可以证明的不变量错误直接拒绝编译。只能在运行时发现的程序故障终止当前执行边界，并至少报告故障类别、文件、行和列；普通 function 不能捕获、忽略或转换程序故障。执行边界由宿主确定，例如一次 CLI 调用，未来可以是一次请求或任务。

数据库、网络等只要存在恢复可能，就必须在未来以显式选择型结果返回，不能伪装成程序故障。

### 11. Numeric Semantics

- Int 是固定 i64；不环绕、不饱和。静态可证明的溢出是编译错误，运行时溢出是程序故障。
- `Int / Int` 先无损提升为 Decimal 再除，因此 `5 / 2` 是 Decimal `2.5`。
- `Int // Int` 向零截断；`Int % Int` 与商保持代数恒等式。除数为零以及 `-9223372036854775808 // -1` 属于溢出故障。
- Decimal 使用 decimal128 的 34 位有效数字和银行家舍入；算术中的正常 inexact/rounded 状态产生舍入结果，指数上溢、下溢、除零或无效运算产生程序故障，不向源码暴露 Decimal NaN/Infinity。
- 金额位数由 `decimal.round(value, places)` 显式处理；核心算术不根据变量名、字段名或业务场景猜测两位小数。
- Float 遵守 IEEE 754 binary64 非捕获行为：溢出产生正负 Infinity，无效运算产生 NaN，除零产生 Infinity 或 NaN。
- Float 的 NaN 与任何值的 `==` 都为 false，`!=` 为 true，有序比较均为 false；使用 `math.is_finite`、`math.is_nan` 和 `math.is_infinite` 分类。
- 常用科学函数通过 `math.sqrt`、`math.sin`、`math.cos`、`math.log` 和 `math.pow` 等普通标准 package function 提供。
- Decimal 与 Float 不隐式混合。任意精度整数/小数、向量、矩阵、复数、统计与单位系统以后由独立 package 提供。

## Standard Names

无需 import 即可使用的核心名称只有：

- Bool、Int、Decimal、Float、Text、Id、Bytes、List、Map、MapEntry、Stream；
- true、false、null、other；
- each、parallel_each、reduce、reduce_until、filter、find、sum、append、first、get、put、remove、entries、length、用于 Stream 的 close；
- 仅用于 function 输入签名的上下文关键词 handler；
- 算术、比较和布尔运算符。

其他标准行为使用完整 package 名，例如 `text.trim`、`text.lower`、`decimal.round`、`float.from_int` 和 `math.sqrt`。这避免大量隐式全局名称，也让标准库与业务 package 遵守同一调用模型。

## CLI Identity And Coexistence

- 语言、编译器产品和正式命令名保持为 `dever`，文档与命令语义以 `dever` 为准。
- Core 0.1 与现有 Dever 框架 CLI 共存期间，系统 `PATH` 中可以暂时用 `deverc` 暴露语言编译器；不得覆盖、替换或遮蔽现有的 `dever` 命令。
- `deverc` 只是指向同一编译器二进制的安装入口，不形成第二套命令、配置或行为契约。
- 仓库构建与自动测试直接调用构建产物，不修改系统 `PATH` 或全局可执行文件；创建、更新或移除全局 `deverc` 入口必须单独获得用户授权。
- 未来是否让语言编译器接管全局 `dever` 命令属于独立迁移决策，不在 Core 0.1 默认实施范围内。

## Complete Package Example

```dever
public type UserStatus {
  Pending
  Active
  Disabled(reason: Text)
}

public type CreateInput {
  id: Id
  name: Text
  email: Text
  phone: Text?
  score: Int
  labels: Map<Text, Text>
}

public type CreateResult {
  Created(user: User)
  InvalidName(reason: Text)
  InvalidEmail(reason: Text)
}

public type User {
  id: Id
  name: Text
  email: Text
  phone: Text?
  score: Int
  labels: Map<Text, Text>
  status: UserStatus
}

public type UserView {
  id: Id
  name: Text
  email: Text
  phone: Text
  score: Int
  can_sign_in: Bool
}

public create(input: CreateInput) (result: CreateResult) {
  normalized = normalize(input)
  result = create_name(normalized, normalized.name)
}

normalize(input: CreateInput) (result: CreateInput) {
  result = input
  result.name = text.trim(input.name)
  result.email = text.lower(text.trim(input.email))
}

create_name(input: CreateInput, name: "") (result: CreateResult) {
  result = CreateResult.InvalidName("name is required")
}

create_name(input: CreateInput, name: other) (result: CreateResult) {
  result = create_email(input, name, input.email)
}

create_email(input: CreateInput, name: Text, email: "") (result: CreateResult) {
  result = CreateResult.InvalidEmail("email is required")
}

create_email(input: CreateInput, name: Text, email: other) (result: CreateResult) {
  user = User {
    id = input.id
    name = name
    email = email
    phone = input.phone
    score = input.score
    labels = input.labels
    status = UserStatus.Pending
  }
  result = CreateResult.Created(user)
}

public activate(user: User) (result: User) {
  result = user
  result.status = UserStatus.Active
}

public disable(user: User, reason: Text) (result: User) {
  result = user
  result.status = UserStatus.Disabled(reason)
}

public can_sign_in(status: UserStatus.Active) (allowed: Bool) {
  allowed = true
}

public can_sign_in(status: other) (allowed: Bool) {
  allowed = false
}

phone_text(phone: Text) (result: Text) {
  result = phone
}

phone_text(phone: null) (result: Text) {
  result = "Not provided"
}

public to_view(user: User) (view: UserView) {
  view = UserView {
    id = user.id
    name = user.name
    email = user.email
    phone = phone_text(user.phone)
    score = user.score
    can_sign_in = can_sign_in(user.status)
  }
}

public to_views(users: List<User>) (views: List<UserView>) {
  views = each(to_view, users)
}

is_active(user: User) (result: Bool) {
  result = can_sign_in(user.status)
}

public active_views(users: List<User>) (views: List<UserView>) {
  active = filter(is_active, users)
  views = each(to_view, active)
}

has_id(user: User, expected: Id) (matches: Bool) {
  matches = user.id == expected
}

public find_user(users: List<User>, id: Id) (user: User?) {
  user = find(has_id, users, id)
}

public put_label(user: User, key: Text, value: Text) (result: User) {
  result = user
  result.labels = put(user.labels, key, value)
}

public label(user: User, key: Text) (value: Text?) {
  value = get(user.labels, key)
}

score_bonus(user: User, rate: Decimal) (amount: Decimal) {
  amount = user.score * rate
}

public total_score_bonus(users: List<User>, rate: Decimal) (total: Decimal) {
  total = sum(score_bonus, users, rate)
}
```

该源码只要求开发者理解 package、type、function、值和普通表达式。它同时覆盖显式业务结果、局部修改、条件分句、可空处理、列表转换、筛选、查找、Map 更新和 Decimal 聚合，所有路径保持平铺且可由编译器完整检查。

## Original Core Deferrals

以下保留原 Core 0.1 的暂缓能力，具体版本安排在后续需求中确定。HTTP、JSON、持久化以及异步/并发 I/O 的应用扩展仍属暂缓 API 提案。

- AI、自然语言转译、画布和业务 DSL；
- Web 页面、React，以及超出首个 API 需要的数据库、网络和部署能力；
- 超出首个 API 需要的 effect/capability、异步、并发和事务能力；不开放任意共享可变业务全局变量；
- package registry、版本解析和项目清单；
- `for/while/break/continue/iterate/recur/repeat`、用户递归和其它通用循环结构；
- class、继承、interface、annotation、macro、unsafe、FFI；
- import、别名、命名参数、lambda、闭包和一般化一等 function；
- 用户泛型 type、递归 type、Set 和动态 Json；首个 API 的 JSON 边界应由静态类型驱动；
- UInt、BigInt、任意精度 Decimal、Percent、日期时间和单位类型；
- 直接 LLVM/Cranelift backend、完成自举、所有权或生命周期表面语法。

这些内容保留为 Core 0.1 暂缓项，不改变 Dever 最终成为通用编程语言的方向。条件 block 已从暂缓清单移除，条件分派始终遵守第 6 节的长期规则。

## Core Roadmap Acceptance Criteria

这些是完整语言路线的验收项。2026-09-07 的基础语法、最小系统接口和后续工具/库分别验收，实际状态与证据见 `implement.md`；发行和自举不因基础里程碑完成而视为已交付。

1. 一个源码根目录中的多个 `.dever` 文件可以被加载；Core 0.1 只读取该根目录和内置标准 package，不隐式联网或搜索外部目录；路径与 package 不一致、保留命名空间、重复 package、未知公开名、未知引用和循环依赖均得到带源码位置的确定诊断。
2. parser 覆盖本 PRD 的 package、两种 type、function、分句、value、statement 和 expression；拒绝已排除的关键字或语法。
3. formatter 对合法程序产生唯一布局，连续执行两次结果不变，并保持注释和程序含义。
4. 编译器强制命名格式、显式 function 边界、位置调用、一文件一 package、连续分句和无递归调用图；同 package 使用短名称，跨 package 必须使用完整限定名，并且只能访问对方 `exposes` 中的名称。
5. type checker 拒绝未知/重复/遗漏字段、半初始化字段写入、未初始化读取、遗漏 output、非法 null、类型漂移、非法 Map key 和未授权隐式转换。
6. clause analyzer 对选择型、可空、Bool、Text 常量、Int/Decimal 范围和多输入组合检查互斥、穷尽与可达性；分句移动顺序不改变执行结果。
7. 本 PRD 的完整 package 示例可以通过解析、格式化、名称解析和类型检查；独立的零输入示例在语义基准解释器和原生程序中产生相同的确定命名输出。
8. 局部赋值和字段修改不会改变调用方或其他名称持有的原值；编译器实现可以优化复制但不能改变可观察语义。
9. `each`、`reduce`、`reduce_until`、`filter`、`find` 和 `sum` 的顺序、handler、context、初始/空结果、停止和类型规则全部有定向测试。
10. List、Map、Bytes 和 Stream，以及 `append`、`first`、`get`、`put`、`remove`、`entries` 遵守可空返回、值/资源语义和稳定顺序。
11. `5 / 2` 得到 Decimal `2.5`，`-5 // 2` 得到 Int `-2`，`-5 % 2` 得到 Int `-1`，并满足商余恒等式。
12. Decimal 能证明 `0.1 + 0.2 == 0.3`，`1 / 3` 使用 34 位有效数字和 round half to even；`decimal.round` 显式控制业务位数。
13. 静态 Int/Decimal 溢出或零除数被拒绝编译；动态故障终止当前执行边界并定位 `.dever` 源码，不环绕、饱和或返回默认值。
14. Float 使用 binary64 和科学计数法，能够产生并通过 `math.is_nan`、`math.is_finite`、`math.is_infinite` 判断 NaN/Infinity；Decimal 与 Float 混用被拒绝。
15. 可恢复失败只能通过选择型输出和穷尽分句传播；源码中不存在 exception、`throw`、`catch` 或自动传播语法。
16. 正式 CLI 名为 `dever`，至少提供 `dever check <source-root>`、`dever fmt <source-root>`、`dever build <source-root> <package.function> --output <path>` 和 `dever run <source-root> <package.function>`；与现有框架 CLI 共存时，临时全局入口 `deverc` 必须调用同一编译器且不得覆盖现有 `dever`；build/run 首版只接受公开的零输入 function，run 执行与 build 相同的原生路径并确定性打印命名输出。
17. 新实现不依赖旧 canonical JSON、SemanticGraph、Refiner、Workbench、温度示例或 Rust/React 生成后端，仓库文档不再把旧路线描述为当前架构。
18. 生产产物不链接通用解释器，不携带运行时 type tag 分派或垃圾回收器；record、choice、List、Bytes 和静态 handler 在 AOT 路径中具有具体原生表示，热循环不发生逐元素动态 handler 分派或无必要堆分配；opaque Stream 只在 I/O 资源拉取边界保留内部生产者抽象。
19. 在同一算法、数据布局、工具链和机器上的 release 基准中，Int/Float 算术及 List `each`/`filter`/`sum` 代表性热路径的中位执行时间不得超过等价 safe Rust 基线的 1.15 倍；Decimal 与有序 Map 分别对使用相同底层语义的 Rust 基线比较，编译时间单独报告。
20. Rust 生成文件只存在于编译器管理的临时构建目录，不是用户源码、公共输出格式或兼容接口；删除这些文件不影响 `.dever` 工程。

## Risks And Deferred Items

- 条件分句的穷尽分析是首版最复杂的静态检查，应先支持有限选择域与一维 Int/Decimal 区间，再组合多个输入；不引入通用定理证明器。
- decimal128 需要成熟实现与一致状态标志。Core 0.1 采用 Rust `dec` crate 的 Decimal128 和可配置 Context，并将其封装在内部数值适配层；语言级测试锁定行为，依赖 API 不得泄漏到 `.dever`。
- 当前仍禁止用户递归，已按批准范围提供显式文件/TCP 系统操作和结构化异步并发；异步网络、一般化 effect 和事务模型尚未实现，因此不是最终完整运行时。
- Float 运算遵守平台 IEEE 754 语义，但任何未来的快速数学优化都必须显式设计，首版不得重排计算。

# Current Source And Module API Contract (2026-09-18)

This user-approved contract supersedes historical `package/exposes`, arbitrary entry arguments and "API deferred" wording above. Source filenames relative to `module/` define package names in both `.dever` and `.dever.md`; top-level type/function declarations default private and may opt into cross-package access with `public`. `deverc run/build <project>` always invokes private `main()` from `module/main.dever` (or `.dever.md`) and accepts no entry argument. Markdown H1 documents path-derived package and public declarations with no executable H1 block.

Each module may own recursive `api/` source containing only private `get_`/`post_`/`delete_` ordinary functions with one `response` output. Routes are statically validated and compiled only when reachable `main()` calls `dever.api.serve()`. GET/DELETE query inputs and POST JSON-object inputs are typed Text/Int/Bool (nullable allowed), and HTTP responses have JSON `code/message/data` envelopes. Listener settings are read only from `config/setting.json`. No application-config environment variable, implicit listener, new auth policy or CMS HTTP business endpoint is part of this milestone. Current canonical contracts are in `LANGUAGE.md`, `MARKDOWN-SYNTAX.md` and `.trellis/spec/backend/`.

# Current AI-Oriented Source Architecture Contract (2026-09-19)

This section supersedes the 2026-09-18 application visibility and package rules above. An application starts only at `module/main.dever` or `module/main.dever.md`; `main()` is implicit and private, and HTTP starts only when it calls `dever.api.serve()`. Application source contains no `package`, `exposes`, `public`, `internal` or import declarations.

Business source uses `module/<component>/<domain>/<role>.dever` or `module/<component>/<domain>/<role>/<topic>.dever`, where role is exactly `app`, `domain`, `model`, `port`, `adapter` or `api`. A role file and its same-name directory are mutually exclusive. Non-API role directories are flat; only `api/` may add nested URL scopes. A role directory containing only one source file is rejected because it creates structure without separation; keep that implementation in the role file instead.

Files group cohesive behavior rather than individual functions. One role file may contain multiple related functions, and one topic file may contain multiple related functions. The topic filename never becomes part of an App call. All App functions are cross-domain capabilities and are addressed as `<component>.<domain>.<function>`; a same-component caller may use `<domain>.<function>`, and source in the same domain may use `app.<function>`. Every other role is domain-private. Cross-domain and cross-component calls may target only App. Model types and ID types may appear in relations and signatures across domains, but generated Model operations remain callable only inside their owning domain. API handlers are HTTP entry adapters and are never source-callable.

App inputs, outputs and inferred failure sets are one public contract. They cannot expose a private Domain/Adapter type; a business error that may leave the domain is declared in App and referenced from same-domain private source as `app.<ErrorType>`.

The dependency direction is `api -> app`, `app -> domain/model/port/adapter` plus another domain's App, while domain/model/port/adapter remain implementation details. Generic dumping grounds such as `common`, `shared`, `utils`, `helper`, `base` and physical `service/` are invalid application architecture. Keep one-use logic inline, same-topic reuse beside its callers, cross-topic domain rules in `domain`, persistence in `model`, and external integrations in `adapter`; introduce `port` only for a real replaceable boundary.

API route prefixes and JSON `code/message/data` envelopes remain as approved on 2026-09-18. Runtime settings still come only from `config/setting.json`; environment-variable configuration remains prohibited. Authentication remains business composition through App/domain capabilities rather than a compiler-owned HTTP policy.

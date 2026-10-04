# 设计

## 1. 边界与复用

继续使用 `test/performance` 的“构建与测量分离”结构：`build.py` 只生成并指纹化产物，`run.py` 只启动清单内的二进制并采样。进程、cgroup、直方图、逐 case 落盘和失败保留逻辑全部复用。新增的唯一核心抽象是 HTTP 连接拓扑：

```text
ConnectionShape
  physical_connections
  streams_per_connection
  request_slots = physical_connections * streams_per_connection
```

HTTP/1.1 要求 `streams_per_connection = 1`；HTTP/2 要求复用连接。这个模型避免继续把并发请求与 TCP 连接混为一个 `connections` 参数，也让报告能解释吞吐和内存差异来自哪里。

## 2. Rust 基准对端与负载端

`network_bench` 的统一负载入口调整为：

```text
network_bench load h1|h2 URL PATH RATE SECONDS PHYSICAL_CONNECTIONS STREAMS_PER_CONNECTION TIMEOUT_MS REUSE CA
```

所有仓库调用者一次性迁移，不保留第二套旧参数解析。

- HTTP/1.1 保留现有“一名 worker 独占一条连接”的实现，并要求 streams 为 1。
- HTTP/2 在计时前主动建立指定数量的物理连接；每条连接由 Hyper HTTP/2 client driver 持有，`SendRequest` 按该连接的请求槽克隆给 worker。总 worker 数为 shape 的乘积，worker 固定映射到连接，不额外建立连接。
- HTTP/2 连接中断后不重试请求或静默补建连接。失败必须进入错误计数，避免恢复逻辑污染吞吐数据。
- 两种协议共享现有速率调度、队列、响应校验、延迟直方图和结果守恒逻辑，不复制 metrics/validation。
- 结果新增 `http_version`、`physical_connections`、`streams_per_connection`、`request_slots` 和 `successful_handshakes`。H2 的握手数必须等于物理连接数。

H2 请求使用完整 scheme/authority/path 和 `Version::HTTP_2`。明文使用 prior knowledge；TLS client/server 配置按协议派生 ALPN，H2 只允许 `h2`，不改变运行时用于 WSS 的共享 TLS 配置。

服务模式新增 `runtime-http2`、`runtime-https2`、`hyper-http2`、`hyper-https2`。`ServiceKind` 集中派生 runtime/TLS/H2 三个维度，避免模式分支散落。直接 runtime 使用 `Http2Limits`；原生 Hyper 使用 `http2::Builder<TokioExecutor>`，显式配置与 N5 相同的 streams、窗口、header/frame/send-buffer 上限。两者继续复用同一响应路由和连接生命周期。

## 3. Dever 夹具与构建清单

HTTP 夹具暴露 `http`、`https`、`http2`、`https2` 四个入口。`serve` 接收 `dever.http.Limits` 参数，四个 worker 只负责选择 TLS 和协议配置，避免复制路由与关闭流程。

`build.py` 增加 HTTP/2 构建参数并写入 manifest：

- `http2_streams = 16`
- `http2_stream_window_bytes = 65535`
- `http2_connection_window_bytes = 262144`

`config.dever` 分别生成 HTTP/1.1 和 HTTP/2 limits；`task_capacity` 继续按有界并发计算，并校验正式拓扑的总请求槽不超过语言上限。旧产物目录和报告不修改，新构建必须使用新的输出目录。

## 4. Python 运行器

新增两个显式 suite：

- `http2`：吞吐、延迟与活跃资源矩阵。
- `http2-recovery`：同一服务进程上的多轮连接建立、请求和断开。

`--http2-connections` 接受物理连接档位，默认 `1,4`；每连接流数从 manifest 读取。`--implementations` 可将 smoke 或 64 MiB 场景限制为 `dever`，正式矩阵默认为 `dever,runtime,hyper`。已有 `http` suite 的 `--concurrency` 和连接新建/复用语义不变。

吞吐 case 的流程为：启动服务并取得 READY，运行独立 warmup，等待连接释放并采样基线，运行正式负载，同时采样服务端与客户端；负载退出后再次采样以记录 FD/RSS 回落。case 名和 JSON 主键包含实现、传输、路径、速率、物理连接、每连接流和 repeat，避免结果覆盖或混淆。

恢复 case 使用独立参数 `--recovery-connections`、`--recovery-rate`、`--recovery-path`，复用 `--cycles` 与 `--duration`。每轮启动一次负载进程，在同一服务上建立固定 shape，完成请求后关闭连接；运行器等待稳定窗口并保存该轮末样本。FD 不回到 READY 基线、请求错误、握手数不符或服务提前退出均使 case 失败。RSS 只报告趋势，不以“没有立即回到启动值”单独判断泄漏。

报告继续保留 `cases.jsonl` 和 `report.json`，并在完整或部分结果上生成结构化 `summary`。摘要按实现、协议、TLS、路径、速率和 topology 分组，对成功 QPS、p99、RSS/PSS/FD 峰值取各 repeat 中位数；不把不同 offered rate 或饱和/未饱和 case 混合。

## 5. 测量矩阵

正式测量固定服务端 1 worker、1 CPU、128 MiB cgroup、swap=0；客户端绑定另一 CPU且位于服务预算之外。每组 0.5 秒预热、2 秒正式负载、3 个独立服务进程 repeat：

| 正文 | offered rate | topology | 实现 | 传输 |
| --- | --- | --- | --- | --- |
| `/plain` 5 B | 5k / 15k / 30k req/s | 1x16、4x16 | Dever/runtime/Hyper | h2c、TLS+h2 |
| `/bytes` 64 KiB | 100 / 500 / 1k req/s | 1x16、4x16 | Dever/runtime/Hyper | h2c、TLS+h2 |

另运行 10 轮、1x16、`/plain` 的恢复矩阵，以及 64 MiB 下 Dever 的 1x16 明文/TLS 活跃代表场景。若当前机器不能创建指定 cgroup，工具必须明确失败；不得回退为无预算结果后仍标记完成。

## 6. 解释与决策

- 主要比较同一 H2 topology 下 Dever、runtime 和 Hyper 的差异，定位语言桥接、运行时结构化任务和底层协议栈各自成本。
- 1x16 与 4x16 比较多路复用和多物理连接扩展；它们不是“连接数相同”的横向比较。
- `/bytes` 的速率按响应字节量降低，避免只测客户端或 loopback 带宽耗尽；65536 字节正文会越过 65535 字节默认单流窗口，可观察 WINDOW_UPDATE 路径。
- `dropped_capacity` 表示负载端无法在计划时间取得请求槽，不算服务错误；分析时必须同时看 actual QPS、请求延迟和服务 CPU，不能只看 offered rate。
- 不设预定胜负线。若 Dever 相对 direct runtime 出现稳定的大幅差距，或内存随 recovery 周期持续增长，则把证据和复现命令交给后续独立优化任务。

## 7. 回退形态

本阶段不迁移生产 API。若 H2 基准实现无法通过独立 smoke，删除新增 suite、服务模式和夹具入口即可回到原有 H1 基准；历史产物目录不受影响。任何正式测量都写入新的 `target/performance/*` 目录，失败结果保留且不覆盖旧报告。

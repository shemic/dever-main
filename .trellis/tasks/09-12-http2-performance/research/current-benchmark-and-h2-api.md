# 现有基准与 HTTP/2 API 调研

## 现有结构

- `test/performance/build.py` 负责生成 Dever fixtures、复制 Rust peers、记录配置和 SHA-256；`run.py` 只执行已指纹化产物。这一分离可直接复用。
- 当前 HTTP suite 覆盖 Dever、direct runtime、raw Hyper，但三者都只使用 HTTP/1.1。`config.dever` 明确写入 `http2 = null`。
- 当前 loader 的 `connections` 同时决定 worker 数和连接数，每个 worker 串行持有一条 HTTP/1.1 连接；它无法证明 HTTP/2 多路复用。
- 现有 runner 已有固定速率调度、三类延迟直方图、进程 RSS/PSS/FD/CPU 采样、cgroup memory/peak/OOM 采样、逐 case 落盘和异常清理，不应重写。
- `live` suite 已有多轮连接后 FD/RSS 差值的报告方式，可复用其 phase 后采样语义，但 HTTP/2 负载和连接创建应继续由 `network_bench` 负责。

## N5 已有合同

- `Http2Limits` 的公开维度为 streams、stream window 和 connection window；默认分别为 16、65535、262144。
- cleartext HTTP/2 使用 prior knowledge；TLS 配置必须协商 `h2`，不能降级或影响 WSS 的 HTTP/1.1 TLS 配置。
- 客户端按流租约复用物理连接，服务端连接和流均受限；固定 16 KiB frame/send-buffer，关闭自适应窗口。
- `/bytes` fixture 为 65536 字节，恰好跨过默认 65535 字节单流窗口，适合覆盖 flow-control 热路径。

## 锁定依赖可用能力

- Cargo.lock 固定 Hyper 1.11.1、h2 0.4.19、rustls 0.23.44 和 tokio-rustls 0.26.5，不需要新依赖。
- Hyper HTTP/2 `SendRequest` 实现 `Clone`，可让多个请求 worker 共享同一物理连接的 driver。
- Hyper HTTP/2 client/server builder 可显式设置 initial stream/connection window、adaptive window、max concurrent streams、max frame、max send buffer 和 header list size。
- Hyper HTTP/2 request future 的取消会对单流发送 RST_STREAM；基准不需要自己实现帧协议。

## 设计结论

1. 物理连接与请求槽必须拆开；只给现有 loader 换 HTTP/2 handshake 会得到错误的“一 worker 一连接”模型。
2. H2 连接应在计时前主动建立，worker 固定映射到已建 sender；这样报告的物理连接数由 loader 自己控制和证明。
3. H1 和 H2 共享调度、计数和响应校验，但连接执行策略分开。不存在足够稳定的公共行为值得引入 trait 或继承式抽象。
4. raw Hyper 对照必须显式复制 N5 的协议参数；使用 Hyper 默认值会把配置差异误判为 Dever/runtime 开销。
5. 性能任务不同时修改生产 runtime。测量发现的瓶颈需要在固定基线后另立任务验证，否则优化前后结果不可复现。

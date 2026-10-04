# 设计

## API 与复用

在官方 `dever.http.Limits` 增加 `http2: Http2Limits?`；`Http2Limits` 三个 Int 字段为 `streams`、`stream_window_bytes`、`connection_window_bytes`。默认关闭 HTTP/2，`default_http2_limits()` 返回 16 流、65535 字节单流窗口、262144 字节连接窗口。沿用现有 serve/serve_tls/serve_live/client/open/open_stream/send，不增加平行 API。

`Limits.check` 统一校验；HTTP/2 streams 和 connections 的乘积不得超过 65536，窗口遵循 HTTP/2 31 位限制（连接窗口至少 65535）。关闭自适应窗口，保持帧和发送缓存有界。源语言 exact-record 检查与原生结构转换同步修改。

## 运行时边界

- 协议配置、Hyper builder 和 HTTP 专用 TLS ALPN 配置集中在 HTTP 模块，原有 TLS trust/session cache 和 Transport 复用。不能修改共享 TLS 配置使 WSS 错误协商 h2。
- 编解码复用 message；按协议校验头字段，HTTP/2 使用路径查询而不是绝对 URI 作为 source Request.target，authority 与 Host 一致。
- HTTP/1 仍独占连接 lease；HTTP/2 持有共享连接与独立流 lease。EOF 或取消释放流，只有连接 owner 结束才释放物理连接许可。空闲回收不能中止活跃流。
- Hyper Executor 必须接入已有结构化任务 Scope，不能直接用脱离作用域的 TokioExecutor。每流清理独立，程序故障明确传给服务任务，peer reset 只结束该流。
- GOAWAY 排空与业务取消分开；应用已有 timeout(server, grace_ms, fallback) 控制最长排空时间。

## 验证

仅运行本次相关 Rust 定向测试、编译器官方库检查和隔离 loopback 原生程序。独立 HTTP/2 对端验证真正单连接多流、SETTINGS/流控、RST_STREAM、GOAWAY、ALPN 和请求语义。保留已有性能报告；缓存可重建部分暂存到专用 /dev/shm 目录以腾出构建空间。

协议依据：[RFC 9113](https://www.rfc-editor.org/rfc/rfc9113.html) 与当前锁定 Hyper 1.11.1 源码。

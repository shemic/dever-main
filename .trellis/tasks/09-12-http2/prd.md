# N5 HTTP/2

用户已批准上一阶段提出的 HTTP/2 接入方案，并要求继续实现。复用 Hyper，面向小机器控制运行内存；不实现旧源码兼容层，不改运行时根任务调度策略。

## 范围与验收

- buffered/live 服务端和 single-use/pooled 客户端支持 HTTP/2；HTTPS 必须通过 ALPN 协商 h2，明文为 prior knowledge。
- `Limits.http2: Http2Limits?` 显式选择协议；null 为 HTTP/1，非空为 HTTP/2。配置包含 streams、stream_window_bytes、connection_window_bytes，连接限额仍由 connections 持有。仓库调用者直接迁移。
- 连接内复用请求，流式上传/下载与 SSE 复用现有正文接口；并发流、窗口、发送缓存有界，单流中止不取消相邻流。
- Hyper 内部任务保留 Dever 的作用域归属；子任务、已启动 blocking、取消及故障完整排空，容量持有到清理完成。
- 监听器关闭发送 GOAWAY 并排空在途流；客户端不向已 GOAWAY 连接派发新请求，不自动重试已发送请求。
- 使用独立线协议对端和原生 Dever 程序验证多路复用、TLS ALPN、流控、取消隔离和生命周期，选取 HTTP/1 与 WS 回归。

## 排除

本阶段不接 RFC 8441 WebSocket-over-HTTP/2、h2c Upgrade、服务器推送、非空 trailers、自动协议降级，不跑全量测试或重新做性能矩阵，不替换全局二进制，不提交 Git。

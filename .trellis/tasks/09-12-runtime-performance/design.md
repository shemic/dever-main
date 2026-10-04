# 测量边界

现有 performance.rs 拥有计算热路径基准，保持原有语义与 1.15 门槛。新增根 test/performance/ 的 Python 标准库驱动，分离构建、进程采样、场景执行和报告。Dever fixtures 经过现有 CLI build 成为独立二进制。Rust 辅助程序作为 dever-tests 的显式 example，复用 workspace 已锁定的 Tokio/Hyper/Rustls。

网络辅助程序同时提供等价 Rust runtime / 原生 Hyper 对照服务和独立负载端；不自行解析 HTTP。固定速率调度以计划发送时间计算延迟，并记录有界并发容量不足或调度逾期导致的丢弃。TLS 校验证书和主机名，复用已有测试 CA；新连接与连接复用分别测量。

进程通过 READY|port（空闲/计算用 READY|0）报告可采样状态。配置统一使用 BENCH_WORKERS、BENCH_CONNECTIONS、BENCH_TIMEOUT_MS、BENCH_LIFETIME_MS、BENCH_CERT、BENCH_KEY 等环境变量。负载 CLI 为 network_bench load URL PATH RATE SECONDS CONNECTIONS TIMEOUT_MS REUSE CA，JSON 标准输出。服务 CLI 为 network_bench runtime-http/runtime-https/hyper-http/hyper-https。各服务 /plain 返回 hello，/json 返回 {"ok":true}，/bytes 返回 65536 个 x；带对应 Content-Type，不启用日志。

Linux /proc 采样独立于子程序；cgroup 为可选且显式启用的进程预算，使用新建的专属子组，不调整宿主或已有服务的 cgroup。负载发生器不进入服务预算。默认不声称隔离或峰值吞吐；报告采样间隔、负载端 CPU 和限额状态。汇总保留每轮原始数据，不建立未经校准的绝对性能门槛。

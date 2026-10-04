# 客户端瓶颈证据

## 现有实现

- `test/dever-tests/examples/network_bench/load.rs` 从 `BENCH_WORKERS` 创建 Tokio multi-thread runtime，范围为 1–64。HTTP/2 的物理连接、请求 worker、速率调度、响应校验和三组 Histogram 均在该进程内完成。
- `test/performance/run.py` 的 `load()` 无条件写入 `BENCH_WORKERS=1`，并通过 `args.client_cpu` 把负载进程绑定到一个 CPU；`live.py` 使用相同单 worker/单 CPU 方式。
- `test/performance/process.py` 的 `Process` 已集中拥有 fork 后亲和性设置，只需把单元素集合推广为调用方提供的集合，不需要新增进程封装。

## 现有结果

`target/performance/http2-plain-128m/report.json` 中，Dever 4x16、30000 req/s 的三轮结果为：

- h2c 成功 QPS 29506–29804，服务端约 0.87–0.92 CPU，客户端约 0.89–0.91 CPU；
- TLS+h2 成功 QPS 28673–29865，服务端约 0.91–0.93 CPU，客户端约 0.92 CPU；
- 请求错误均为 0，但存在容量丢弃，因此该档位不能确定是哪一端首先饱和。

## 方案比较

| 方案 | 优点 | 问题 | 结论 |
| --- | --- | --- | --- |
| 多进程客户端 | 调度器天然分片，可跨多个 CPU | 每进程独立直方图，现有分位数无法正确合并；还需拆分 rate、连接和生命周期 | 本阶段不采用 |
| 单进程多线程 | 已有 Tokio 支持；保留单一计数、校验和直方图；改动只在 Python 配置 | 单一调度协程仍可能成为更高档位瓶颈 | 先采用并用结果验证 |
| 引入 h2load 等外部工具 | 成熟、高吞吐 | 新依赖、响应合同和报告格式不同，破坏现有可比性 | 不采用 |

## 结论

当前最小实验不是重写负载器，而是解除 Python 对已有多线程能力的限制。只有在多 CPU 客户端仍显示调度端瓶颈时，才有证据支持后续分片调度或多进程直方图协议。

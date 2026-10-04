# Execution

- [x] 主代理：async_bench、Python 集成、HTTP 扫描、采样、结果分析与 HTTP/2 就绪评估。
- [x] trellis-implement：long connection Rust 对端及 Dever fixture，独立文件所有权，不修改生产代码。
- [x] 定向 offline release 构建、生成原生 fixture、校验工具错误路径、执行有界矩阵。
- [x] trellis-check：复查统计、语义、连接清理和最终结果；主代理复核关键证据。
- [x] 更新现有性能 README、任务与日志，不提交。

磁盘空间有限；复用 target/native-runtime 的 release 依赖。原生构建 TMPDIR 使用任务独有 /dev/shm 临时目录，结束移除自有目录。不并行执行 Cargo 构建。

## 已取得证据

- HTTP/HTTPS 高负载完整 54 组，report: target/performance/http-load-p2-final/report.json。1800000 计划、1500830 实际成功、0 错误、299170 未发出；三实现均有压测槽位丢弃，不能推断服务端吞吐上限。无 OOM。
- 首次 http-load-p2 在子进程退出采样竞态中断：/proc memory teardown 早于 waitpid 状态。已修复采样拥有层，补定向回归，保留中断报告。
- run/wait Rust 对照用生产 task runtime、Bytes 和 checked Int；build 工具使用与 native 相同优化参数和 runtime rlib。async-p2 已完成 18 组：Dever root/worker 15.50/2.003 µs，Rust 15.59/1.984 µs，三轮校验均通过且无 OOM。独立 perf 确认 root/worker 线程切换为 465149/56 次；生产运行时未修改。
- 独立 review 修正了 live 总超时遗漏关闭阶段预算、SSE 保持期无心跳仍通过的问题；静态发现并修正 SSE driver 早期错误所有权和任意截止点半帧误判。所有修改均在 benchmark 工具/fixture 内。
- 首次 live-p2 消息校验通过，但发现阶段读/采样间跨轮竞态：下一轮 FD 混入上轮 disconnected。修复为采样前后 progress 必须一致，丢弃过渡阶段观察，并新增回归。最终 live-p2-final 完整重跑 27 组、18144 连接、377061 消息；错误/OOM 为零，81 次断开 FD 差值均为零。
- live-cycles-p2 对三个协议各做 512 连接十轮重连、每轮 3 秒；15360 连接、471207 消息，错误/OOM 为零，30 次 FD 均回基线。SSE RSS 在前三轮升到约25 MiB，后七轮在25.24–25.36 MiB；TCP约8.8、WS约17 MiB。短时证据不宣称小时级稳定性。
- 12 个 Python 运行器定向检查、4 个独立 TCP/SSE 对端检查通过；11 个 Dever 入口生产优化构建和两个新 Rust example 离线 release/原生优化构建通过。Dever fmt-check、Python解析、源码空白/冲突和范围检查通过；Rustfmt/Clippy 不可用，未运行。
- /dev/shm/dever-perf-p2.E84QV8 已空并移除。保留原始构建/剖析/中断与最终报告以供复核；不提交，不运行自动归档或自动日志提交脚本。
- 最终独立检查无剩余阻断；主代理复核四份最终报告、全部 OOM/FD 计数及无残留预算组。P2 完成，可进入 HTTP/2 阶段，未将实验调度变化当作已实现的生产优化。

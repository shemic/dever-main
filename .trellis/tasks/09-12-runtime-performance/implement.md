# 执行顺序

- [x] 主代理实现 Dever 场景和 Python 构建/采样/报告入口。
- [x] 独立 implement 代理实现 test/dever-tests/examples/network_bench.rs 及其私有模块；复用现有网络依赖。
- [x] 编译定向 fixtures/example，最小验证测量统计和清理，短时 loopback HTTP/HTTPS 对照。
- [x] 检查最终文件、响应校验、延迟分母、丢弃/失败计数、资源预算和退出清理；记录初始结果与限制。
- [x] 更新 test/performance/README.md、现有质量规范及任务证据；不提交。

工具链使用 /root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin 的命令级 PATH；所有 Cargo 构建 offline。只运行本任务的最小定向检查，不执行旧 performance target 或全量测试。

## 最终证据

- 七个 Python 运行器定向测试和两个独立 Python HTTP 对端测试通过；负例响应正文必须记 error、成功延迟为空。覆盖热身去除、正确结果/数量、完整窗口、计数守恒、已退出进程和缺控制器时清理。
- Rust example 离线 release check/build 通过；七个 Dever 入口经 CLI 格式化、语义检查和生产优化原生构建。Rustfmt/Clippy 未安装，未报告为通过；显式源码空白/冲突标记检查与 Dever fmt-check 通过。
- 最终基线 78 组：15 内存、9 异步、54 HTTP/HTTPS；另 18 组新连接和 15 组 64 MiB 空闲基线。各组实际调用环境见 target/performance/*/report.json，结果解释以 test/performance/README.md 首轮结果为准。
- 54 组正式网络共计划 108000 请求，发出并成功 107962，错误 0；未发出 38（窗口过期 32、容量不足 6）。预热单独记录。新连接 900 请求全部成功，无丢弃；全部自有预算组无 OOM，结束后没有残留 dever-perf-* cgroup。
- 对照复查修正了大正文逐请求重复分配、短窗口吞吐分母、定时器抖动错误计丢弃、请求 driver 取消所有权、负载线程环境污染、缺控制器清理以及实际请求延迟校验。独立只读 review 最终没有剩余阻断。
- 仅构建/测试工具、fixtures、Cargo 测试依赖和文档发生变化；生产编译器/运行时未修改。保留产物和原始报告，不提交、不运行自动归档脚本。更高负载、长连接/持续稳定性和运行时优化是后续工作。

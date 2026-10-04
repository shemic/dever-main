# 执行计划

## 1. 启动与上下文

- [x] 用户确认后运行 `task.py start`；启动前不修改基准源码。
- [x] 使用 `trellis-before-dev` 重新加载 backend 规范，核对当前 diff、磁盘、CPU、cgroup 和正式构建产物；`build-http2-final` 已不在工作树，改用参数满足需求的 `typed-orm-build-v2`，仍须校验指纹。

## 2. 客户端并行配置

- [x] 将 `Process(cpu=...)` 收敛为 `Process(cpus=...)`，所有调用者一次性迁移；保持亲和性只作用于自有子进程。
- [x] 在 `run.py` 增加 CPU 集合解析、集中分配校验、`--client-cpus` 和 `--client-workers`，删除旧 `--client-cpu`。
- [x] 让 HTTP/HTTP2 与 live 客户端共同使用相同 worker 数和 CPU 集合；更新 report environment/arguments。
- [x] 清理重复解析与分支，确保进程生命周期、运行器合同和协议负载职责仍分离。

## 3. 定向验证

- [x] 扩展 `test_runner.py`，覆盖 CPU 集合解析、重复/越界/重叠拒绝、子进程实际亲和性和报告配置。
- [x] Python 语法解析与 runner 34/34 通过；HTTP/2 产物指纹校验通过，未运行全量测试。
- [x] `http2-ceiling-smoke-09-18`：h2c/TLS+h2 各 30/30 成功、16/16 握手、FD 差 0、OOM 0，报告确认 4 workers、CPU 1–4、客户端 5 线程。

## 4. 有界正式测量

- [x] `http2-ceiling-scan-09-18`：128 MiB、16x16、30k/45k/60k/90k、h2c/TLS+h2、5 秒、2 repeats 共 16/16 完成，错误/OOM/FD 差为 0。
- [x] 30k 档 h2c/TLS+h2 吞吐约 29.9k/29.8k；45k 以上 h2c 约 34–40k、TLS+h2 约 37–39k，服务端接近 1 核、客户端 1.4–2.0 核，请求槽丢弃主导，调度过期很少。28k 的 60 秒首轮分别只有 98.68%/97.91% 成功；24k 的 10 秒校准 TLS+h2 只有 99.18%。20k 的 10 秒校准分别为 99.96%/99.92%，据此将正式共同稳态输入定为 20,000 req/s（低于较低观测平台的 80%）。
- [x] `http2-ceiling-steady-128m-20k-09-18` 与 `http2-ceiling-steady-64m-20k-09-18`：h2c/TLS+h2 各 60 秒，成功率依次为 99.77%/99.92% 和 99.94%/99.80%，吞吐约 19.95k–19.99k req/s；全部零错误/OOM、FD 差 0。28k 首轮两项不足 99%，保留该报告为探索证据。
- [x] `http2-ceiling-recovery-09-18`：128 MiB 下 h2c/TLS+h2 各 100/100 轮，分别成功 9,944/10,000 请求，余下 56 次/组为调度过期；每轮至少成功 95/98 个，16 次握手、FD 差 0，请求错误/OOM 为 0。首末轮断开后 RSS 为 4,845,568→5,357,568 B / 5,959,680→6,647,808 B，完整逐轮序列保留。

## 5. 收尾

- [x] 更新 `test/performance/README.md` 与 backend 性能规范中的 CLI、测量结果和限制；HTTP/HTTPS 30/30、TCP/WS/SSE 16/16 最小相邻入口回归通过。
- [x] 检查了入口到子进程的 CPU/worker 传递、重复、职责、命名和无关改动；未改生产运行时或覆盖历史报告，空 frontend 模板是单独已归档的 bootstrap 任务。
- [x] `trellis-check`：runner 定向单测 34/34、Python AST、构建清单指纹、扫描/稳态/恢复报告计数与 OOM/FD/握手不变量、空白检查均通过；HTTP/HTTPS 与 TCP/WS/SSE 最小回归通过。未运行全量测试、Cargo build、rustfmt/clippy；未提交 Git。

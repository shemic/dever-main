# Design

现有 test/performance 负责产物和采样，Rust examples 负责压力对端。保留该分工，不增加第二套预算与报告框架。

异步对照使用独立 async_bench example：相同 run/wait、checked Int 加法、输入与批次；runtime-root 与 runtime-worker 仅改变批次所在任务。worker 模式增加一个结构化外层任务，不修改生产调度。对照程序输出 P1 SAMPLE 协议，Python 复用校验和与样本汇总。

长连接使用新增 Dever fixture 与 Rust 对端。Rust 对端有固定连接上限、阶段标记、消息校验和超时；Python 根据阶段采样同一个服务进程，复用 Process/Budget。每轮连接、保持、断开后等待采样，最后重连探测服务可用。TCP/WS 做小正文交互；SSE 校验有限事件，保留流直到断开。

高负载扫描复用现有 HTTP load，不重写压测器。使用独立 CPU 亲和性和 128 MiB 服务预算，合理短窗口与重复次数；保留客户端瓶颈和短窗口限制。

HTTP/2 只做本地依赖/池/协议边界核查：Hyper h2、TLS ALPN、流与连接预算、复用客户端、GOAWAY、取消隔离；HTTP/1 WebSocket 升级保持独立。

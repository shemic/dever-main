# 设计

## 1. 最小改动边界

本阶段只扩展 `test/performance` 的负载进程配置和测量文档。现有 Rust `network_bench` 已具备多线程 runtime、固定 HTTP/2 连接拓扑、速率调度、完整响应校验和延迟直方图，因此不修改 Rust 负载器，更不修改生产运行时。

多进程客户端会产生多组独立直方图。只合并 p99 会得到错误分位数，输出全部 4096 个桶再聚合又会扩大协议和实现范围。一个多线程进程已经能把请求 worker 分布到多个 CPU，同时保留单一调度和精确的进程内直方图，因此采用该方案。

## 2. CPU 与 worker 合同

`Process` 的亲和性参数从单个 `cpu` 改为可空的 `cpus` 集合，并在 fork 后、exec 前一次性调用：

```text
os.sched_setaffinity(0, cpus)
```

服务端仍把 `--server-cpu` 转为单元素集合。运行器将 `--client-cpu` 直接迁移为 `--client-cpus`，接受无重复的逗号分隔非负整数；新增 `--client-workers`，默认 1。运行前集中校验：

- 所有指定 CPU 必须属于当前进程允许集合；
- 服务 CPU 不得出现在客户端 CPU 集合；
- 正式压测显式使用 `--client-workers 4 --client-cpus 1,2,3,4`。

不强制 worker 数等于 CPU 数，避免把合理的欠订阅/超订阅变成运行器错误；报告必须同时记录两者，使结果可解释。HTTP `load()` 和 `live.load_live()` 都把 `BENCH_WORKERS` 设置为同一个 `args.client_workers`，并把同一个 CPU 集合交给 `Process`。

## 3. 数据流与复用

```text
run.py 参数与集中校验
  -> Process(cpus=client_cpus)
  -> BENCH_WORKERS=client_workers
  -> 现有 network_bench 单进程 Tokio runtime
  -> 现有 H2 连接/请求槽、调度、校验、Histogram
  -> 现有 load JSON、进程采样、case/report 汇总
```

`Process` 继续只负责子进程生命周期、亲和性和采样；它不解释 server/client 角色。`run.py` 负责参数合同与服务/客户端隔离。`live.py` 只消费已经校验的参数，不新增一套 CPU 解析逻辑。

## 4. 测量阶段

所有正式场景只测 Dever，服务绑定 CPU 0、1 worker、1 CPU quota、swap 0；客户端绑定 CPU 1–4、4 workers，位于服务 cgroup 之外。使用现存 `typed-orm-build-v2` 指纹产物和 16 流配置；先验证其 HTTP/2 二进制、TLS 文件和清单，不重建生产产物。

### 饱和扫描

- 预算：128 MiB
- 路径：`/plain`
- 传输：h2c、TLS+h2
- 拓扑：16x16（256 请求槽）
- offered rate：30000、45000、60000、90000 req/s
- 每 case：1 秒预热、5 秒测量、2 个 repeat

若 90000 仍未出现平台，则只报告“扫描上界不足”，不继续无界提高负载。观测平台取成功 QPS 不再随 offered rate 近似线性增长的区间，不把容量丢弃计为服务错误。

### 60 秒稳态

从扫描得到的 h2c/TLS+h2 较低平台值向下取整到不高于 80% 的明确整数 rate，确保两种传输使用相同负载。128 MiB 和 64 MiB 各运行 h2c/TLS+h2 一个 60 秒 case。99% 成功率门槛只用于证明选定 rate 可持续，不用于定义服务极限。

### 连接恢复耐久

128 MiB 下，h2c/TLS+h2 各在同一服务进程运行 100 轮；每轮 16 条物理连接、每连接 16 流、0.1 秒、1000 req/s，随后使用既有 0.3 秒断开观测。每轮校验握手、请求错误和 FD 回落，整组检查 cgroup OOM 与 RSS 序列。

## 5. 风险与解释

- 多线程负载器仍有单一速率调度协程。如果扫描中客户端 CPU 未扩展、dispatch expired 显著而服务端未满 1 核，本阶段应如实判定压测器仍受调度器限制；是否实现分片调度另建任务，不能伪称服务端平台。
- 客户端和服务端仍共享同一虚拟机，只是 CPU 集合隔离；缓存、内存带宽和宿主机噪声仍然共享。
- 64 MiB cgroup 的文件页计费受热缓存影响，继续同时报告进程 RSS/PSS 和 cgroup peak，不以任一单值替代另一个。
- 100 轮是分钟级重复回收证据，不是小时级泄漏或生产网络稳定性证明。

## 6. 回退

改动不涉及生产代码。若 CPU 集合支持造成回归，回退 Python 运行器、测试和文档即可；已有构建产物和历史报告不修改，新测量始终写入新输出目录。

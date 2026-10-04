# 独立二进制性能基准

这里测常驻内存、异步调度、HTTP/1.1、HTTP/2 和 TCP/WebSocket/SSE 长连接；不改运行时。构建单独执行，运行阶段只启动已编译产物。全部程序绑定自己创建的 loopback 随机端口，不接触已有服务，不进入默认 `cargo test`。

## 构建与短时验证

在仓库根目录运行，使用已经安装的 Rust 工具链与锁定的离线依赖：

```bash
cargo build --offline -p dever-cli --bin dever
cargo build --offline --locked --release --target-dir target/native-runtime -p dever-tests --example network_bench --example live_bench --example async_bench
python3 test/performance/run.py build --output target/performance/build-current --workers 1 --connections 512
python3 -m unittest discover -s test/performance -p test_runner.py -v
python3 test/performance/test_peer_settings.py -v
python3 test/performance/test_network_bench.py -v
python3 test/performance/test_live_bench.py -v
python3 test/performance/run.py run --artifacts target/performance/build-current --output target/performance/smoke-current --suite all --duration 0.5 --warmup 0.2 --rates 100 --paths /plain --repeats 1
```

只复验当前 Dever/Markdown 双源码 CMS 和 base/SQLite/PostgreSQL/both 四个 profile 时，可使用
`python3 test/performance/run.py build --selection cms-profiles --output target/performance/cms-build-current`。
该选择不构建无关的 HTTP、长连接、ORM 场景和 peer；生成的产物仍可用
`run --suite cms` 分别测量 `cms`（Dever）和 `cms-md`（Markdown）的真实登录、文章创建/替换、发布和前台查询；旧 `READY|3` 计数 fixture 已删除。仅做四 profile 构建可选 `--selection profiles`。PostgreSQL profile 使用不连接数据库的编译专用声明，不需要真实 URL；真实 PostgreSQL ORM/压力测试仍必须显式配置根 `config/setting.json` 的 `database.postgres_test`。

仅复验 runtime/HTTP/TLS/HTTP2/TCP/WebSocket/SSE 时使用 `--selection protocols`，不构建 ORM/CMS 或重复四 profile；三种 Rust peer 可用 `--network-bench`、`--live-bench`、`--async-bench` 指定明确准备的路径。

输出目录必须是新路径，保留先前报告。当前 Dever 产物统一经公开 `dever build` 使用 LLVM O2、LLD 和明确准备的 runtime archive；构建日志及二进制 SHA-256、大小、耗时保存在 `manifest.json`。runtime/http/live 每组构建一个包含该组 CMD 的应用，各测量通过不同命令启动，大小不能解读为仅一个函数的程序。`profile-report.json` 仍使用同一份最小 CMD 源码生成 base、SQLite、PostgreSQL 和 both 产物，记录 bridge 的锁定依赖。作者必须准备匹配源码的四个 archive；编译器自身的 debug/release 不决定 archive 优化级别。Rust 对照程序由上面的 Cargo release 命令另行准备并记录二进制摘要。Dever 使用生产默认调度，`--workers` 和 `task_capacity` 只配置 Rust 对照，不声称两者调度配置相同；可用专属 CPU affinity 统一核数。运行器要求 `application_configuration=llvm-cmd-v1`，拒绝旧 Rust 后端产物冒充当前 LLVM 测量。测量不调用 Cargo。

双源码 CMS 也有独立入口，不构建网络 peer 和无关场景：

```text
python3 -B test/performance/cms.py build --output target/performance/cms-publish-build
python3 -B test/performance/cms.py run --manifest target/performance/cms-publish-build/manifest.json --output target/performance/cms-publish-128 --cgroup-parent /sys/fs/cgroup --memory-mib 128
python3 -B test/performance/cms.py run --manifest target/performance/cms-publish-build/manifest.json --output target/performance/cms-publish-64 --cgroup-parent /sys/fs/cgroup --memory-mib 64
python3 -B -m unittest discover -s test/performance -p test_cms.py -v
```

运行使用仅含二进制和私有 `config/setting.json` 的副本，不保留 module。先在自有 SQLite 平台库 bootstrap、迁移租户库并给 admin/front 分别建立 Owner，再启动随机 loopback 端口。服务启动与登录/发布/查询在专属 cgroup 内，bootstrap 和部署命令不在该预算内。保留 Cookie/Origin、真实身份和租户验证，不伪造登录身份。逐篇创建、替换、发布并核对全部前台结果；重复发布必须返回 409，两套源码结果必须一致。默认 16 篇顺序业务基线报告启动时间、每种请求样本数/中位/最大延迟、RSS/PSS 与 cgroup peak/OOM，不称为并发饱和吞吐或 p99。测试手动传送服务返回的 Secure Cookie，只验证 owned HTTP 协议，不代表浏览器 HTTPS 验收。日志阈值固定 error，出现服务错误、超时、OOM 或清理失败均失败。

追加 `--concurrency 8 --read-seconds 10` 会在发布后运行八个独立连接的已登录前台列表查询，每次都核对完整业务结果。报告另列 `concurrent_reads` 的实际成功 QPS、p50/p95/p99、测量期资源趋势和断连 FD 回落；这是闭环读负载，不能解释为写入吞吐或预设请求到达率。并发数最多64、时长最多3600秒，同一服务进程内支持30分钟持续读取；客户端使用固定2048桶的对数延迟直方图，分位数报告包含请求及结果核对的桶上界，避免随请求数保存无限样本。首次失败停止其他客户端，所有连接按原请求期限关闭；不记录 Cookie 或密码。完整Linux门及64/128MiB长测入口见 [test/ci/README.md](../ci/README.md)。

真实 PostgreSQL 的 Owner HTTP/双物理租户验收另由共享 Rust fixture 管理数据库生命周期：

```text
cargo test --offline --locked -p dever-tests --features api,postgres --test postgres_api postgres_cms_http_isolates_two_tenant_databases -- --ignored --exact --nocapture
```

根 `database.postgres_test` 必须指向专属测试实例，URL 数据库名包含 `{case}`，账号允许创建/删除测试库；默认不运行、不探测外部服务。fixture 使用随机控制库及租户前缀，只清理本轮拥有的库，失败也清理，清理失败不能记录 passed。`cms.py postgres-case` 始终先复制传入项目到自有临时目录，端口/密钥/配置/产物仅写副本，输出目录保留日志和不含凭据的结果。Rust 目标额外注入仅在 `test/` 的普通成员 CMD，并显式启用 `--rbac-fixture`：覆盖 Owner、两个租户各两篇发布、相同 slug 独立数据、跨站 Cookie/跨租户登录拒绝，以及非 Owner 多角色、跨站同名角色和原会话撤销立即失效。单独调用不启用该测试 fixture 的工具仍仅证明 Owner 合同。权限键分别读取实际站点目录，GET/DELETE 参数统一走 query，生产 CMS 和 Owner 保留角色规则不改。

runtime/HTTP/live 夹具已使用当前 App→Port→Adapter 结构，CMD 根有真实 Bool 输出；live 的 typed Adapter setting 在每个产物的私有配置中独立绑定。ORM seed 是测量前的 CMD 部署步骤，count/cancel 是普通 API，取消测试使用单连接池并核对计数和连接归还，不能把 handler 超时冒充客户端取消。

2026-10-02 LLVM 基线使用 `protocols-llvm-10-02`、`cms-llvm-10-02` 及 `profiles-llvm-10-02` 的准确产物摘要，私有作者编译器来源记录在 `target/closure-current-compiler/provenance.json`。四个最小 CMD profile 大小分别为 base 2,083,880、SQLite 4,139,976、PostgreSQL 2,265,224、both 4,238,568 字节。Dever/Markdown CMS 分别 8,323,880 / 8,326,120 字节。

| 当前测量 | 条件 | 结果与报告目录（均位于 target/performance） |
| --- | --- | --- |
| 双 CMS 发布 | 64/128 MiB、每组顺序16篇 | 零错误/OOM，启动44.6–51.9ms，RSS峰值28.20–28.33MiB；`cms-llvm-{64,128}-10-02` |
| 双 CMS 登录后列表读取 | 64/128 MiB、8并发、10秒、不绑核 | 1210.6–1322.2 QPS，p99 11.48–13.59ms，RSS峰值30.81–31.04MiB，零错误/OOM；`cms-read-llvm-{64,128}-10-02` |
| HTTP/2 明文及 TLS 稳态 | 64/128 MiB、单核、4连接×16流、目标10k、60秒 | 成功/计划99.718–99.794%，9971.6–9979.2 QPS，p99 2.912–3.424ms，RSS峰值4.88–5.72MiB，零错误/OOM/断开后FD差；`http2-steady-llvm-{64,128}-10-02` |
| HTTP/2 连接恢复 | 64 MiB、单核、两种传输各100轮 | 合计19,887个完整成功请求，200次断开后FD差均0，零错误/OOM；`http2-recovery-llvm-64-10-02-v2` |
| TCP/WS/SSE 连接恢复 | 64/128 MiB、单核、32/128连接、各10轮 | 12个case、120次断开后FD差均0，完整消息校验通过，零错误/OOM；`live-llvm-{64,128}-10-02` |

这些是本机 loopback 与热文件缓存的有限测量。CMS使用闭环客户端，HTTP/2使用固定速率且保留少量未派发计数，两者不能直接比较。15k TLS短时校准未达到99%计划成功门，所以当前一分钟基线选择10k，不把下文历史20k/饱和结果当作本次结果。首次恢复因运行器重复复制peer耗尽磁盘的失败目录保留，最终恢复证据仅用v2。历史 `cms-publish-*-09-30-*`、`orm-runtime-64-09-30-v1` 与 `live-tcp-64-09-30-v1` 不代表本次重跑。

共享编译的本机基线记录于 `target/closure-shared-compile-measurement.json`：私有debug核心、热OS缓存下，首次请求7.318秒、命中5.564秒；两个同步客户端提交同一份新源码分别7.463/7.838秒且产物一致。时间包含签名输入复核及IPC，不能当纯编译耗时或正式优化版CLI成绩。每个产物4,239,368字节，两条缓存合计8,478,736字节，校验无损坏；跨真实UID共享另有独立定向验收。

产物目录含二进制、测试证书、构建配置，可整体复制到兼容的 Linux 机器；运行器仅依赖 Python 3.11+ 标准库和 `/proc`。TLS 证书来自 `test/dever-tests/fixtures/tls/`，仅用于测试，执行时校验证书链和 localhost 名称。测试证书过期后需更新 fixture，不得关闭验证。

两个独立 peer 测试从仓库 `config/setting.json` 的 `performance` 读取 `network_peer`、`live_peer`，例如：

```json
{"performance":{"network_peer":"target/performance/build-current/network_bench","live_peer":"target/performance/build-current/live_bench"}}
```

把该 `performance` 对象合并进已有设置，保留原有数据库配置。基准进程配置不读取环境变量。运行器在同文件系统硬链接已验证的只读 peer 二进制，仅跨文件系统时复制；每轮仍在其旁边以 `0600` 写入独立 `config/setting.json` 的 `benchmark` 对象。测量期间不得修改输入二进制。字段为 `workers`、`connections`、`task_capacity`、`timeout_ms`、`lifetime_ms`、HTTP/2 参数和 TLS 证书路径。CLI 负载参数仍显式传递。缺少配置、非法类型或越界值会报错；构建器通过不启动网络的 `check-config` 检查 peer 的 JSON 配置协议。旧协议的 manifest 不再接受，应重新构建产物，旧报告仍保留历史意义。

## 场景与统计口径

| 组 | 场景 | 计时/验证 |
| --- | --- | --- |
| memory | 同步空闲、异步空闲、256 个等待 Channel 的任务、HTTP/HTTPS 监听 | READY 后采样 RSS/PSS、VmHWM、线程/FD；parked 用取消场景的等待阶段，不另建实现 |
| async | 16384 次 run/wait、16384 次有界 Channel 发送与消费、取消并排空 256 个已启动任务 | 源码内单调时钟；计算结果/数量必须正确，打印在计时外；前两项每进程 10 轮、去掉首轮，取消保留完整排空耗时 |
| http | HTTP/HTTPS × Dever/Rust-runtime/原生 Hyper × 三种响应 | 固定速率、有限并发；状态码、Content-Type 和完整正文必须匹配；连接复用与新建连接可分别运行 |
| http2 | h2c prior knowledge/TLS ALPN × Dever/Rust-runtime/原生 Hyper × 物理连接/流拓扑 | 计时前建立固定物理连接；校验握手数、状态码、Content-Type、完整正文、吞吐、延迟和资源 |
| http2-recovery | 同一服务进程内重复建立、使用、关闭 HTTP/2 连接 | 每轮校验请求总账和握手数，断开后 FD 必须回到 READY 基线，保留 RSS 序列 |
| live | TCP/WebSocket/SSE × 连接档位 × 多轮连接/保持/断开 | TCP/WS 校验带轮次和连接标识的回显；SSE 校验初始事件与心跳；对比各轮断开后的 FD/RSS |
| orm | SQLite/PostgreSQL 的 idle、CRUD、list/cursor/stream、pool saturation、transaction cancel、HTTP+JSON+DB | 每个 case 使用独立 bundle 和数据库；校验精确结果，再报告数据库操作延迟、吞吐、资源和连接回收 |
| cms | `examples/cms/dever` 和 `examples/cms/md` 的 Model、Seed、用户注册和发布事务 | 相同入口、单独 SQLite bundle，业务初始化与事务完成后分别采样 RSS/PSS/VmHWM 和可用时的 cgroup peak |

## ORM 基准

所有 Dever 产物在 `build` 阶段通过公开 `dever` 项目路径生成；无数据库声明的 runtime/http/live 使用 base archive。`run` 阶段只复制和执行已有二进制。每个数据库 case 都创建独立 bundle；SQLite 使用该 bundle 自己的 `data/db/orm.db`，重复轮次不会复用数据库状态。

构建器把测试源码暂存到 `module/benchmark/<domain>/app.dever`，共享能力位于 `benchmark/bench`、`benchmark/config`，由 `api.dever` 声明 CMD。fixture 不生成 main/package/exposes，也不向 `dever build` 传任意 entry。

CMS 从原始双源码项目构建，通过真实登录/租户/发布 HTTP 流程测量，不注入新的业务入口。普通 `build` 只构建 Dever CMS，`cms-profiles` 构建双源码。下方历史 `READY|3` 报告不代表当前实现。

2026-09-18 的当前源码专项报告保存在 `target/performance/cms-dual-build-09-18-v2/{manifest.json,profile-report.json}` 和 `target/performance/cms-dual-rss-09-18/report.json`。base/SQLite/PostgreSQL/both 原生程序分别为 411,352 / 2,558,200 / 2,708,112 / 4,672,888 字节。CMS Dever/Markdown 产物分别为 3,151,352 / 3,151,416 字节；各运行 3 次，READY 全部为 3，完成时间中位数分别为 19.87 / 19.69 ms，空闲 RSS 中位数同为 5.37 MiB，PSS 分别为 3.28 / 3.33 MiB，VmHWM 中位数同为 5.37 MiB。采用本机热缓存、独立 SQLite bundle、每次 READY 后 2 秒采样；无吞吐负载，不能把启动时间或空闲内存解释为业务吞吐/峰值容量。当前 cgroup 父目录不可写，未施加 64/128 MiB 硬限额，也未记录 cgroup peak。

默认只构建和运行 SQLite。真实 PostgreSQL 统一读取仓库根 `config/setting.json` 的 `database.postgres_test`，不接受环境变量或命令行连接覆盖；该本地文件已被 Git 忽略。其 `url` 必须在数据库名中包含唯一 `{case}`，例如 `postgres://bench:secret@127.0.0.1/dever_{case}`，查询参数或用户名中的占位符不算隔离。运行器按“输出目录短哈希 + driver + entry + repeat”展开 case，因此不同报告不会复用数据库；构建器和运行器以 create-new 和 `0600` 权限原子写入 bundle 的 `config/setting.json`，构建结束会删除含连接信息的临时配置，URL 不写入 manifest 或 report。运行器不会创建或清理 PostgreSQL 数据库，因此模板必须指向预先准备的专用测试数据库，不能指向已有业务库。

```json
{
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/test.db"
    },
    "postgres_test": {
      "type": "postgres",
      "url": "postgres://bench:secret@127.0.0.1/dever_{case}",
      "tls": "disabled",
      "min_connections": 0,
      "max_connections": 4,
      "max_page_size": 100,
      "wait_timeout_ms": 5000,
      "io_timeout_ms": 30000
    }
  }
}
```

```bash
python3 test/performance/run.py run \
  --artifacts target/performance/build-current \
  --output target/performance/orm-smoke \
  --suite orm --duration 1 --warmup 0.2 --repeats 1

python3 test/performance/run.py run \
  --artifacts target/performance/build-current \
  --output target/performance/orm-postgres-smoke \
  --suite orm --duration 1 --warmup 0.2 --repeats 1
```

ORM 报告包含 binary size、RSS/PSS/VmHWM、cgroup current/peak/OOM、线程/FD、吞吐和 p50/p95/p99。当前生产 runtime 没有低成本的 pool wait、SQL 数和数据库连接数观测点，报告将这些字段明确标为 `unavailable`，同时记录配置的 pool capacity 和进程 FD，不用推算值冒充真实指标。transaction cancel 先验证单连接 pool 可用，再由长事务占满 pool、在响应完成前断开客户端，并要求连接在 HTTP handler timeout 前归还、计数仍为 0；这样不会把 handler timeout 回滚误报为客户端取消，也不会绕过语言对 transaction 内并发任务的静态限制。

run/wait 同时跑 Dever 与 Rust-runtime 的 root/worker 两种位置。root 在 `run_entry_with` 入口执行；worker 在一个计时外创建的结构化外层任务内执行。二者内部都用原有 `task::run/wait`、Bytes 输入和 checked Int 加法，保留子任务额度、取消和清理；没有以裸 Tokio task 代替语言任务。这个实验不修改默认调度。Channel/取消暂时仍是 Dever 单方基线。`/proc` 上下文切换计数汇总采样时仍存活的线程，线程退出会使累计值减少；总量分析用独立 `perf stat`，不能将采样差值当作精确任务计数。

live 用 `--connection-tiers 32,128,512 --cycles 3 --duration 2` 配置，每个档位独立启动服务，每轮保持约 2 秒，再关闭全部客户端并等待 300 ms。Rust 对端发布零基轮次的 `connecting/connected/closing/disconnected` 四阶段；Python 只用 connected/disconnected 保持窗口比较资源，记录最后一次断开样本和相对基线的 FD/RSS 差值。采样前后阶段变化的过渡样本不归属任何阶段，也不写入阶段报告。正文不匹配、阶段缺失、SSE 保持期无心跳或未回收全部客户端都失败；RSS 保留可能来自分配器缓存，不能单凭不回落断言泄漏。默认只测 32 条，档位不能超过构建时的 connections；`--duration` 为 0.3–60 秒，采样间隔最多 100 ms。live 的 SSE/WS 是明文 HTTP/1 场景，不包含 WSS 或 HTTP/2。

HTTP/2 用 `物理连接数 x 每连接并发流数` 描述拓扑。当前固定每连接 16 流、单流接收窗口 65535 字节、连接接收窗口 262144 字节、frame/send-buffer 上限 16384 字节。客户端在计时前完成精确数量的 h2c 或 TLS+h2 握手，每个连接的 `SendRequest` clone 由其 16 个请求槽共享；测量期间不重试、不替换连接、不自动扩容。`request_slots` 必须等于两个维度的乘积，`successful_handshakes` 必须等于物理连接数。断开后等待 300 ms，再以 READY 基线核对 FD。

三个路径为 `/plain`（`hello`）、`/json`（预制 `{"ok":true}`）、`/bytes`（65536 个 `x`）。JSON 路径测 JSON 响应传输，不代表动态 JSON 解析/序列化性能。`/bytes` 跨越 HTTP/2 默认单流接收窗口，用于验证流控下的大正文传输。大正文在服务启动时准备，各服务均无业务日志。

网络记录 `scheduled/sent/completed/succeeded/errors/dropped`；`dropped_capacity` 表示无可用请求槽，`dispatch_expired` 表示负载发生器未能在窗口内发出。`latency_ms` 从**计划发送时间**算到成功完成，包含排队/调度；`request_latency_ms` 单独测实际发出到成功完成，`dispatch_lag_ms` 记录调度延迟。不能用两个 p99 相减估算请求耗时。没有成功样本时成功延迟为 null，失败不会稀释成功分位数。Tokio 定时器会让高频请求成批到期，这个发生器用于可复现的有界负载和相对比较，不适合独立证明微秒级服务延迟或极限吞吐。检查负载端 CPU、lag、丢弃，再解释服务结果。

延迟使用有界直方图：1 ms 内按微秒分桶，之后每个二倍区间分 64 桶，分位数取桶上界。每轮保留原始计数和资源采样，不把多个 p99 再平均成整体 p99。异步的 `p95_ns_per_operation_sample` 是批次平均耗时的分位数，**不是逐任务 p95**。`ready_seconds` 是父进程启动到观察 READY 的耗时，包含创建进程和最多约 5 ms 的观察误差。

## 小机器运行预算

建议先测 1 核/128 MiB，再试 64 MiB 和 2 核/256 MiB。这里是应用预算，不包含操作系统自身内存。`--server-cpu` 固定服务端 CPU，`--client-cpus` 指定逗号分隔的客户端 CPU 集合；`--client-workers` 指定 HTTP/1、HTTP/2 和 live 负载的 worker 数。客户端 CPU 不能包含服务端 CPU；不指定客户端集合时继承当前允许的 CPU。`--cpu-quota` 是计算份额上限，不能代替绑核。

```bash
python3 test/performance/run.py run \
  --artifacts target/performance/build-current \
  --output target/performance/budget-128m-1 \
  --suite all --duration 10 --warmup 2 --repeats 3 \
  --rates 100,1000 --paths /plain,/json,/bytes \
  --concurrency 16 --connection-mode both \
  --http2-connections 1,4 --recovery-connections 1 \
  --connection-tiers 32,128,512 --cycles 3 \
  --server-cpu 0 --client-cpus 1 --client-workers 1 \
  --cgroup-parent /sys/fs/cgroup/system.slice \
  --memory-mib 128 --cpu-quota 1
```

上例 cgroup 路径只适用于有相应权限且父组已启用 cpu/memory 控制器的机器。工具为每个被测进程创建新的 `dever-perf-*` 子组，仅把自己的子进程放进去；不会修改父组、其他服务或全局 swap。指定内存限额时把该子组 swap 限额设为零。没有权限会明确失败，不回退成“无限额通过”。负载端和 Python 采样进程留在预算之外。

`memory.current/peak` 包含该组被计费的用户内存、内核/网络资源等；不能与进程 RSS 混用。报告同时保留两者、OOM 事件和 CPU 节流统计。进程由 Python 子进程 exec 启动，cgroup 峰值也可能包含 exec 前短暂的 Python 启动页，空闲基线应看 READY 后的 current/RSS/PSS。50 ms 默认采样可能漏掉短暂峰值，VmHWM 和 cgroup peak 补充整个生命周期峰值。

构建和指纹读取会预热文件缓存，已缓存的可执行文件/共享库页可能已计费到父组或其他组，因此本工具的 cgroup current 可以小于进程 RSS。它不是冷启动总 RAM 成本；三者应交叉观察。本阶段测热文件缓存下的新进程，不操作系统 `drop_caches`，也不据此宣称整台机器最低内存。

运行器在结束/异常/中断时终止并回收自己创建的进程，删除自己的空 cgroup。memory/http/http2/live 服务最终由运行器结束进程，**不把这个过程当作语言优雅退出验证**；http2-recovery 和 live 单独验证连接多轮断开后的回收。小时级 soak、故障网络和真实小机器矩阵需另行执行。

每轮 Rust 负载程序使用同一已验摘要的不可变二进制硬链接，各自保留独立 `config/setting.json` 和日志；跨文件系统才复制。测量期间不得改写输入二进制。这样 100 轮连接恢复不会累积 100 份相同程序，也不通过符号链接让配置解析回到源程序目录。

## 对照边界与报告

- Rust-runtime 服务使用与 Dever 相同的 HTTP/TLS/task runtime，帮助定位源代码桥接开销。
- 原生 Hyper 服务匹配本测试正常 GET 的响应、连接数、头/正文上限和 TLS 配置；没有复刻 runtime 的完整 WriteTimeout、程序故障传播和结构化 Scope。不能由这个对照推断慢读/慢上传、异常报文的能力等价。
- 每轮倒转实现顺序，减轻固定执行顺序造成的偏置。构建配置、CPU 型号、CPU 亲和性、父 cgroup、二进制指纹、原始日志、资源采样、负载端占用写入报告。
- `cases.jsonl` 在每个场景完成后追加；`report.json` 即使中途失败也保留已完成结果。`status=complete` 表示测量完整，不表示没有请求错误或已经达到性能目标。先查看 errors/dropped/OOM，再查看延迟和内存。
- 不预设绝对 QPS 或最低内存。已有计算热路径的 1.15 对照门槛仍在 `test/dever-tests/tests/performance.rs`，不推广成 HTTP 或整个语言的性能承诺。

## 首轮结果：2026-09-12

环境为 Linux x86_64、Intel Xeon Processor (Skylake, IBRS) 虚拟 CPU。服务绑定 CPU 0，负载端绑定 CPU 1；服务 runtime 1 worker、2 blocking slots、272 task slots、32 connections，负载端 16 个请求槽。主测每场景预热 0.5 秒、测量 2 秒，重复 3 个独立进程，目标速率 1000 请求/秒。不是持续压力或吞吐上限测量。

| Dever 空闲场景 | 三轮 RSS 中位值 |
| --- | ---: |
| 同步程序 | 1.90 MiB |
| 异步运行时 | 2.55 MiB |
| 256 个已启动、等待 Channel 的任务 | 3.38 MiB |
| HTTP 监听 | 3.45 MiB |
| HTTPS 监听 | 4.37 MiB |

以上为 128 MiB 专属 cgroup 下的 READY 后采样。另用 64 MiB 限额重复 3 轮空闲场景，全部完成、无 OOM。READY 的一次输出会启动异步程序的 blocking worker，异步场景观察到 3 个线程；这是该 fixture 的基线，不是“完全不打印”的最小运行时。

异步三轮中位值：完整 `run/wait` 一轮约 15.90 µs，Channel 生产与消费平均每项约 0.89 µs；停止并排空 256 个已启动等待任务约 0.61–0.67 ms。前两项每进程校验 10×16384 项，去掉首批预热；包含结构化任务语义，不能把 run/wait 数字称为纯任务分配耗时。尚未提供这几组异步操作的同语义 Rust 对照。

HTTP/HTTPS 三实现、三路径共 54 组正式测量，108000 个计划请求中 107962 个发出且成功，0 请求错误。38 个未发出：32 个超过发送窗口、6 个无空闲并发槽；预热另有 43 个未发出，原始记录保留。各组成功速率约 999 请求/秒，受目标速率限制，不能宣称最大吞吐为 1000。

| Dever 路径 | HTTP 每轮实际请求 p99 范围 | HTTPS 每轮实际请求 p99 范围 |
| --- | ---: | ---: |
| /plain | 0.243–1.248 ms | 0.580–0.909 ms |
| /json | 0.280–0.342 ms | 0.897–2.752 ms |
| /bytes | 0.392–0.500 ms | 0.839–2.400 ms |

表中是 dispatched→completed，每列列出三个独立 p99 的范围，没有把它们合并成全局 p99；计划发送延迟和负载端 lag 另在 JSON 中。短时虚拟机抖动明显，这些结果不用于宣称哪种实现更快。Dever 在这组负载的 RSS 中位值约 HTTP 3.9 MiB、HTTPS 4.9–5.1 MiB。

另以每次新建连接、100 请求/秒测试 HTTP/HTTPS 全路径三实现，共 18 组、900 个正式请求，全部成功且无丢弃。主测 78 组（15 内存、9 异步、54 网络）、新连接 18 组、64 MiB 内存 15 组均无 OOM。

原始结果及对应产物保存在仓库 `target/performance/`：

- `baseline-128m/report.json`：最终三轮基线，对应 `build-2/manifest.json`。
- `connections-new-128m/report.json`：新连接模式，对应 build-2。
- `memory-64m/report.json`：64 MiB 空闲基线，对应 build-1；该组使用的 Dever 二进制与 build-2 一致。
- `smoke-1/report.json`：早期连通检查，保留当时指纹，不用于最终网络比较。

以上首轮结果保留当时的测试范围；后续测量见下节。

## 第二轮：高负载与调度定位

`http-load-p2-final/report.json` 复用 build-2 产物，1 worker、32 服务连接和 32 客户端请求槽、服务 1 CPU/128 MiB。仅 `/plain`，预热 0.5 秒、测量 2 秒；HTTP/HTTPS、三个实现、5000/15000/30000 请求每秒，均重复三轮，共 54 组。

正式窗口共计划 1800000 个请求，1500830 个实际发出且成功，错误 0。未发出 299170 个：容量丢弃 298476、发送窗口过期 694；所有预算组无 OOM。下表是三个独立进程的成功 QPS 中位值，不是最大吞吐。

| 目标请求/秒 | Dever HTTP | Rust-runtime HTTP | Hyper HTTP | Dever HTTPS | Rust-runtime HTTPS | Hyper HTTPS |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 5000 | 4993 | 4994 | 4993 | 4970 | 4972 | 4970 |
| 15000 | 14904 | 14897 | 14922 | 14839 | 14756 | 14726 |
| 30000 | 21866 | 22187 | 22324 | 21217 | 21320 | 22236 |

最高输入档位 Dever 服务 CPU 中位值为 HTTP 0.53 核、HTTPS 0.62 核，客户端为 0.77/0.86 核；Dever RSS 约 4.12/5.32 MiB。32 请求槽与定时器成批发送已经影响结果，不能据此断言服务端上限是每秒两万。提高并发槽、使用独立负载机器应作为下一次吞吐上限测量的条件。

`http-load-p2/report.json` 保留初次中断：Linux 退出时 `/proc` 内存字段先于 waitpid 消失，导致采样工具失败。修复后才完整重跑；不能把初次中断算成语言服务失败，也不将两次样本混合。

`async-p2/report.json` 使用 build-p2、1 worker/1 CPU/128 MiB、1040 task slots、每批 16384 次操作，重复 3 个进程；每进程十批、去掉首批。共 18 组，校验全部通过且无 OOM。下表为进程内中位数再取三进程中位数：

| run/wait 位置 | Dever | 同 runtime、同优化参数的 Rust |
| --- | ---: | ---: |
| 入口 root | 15.50 µs/次 | 15.59 µs/次 |
| 外层结构化任务 worker | 2.003 µs/次 | 1.984 µs/次 |

这个串行微任务场景中，调度位置使 Dever 平均单次耗时降低约 87%，约 7.7 倍速度；不推广为所有业务程序加速。两种语言路径数值接近。源码路径显示 `run_entry_with` 用入口线程 block_on，run body 和 supervisor 在 Tokio worker 执行；单核下逐项等待造成频繁 OS 线程切换。`profile-p2/final-runtime-*.stat` 是独立 perf stat 的整体软件计数，对应 build-p2/async_bench；未加 cgroup、固定 CPU 0。虚拟机不支持 cycles/instructions，未报告硬件 IPC 或采样火焰图。

同组 Channel 约 0.912 µs/项；256 个任务停止并排空三次共 0.688/0.916/1.045 ms。这里只新增对照和证据，生产入口调度尚未优化。后续优化入口调度时需确认入口 future 的 Send 条件，根任务不应挤占子任务配额，并验证入口故障、子任务取消和 blocking 清理；不应直接照搬实验外层 run 或删除结构化任务语义。

## 第二轮：长连接容量与回收

`live-p2-final/report.json` 使用 build-p2-final、1 worker/1 CPU/128 MiB、512 connection slots、1040 task slots；32/128/512 条连接各重复三进程，每进程三轮连接/保持 2 秒/断开。TCP/WS 每约 100 ms 校验一次小消息回显，SSE 校验初始事件和 100 ms 心跳。

27 组共建立并关闭 18144 条连接，校验 377061 条消息，无协议错误和 OOM。81 次断开后的服务 FD 均回到启动基线。下面是第三轮保持窗口 RSS 的三个独立进程中位值，包含分配器在此前两轮保留的内存：

| 同时连接数 | TCP RSS | WebSocket RSS | SSE RSS |
| ---: | ---: | ---: | ---: |
| 32 | 3.99 MiB | 5.11 MiB | 5.03 MiB |
| 128 | 4.90 MiB | 9.44 MiB | 9.16 MiB |
| 512 | 8.70 MiB | 17.00 MiB | 24.97 MiB |

512 条连接保持时服务 CPU 约 TCP 0.15–0.16 核、WS 0.18–0.20 核、SSE 0.12–0.14 核。这里测的是持续小消息和心跳连接，不是大消息吞吐或完全不读的客户端。

初次 `live-p2/report.json` 也完成了全部消息校验，但有一条下一轮的采样被误归属上轮 disconnected，产生假的 FD +15。最终报告使用采样前后阶段一致性判断，完整重跑；资源回收结论只引用 final 报告。

随后 `live-cycles-p2/report.json` 对每种协议保持 512 条连接，十轮重连、每轮保持 3 秒，单场景约 34 秒。共 15360 次连接、471207 条校验消息，错误/OOM 为零，30 次 FD 回落均为零差值。TCP 断开后的 RSS 最后稳定在约 8.6–8.8 MiB，WS 约 17 MiB；SSE 前三轮 21.33/24.48/25.12 MiB，后七轮为 25.24–25.36 MiB，未观察到按轮持续增长。这是短时重复回收证据，不是小时级泄漏或生产网络稳定性证明。

复现本轮长连接两组命令（输出目录另取新名称）：

```bash
python3 test/performance/run.py run --artifacts target/performance/build-p2-final --output target/performance/live-repeat --suite live --duration 2 --connection-tiers 32,128,512 --cycles 3 --repeats 3 --server-cpu 0 --client-cpus 1 --client-workers 1 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/build-p2-final --output target/performance/live-more-cycles --suite live --duration 3 --connection-tiers 512 --cycles 10 --repeats 1 --server-cpu 0 --client-cpus 1 --client-workers 1 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
```

本轮 12 个运行器检查、4 个独立 TCP/SSE 对端检查通过，11 个 Dever 入口及两个新 Rust example 离线构建通过。Rustfmt/Clippy 未安装，未运行；Dever 格式、Python 解析、源码空白与最终范围检查通过。生产编译器/运行时没有修改，未运行全量测试。

## 第三轮：HTTP/2 吞吐、流控与连接回收

2026-09-13 使用 `build-http2-final` 产物完成正式矩阵。环境仍为 Linux x86_64、Intel Xeon Processor (Skylake, IBRS) 虚拟 CPU；服务绑定 CPU 0，负载端绑定 CPU 1。服务使用 1 worker、128 MiB cgroup、1 CPU quota、swap 0；负载端不计入服务预算。每个 case 预热 0.5 秒、测量 2 秒、启动独立服务进程，三实现与两种传输的执行顺序逐轮倒转。

`/plain` 共 108 个 case、3600000 个计划请求，其中 2729242 个发出且完整成功，错误 0。870758 个未发出请求中，869368 个受请求槽限制，1390 个超过调度窗口；全部计数守恒。108 次正式负载和 108 次预热的握手数均与物理连接数一致，断开后的 FD 均回到 READY 基线，无 OOM。下表是 30000 请求/秒输入档位的三轮中位数；p99 是实际发出到完整响应的请求延迟。

| 实现 | 传输 | 1x16 成功 QPS | 4x16 成功 QPS | 4x16 请求 p99 | 4x16 服务 RSS |
| --- | --- | ---: | ---: | ---: | ---: |
| Dever | h2c | 10578 | 29575 | 2.208 ms | 4.60 MiB |
| Rust-runtime | h2c | 10804 | 28996 | 3.392 ms | 5.00 MiB |
| Hyper | h2c | 11956 | 29108 | 1.984 ms | 4.59 MiB |
| Dever | TLS+h2 | 9957 | 29814 | 1.840 ms | 5.59 MiB |
| Rust-runtime | TLS+h2 | 10122 | 28681 | 2.880 ms | 6.07 MiB |
| Hyper | TLS+h2 | 11555 | 28719 | 3.200 ms | 5.68 MiB |

Dever 4x16 在 5000、15000、30000 三档的 h2c 成功 QPS 中位值分别为 4995、14983、29575，TLS+h2 分别为 4994、14977、29814。1x16 在高输入下只有约 10k–10.6k 成功 QPS，而 4x16 接近 30k，说明这次单连接结果受固定 16 个请求槽明显约束；不能把任一数字解释成服务端极限。最高档 4x16 的 Dever 服务 CPU 中位值为 h2c 0.89 核、TLS+h2 0.93 核，负载端也约 0.9 核，进一步限制了极限吞吐结论。三种实现的短时差异不具备跨机器性能承诺意义。

`/bytes` 共 108 个 case、115200 个计划请求，115180 个完整成功、错误 0、丢弃 20、无 OOM，握手和 FD 检查全部通过。三种实现、两种传输、两种拓扑在 100/500/1000 请求每秒档均维持目标速率；这相当于单 case 最高约 62.5 MiB/s 的响应正文。Dever 的 RSS 中位值为 h2c 4.04–4.23 MiB、TLS+h2 5.05–5.30 MiB。短 case 的 p99 有明显虚拟机抖动，原始三轮数据比单个汇总值更适合定位异常。

`http2-recovery-128m` 对三种实现、h2c/TLS 各在同一服务进程中运行 10 轮 1x16 连接周期，共 60 轮、30000 个计划请求；29984 个完整成功、错误 0、丢弃 16。每轮握手数正确，60 次断开后的 FD 差值均为 0，无 OOM。六组最后一轮相对第一轮的断开后 RSS 变化为 8–100 KiB，短时序列未观察到按轮持续增长；这不是小时级泄漏证明。

另在 64 MiB、1 CPU、swap 0 下运行 Dever 1x16、5000 请求每秒的 h2c/TLS 代表场景。20000 个计划请求中 19991 个完整成功、错误 0、丢弃 9、无 OOM；h2c/TLS 成功 QPS 为 4997/4996，RSS 为 4.04/5.17 MiB，PSS 为 2.00/3.10 MiB。cgroup peak 只有 1.30/1.35 MiB，是热文件页可能已计费给其它组所致，不能代替进程 RSS/PSS 或冷启动总内存。

正式结果和构建清单位于：

- `build-http2-final/manifest.json`：16 个二进制的指纹、大小、构建配置和固定 HTTP/2 limits。
- `http2-plain-128m/report.json`：`/plain` 128 MiB 正式矩阵。
- `http2-bytes-128m/report.json`：`/bytes` 128 MiB 正式矩阵。
- `http2-recovery-128m/report.json`：128 MiB 十轮连接恢复矩阵。
- `http2-active-64m/report.json`：64 MiB Dever 活跃代表场景。

历史 `build-http2-final` 产物已不在当前工作树；以下是当时使用的命令记录，精确复现历史二进制须先恢复匹配的旧产物。本机当前可直接运行的命令见下方第四轮，复现时必须换新输出目录：

```bash
python3 test/performance/run.py run --artifacts target/performance/build-http2-final --output target/performance/http2-plain-repeat --suite http2 --duration 2 --warmup 0.5 --rates 5000,15000,30000 --paths /plain --http2-connections 1,4 --repeats 3 --server-cpu 0 --client-cpus 1 --client-workers 1 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/build-http2-final --output target/performance/http2-bytes-repeat --suite http2 --duration 2 --warmup 0.5 --rates 100,500,1000 --paths /bytes --http2-connections 1,4 --repeats 3 --server-cpu 0 --client-cpus 1 --client-workers 1 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/build-http2-final --output target/performance/http2-recovery-repeat --suite http2-recovery --duration 0.5 --warmup 0.1 --cycles 10 --recovery-rate 1000 --recovery-path /plain --recovery-connections 1 --repeats 1 --server-cpu 0 --client-cpus 1 --client-workers 1 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
```

N5 的接口与约束见 [LANGUAGE.md](../../LANGUAGE.md)。这轮只建立 loopback、热文件缓存、短时固定速率下的相对证据，不包含跨主机网络、故障注入、小时级 soak 或多平台矩阵。HTTP/2 不保证所有负载都比 HTTP/1.1 快；普通 WebSocket 继续走 HTTP/1，RFC 8441 扩展 CONNECT 尚未实现。

## 第四轮：多核负载端、HTTP/2 饱和与稳态

2026-09-18 使用 `typed-orm-build-v2/manifest.json` 中校验过指纹的现存 HTTP/2 二进制和测试证书；原 `build-http2-final` 不在当前工作树，未重建产物。服务端绑定 CPU 0、1 worker、1 CPU quota、无 swap；负载端为 CPU 1–4、4 workers，位于服务 cgroup 外。机器允许 CPU 0–4。服务预算为 128 MiB，稳态另测 64 MiB。所有场景使用 `/plain`、16 条物理连接、每连接 16 流，只有自有 loopback 端口；这是同一虚拟机的热缓存观测，不是跨机器或生产容量。

128 MiB 扫描覆盖 h2c/TLS+h2 的 30k/45k/60k/90k 输入，每档预热 1 秒、计时 5 秒、2 个重复。16/16 case 计数守恒，请求错误、OOM 和断开后的 FD 差均为 0；每次握手数为 16。下表为成功 QPS 的两轮中位值：

| 传输 | 30k | 45k | 60k | 90k |
| --- | ---: | ---: | ---: | ---: |
| h2c | 29,899 | 36,690 | 36,027 | 39,486 |
| TLS+h2 | 29,770 | 38,205 | 37,410 | 38,136 |

45k 以上服务 CPU 约 0.96–1.00 核，客户端约 1.43–1.97 核；未发送请求主要因 256 个请求槽满，调度窗口过期数量相对很少。因此观测到当前拓扑约 36k–40k req/s 的吞吐平台，不能把它归结为纯服务端极限或把输入 90k 当成实际吞吐。28k 的首轮 60 秒成功率为 h2c 98.68%、TLS+h2 97.91%，未达 99% 验收门槛；24k 的 10 秒校准 TLS+h2 为 99.18%，余量仍小。20k 的 10 秒校准分别为 99.96% 和 99.92%，所以正式稳态统一采用 20k，低于较低观测平台的 80%。这些探索报告保留，不算正式稳态通过。

正式每个 case 预热 1 秒、测量 60 秒、计划 1,200,000 次请求。成功率是成功请求除以计划请求，容量丢弃仍在原始报告中，不称为零丢弃：

| 服务预算 | 传输 | 成功/计划 | 成功 QPS | 请求 p99 | 服务 RSS 高水位 | OOM/错误/FD 差 |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 128 MiB | h2c | 1,197,220/1,200,000 (99.77%) | 19,953 | 5.376 ms | 6.78 MiB | 0/0/0 |
| 128 MiB | TLS+h2 | 1,199,036/1,200,000 (99.92%) | 19,983 | 4.800 ms | 7.83 MiB | 0/0/0 |
| 64 MiB | h2c | 1,199,221/1,200,000 (99.94%) | 19,986 | 4.672 ms | 6.43 MiB | 0/0/0 |
| 64 MiB | TLS+h2 | 1,197,545/1,200,000 (99.80%) | 19,958 | 5.184 ms | 7.90 MiB | 0/0/0 |

128 MiB 下在各自的同一服务进程上完成 h2c/TLS+h2 各 100 轮 16x16 恢复；每轮 0.1 秒、1000 req/s，随后观察断开 0.3 秒。两组各计划 10,000 次、成功 9,944 次，其余各 56 次为调度窗口过期，请求错误和 OOM 为 0。每轮至少成功 95（h2c）/98（TLS+h2）次，握手始终 16，200 次断开后的 FD 差全部为 0。断开后 RSS 从首轮到末轮为 h2c 4,845,568→5,357,568 B，TLS+h2 5,959,680→6,647,808 B；保留完整逐轮序列，不将分钟级增长或回落解释成小时级泄漏结论。邻接的 HTTP/HTTPS 各 30/30 成功，TCP/WS/SSE 各 16/16 连接完成关闭，零错误。

复现命令（输出目录必须换新名称；运行前确认 CPU 0–4 和 cgroup 权限，不接触现有服务）：

```bash
python3 test/performance/run.py run --artifacts target/performance/typed-orm-build-v2 --output target/performance/http2-scan-next --suite http2 --duration 5 --warmup 1 --rates 30000,45000,60000,90000 --paths /plain --http2-connections 16 --repeats 2 --implementations dever --server-cpu 0 --client-cpus 1,2,3,4 --client-workers 4 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/typed-orm-build-v2 --output target/performance/http2-steady-128-next --suite http2 --duration 60 --warmup 1 --rates 20000 --paths /plain --http2-connections 16 --repeats 1 --implementations dever --server-cpu 0 --client-cpus 1,2,3,4 --client-workers 4 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/typed-orm-build-v2 --output target/performance/http2-steady-64-next --suite http2 --duration 60 --warmup 1 --rates 20000 --paths /plain --http2-connections 16 --repeats 1 --implementations dever --server-cpu 0 --client-cpus 1,2,3,4 --client-workers 4 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 64 --cpu-quota 1
python3 test/performance/run.py run --artifacts target/performance/typed-orm-build-v2 --output target/performance/http2-recovery-next --suite http2-recovery --duration 0.1 --cycles 100 --recovery-rate 1000 --recovery-connections 16 --repeats 1 --implementations dever --server-cpu 0 --client-cpus 1,2,3,4 --client-workers 4 --cgroup-parent /sys/fs/cgroup/system.slice --memory-mib 128 --cpu-quota 1
```

本轮原始结果位于 `target/performance/http2-ceiling-{scan,steady-128m-20k,steady-64m-20k,recovery}-09-18/`；28k 首轮及 24k/20k 校准在相邻命名目录。吞吐平台、20k 持续负载和 100 轮恢复是不同问题，不能互相替代；服务与负载仍共享虚拟机和热缓存，结果不外推到真实网络、小物理机或小时级运行。

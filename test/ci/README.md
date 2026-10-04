# Linux 质量门

`run.py` 仅串行编排既有 Cargo、Python、SDK、PostgreSQL fixture 和 performance 运行器；没有下载工具、全局安装或环境变量产品配置。根 `config/setting.json` 保留原有字段，在既有 `performance` 对象内增加 `linux_ci`，与 peer 路径同级。顶层 `linux_ci` 不受支持，会被 runtime 严格配置解析拒绝：

```json
{
  "performance": {
    "network_peer": "/absolute/network_bench",
    "live_peer": "/absolute/live_bench",
    "linux_ci": {
      "tools": {
        "cargo": "/absolute/toolchain/bin/cargo",
        "cargo_fmt": "/absolute/rustfmt/bin/cargo-fmt",
        "cargo_clippy": "/absolute/clippy/bin/cargo-clippy",
        "python": "/absolute/python3",
        "node": "/absolute/node",
        "go": "/absolute/go"
      },
      "cargo_config": [
        "profile.dev.package.miniz_oxide.opt-level=3",
        "profile.dev.package.flate2.opt-level=3",
        "profile.dev.package.sha2.opt-level=3"
      ],
      "output": "/absolute/repository/target/linux-ci",
      "command_timeout_seconds": 14400,
      "remove_test_executables": true,
      "cgroup_parent": "/sys/fs/cgroup/system.slice/dever-linux-ci.service",
      "inputs": [],
      "required_inputs": {},
      "author_outputs": [],
      "author_network": false,
      "soak": {
        "artifacts": "/absolute/prepared-protocols",
        "cms_manifest": "/absolute/prepared-cms/manifest.json",
        "seconds": 1800,
        "cms_concurrency": 16,
        "http2_rate": 5000,
        "rss_late_growth_mib": 8
      }
    }
  }
}
```

替换路径，勿直接复制示例运行。下文 CI 字段均位于 `performance.linux_ci`。`inputs` 列出固定原始作者输入及准备好的资产文件/目录，逐文件计 SHA-256；工具、协议产物、CMS manifest/可执行文件/配置和 `performance.network_peer/live_peer` 也自动计摘要，`linux_ci` 对象不作为 peer 路径解析。输出目录不得包含在 inputs 中。报告只存摘要，不复制数据库密码。

`required_inputs` 为 `author`、`native`、`postgres`、`sandbox`、`bounded-perf`、`soak` 各提供非空绝对路径列表，必须落在 `inputs` 声明范围内。缺路径记录 blocked，不算 pass。作者 fixture 的具体固定路径/准备命令仍以 `sdk/native-release.md`、`test/ecosystem-release/prepare.py`、`test/ecosystem-release/build_inputs.py` 和对应 ignored 原因文本为准；gate 不把现存文件冒充由当前源码重建的 provenance。

显式 LLVM Worker 验收读取 `target/native-release-inputs/component-fixture`：先用当前源码构建 `dever-tests` 的 `component-fixture` bin，再将程序复制到该准备目录，记录源码/程序摘要，并把固定副本列入 `inputs` 和 `required_inputs.native`。不能将 `target/debug/component-fixture` 当作不可变输入：Cargo 在 default/all-feature 组合间会重新链接这个输出。普通 Worker 测试仍使用 Cargo 为各自组合生成的 `CARGO_BIN_EXE_component-fixture`，不改质量门的输入变更拒绝规则。

`author_outputs` 精确声明 author 会制作的 pack、下载结果和 receipt 路径。author 阶段只将其余原始输入当作不可变输入，另外记录完整输出摘要；后续验收按准备后完整输入摘要绑定。不要把整个作者输入根排除掉。显式 `author_network: true` 才能执行已分类的固定官方下载/源码构建。新 ignored 必须人工更新 `ignored.json`；inventory 核对源码声明及实际 all-feature libtest 名单，执行时一律使用完整 `--exact` 名称。内部 probe 记录 parent-only，只由所属父用例运行。

资源准备归外部托管者：自有空父启用 memory/cpu，限制总 CPU<=2、memory<=4GiB、swap=0；runner/Cargo/负载客户端放 `control` 子组，自有 PostgreSQL 可放兄弟子组，64/128MiB 服务子组由现有 performance owner 创建。gate 只读检查现有父限制与自身归属，不创建/修改主机策略。作者工具所需固定 PATH/动态库路径可由外部托管者显式准备，不写入产品配置。

```sh
python3 -B test/ci/run.py plan
python3 -B test/ci/run.py run --stage static
python3 -B test/ci/run.py run --stage default
python3 -B test/ci/run.py run --stage all-features
python3 -B test/ci/run.py run --stage features
python3 -B test/ci/run.py run --stage inventory
python3 -B test/ci/run.py run --stage author
python3 -B test/ci/run.py run --stage python
python3 -B test/ci/run.py run --stage sdk
python3 -B test/ci/run.py run --stage native
python3 -B test/ci/run.py run --stage postgres
python3 -B test/ci/run.py run --stage sandbox
python3 -B test/ci/run.py run --stage bounded-perf
```

以上命令由同一受限托管进程串行执行。若 author 改变输入，应在作者准备完成后再跑正式静态/default/all-feature门；较早输入版本的证据会显示过期。每阶段写入新的独占时间目录，开始即记录 running；缺输入记录 blocked，失败命令保留 exit/count/日志与原始异常。`summary` 与 inventory 消费者只接受最新一次尝试的当前源码/输入证据，不能越过较新的失败或中断回用旧 pass；所有必选阶段通过才返回0。源码摘要覆盖未提交文件，不只看 Git HEAD。

default/all-features 由实际 Cargo metadata 注册 target 和已解析 feature 形成串行矩阵，覆盖 lib/bin/integration/doctest；default 显式保留 workspace 解析的 package features，最小 feature 阶段另按单 package 验证。只有显式启用 `remove_test_executables` 时才删除 Cargo 本次返回、`profile.test=true` 且位于 target 下 `deps/` 的测试可执行文件，失败阶段同样清理；不删除作者工具、静态 init、依赖 archive 或整个 target。删除对象在日志 JSON 中可审计，重跑 Cargo 可恢复。inventory 按 package/target/name 三元组核对完整性，跨 target 同名用例不能代替缺失项。

Python 阶段包含 `test/ci`、`test/performance`、`test/ecosystem-release` 三个目录；后两者必须已配置 `performance.network_peer/live_peer`，真实自有 loopback检查由主代理执行。三 SDK 路径全部必需，三个明确成功标志全部出现才通过。

PostgreSQL 只接受显式 loopback端口和恰好一个 `{case}` 库名模板，拒绝 query 覆盖 host/dbname 等。增加 `performance.linux_ci.owned_postgres: {"data_directory":"/absolute/owned-cluster","port":51234}`，由托管者创建所需基库并在 finally 停止该自有实例；gate复用现有用例的 schema/tenant 清理，不替用户启动或连接既有业务数据库。

长测复用原 runner，逐阶段分别执行：

```sh
for stage in cms-64 cms-128 http2-64 http2-128 recovery-64 recovery-128 live-64 live-128; do
  python3 -B test/ci/run.py run --stage "$stage" || exit "$?"
done
python3 -B test/ci/run.py summary
```

每个 CMS 源码/h2c/TLS/恢复协议/TCP/WS/SSE 子场景至少30分钟，服务在整个子场景保持同一进程。HTTP/2选代表 JSON 路径；其他body路径仍由短合同测试覆盖。live/recovery 使用60×30秒循环，受现有1小时生命周期限制；总默认矩阵约9小时加准备。HTTP/2要求零错误、成功>=99%计划请求；CMS逐回复验证实际发布列表。全部要求OOM=0、断连FD恢复。RSS记录原始采样和四段中位数，最后两段中位数增量默认不得超过8MiB；这是本轮可配置回归预算，不把 allocator保留直接等同泄漏。失效报告和失败负载保留，校准降速后须完整重跑。

CMS读取时长支持0.1..3600秒；长测客户端用固定2048桶统计延迟，不随请求数保存无限样本。分位数为对数桶上界，包含请求与结果核对时间。报告记录实际时长及统计方式，不与此前精确短测分位数直接混算。

CMS先关闭控制客户端，再记录FD基线；断连后按FD目标及重复数核对所有非数据库资源。负载前已确定的SQLite主文件/WAL句柄各不得超过配置的`max_connections`，同进程SHM句柄不得超过1。`descriptors_before`、`descriptors_after`和`pooled_descriptor_limits`保存实际依据；总`fd_delta_after_disconnect`保留为观测值，不把正常池扩容当成连接泄漏。每个源码目录的`report.json`在失败时也保存已完成的发布/读取数据、资源采样、预算及错误，总报告收集该失败case。HTTP/2/live无数据库池，继续使用原来的完整FD回落门。

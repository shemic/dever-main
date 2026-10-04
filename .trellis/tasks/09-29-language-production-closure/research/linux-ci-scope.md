# Research: Linux CI 与持续压力验收范围

- Query: 根据当前源码设计完整 Linux 质量门；区分默认合同、显式验收、作者准备与内部探针；复用现有长测 runner。
- Scope: internal；仅调查 `/data/project/dever` Rust 语言仓库，不适用 Go Dever 的 Model/Page/Service 开发流程。
- Date: 2026-10-02
- Task: 主代理明确指定本任务；child session 的 `task.py current --source` 返回 none，主代理确认父会话 active=in_progress，因此不创建或切换任务。

## Findings

### 代码与规范入口

| 文件 | 用途 |
| --- | --- |
| `Cargo.toml` | 六个 workspace member、Rust 1.95/edition 2024、dev/test 禁用 debug 与 incremental、release/runtime-pack 优化 |
| `crates/*/Cargo.toml`、`test/dever-tests/Cargo.toml` | target 与 feature 真实注册来源；不能按 test 文件名猜测 Cargo target |
| `crates/dever-backend-bridge/build.rs:12` | embedded 需要 `target/backend-sdk/usr/lib/llvm-18` LLD headers/static archives 与系统 LLVM-18 |
| `test/dever-cli-tests/support/llvm_inputs.rs:4` | GNU 作者夹具的 LLVM 18、glibc 2.39、GCC 13 路径与动态依赖闭包 |
| `sdk/native-release.md`、`sdk/native-release.rs` | 四 profile、作者打包工具、签名安装与三生态验收规范 |
| `test/dever-cli-tests/native_release/acceptance.rs:120` | 正式验收读取 `target/native-release-inputs/{base,sqlite,postgres,both}.a` |
| `test/ecosystem-release/{prepare,build_inputs}.py` | 固定摘要的显式作者输入准备；不是默认联网安装器 |
| `test/dever-tests/tests/support/postgres.rs` | `{case}` 数据库模板、唯一 schema/tenant 数据库、失败清理 |
| `test/performance/{run,build,process,live,cms,settings}.py` | 已有构建、隔离进程、cgroup、协议负载、CMS、报告 owners |
| `test/external-sdk/check.py:192` | 三 SDK wire/typed 合同，必须显式给绝对解释器路径 |
| `test/dever-sandbox-tests/isolation.rs` | 正式 sandbox 测试与内部 helper probe；不能全量 `--ignored` |

相关规范：`.trellis/spec/backend/index.md`、`quality-guidelines.md`、`directory-structure.md`、`error-handling.md`，以及 `toolchain-and-library.md` 的独立 runtime/embedded、作者输入与发行验收合同，`database-guidelines.md` 的隔离数据库合同。任务旧 PRD 仍含跨平台目标与历史“未完成”背景；本轮采用父代理转述的最新授权：完整 Linux CI/长测、AppArmor 非 root Worker；其他 OS 暂缓。历史 TODO 不是本研究的当前实现判断。

### 最小复用设计

建议只有一个根 `test/` 下的 Linux gate runner 和一个声明式阶段/ignored 用例分类表；`.github/workflows/` 仅负责安装确定工具/启动该 runner/保存日志，不复制测试矩阵。当前 `.github` 不存在。长测继续调用 `test/performance`，不另写负载发生器。每阶段保存命令、开始/结束、退出状态、实际测试数、输入指纹；失败要 fail closed，blocked/skipped 不能计入通过。

由 `cargo metadata --offline --locked --no-deps --format-version 1` 获取注册 target/required-features；从编译后的 libtest `--list` / `--ignored --list` 获得精确用例清单。审计分类表与清单的差集：新 ignored 必须明确分类；不能简单 `cargo test --workspace --all-features -- --ignored`。Python `unittest discover` 默认是否递归不能当覆盖证明，应分别指定现有目录。

### Cargo 覆盖与顺序

workspace 成员：dever-core、dever-runtime、dever-cli、dever-backend-bridge、dever-sandbox、dever-tests。

- core 只有 `reference`（启用 runtime crypto）；生产 CLI 不应启用它。
- runtime 默认空；独立能力 `auth-crypto,crypto,wire,external,api,database,sqlite,postgres`。上层依赖的 feature unification 会掩盖最小 feature 编译错误。
- bridge 默认空；`embedded` 与 `runtime-abi` 必须独立；其余 `runtime-database,runtime-api,runtime-external,runtime-sqlite,runtime-postgres`。`--all-features` 是合同覆盖，不是可交付 archive 的构建方式。
- dever-tests 默认空；`reference,sqlite,postgres,api,crypto,component,external`，许多 target 有 required-features。
- CLI：lib，deverc/dever/deverd 三 bin，llvm_cli/native_release/cms_project/shared_toolchain/external_libs/external_workers/packages 七 integration targets，native-release/native-acceptance-init 两个 `test=false` example。
- sandbox：lib/guard bin/isolation target。bridge 有 16 个显式 integration targets；dever-tests 设置 `autotests=false`，只测 manifest 注册者。`test/runtime_logging.rs`、安全 cases 等路径还可能被其他 target `#[path]` 引入，不能据未注册就判遗漏。
- Rust peers `network_bench/live_bench/async_bench` 是 dever-tests 自动 example；`--all-targets` 静态门检查它们，真实运行另由 Python 使用。

主代理统一 Cargo `-j1 --offline --locked`。建议阶段命令（并非本研究执行结果）：

```sh
cargo fmt --all --check
cargo clippy --offline --locked -j1 --workspace --all-targets -- -D warnings
cargo clippy --offline --locked -j1 --workspace --all-targets --all-features -- -D warnings
cargo test --offline --locked -j1 --workspace -- --test-threads=1
cargo test --offline --locked -j1 --workspace --all-features -- --test-threads=1
cargo check --offline --locked -j1 -p dever-runtime --lib --no-default-features
cargo check --offline --locked -j1 -p dever-backend-bridge --lib --no-default-features
cargo check --offline --locked -j1 -p dever-backend-bridge --lib --no-default-features --features embedded
cargo tree --offline --locked -p dever-cli --edges features
```

还需按上述 runtime 八个能力逐一 `cargo check -p dever-runtime --lib --no-default-features --features <feature>`；bridge 分别 runtime-abi/runtime-database/runtime-api/runtime-external/runtime-sqlite/runtime-postgres 与两驱动组合。不能在这些最小能力命令上加 `--workspace`，否则 workspace feature 合并消除隔离意义。四个正式 profile 构建可以覆盖相应 release 组合，但不能替代 runtime-only 最小能力。生产 CLI 的 feature-tree 检查不得从 all-features 测试解析结果推出 reference 不进入生产。

默认与 all-features 会有意重复部分合同，这是覆盖不同配置的证据；无需再对每个 feature 重跑全套功能测试。按 target 串行执行可以降低一次留存的链接产物，仍需记录全 workspace 默认和全特性两个集合的总结果，doctest/lib/bin 不能被 integration-only 循环漏掉。

### 作者准备与显式运行

SDK 输入必须先检查存在、版本与摘要：Rust>=1.95；LLVM/LLD 18；GNU x86_64 夹具固定 GCC 13/glibc2.39 位置；sandbox bwrap bind-fd/guard/loader 闭包；三生态固定发行与源码构建夹具。`prepare.py` 不负责下载，脚本常量固定 CPython3.12.14、Node24.15.0 下载档及 Go pack 摘要；以源码当前摘要为准。现存文件不证明由当前源码构建，记录 provenance。

独立正式 profile 用同一个 `target/native-runtime-abi`：

```sh
cargo build --offline --locked -j1 -p dever-backend-bridge --lib --profile runtime-pack --no-default-features --features runtime-api,runtime-external --target x86_64-unknown-linux-gnu --target-dir target/native-runtime-abi
```

分别增加 `runtime-sqlite`、`runtime-postgres`、两者；每次立即独立保存 archive 到 `target/native-release-inputs/<profile>.a` 后再构建下一个。禁止 all-features/embedded、把 both 复制四份、原地 strip 已硬链文件。许多 LLVM helper 另读取 `target/native-runtime-abi/debug/libdever_backend_bridge.a`，需要显式完整 runtime 测试 archive；不能以正式 profile 名义蒙混。构建 author example（sdk/native-release.md 给 miniz_oxide/flate2/sha2 opt-level3 配置）、三 CLI bin、guard、component-fixture、静态 native-acceptance-init，之后再运行验收。

正式 ignored 分类：

1. bridge native LLVM targets 中依赖 runtime archive 的源码执行、资源/异步/错误/HTTP/Job/API/外部 Worker 全部属于显式运行；`llvm_database_source` 与 `llvm_api_source` 含 PostgreSQL 分支，使用包含 runtime-postgres 的 target build。不能用 object-only/非 ignored 结果替代。
2. PG：`postgres_orm_uses_the_configured_isolated_database`、`postgres_authorization_preserves_site_scoped_roles_and_catalog`、`postgres_cms_http_isolates_two_tenant_databases`，及 LLVM API/database 的 PG ignored。用 `cargo test -p dever-tests --all-features --test postgres_orm --test postgres_api --test authorization_postgres -- --ignored --test-threads=1`，并运行 bridge 对应 targets 的 ignored。只有目标内全为验收用例时才整体 ignored。
3. `native_release` 的四个真实链路：`acceptance::optimized_profiles_make_reproducible_signed_release_and_run_through_daemon`、`acceptance::signed_core_builds_without_host_compiler_libraries_or_tools`、`acceptance::bootstrap::signed_bootstrap_installs_and_serves_two_projects_without_host_libraries`、`acceptance::ecosystems::signed_ecosystems_build_and_run_without_system_language_installations`（最终模块名请以 libtest --list 确认）。另外 bootstrap trust/crash-recovery ignored 必须执行。
4. `packages` 的 installed package check/test/run/build/standalone、managed_compile 的成功/不同 UID/取消清理；shared_toolchain 不同 UID；external_workers 的官方 Go/Python wheel/PEP517/npm、直接 Node/Python adapter 和独立运行；external_libs 的 managed lifecycle/越界/alias/PEP517 合同；native_cache 新鲜 CLI ignored。这些是显式验收，不是可忽略的“可选测试”。
5. sandbox 的 `enforces_files_network_process_and_threads`、`pins_grants_against_directory_replacement`、`rejects_writable_aliases_of_executable_inputs`、`prepared_python_and_node_run_without_host_files_or_process_creation`、`enforces_unprivileged_namespace`、`terminates_detached_descendants_on_kill`、`rejects_missing_dependency_before_host_loader_can_supply_it`、`kernel_rejects_namespace_ptrace_and_truncated_ioctl_requests` 精确执行。C syscall probe 需提前显式编译至 fixture 指定位置。
6. `performance` 两个 native kernel/large-map 基准、`concurrency` 调度基准是单独 bounded perf；真实长压测在下节，不互相替代。

不要直接运行内部 probe：isolation 的 `pinned_grant_probe/restricted_probe/tree_probe/descendant_probe/granted_probe`，packages 的 `installed_managed_package_worker_guard`。它们由父用例在签名/namespace 环境内 `--exact --ignored` 调用。

作者 preparation 用例单独列状态，避免无意联网与重复下载：external_libs wheel 官方 fetch、sumdb 官方 proxy fetch、build.rs/native-build-inputs、npm_build.rs/hashed 输入组装与官方源码构建；它们准备下游执行需要的固定锁定输入。已有内容摘要与合同匹配则复用，但真实 registry 解析/官方源码构建若属于本轮 CI 要求，应在显式 author 阶段执行一次，不能因为下游 fixture 已存在就声称该阶段通过。

### Python、SDK 与 PostgreSQL 假通过防线

```sh
python3 -B -m unittest discover -s test/performance -p 'test_*.py' -v
python3 -B -m unittest discover -s test/ecosystem-release -p 'test_*.py' -v
python3 -B test/external-sdk/check.py --python /absolute/python --node /absolute/node --go /absolute/go
```

performance 的 network/live 测试会启动自有 loopback；必须事先准备 release peers，并在根 setting.json 的 `performance.network_peer/live_peer` 配置路径。SDK checker 没给解释器时打印 unavailable 后继续，甚至三个都没给仍可零退出；CI 必须强制三个路径并断言三生态结果，不只检查退出码。

Rust PG helper `load_setting` 强制 `database.postgres_test` 和数据库名中恰好一个 `{case}`，拒绝 query dbname 覆盖；schema 用唯一 PID/time/counter，tenant 用本轮唯一前缀并仅删除规范正整数 ID。schema fixture 不创建基础数据库，需提前准备 `postgres_orm/authorization_postgres/llvm_database` 等每个调用 case 所需数据库；tenant fixture 会创建控制库和租户库，配置角色需要对应权限。全部连接必须指向本次自有 PostgreSQL instance。owned cluster 在 runner finally 停止并清理；失败原始信息与清理错误都要保留。

性能 `build.load_postgres_setting` 没配置会返回 None，run orm 自动退化为 SQLite-only；CI PG 阶段先检查配置，报告必须包含 postgres driver。其 Python URL validator 没有 Rust helper 的 query dbname 禁止规则，推荐 fixture 创建者只输出固定安全 URL，不接受任意外部 override。

### 长压测：可直接复用的命令与界限

构建 `run.py build --selection protocols` 准备 protocols，`--selection cms-profiles` 准备双源码 CMS/四 profile；ORM 需 all 构建并已配置 PG。Rust peers 单次 release 构建后复制指纹，run 阶段不运行 Cargo。记录 `application_configuration=llvm-cmd-v1`、`peer_configuration=setting-json-v1` 与所有实际二进制 hash；旧 manifest 被拒绝。

下面每组分别在 memory=64、128 执行，output 用新的独占目录；`CGROUP` 必须是已经委派且有 memory controller 的自有父组。每个子场景是真实持续至少1800秒。CPU affinity 从允许 CPU 集合选，server/client 不重叠；没有隔离不声明独占 CPU。

```sh
python3 test/performance/run.py run --artifacts "$ARTIFACTS" --output "$OUT_HTTP" --suite http --implementations dever --duration 1800 --warmup 5 --rates 1000 --paths /plain,/json,/bytes --repeats 1 --concurrency 16 --connection-mode reuse --memory-mib "$MEMORY" --cgroup-parent "$CGROUP"
python3 test/performance/run.py run --artifacts "$ARTIFACTS" --output "$OUT_H2" --suite http2 --implementations dever --duration 1800 --warmup 5 --rates 1000 --paths /plain,/json,/bytes --http2-connections 1 --repeats 1 --memory-mib "$MEMORY" --cgroup-parent "$CGROUP"
python3 test/performance/run.py run --artifacts "$ARTIFACTS" --output "$OUT_RECOVERY" --suite http2-recovery --implementations dever --duration 30 --cycles 60 --recovery-rate 1000 --recovery-connections 1 --repeats 1 --memory-mib "$MEMORY" --cgroup-parent "$CGROUP"
python3 test/performance/run.py run --artifacts "$ARTIFACTS" --output "$OUT_LIVE" --suite live --duration 30 --cycles 60 --connection-tiers 32 --repeats 1 --interval 0.1 --memory-mib "$MEMORY" --cgroup-parent "$CGROUP"
python3 test/performance/cms.py run --manifest "$CMS_MANIFEST" --output "$OUT_CMS" --concurrency 16 --read-seconds 1800 --memory-mib "$MEMORY" --cgroup-parent "$CGROUP"
```

1000 req/s 是保守起点而非宣称性能阈值；先独立短时 calibration，长测要求 errors=0、OOM=0、非零成功，建议 >=99% scheduled 成功作为该次目标负载验收，失败需降低目标后重新完整跑并保留失败数据。不要把历史 20k 短测当当前机器长测门槛。

runner 当前 profile lifetime=3600000ms（build.py:121），live duration 限制0.3..60、cycles<=100（run.py:1161）；所以 live 不能传1800，可用60×30秒的真实连续同一进程生命周期。HTTP2 recovery 同理，周期总量需小于1h。HTTP/H2 各两种 transport×3 paths，64/128 合计各6小时；recovery 两 transport 两预算约2小时；live 三协议两预算约3小时；CMS 双源码两预算约2小时。以上完整路径矩阵约19小时加准备；若只选一个代表 body 路径，HTTP/H2合计缩为4小时，总约11小时，但必须显式记录未长测另两路径，短合同测试继续覆盖它们。

`run --suite cms` 只顺序发布16篇，与 --duration 无关；长测用独立 cms.py 的 read-seconds。`memory` parked fixture 只有3秒窗口，`async` 与 ORM CRUD/list/cursor/stream/pool 是有限操作，给 duration1800也不是长测。ORM 的 HTTP entry 使用 duration 可测30分钟，但有限项需在同一进程内重复 workload 才证明长期资源稳定；单纯外层重复短进程会掩盖泄漏。现有 CMS postgres-case 是验收，不自动成为长压测。若用户要求这些也长测，需在现有 runner owner 上增添持续模式，不冒称现有参数已覆盖。

资源/失败判定：run.py server context 最终检查 cgroup OOM；HTTP/2 `observe_disconnected` 要求 FD 精确恢复；live.py 验证 phase 顺序、连接计数、payload 和 heartbeat，run.py:627 当前只记录 FD/RSS delta，不强制 delta=0。必须加报告 gate 或局部 owner 校验 live FD 恢复。RSS 保存每周期基线、断开后值、末段范围/趋势；allocator 稳定保留不等于泄漏，不要求绝对回到首次 RSS，但持续增长、OOM、资源无法回收应判失败。完整进程组 terminate/reap、cgroup 清空/删除、失败也保留 cases.jsonl/report 已完成部分；还应验证控制器缺失明确失败、错误 payload 失败、超时/中断清理的测试。`status=complete` 不单独代表无错误。

### 2.1 GiB 空间下的串行策略与耗时

只读 df 已确认根分区可用2.1GiB。单个 Cargo -j1 控制编译并发但不限制产物总量；不要承诺完整 all-targets 全量链接可装进2.1GiB。优先共享当前 target/dependency cache，先静态，再默认/全特性按 target 执行并保存结果，只清理本轮可重建的精确输出，不能删作者输入、其他任务缓存或整个 target。任何安全清理计划由主代理解析实际路径/所有权后实施。

原生 ABI 四次 release 编译共享一个 target，保存成品后再切 feature；大三生态/安装验收串行且不和 Cargo/PG压力同跑。sdk/native-release.md 已支持 `target/native-release-inputs/config/setting.json` 的 `machine_temporary_root`，只移动 owned image/machine；可选较大且可执行的文件系统。若用 /dev/shm，必须同时预算真实 RAM/该进程 cgroup，不能把磁盘问题变成宿主 OOM。报告只需小量 JSON/日志；live 0.1s采样每30分钟约18000点，多 raw 副本占用需计入预算。run.py stage_case_binary 对 Dever 每case复制，settings.stage_peer 对 immutable peer用硬链；不要假定所有 binaries 都已零复制。

耗时只能估计：已准备依赖的静态/合同门数十分钟至数小时；冷四profile fat-LTO每个数分钟到数十分钟，必须实际记录；三生态重打包/源码构建数分钟至数十分钟，临时空间数GiB；19小时长测是按明确场景串行计算的下限，不能用并行负载污染CPU测量或拿超短烟测替代。新增 runner 可提供阶段选择用于重跑失败阶段，但 full gate 必须验证所有必选阶段拥有同一源码/输入版本的通过证据。

### 主代理补充限制：总2 CPU / 4GiB

`process.py:47` 的 Budget 接受任意显式 parent，在其下建唯一子组，并设置 memory.max/swap.max/cpu.max；`Process:105` 只把自有被启动 child迁入所给Budget。现有 server 的预算仅覆盖服务，HTTP/live负载端没有server Budget，默认继承 Python runner 的cgroup。因此支持统一上层限额，但必须由外部CI入口先把 runner/作者准备/Cargo/自有PG都放入共同的4GiB/2 CPU父组，不能只传server的 `--cpu-quota 2` 冒称全局2 CPU。

推荐结构：owned parent设置 `memory.max=4294967296`、swap.max=0、`cpu.max=200000 100000`，空父组启用memory/cpu controllers；其下 `control` 放CI/Python/Cargo/负载端，自有 `postgres` 放PG，再传该空父组给performance让其创建server64/128MiB兄弟组。这满足cgroup v2 no-internal-process约束；不要把runner放在已启用domain controller且还要建子组的父组本身。所占CPU可在允许affinity里挑2个，server/client各1个，总quota仍统一2。PG与server受父级内存合计限制，/dev/shm作者验收同样计入4GiB；必须预估并保留headroom，不能假设搬到tmpfs无限可用。

主代理报告格式门已实际失败，`target/linux-ci-initial-format.log`有1174处Diff位置（不是1174个文件）。这是待实施的格式修复前置证据；本research未亲自执行格式化，不把其当通过。主代理可将HTTP2正式稳定负载选5000/s并先做短calibration；长测仍需完整30分钟，不能宣称该速率是吞吐上限。

## Caveats / Not Found

- 本研究未运行任何 Cargo、编译、测试、服务、下载或 Git 命令；上述命令是实现建议，不是执行证据。
- 系统 AppArmor 实际策略与非 root Worker 修改由另一工作流负责；现有 `enforces_unprivileged_namespace` 在root时降为65534，只证明当前 host策略下该夹具的执行，不等价正式部署 profile 的路径覆盖。禁止以 root pass替代非root；禁止通过全局关限制获得绿色。
- CI 配置当前不存在；作者/fixture paths有明确 Linux GNU x86_64/GCC13/glibc2.39 依赖，不能把普通ubuntu-latest或跨平台可用当事实。
- ignored 全量精确模块名应由实际 libtest inventory确认；本文件列源码 owners和明确名称，不替代机器可审计清单。
- PG、SDK缺配置、finite runner duration未生效、内部probe被错误执行、仅all-features验证profiles，是主要 false-pass 风险。

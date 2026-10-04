# Dever 应用测试体系实施清单

## Start Gate

- [x] 用户在最终规划摘要之后明确批准实施。
- [x] 运行 `python3 ./.trellis/scripts/task.py start 09-19-application-testing`，确认任务进入 `in_progress`。
- [x] 重新加载 `trellis-before-dev`，核对当前 PRD、设计、执行清单和 backend specs。
- [x] 开工前记录实际 git/worktree 状态，只处理本任务拥有的文件，不清理用户已有改动。

## Phase 1: Test Source Layout And Static Contract

- [x] 在 `source.rs` 增加编译器赋予的 Test 来源、显示/逻辑路径和组合项目加载；保持现有生产 `SourceMap::load` 合同。
- [x] 在 layout/parser/checker 中加入严格的 `<component>/<domain>/<topic>` Test 布局，不增加源码关键字或第二套 parser。
- [x] 在检查结果中建立按身份排序的 `TestCase`，强制同名 ordinary 零输入零输出入口和文件内私有辅助声明。
- [x] 扩展 role-aware function/type resolution：同领域 App+Domain、跨领域 App、禁止 Model CRUD/Adapter/Port/API/其他测试。
- [x] 把来源/逻辑路径加入 check cache identity，防止 Test 与普通 strict source 误复用。
- [x] 增加路径、入口、重复身份和可见性的定向正反测试。

## Phase 2: Assertions And Native Entry

- [x] 复用 `Intrinsic` HIR 形状加入仅 Test owner 可构造的 `assert`/`assert_eq`，保持普通同名业务函数语义不变。
- [x] `assert` 强制 Bool；`assert_eq` 强制相同 comparable 类型并保持左右各求值一次、源顺序不变。
- [x] 在 native emitter 生成源码定位的 assertion failure，复用现有 `Render` 输出 actual/expected。
- [x] 增加从 checked `TestCase` 编译私有入口的 native API，不放宽生产 `Program::entry` 的公开性要求。
- [x] 验证标量、record、choice、nullable、List、Map、MapEntry、Model ID，以及不可比较/类型不匹配/生产源码调用断言的拒绝。

## Phase 3: CLI Runner And Isolation

- [x] 在 CLI 增加 `test <project-root>`，复用现有 module 检查、warning 和 API baseline 路径。
- [x] 建立单一 `test_runner` owner，负责排序、单套件 compile、逐用例独立进程、输出捕获、状态汇总和临时目录生命周期；不创建通用 runner/framework 层。
- [x] 无测试时输出 `0 tests` 成功；源码检查失败时零执行；单用例失败后继续后续用例。
- [x] 对无数据库 effect 的用例使用 base profile，不读写 setting。
- [x] 对有数据库 effect 的用例生成临时 SQLite setting，复用正式 Settings 校验、Model schema/Seed/transaction/shutdown。
- [x] 每用例使用独立临时项目和数据库；确认项目 `config/setting.json`、`data/` 与环境变量不参与数据库选择。
- [x] 失败时分区展示捕获的 stdout/stderr，runner 状态行不与测试输出混合。
- [x] 覆盖编译失败、启动失败错误映射、断言失败、未捕获业务失败、runtime fault 和清理。

## Phase 4: Formatter And Maintained CMS

- [x] 调整 CLI formatter，使 module/test 两个可选源码集合先完整诊断和 prepare，再统一 commit；任一失败时零改写。
- [x] 为 CMS Dever 项目增加 `test/<component>/<domain>/<topic>.dever` 业务用例。
- [x] 为 CMS Markdown 项目增加相同路径、入口和合同的 `.dever.md` 用例，遵守现有 Markdown 文档协议。
- [x] 删除 CMS CLI 回归中被正式应用测试替代的临时 probe 源码注入，仅保留仍验证 build/package 的独立职责。
- [x] 更新 `LANGUAGE.md`、`MARKDOWN-SYNTAX.md`、根 README 和 examples README 的正式测试合同与命令。

## Phase 5: Refactor And Verification

- [x] 检查 SourceMap 双根加载、test metadata、assert lowering、runner 报告和 temp ownership 是否存在重复 owner 或平行逻辑。
- [x] 搜索并删除调试输出、过渡 API、无用 helper、死分支、临时测试文件和过期 CMS probe。
- [x] 确认生产 source set、API snapshot、main entry、native cache、runtime profile 与无测试项目行为不变。
- [x] 检查磁盘空间后再运行会链接原生程序的定向测试；不运行 workspace 全量测试、真实 PostgreSQL 或性能压测。

## Verification Record

- `cargo check --offline -p dever-core --features reference -p dever-cli -p dever-tests --test application_testing`：通过。
- `application_testing`：8/8 通过；包含 plain/Markdown 发现、可见性、类型、`result(call)`、复合断言、源码位置及含未引用 Model 的 base profile。
- `formatter`：12/12 通过；module/test 统一 prepare 和只读失败零改写通过；两套 CMS `fmt --check` 通过。
- `source_architecture`：8/8 通过；`markdown_source`：20/20 通过。
- `source_visibility`：10/10 通过；`contract_execution`：9/9 通过；`api_routes`（`api` feature）：7/7 通过。
- `cms_project_checks_runs_and_builds_from_an_isolated_copy`：通过；Dever/Markdown 各连续测试两次，部署 setting 被替换为非法 JSON 时测试仍为 2/2，通过后 check/run/build/打包程序仍通过，且无效 test 源码不进入生产命令。
- CLI 零测试、失败继续/输出分区、成功/失败临时目录清理：分别通过；`RUSTC=/bin/false` 定向故障注入确认两个编译失败均汇总且零临时残留。
- 未捕获业务失败和动态数值 fault 的既有原生定向用例：通过。
- `git diff --check` 和变更文件尾随空白扫描：通过；CLI 默认依赖仅启用 `dever-core` default feature，不含 reference。
- `rustfmt`、`clippy` 组件未安装，未运行；未运行 workspace 全量测试、真实 PostgreSQL、性能/内存压测或持久服务。

## Phase 6: Test Suite Compilation Performance

- [x] 在 native owner 中生成一个包含全部 checked TestCase specialization 的测试套件，不增加解释器或动态注册表。
- [x] 复用 native 构建缓存并使用测试专用快速编译参数；生产 `run/build` 优化参数保持不变。
- [x] CLI 只编译一次套件，按稳定用例编号逐个启动独立进程并保留失败继续、输出捕获和临时目录清理。
- [x] 混合数据库/非数据库套件仅在数据库分支加载临时 SQLite 配置并初始化 Model。
- [x] 增加多入口分派、一次编译、缓存命中、配置隔离和失败继续的定向回归。
- [x] 记录 CMS 冷/暖执行耗时和峰值 RSS；不把本机数据外推为生产性能。

### Phase 6 Verification Record

- `cargo check --offline -p dever-core -p dever-cli -p dever-tests --test application_testing --test native_cache`：通过。
- `application_testing`：8/8 通过；一个 native suite artifact 按两个稳定编号执行成功/失败入口，并确认第二次编译命中 native cache。
- `native_cache`：6/6 通过，另有 1 个既有显式 ignored；runtime 内容变化、工具链/参数变化、并发发布、损坏产物拒绝、原始快照与元数据摘要备忘均通过。
- CLI 定向回归：混合数据库/非数据库套件 3/3 通过；失败后继续与 stdout/stderr 捕获通过；零测试不调用编译并成功；套件编译失败时 2/2 用例统一标记失败并清理临时目录。
- 普通单入口 native hello 定向用例通过，确认 `run/build` 共用的生产发射路径未被套件抽取破坏。
- CMS Dever 实际 `deverc test`：2/2 通过。稳定 runtime 下首次单套件链接为 21.37 秒、峰值 346068 KiB；随后内容缓存命中为 1.79 秒、峰值 54132 KiB。优化前同项目为 119.40 秒、594772 KiB。
- 在 Cargo 测试环境的绝对 `RUSTC` 与普通 PATH `rustc` 之间切换会触发 Cargo 重建 native runtime；一次 75.56 秒、594976 KiB 的观测包含该重建，不作为测试套件链接基线。
- runtime 摘要目录只保留当前元数据身份；`/tmp` 无 `dever-application-test-*`、`dever-build-*` 或 `dever-cms-project-*` 残留，CMS 示例无数据库或测试可执行文件残留。
- `git diff --check` 与变更文件尾随空白扫描通过。`rustfmt`、`clippy` 组件仍未安装；未运行 workspace 全量测试、真实 PostgreSQL、64/128 MiB/cgroup 压测或持久服务。

## Focused Validation

允许且计划执行的最小检查：

```bash
cargo check --offline -p dever-core -p dever-cli
cargo test --offline -p dever-tests --test application_testing
cargo test --offline -p dever-tests --test source_architecture
cargo test --offline -p dever-tests --test markdown_source
cargo test --offline -p dever-cli --test cms_project
git diff --check
```

如 `rustfmt`/`clippy` 组件可用，再执行：

```bash
cargo fmt --all -- --check
cargo clippy --offline -p dever-core -p dever-runtime -p dever-cli -p dever-tests --test application_testing -- -D warnings
```

不运行：

- workspace 全量测试；
- `cargo build` 或应用全量发布构建；
- 真实 PostgreSQL；
- 64/128 MiB、cgroup 或吞吐压测；
- 持久 HTTP/网络服务。

## High-risk Files And Review Points

- `crates/dever-core/src/source.rs`、`check.rs`、`check/layout.rs`、`check/symbols.rs`：来源身份和可见性不能泄漏到生产。
- `crates/dever-core/src/check/expressions.rs`、`intrinsic.rs`、`native.rs`、`native/intrinsics.rs`、`native/records.rs`：断言必须静态类型化、单次求值并保留源码位置。
- `crates/dever-core/src/native/build.rs`、`model.rs`：私有测试入口不能放宽生产 entry，SQLite profile 必须来自测试生成配置。
- `crates/dever-cli/src/main.rs`、`format.rs`、新增 runner：临时目录、输出捕获、失败继续和格式化原子性。
- `examples/cms/{dever,md}`：两套测试合同必须同步，不给生产 App 增加测试专用能力。

任何实现如果需要 Test 关键字、共享动态值、生产公开测试函数、环境变量数据库配置或真实 PostgreSQL，均视为超出当前设计，回到 planning 复核。

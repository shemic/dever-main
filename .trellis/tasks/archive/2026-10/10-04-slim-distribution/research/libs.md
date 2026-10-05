# Research: Lib 按需准备与精确锁恢复

- Query: 追踪 Lib、Worker、Package 的准备/离线消费路径，设计 `dever lib install <root>` 与扩展管理器接点。
- Scope: internal；仅源码阅读，无测试、构建、下载、Git 操作。
- Date: 2026-10-04

## Findings

### 归属与已读规范

实际研究仓库为 `/data/project/dever`，独立 Rust 语言编译器，不是 `/data/project/shemic` 的 Go 应用。已读仓库 `AGENTS.md`、`.trellis/workflow.md`、当前 `prd.md`、backend index 与 toolchain-and-library 中目标、生态、锁、缓存和 Worker 合同，以及仓库 `skills/SKILL.md`。全局 shemic-dever/framework reference 描述 Go 项目，不能覆盖本仓库事实。

`task.py current --source` 返回 none；输出目录由主代理明确指定，未建立/修改会话指针。

### Files found

| 文件 | 作用 |
| --- | --- |
| `crates/dever-cli/src/libs.rs` | 锁结构、显式命令、已安装 runtime 读取、校验与离线嵌入 |
| `crates/dever-cli/src/libs/mutation.rs` | 稳定 inode 的项目依赖互斥锁 |
| `crates/dever-cli/src/libs/registry.rs` | 官方源安全传输、PyPI/npm/Go 解析及产物缓存发布 |
| `crates/dever-cli/src/libs/registry/npm.rs` | 独立 npm 环境、精确实例图、归档展开与需构建判断 |
| `crates/dever-cli/src/libs/registry/sumdb.rs` | Go 已锁透明日志证据与 ZIP 校验 |
| `crates/dever-cli/src/libs/build.rs` | 构建 Inputs、Session、完整 Receipt、离线收据校验 |
| `crates/dever-cli/src/libs/build/python.rs` | sdist 解包、PEP517 build 依赖解析与隔离执行 |
| `crates/dever-cli/src/libs/build/npm.rs` | npm 生命周期执行、optional 剪枝、最终安装归档 |
| `crates/dever-cli/src/libs/build/npm/environment.rs` | Node/Python/native 工具与 sandbox 环境 |
| `crates/dever-cli/src/workers/runtime.rs` | maker/consumer 共享 runtime descriptor 校验 |
| `crates/dever-cli/src/workers/go_build.rs` | 已准备 host Go 工具编译 target Worker |
| `crates/dever-cli/src/packages.rs` | Package 解析、精确 ZIP、源码/Worker/Lib 汇入公共锁 |
| `crates/dever-cli/src/toolchain/service.rs` | daemon ArtifactGet/Put IPC |
| `crates/dever-cli/src/toolchain/launcher.rs`、`src/main.rs` | 选版本与公共参数路由 |

### 现有调用链与准备入口

1. `libs::execute_for_target` (`libs.rs:1870`) 在 add/update 中先读声明、`source_workers`、Package libs，再 `resolve_project`，最后写 setting/lock。不能将 install 实现为 add/update 别名。
2. `source_workers_with_packages` (`libs.rs:2173`) 加载源码、check，再 `bound_program_workers` (`1236`)。后者立即调用 `InstalledRegistry::runtime` 绑定每个非 exec Worker 的 runtime。因此只在 `resolve_project` (`612`) 加 ensure 太晚：全新精简安装会先失败。无 Lib 的 managed Worker 也需要 runtime。
3. `resolve_project` 为实际生态调用 `InstalledRegistry::runtime` (`648`)，创建 `HttpRegistry::official`、sumdb、`build::Session`，交给 resolver。fixture 全 exec 分支完全离线，不应强制要求 managed 安装。
4. `packages::execute_locked` (`1109`) 的 add/update 解析 Package closure，然后 `source_workers_with_packages` (`1173 附近`) 和 `resolve_project` (`1193 附近`)。Package 的传递 Lib 与 Worker 必须享有同一显式准备能力；remove 则 `retain_locked_libs`，不得顺便安装扩展。
5. `prepare` (`libs.rs:1139`) 名称易误导：它只校验声明是否在锁中并统计大小，不下载、不校验缓存完整性。
6. `embedded_resources_for_target` (`1268`) 在离线路径核对 runtime identity、目标、每个归档摘要/长度和 Go sumdb，再把字节作为资源。`prepare_resources` (`1418`) 追加目标 sandbox，Go 另外需要 host sandbox，随后 `workers::prepare_for_target`。
7. `workers/go_build.rs:61` 的 `compile_worker` 是离线构建；Go 编译器/链接器/selector/target stdlib 目前在 Go **runtime pack** 中，非 `build/go` 原生扩展包。保持 host 工具 + target 输出区分。

### 扩展管理器最小 API 接点

主代理提出使用 daemon 可写的不可变 `cache/extensions`，catalog 绑定已签名核心。Lib 层可保留当前逻辑路径，无需另一份生态业务实现：

- `ensure_extension(version, kind, target)`：仅显式准备阶段调用；kind 至少 runtime(pip/npm/go)、build(pip/npm)、target。目标 extension 包含 native runtime/target sandbox；生态 target runtime 单独选取。
- `read_verified_extension_artifact(version, kind, target, relative)` 与有界 artifact listing：只读验证已安装 extension，返回缺资源的准确准备命令。`InstalledRegistry::runtime` (`791`)、`build::Inputs::tools` (`726`)、`sandbox_resources` (`839`) 共用这个验证 owner，避免保留各自手写签名读取。当前 `signed_artifact` (`817`) 仅从 core manifest.artifacts 查叶子，需切换到合并后的已验证资源读取。
- 显式 preparation wrapper 实现 `build::Inputs`，runtime/tools 可先 ensure 再读。普通 InstalledRegistry 永远只读；或者使用显式、不可隐式默认的 preparation policy，并沿 worker 绑定路径传递。
- `source_workers_with_packages` 保持只读供 doctor/remove/run/build；新增同一底层源码扫描的显式准备入口，先收集未绑定 Worker，再 ensure、绑定。Package add/update 必须走该入口。不要复制 SourceMap/check 逻辑。
- npm build environment 自身请求辅助 Python (`build/npm/environment.rs:57`)；按需准备需覆盖此调用，不能只看顶层 npm 声明。
- cross target 的本机 runtime、sandbox 与目标资源由类型化 target 选择，不能全局修改 host identity。

### 何时真的需要 build extension

- Python：优先兼容 wheel；只有进入 `RegistryResolver::python_source_candidate` (`registry.rs:933`) 的 sdist 分支才调用 Session.python。`Environment::new` (`build/python.rs:239`) 读取 build/pip；提前为每个 pip 根都安装 build/pip 不合理。
- npm：`Session::npm` (`build/npm.rs:15`) 先展开户籍归档，`npm::requires_build` 为 false 在第 43 行前后返回。只有确需生命周期/native 检查才创建 Environment，读取 build/npm。
- 当前 npm 的任何需执行 hook 路径都加载 Python + native build pack；当前 Python 的任何 sdist 也加载完整 build/pip。若 PRD 的“确有必要”要求进一步区分纯 JS hook / 纯 Python sdist 与 native 编译，需要收紧现有 environment 合同，而不仅加 lazy ensure；不能声称已做到逐个原生编译器按需。

### 当前锁记录什么、未记录什么

| 记录 | 源码依据 | 能力/缺口 |
| --- | --- | --- |
| LockedArtifact | `libs.rs:238` | target/path/bytes/sha256；path 是生成的逻辑路径，不是下载 URL |
| RuntimePack | `libs.rs:247` | name/version/sha256；运行时元数据从已签名版本读取 |
| LockedLib | `libs.rs:255` | 精确 spec、依赖、runtime、artifacts、schema、build receipt identity |
| LockFile | `libs.rs:267` | libs/workers/go_sumdb/npm/builds/packages；decode 要求规范字节与当前 TOOLCHAIN (`353`) |
| npm Environment | `registry/npm.rs:73` | 每个 Worker 独立 roots、实例路径、owner archive/prefix、edges、omission、build id |
| build Receipt | `build.rs:52` | roots/sources/graph/runtime/aux runtime/tools/frontend/Python backend/dynamic deps/递归依赖锁/output |
| LockedPackage | `packages.rs:75` | name/version/zip sha256/bytes/精确依赖/manifest sha256；来源在 setting 中已有 registry |

普通 PyPI wheel filename/URL 在解析后丢失（`registry.rs:905`）；npm dist URL/SRI 未持久化（`988`）；Go ZIP 地址可由已锁 spec 按现有 go_path 规则导出（`1121` 附近）。不改锁格式的恢复可查询**已锁精确版本**的 metadata 并仅选匹配锁 SHA/长度的文件，绝不重新选择版本或依赖；若文件撤下或变字节，明确失败。下载仍通过现有官方源/redirect/长度策略。

构建产物不是远端 registry 文件，必须优先从共享 cache 恢复；缺失时用固定 receipt 重放并比对锁定输出 SHA/长度，不能把新输出写回锁。

### 锁恢复的完整最小流程

1. install 参数禁止 specs，保留明确 target 选择。读且保存原始 setting/lock 字节；无锁立即报错；`LockFile::decode` + `doctor` 校验。将 install 纳入 `ProjectMutation::for_command` 的稳定互斥锁 (`mutation.rs:13`)，不调用 setting 或 lock writer。
2. Package 必须先恢复：从已有 registry 的 `/v1/packages/<name>/<version>.zip`（`packages.rs:467` 的现有格式）取**锁定** ZIP，以 lock 摘要/长度验证并发布共享 artifact cache；不读版本索引、不选版本。然后复用 `owned_files_with` (`726`)、`validate_locked_roots` (`811`) 核对 manifests、依赖、源码归属和本地冲突。
3. 收集 config Lib、Package manifest Lib、源码 Worker 合同，验证锁 roots/closure；现有 `prepare` 只查声明包含性，不拒绝无主锁 libs。可借 `retain_locked_libs` (`2072`) 的闭包投影与原锁比较（不发布），同时核对 workers；注意 npm receipt 的原始图与 remove 后视图允许不同，不误判合法离线 remove 结果。
4. 仅为锁中实际 runtime 及 Worker ensure 对应版本/target 扩展，然后比对 RuntimePack；目标错配先报错，不替用户切换锁。对 fixture 条目复用内置字节与完整 metadata 校验。
5. 实现 `restore_locked` 的精确归档恢复，接受 `RegistryTransport`、`ArtifactStore`、build Inputs，便于 fixture 注入。按 SHA/bytes 查缓存，真实 miss 才取精确版本 metadata/归档。Go 使用锁中 `go_sumdb` 验 ZIP，不调用新的 MVS/checkpoint 解析。
6. 对缺失的构建产物递归恢复 receipt.sources 和 receipt.dependencies；验证 frontend、tools、runtime identity；通过同一个 sandbox/environment 实现固定重放。Python 重放使用锁定 backend/static+dynamic 依赖，不调用 `build_requirements`；npm 使用锁定实例图与已记录 optional 结果，不再进行 semver 解析。
7. 恢复后复用 `embedded_resources_for_target` 验最终产物，确认 setting/lock 原字节未变。任何失败保留已存在可用数据；新增正确 cache 项可留作重试复用。

### 必须正视的重放缺口

- `Session::python` (`build/python.rs:37`) 在 73/94 行调用 resolver.build_requirements，直接重用会重新解析，违反 install 合同。应提取固定依赖的执行阶段，供首次解析和锁重放共用。
- Python receipt source 被写成 `build/source/<hash>/source` (`python.rs:144`)，没有 filename/格式；`source_files` (`196`) 需要 `.tar.gz` 或 `.zip`。可用 receipt 根的精确 PyPI metadata 按 source hash 找回文件名，但若 metadata 不再包含该源，必须报缺失。
- npm `sources` 在 optional 构建失败剪枝**前**采集 (`build/npm.rs:46`)，receipt.graph 在 `prune_failed` **后**保存 (`72/87`)。失败节点 spec/owner 可能已丢，sources 只有散列路径，故当前 receipt 不保证可以完整重放原环境；不能凭摘要路径猜原包名。
- 若需要对新生成锁完整恢复，应在首次解析时保存 source locator/filename 与 npm 原始执行图（不同于最终消费图），并在 doctor/identity 中校验。不要添加旧格式兼容分支；按仓库“新语言直接迁移合同”规则迁移 fixtures/callers。已有锁缺最终产物且缺恢复信息时，明确说明不可恢复，不能静默 resolve。
- 即使输入齐全，任意第三方 hook/wheel 打包不保证字节确定性。重放输出必须匹配原 SHA，否则明确失败；没有远端保存原产物的机制就不能保证所有第三方构建永远能在全新机器复现。PRD 的精确恢复应以这一合同验证，不能以“版本相同”代替“字节相同”。
- `ManagedArtifactStore::get_exact` (`libs.rs:893`) 调 `artifact_get(...).map(Some)`；当前 miss、损坏、IPC 失败都 Err。`service.rs:327/515` 未提供 typed miss。必须增加明确 absence 结果或专用有界 existence/read 协议，不能 `Err => 下载`、不能匹配错误文本。缓存损坏和 service 不可达应继续失败。

### target add 与 CLI

`main.rs:435` 的 Lib operation 白名单和两个 USAGE 都需 install；launcher `project_root` (`launcher.rs:174`) 已识别任意 lib operation，可原样按项目版本转发。

`dever target add linux-aarch64` 没有 root 参数，应在 launcher dispatch 使用当前选定的 active 版本并调用 extension owner；若期望 cwd pin，则需要与主设计明确一致，不能悄悄扩大既有 project_root 行为。它仅准备 target runtime/sandbox；第三方 ARM Lib 仍 `lib ... --target linux-aarch64`。

Python sdist/npm hook 在非 host target 上由 `build::require_execution_target` (`build.rs:13`) 明确拒绝，需要匹配隔离执行环境或完整目标产物。target add 不能解除这个约束。Go runtime 则支持 host tools + target stdlib。

### 最近的测试夹具（本次未执行）

- `test/dever-cli-tests/external_libs.rs:331/400/418`：LocalRegistry、FixtureArtifactStore、三生态精确归档；`:451` 目标选择；`:1333` 离线确定性；`:1397` setting 不变；`:1469` 错配配置；`:1508` Worker 合同；`:1649` 无 Lib Worker。
- `test/dever-cli-tests/external_libs/build.rs:241`：不兼容 sdist 在装工具前拒绝；`:260/359` backend fixture / PEP517 hooks；`:422` 官方 sdist 为显式重测试，不默认运行。
- `test/dever-cli-tests/external_libs/npm_build.rs:68`：完整固定 receipt；`:264` 不重跑 hooks 的锁图投影；`:421` optional 失败/bin/offline remove；可增加记录请求次数的精确 install 用例。
- `test/dever-cli-tests/packages.rs:121/174`：注入 transport/store、传递 Package→Lib；`:621` 移除不请求 registry；`:725` 原 setting 字节回滚；`:909` signed managed 全链路（重验收，不默认执行）。
- 新定向验收应覆盖冷 cache install 后锁字节不变、metadata 新增更高版本仍不选、Package→managed Worker、空 specs/无锁/声明漂移、typed miss 与损坏差异、固定 Python build deps、npm optional 原执行图、输出摘要不一致失败、ARM mismatch、已缓存构建输出不加载 build extension，以及普通 run/build 绝不发 ensure/download。

### 建议定稿的数据合同与实施所有权

当前源码实际是 `LOCK_FORMAT = "dever-lock-v5"` (`libs.rs:25`)，部分 spec 文案仍写 v2；本次直接升级 **dever-lock-v6**，无旧格式 fallback。建议单一、最小模式：

```text
LockedArtifact += source: Option<RegistrySource>
RegistrySource = { ecosystem: Ecosystem, locator: String, filename: String }
Receipt.dependencies -> Receipt.inputs: Box<LockFile>
```

`source` 对 registry 原始归档必填，对最终本地构建 output/fixture 可为 None；locator 是受现有 RegistryTransport 官方 origin/path 策略约束的精确地址，filename 只允许单文件名并承担解包格式选择，不从 URL query 猜格式。SHA/长度/target 沿用同一 LockedArtifact，不引入第二份内容身份。既然只接受 v6，source 必须成为已知字段，不需要兼容性 serde default；可序列化为 null。receipt 没有独立外部文件，**锁 v6 就是其 schema 版本**，不要再加无独立生命周期的 receipt 格式版本。

`inputs` 复用现有 LockFile：Python 为固定 build requirements 的精确递归锁；npm 为剪枝前 `libraries + npm graph`，图中 build=None，inputs.builds 为空，彻底避免自身 receipt 环。当前 receipt.graph 继续保存完整构建结束后的结果图（包括 prune 结果），供离线消费者与 `remove` 后投影使用。`sources` 保留为执行输入摘要列表并由 doctor 确认等于 inputs 的原始归档集合（Python 为单 sdist，build requirements 另在 inputs）。npm receipt.input 图不可被 remove 剪枝；最终 lock.npm 可以投影，原 receipt 保持不变。

实施所有权建议：

| Owner | 文件/API | 验证责任 |
| --- | --- | --- |
| extension/cache worker | toolchain extension owner、service/cache；ensure/read/list 与 `artifact_get_optional` typed miss | 签名、只读缺失/损坏区分、跨 UID、并发、失败原子性 |
| libs integration worker | libs.rs、main.rs、packages.rs、mutation.rs；明确 preparation policy、`install_locked`、Package exact restoration | 公共命令、Package 先恢复、Worker 提前准备、锁字节不变、离线边界 |
| registry/receipt worker | libs/registry.rs、build.rs、build/python.rs、build/npm.rs；v6 source、`restore_locked_artifacts`、`Session::replay` | 固定来源/输入、拒绝重新解析、嵌套依赖、输出 hash 不同失败 |

这些是逻辑所有权，可由同一 implementer 串行完成；`libs.rs` 类型/API 应由主实现者统一修改，避免多个 worker 同写。

具体公共/内部接点建议：

```text
toolchain::artifact_get_optional(layout, sha256, bytes) -> Result<Option<Vec<u8>>, String>
packages::restore_locked(project_root, lock, transport, store) -> Result<(), String>
libs::restore::install(project_root, target, preparation_inputs, transport, store)
  -> Result<PreparationReport, String>
registry::restore_artifact(source, artifact, transport, store) -> Result<(), String>
build::Session::replay(receipt, store, signed_runtimes) -> Result<(), String>
```

`Session::replay` 绝不接收可解析版本的 RegistryResolver；将 Environment.run 中只用到的 store/runtime 提取成参数，首次 resolve 与 replay 共用原隔离执行器。Python inspect/requires 若执行，只用来比对已锁 backend/requirements，不能更新 inputs；metadata/wheel 阶段沿用现有检查。npm 执行 input 图，验证产出的 optional outcomes/native entries/rejections/结果图及 output SHA 与 receipt 相等后 publish。先完成全部验证再发布最终 output，不能先 publish 后发现不一致。

最小可测直接路径：现有 `build.rs:260` 的内联 backend fixture 无网络包管理器，验证固定动态 build requirements；`npm_build.rs:68` 固定安装收据与 `:421` optional 生命周期 fixture 验证原始图；在内存/目录 ArtifactStore 清空最终 output 后重放，两次字节一致。另加 hook 有意生成变化输出的 fixture，要求明确失败且锁字节保持。真实 C extension/node-gyp 仍复用现有显式 native fixture，不能把纯脚本 replay 通过报告成原生编译验收。

### External references

未查外部网络；本研究以仓库实现为准。协议事实来自现有代码：PyPI PEP440/508 + wheel、npm semver/SRI/实例图、Go proxy/MVS/sumdb、PEP517。版本绑定以 lock.TOOLCHAIN 和 signed RuntimePack/pack::Descriptor 为准，不假定系统 Python/Node/Go 版本。

## Caveats / Not Found

- 未实现任何功能、未运行测试/build/download、未修改产品或 specs。
- Extension catalog/IPC 的最终结构由发行路径研究和主代理决定；本文提供实际 caller 需求，不重复设计签名/安装 owner。
- 现有锁可精确获取大多数普通 registry 归档，但并非所有构建收据都包含完整重放信息；这是当前实现完整性的实际缺口，不是通过更多 retry 可解决的问题。

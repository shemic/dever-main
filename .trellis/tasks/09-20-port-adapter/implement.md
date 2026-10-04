# Dever Port 与 Adapter实施计划

## Start Gate

- [x] secure-data-contracts 和 time-typed-codec 已完成。
- [x] 用户批准父任务规划。
- [x] 读取 source layout、parser declarations、symbol access、effects、application testing 和 config owners。

## Implementation

- [x] 增加 Port bodyless contract、fails modifier、AST/formatter/Markdown contract。
- [x] 注册 Port types/contracts 和 Adapter/Test qualified implementations。
- [x] 集中实现 role call/type matrix，删除宽泛 private-role互调分支。
- [x] 检查 implementation signature、outputs、failures、effects 和覆盖完整性。
- [x] 生成单实现直接调用和多实现 closed dispatch。
- [x] 增加 Adapter setting declaration、共享 codec 接入和严格 setting.json binding。
- [x] 把 Port effect 纳入 specialization/transaction/concurrency 检查。
- [x] 扩展 TestCase fake binding 和 suite case隔离。
- [x] 更新 LANGUAGE、Markdown、compiler/toolchain/application-testing specs。

## Implementation evidence (2026-09-20)

- 保留 Port 的 Function ID，附加生产实现 successors；shared dependencies/effects/specialization 纳入这些边，failure 直接 seed 声明全集。新 `check/ports.rs` 拥有声明、完整实现、setting schema 和 fake 绑定验证；`native/ports.rs` 生成直接调用/closed match 与不可变 setting/binding。
- 合同修正：App 只有公开函数且语言用函数分句匹配，故“Port 私有失败再由 App 私有 helper 映射”不可表达。采用主代理确定的 `fails app.ErrorChoice`；App/Port/Adapter 共享已公开错误合同，禁止反向业务调用，不放宽私有类型导出。
- 每个测试进程选定 case fake；check 与套件生成共用逐 case 的闭合图，Port successors 只替换为该 case 的 fake，生产 Adapter/配置不可达。每个 case 独立原生模块与同步/挂起入口，仍只编译一次套件。一个 case 可完整 fake 多个 Port；缺 fake 编译拒绝。
- 新反例发现 Port 无 HIR body 导致原有挂起阻塞检查漏过合成 dispatch；已在同一 shared effects owner 校验 Port successors，拒绝挂起 Port 内联选择阻塞实现。
- 当前已通过：`port_adapter` 10/10（单实现无配置、多实现同二进制切换、Secret setting、错误配置/重复字段、Markdown/fails、角色与 effects、逐 case fake 隔离）；`source_architecture` 8/8；`source_visibility` 10/10；`application_config` 3/3；`markdown_source` 20/20。
- 最终 `core/runtime/CLI` feature check（reference + api + postgres）通过；`application_testing` 8/8；`error_effects::result_capture_requires_matching_success_and_error_variants` 1/1；`git diff --check` 通过。
- `structured_concurrency` 3/11 通过、8 个旧 fixture 失败：仍把 time/task.sleep 的 Unit 当 SleepResult、chunks 当 ReadStreamResult、直接构造 error variant，以及 Markdown H1 放代码。失败先发生在这些旧协议校验，未扩到无关 fixture 迁移；新 Port 挂起/阻塞定向反例已通过。
- 未运行全量、外部服务、真实数据库或性能测试；rustfmt/clippy 组件未安装。Port 当前明确拒绝 handler 参数和输入/输出范围 bounds；配置选择/setting 仅原生后端执行，reference 对单个无 setting 实现可执行，其余给显式不支持错误。
- 已完成清理自审：声明注册与实现注册分开；role I/O 验证收敛到 shared effects；配置可执行目录定位复用同一函数；Port/setting 合成函数不参与重复实现建议。任务状态留给主会话复核后推进，未提交或归档。

## Independent review (2026-09-20)

已修复四项实际问题：

- 测试 fake 曾在 native 阶段跨 case 合并；一个阻塞、一个挂起 fake 会触发错误的 Port 阻塞诊断。生产合同推断也曾使用 Test body。现在生产推断排除 Test body，check/native 共用 `ports::test_program` 的 case 闭合图；共享 dependency owner 保留未实例化 helper 的声明检查，逐 case 校验所有 fake operation 的 failure 子集。原生套件按 case 生成独立模块与同步/异步入口，仍一次 rustc、一个二进制。混合 fake 回归先失败、修复后通过；同 case 阻塞违规、未调用 fake 的越界 failure、未实例化 pure helper 的副作用均被拒绝。
- `setting()` 普通函数曾被 parser 误判为 setting record；现在只在 `setting {` 时识别 contextual 声明。
- 显式 `adapter: null` 曾被 serde Option 吞成缺省，单实现可意外启动；保留原始 null 交给 shared wire validator 拒绝，单实现无文件仍可运行。
- Port-local DTO 内嵌 App DTO 曾在合同图展开之前触发 C006。现在在 shape access check 前按既有名字解析规则展开合同类型图；合法嵌套 DTO 通过，无关 App 类型仍被拒绝。正例已先复现失败再通过。

最终验证：`port_adapter` **10/10**、`application_testing` **8/8**、`source_visibility` **10/10**；core/runtime/CLI `cargo check`（reference + api + postgres）通过。命令使用 stable PATH、CARGO_INCREMENTAL=0、locked/offline、dev/test debug=0、serial tests。`git diff --check` 与本轮修改文件逐文件 `git diff --no-index --check /dev/null` 均无空白错误（后者有文件差异时退出 1，不代表空白错误）。LANGUAGE 与 compiler-contracts 已同步。

rustfmt/clippy 未安装，未运行且未安装组件。未运行全量测试、服务、真实数据库或性能测试。此前 structured_concurrency 的旧 fixture 失败记录仍适用，不在本轮扩改。无新增未修复的 in-scope 阻塞；未提交、未归档、未更改任务状态。

## Focused Verification

```bash
cargo check --offline -p dever-core -p dever-runtime -p dever-cli
cargo test --offline -p dever-tests --test source_architecture
cargo test --offline -p dever-tests --test source_visibility
cargo test --offline -p dever-tests --test error_effects
cargo test --offline -p dever-tests --test structured_concurrency
cargo test --offline -p dever-tests --test application_config
cargo test --offline -p dever-tests --test application_testing
cargo test --offline -p dever-tests --test markdown_source
git diff --check
```

不运行外部网络、全量测试、真实数据库或性能基准。Adapter runtime 正例使用 test-owned本地确定性实现，不连接服务。

## Review Risks

- Port implementation不能被普通 name lookup 当成可调用函数。
- 多实现 effect/failure union不能因当前 setting 选择而缩窄静态合同。
- setting schema identity必须包含实现选择，且错误不得回显 Secret。
- 单 suite 的 fake registry不能跨 TestCase共享可变状态或生产绑定。
- role matrix应由一个表/函数拥有，避免 call与type access规则漂移。

# Implementation Plan

## 1. Cargo Development Footprint

- [x] 在根 `Cargo.toml` 固化 dev/test 的 `debug = 0` 与 `incremental = false`，保留 release profile。
- [x] 增加静态回归，确认 profile 合同及所有既有 `[[test]]` 目标仍存在，不移动或合并测试源码。

## 2. Shared Native Runtime Target

- [x] 把 runtime feature/profile 规范化为单一 build key，复用现有 runtime 身份与 native cache owner。
- [x] 使用一个 `target/native-runtime/cargo` 执行 locked/offline Cargo build，并解析 Cargo JSON 产物消息。
- [x] 原子发布只包含当前构建图精确文件的不可变 `inputs/<identity>`；复用公共文件且拒绝符号链接、缺失文件和不完整 staging。
- [x] 让 `RuntimeInputs` 消费显式文件清单，不再扫描整个 `release/deps`。
- [x] 增加 base、API、SQLite、PostgreSQL、component/external profile 隔离、复用、失败和并发定向测试。

## 3. Temporary Build Cleanup

- [x] 为 `BuildDirectory` 增加 owner marker 与统一 cleanup summary；正常 `Drop` 行为保持。
- [x] 在创建 native build 前执行有界的 24 小时过期清理，跳过当前、未知、未过期和符号链接目录。
- [x] `run_native` 直接执行 `NativeProgram::executable()`，移除项目根 staging 和 `StagedExecutable`。
- [x] 增加成功、编译错误、子进程失败、过期目录和恶意同名路径的定向回归。

## 4. Clean Command

- [x] 给版本核心增加 `clean <project-root>` 解析和执行；输出删除目录、文件及字节汇总。
- [x] 给稳定 launcher 增加项目根识别和帮助文本，保持 `dever cache clean` 的认证服务失败合同。
- [x] 兼容回收严格匹配且过期的旧 `.dever-run-*` 普通文件，不删除显式 output 或其它隐藏文件。
- [x] 增加 CLI 参数、版本分派、限定删除和重复 clean 幂等测试。

## 5. Documentation And Verification

- [x] 更新 README、backend directory/toolchain/error/quality 规范，说明三种清理边界与默认低磁盘 profile。
- [x] 运行 `git diff --check`。
- [x] 运行 core/CLI/runtime 的 offline focused check，不运行 workspace 全量测试。
- [x] 分别选择 default、API、SQLite、PostgreSQL、component 的最小定向编译或测试；PostgreSQL 只编译，不连接数据库。
- [x] 测量 fresh target 和重复执行后的占用，验证低于 16 GiB 基线的 40% 且无变化重复增长不超过 5%。
- [x] 复查磁盘空间、临时目录和项目根，确认没有遗留本轮测试产物。

## Completion Evidence (2026-09-29)

- `cargo check --offline --locked -p dever-core -p dever-cli` passed.
- `deverc` argument tests passed 5/5; local clean, selected-version launcher dispatch and authenticated `cache clean` refusal regressions passed.
- Exact runtime input publication/reuse/concurrency regression passed. Actual native base, API, SQLite, PostgreSQL and external/component profile checks passed; PostgreSQL was compile-only and opened no database.
- The runtime layout contains one `target/native-runtime/cargo` plus content-addressed `inputs`; no per-feature Cargo target remains. Repeating the external native check changed target usage from 1,625,111,299 bytes to 1,625,111,299 bytes (0%).
- The final focused matrix used 1,642,122,909 bytes (about 1.64 GB), 10.2% of the former 16 GiB baseline and below the 6.4 GiB budget. `target/debug/incremental` is an empty Cargo-created directory (0 files, 4 KiB), not an incremental object cache.
- No `/tmp/dever-build-*` or project `.dever-run-*` artifact remained. Final filesystem availability was about 17 GiB.
- `cargo fmt --all --check` and `cargo clippy ...` were unavailable because the installed toolchain lacks rustfmt and clippy. Workspace-wide tests, real PostgreSQL, frontend builds and performance/RSS benchmarks were not run by scope.
- Two pre-existing source fixtures were not counted as task verification: `rest_model::rest_field_bindings_are_checked_and_emit_static_search_sql` references missing generated `api_require_components`/`job_migrate_tenant`, and `sqlite_orm::database_failures_are_captured_after_transaction_rollback_and_task_wait` exposes a private `ReadResult` from `public main`. The selected SQLite and PostgreSQL profile checks passed through independent valid fixtures.

## Final Review Evidence (2026-09-29)

- Final review found and fixed three correctness gaps: runtime publication now selects the current `dever-runtime` manifest's hashed `release/deps` rlib instead of the mutable top-level Cargo alias; runtime input views reject symbolic links and unknown entries; stale cleanup preserves artifacts owned by any still-active recorded PID, not only the current process.
- The clean regression now keeps both a marked build directory and legacy `.dever-run-*` file for a live child process. Input-view regressions cover concurrent publication, exact dependency sets, unknown files and symbolic links.
- `cargo check --offline --locked -p dever-core -p dever-cli` passed after the fixes.
- `cargo test --offline --locked -p dever-tests --test native_cache runtime_input_views -- --nocapture` passed 2/2; the real concurrent native publication test passed twice and left no temporary artifact.
- `cargo test --offline --locked -p dever-cli --test cms_project clean_removes_only_stale_owned_dever_artifacts -- --exact --nocapture` passed 1/1.
- Selected-version launcher dispatch and authenticated `cache clean` refusal passed 1/1 each; `deverc` argument tests passed 5/5; the Cargo profile/test-target contract passed 1/1.
- Repeating the same concurrent native check left `target/` at exactly 1,689,816,322 bytes (0% growth). Final follow-up checks left it at 1,689,817,710 bytes (about 1.69 GB, 10.6% of the former 16 GiB baseline). Only `cargo` and `inputs` exist directly under `target/native-runtime`; `target/debug/incremental` contains zero files.
- No `/tmp/dever-build-*` or repository-root `.dever-run-*` remained. `git diff --check` passed and about 17 GiB remained available.
- `rustfmt` and `clippy` remain unavailable because those toolchain components are not installed. Workspace-wide tests, real PostgreSQL, frontend builds and performance/RSS benchmarks remain not run by scope.

## Risks And Rollback Points

- Runtime input discovery depends on Cargo's JSON artifact contract. Missing or ambiguous current-manifest runtime artifacts fail explicitly instead of scanning historical files.
- Process-state detection is conservative: an inspection failure preserves the candidate, so cleanup may leave an old artifact rather than risk deleting an active one.
- The shared target and input views are rebuildable. Rollback removes this layout through `cargo clean`; no project data migration is involved.
- Native test linking can still consume significant time and space even with deduplicated runtime inputs, so broad matrices remain opt-in.

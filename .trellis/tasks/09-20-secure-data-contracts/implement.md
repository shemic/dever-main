# Dever 安全数据合同实施计划

## Start Gate

- [x] 用户批准父任务最终规划后启动本子任务。
- [x] 读取 compiler-contracts、database、logging、toolchain/library specs。
- [x] 记录当前 private 字段、Model field access、Render/JSON/log 和 process intrinsic 所有调用点，见 research/secure-contracts-source-map.md。

## Implementation

- [x] 扩展类型属性和字段访问 owner，允许 Model private 并限制为所属 App。
- [x] 在现有 App/API/入口/错误/测试边界复用 observable 检查；Job 的消费检查由 durable-jobs 子任务接入相同属性。
- [x] 增加 Secret 类型、语法类型解析、HIR/native/reference 表示和禁止能力。
- [x] 增加 crypto 官方 package、Intrinsic、runtime 实现和依赖；保证失败类型化与错误脱敏。
- [x] 删除 environment 官方函数、Intrinsic/runtime 分支、effect 分类、文档与调用方。
- [x] CMS 账户使用 private password_hash: Text(255)，AccountView/ArticleView 隐藏私有值及作者关联；两套源码同步，没有新增登录 API。
- [x] 更新 LANGUAGE/Markdown/compiler/database/logging canonical docs。

## Focused Verification

```bash
cargo check --offline -p dever-core -p dever-runtime -p dever-cli
cargo test --offline -p dever-tests --test model_syntax
cargo test --offline -p dever-tests --test source_visibility
cargo test --offline -p dever-tests --test contract_semantics
cargo test --offline -p dever-tests --test standard_library
cargo test --offline -p dever-tests --test logging
cargo test --offline -p dever-tests --test markdown_source
git diff --check
```

如组件存在，增加受影响 package/target 的 rustfmt 和 `clippy -D warnings`。不运行全量测试、真实数据库、服务或性能测试。

## Review Risks

- private Model 字段不能破坏生成 SQL 列顺序、迁移 fingerprint 或 row decode。
- Secret 不得因派生 Render/Debug、错误路径或 generic collection 自动获得观察能力。
- crypto 失败不得包含输入；随机源和 password verify 不得 panic。
- 删除 environment 后必须穷尽 checker/emitter/reference/effect match，不能留不可达旧分支。

## Implementation Notes — 2026-09-20

- Secret 位于基础 runtime/secret.rs，Clone 保持值语义、Debug 固定脱敏、zeroize 清除已持有字节；argon2/sha2/hmac/getrandom/subtle 由独立 crypto feature 控制。原生构建根据已裁剪代码中的 crypto 调用选特性，也覆盖直接 system intrinsic。
- Model 私有字段访问统一经过 Context::owns_private_fields。类型安全属性缓存沿 Related 关联图做单调反向传播，不把 ORM 关联环当成布局递归。
- Argon2 0.5.3 官方文档核对后采用 Argon2id v19 / Params::default；verify 在计算前限制 PHC 算法、版本、成本及结果长度。执行一次 cargo fetch，锁定 12 个新增依赖包；zeroize 复用已存在的 1.9.0。
- `secure_contracts` 新定向目标 8/8 通过（含 reference、原生与 Test source 执行）。覆盖隐私所有权/Model schema 不变、嵌套非观察边界、Related 自环、环境入口拒绝、密码错误/损坏/资源上限、SHA/HMAC 已知向量、token 独立性及脱敏 Debug。
- 编译检查 core/runtime/cli + crypto/reference 通过。首次验证因磁盘只剩 65 MiB 导致 source_visibility 链接 Bus error，随后 rlib 报 No space left on device；主会话清理 target/debug/incremental 后恢复验证。后续使用 `--config build.incremental=false --config profile.dev.debug=0 --config profile.test.debug=0`，始终 offline + locked，原生测试串行。
- rustfmt/clippy 组件未安装；没有安装全局工具。没有运行全量测试、真实 PostgreSQL、持久服务、CMS 打包运行或性能测试；OS 随机源不可用分支未做系统故障注入。
- contract_semantics 完整目标初次 6/12：6 个旧样例直接构造 error variant，在既有 `construct_variant_with_kind` 禁令处收到 C005；本次没有改该 error 构造语义，也未重写这些旧样例。最终报告保留未通过，不据此宣称全目标成功。

## Verification Results

| 定向验证 | 结果 |
| --- | --- |
| cargo check：core/runtime/cli + crypto/reference | 通过 |
| secure_contracts（crypto,reference，串行） | 8/8 通过 |
| model_syntax | 3/3 通过 |
| source_architecture | 8/8 通过 |
| source_visibility | 10/10 通过 |
| markdown_source | 20/20 通过 |
| standard_library | 5/5 通过 |
| logging | 1/1 通过 |
| differential::time_failures_are_source_defined_results | 1/1 通过；移除 environment 样例后保留显式 result 的时间失败 native/reference 一致性 |
| contract_execution | 首轮 7/9；两个 CMS 失败因关联传播要求 ArticleView，修复后 CMS parity 与 maintained root checker 各 1/1 通过；未重复整个目标 |
| contract_semantics | 完整目标 6/12；6 个既有 error 直接构造样例未重跑；当前 private_fields_protect_access_mutation_and_construction 精确用例 1/1 通过 |
| git diff --check / 本次文件尾随空白扫描 | 通过；项目源码目前未跟踪，因此额外执行显式文件扫描 |
| rustfmt/clippy | 未安装组件；rustfmt 启动失败，未安装或绕过 |

contract_semantics 未通过用例：failure_cannot_be_discarded_overwritten_logged_or_laundered、errors_can_be_returned_translated_or_explicitly_recovered、failure_summaries_follow_long_calls_and_unused_handler_dependency_cycles、nested_failure_survives_value_copies_and_projection、empty_traversal_does_not_handle_context_and_singleton_projection_transfers_failure、possibly_empty_or_null_outputs_cannot_hide_another_failure。首个诊断均为 C005 `error choice variants can only be used with fail`。

当前没有新配置入口、没有登录业务、没有真实数据库/全量/服务/性能验收。Model create/update/读取 private 字段通过静态检查与 schema 不变验证；未据此声称已完成真实数据库运行验收。

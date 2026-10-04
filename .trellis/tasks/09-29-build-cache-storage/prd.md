# Dever 构建缓存与磁盘占用治理

## Goal

在不改变 Dever 程序语义、发布构建性能和现有定向测试入口的前提下，显著降低编译器开发与原生运行时构建的磁盘占用，并让 Dever 自有临时产物可以可靠回收。

## Background

- 2026-09-29 清理前，仓库 `target/` 占用约 16 GiB：`target/debug/deps` 约 11 GiB、`target/debug/incremental` 约 2.3 GiB、`target/native-runtime` 约 2.3 GiB。
- `test/dever-tests` 有 67 个独立集成测试目标。每个目标都会单独链接 Dever runtime；清理前单个 debug 测试程序约 120-173 MiB。
- 多个既有任务已经通过命令行反复使用 `profile.dev.debug=0`、`profile.test.debug=0` 和 `build.incremental=false` 避免磁盘耗尽，说明这些设置是已验证的仓库开发约束，但尚未固化。
- `crates/dever-core/src/native/build.rs` 当前按 `base/database/sqlite/postgres` 与 `api/crypto/wire` 组合创建独立 Cargo target；公共依赖因此被完整复制到每个 profile 目录。
- `BuildDirectory` 已在正常返回和普通错误时通过 `Drop` 清理，但进程被强制终止或链接器崩溃时仍会留下 `/tmp/dever-build-<pid>-<id>`。
- `dever cache clean` 已属于机器级共享缓存合同，必须等待认证 `deverd` IPC；本任务不得以本地直写方式绕过该安全边界。

## Requirements

### R1. Cargo Development Profiles

- 在仓库 Cargo 配置中固化 dev/test 的低磁盘设置，不再依赖每条验证命令重复传入临时覆盖项。
- dev/test 不生成完整调试信息且禁用增量对象；release 的 `fat LTO`、优化级别和最终程序行为保持不变。
- 保留现有独立测试目标及 `cargo test --test <name>` 合同；本阶段不通过合并测试文件换取空间。

### R2. Shared Native Runtime Build Target

- 所有 runtime feature profile 共享一个 Cargo 构建 target，公共依赖只保存一份。
- 每次 native 编译只消费当前 Cargo 调用实际报告的 runtime 与依赖产物，不把其它 profile 的历史依赖混入缓存身份或链接输入。
- 不同数据库/API/加密/wire/external profile 仍有独立、完整且可验证的 runtime 身份；并发构建不得读到正在改写的半成品。
- 继续使用 locked/offline Cargo，不引入网络、环境配置或第二套构建协议。

### R3. Temporary Build Ownership

- 正常成功、编译失败和运行结束继续即时清理本次临时目录。
- `run` 直接执行 `NativeProgram` 已拥有的临时可执行文件，不再向项目根复制 `.dever-run-<pid>`。
- 临时目录写入 Dever 自有标记；启动构建和显式清理只处理名称、标记、类型、属主和年龄均符合合同的目录。
- 清理拒绝符号链接、未知目录、活跃或未达到过期阈值的目录；清理失败不得掩盖原始编译或运行错误。

### R4. Local Clean Command

- 版本核心提供 `clean <project-root>`，开发入口 `deverc` 与正式 launcher 分派后的 `dever` 使用同一实现。
- 命令只回收当前用户的 Dever 过期临时构建目录和旧版遗留的过期 `.dever-run-*`，并输出删除数量与字节数。
- 命令不得删除显式 build 输出、项目源码/配置/data、Cargo registry、Lib 缓存、机器共享缓存或其它项目文件。
- `dever cache clean` 的名称、失败合同和认证 IPC 边界保持不变。

### R5. Documentation And Regression Coverage

- README 和 toolchain 规范明确区分 `clean`、`cache clean` 与编译器仓库维护用的 `cargo clean`。
- 测试覆盖参数解析、限定删除、符号链接拒绝、未过期目录保留、runtime profile 隔离和共享依赖复用。

## Acceptance Criteria

- [ ] dev/test profile 默认不生成完整 debuginfo，也不创建 incremental 目录；release profile 保持原值。
- [ ] 现有 `cargo test --test <name>` 入口不变，至少一个默认、API、SQLite、PostgreSQL 和 component 定向目标可以继续被单独选择。
- [ ] 连续准备至少两个不同 runtime profile 时只存在一个 Cargo target，公共依赖不再按 profile 复制；每个 profile 链接到自身准确的 runtime 产物。
- [ ] 并发或失败的 runtime 准备不会发布半成品，也不会让一个 profile 复用另一个 profile 的 runtime 身份。
- [ ] `run` 不再创建项目根 `.dever-run-*`；成功和普通失败后本次临时目录不存在。
- [ ] `clean <project-root>` 只删除符合 Dever 标记和过期规则的自有临时产物，并报告回收结果；未知文件、符号链接、未过期目录及显式输出保持不变。
- [ ] `dever cache clean` 仍明确走机器共享缓存服务，未实现认证 IPC 时不允许本地回退。
- [ ] 在全新 target 上完成允许的代表性 feature 定向检查后，`target/` 总占用不超过清理前 16 GiB 基线的 40%；重复同一组检查且源码不变时占用增长不超过 5%。
- [ ] `git diff --check` 和受影响 crate 的离线定向检查通过；不运行 workspace 全量测试、真实 PostgreSQL、前端构建或性能压测。

## Out Of Scope

- 实现或绕过尚未完成的认证 `deverd` IPC 和跨用户共享缓存清理。
- 删除用户业务数据、显式构建输出、第三方包缓存或正在使用的缓存。
- 改变 release 优化、最终程序语义、Dever 语言语法或应用配置格式。
- 本阶段合并 67 个测试目标；只有 profile 与 runtime 去重后仍不能达到磁盘预算时再单独评估。
- 把 Cargo 开发目标暴露成 Dever 应用运行时配置。

## Key Decisions

- 根因是完整 debuginfo、增量对象、独立测试链接和 profile target 重复，不是源码本身或最终 Dever 二进制达到 16 GiB。
- 保留小而可定向的测试目标比合并成少量巨大 runner 更有维护价值；先使用低磁盘 Cargo profile 解决体积。
- 本地 `clean` 和机器级 `cache clean` 是不同安全边界，不能为了“一个命令”让普通项目进程直接写共享缓存。

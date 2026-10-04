# Design

## Boundaries

本任务只修改编译器仓库构建策略、native runtime 准备路径和 CLI 临时产物清理。Cargo 开发产物由 Cargo profile 管理；native runtime 依赖由 `dever-core::native::build` 管理；项目命令解析由 `dever-cli` 管理。机器级缓存继续归 `deverd` 合同所有。

## Cargo Profiles

根 `Cargo.toml` 增加 dev/test profile：

```toml
[profile.dev]
debug = 0
incremental = false

[profile.test]
debug = 0
incremental = false
```

这把多个任务已经稳定使用的命令级覆盖固化成仓库默认值。release profile 不变。测试目标、文件和选择命令全部保留。

## Native Runtime Layout

当前每个 profile 都有完整 target：

```text
target/native-runtime/<profile>/release/...
```

改为一个共享 Cargo target 和不可变的精确输入视图：

```text
target/native-runtime/
  cargo/                    shared Cargo target
  inputs/<identity>/        exact runtime/dependency files for one build identity
  operations/               build locks/staging
```

`runtime_library` 根据 database、api、crypto、wire、external 得到规范化 feature 集。Cargo 使用 `--message-format=json-render-diagnostics` 报告本次图中的产物；代码通过结构化 JSON 解析收集当前 `dever-runtime` 及其依赖的 rlib/dylib，不扫描并吞入整个历史 `release/deps`。

收集后的文件先写入同一文件系统的 staging，再按内容身份原子发布到 `inputs/<identity>`。相同文件优先使用硬链接以复用共享 target 的磁盘块；不支持或失败时复制。已发布输入视图只读且不可变，`RuntimeInputs` 继续对精确文件集合计算身份并在编译前复核。并发操作使用 create-new 锁和 staging，失败只清理本次 staging。

## Temporary Directory Contract

`BuildDirectory` 保留 RAII 清理。创建目录时增加固定格式 owner marker，至少包含格式版本、创建时间、进程 ID 和当前编译器身份。清理逻辑集中在 `native/build`，不在 CLI 复制一套删除规则。

自动清理和显式 clean 共同调用这一逻辑：

1. 只枚举系统临时目录直属的 `dever-build-<pid>-<id>`。
2. 使用 `symlink_metadata`，拒绝符号链接和非真实目录。
3. marker 必须完整匹配且创建时间达到 24 小时过期阈值。
4. 当前进程目录永不删除；不满足任一条件即跳过。
5. 删除前统计普通文件字节数，返回结构化 summary。

24 小时阈值避免误删并发或长时间构建；正常流程仍由 `Drop` 立即清理，因此阈值只影响异常退出遗留物。

## Run Flow

`NativeProgram` 已拥有可执行文件和临时目录，`run_native` 直接执行 `native.executable()`。借用在子进程退出前持续有效，随后 `NativeProgram` 的 `BuildDirectory` 统一回收。删除 `StagedExecutable` 和项目根二次复制路径。

## Clean Command

CLI 增加 `Action::Clean` 和 `clean <project-root>` 参数合同。launcher 把它视为普通项目命令，用项目的 `config/setting.json` 选择版本后原样分派。

核心不解析或修改项目源码，只调用 native build 的限定清理 API，并兼容清理项目根中符合旧 `.dever-run-<pid>` 命名、普通文件、当前用户和 24 小时阈值的历史遗留物。输出固定摘要，不把未删除的未知文件当作成功删除。

该命令不接触 `target/debug`；编译器贡献者仍使用 `cargo clean` 管理 Cargo 开发产物。它也不接触机器共享缓存；后者仍是 `dever cache clean`。

## Compatibility And Rollback

- Dever 源码、配置、语言行为和 release 构建参数不变。
- 现有测试目标名与定向命令不变。
- 旧 `target/native-runtime/<profile>` 不再读取；可由 `cargo clean` 删除，不提供双路径兼容。
- 共享 runtime target 若出现问题，可回滚 `native/build` 布局而不迁移项目数据；所有相关内容均可重建。
- clean 只删除严格识别的过期临时产物，回滚不需要恢复数据。

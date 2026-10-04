# Research: native runtime pack 制作与 profile 裁剪

- Query: 既有制作入口是否仅 fixture；四 profile 的真实 archive 制作、裁剪、复用及本机资源边界。
- Scope: internal；只读源码与本机工具元数据，不构建、不测试、不下载、不研究签名/CLI owner。
- Date: 2026-10-02

## Findings

### 文件与当前证据

- `test/native-runtime-pack.rs:1` 明示 private Linux author fixture；main 仅调用 `llvm_pack::prepare`。
- `test/dever-cli-tests/support/llvm_pack.rs:56` 的 `native_inputs` 固定引用 `target/native-runtime-abi/debug/libdever_backend_bridge.a` 和本机 glibc/GCC CRT/static libs；没有 Cargo 调用。
- 同文件 `document`（88–101）把 base/sqlite/postgres/both 全映射至同一个 `runtime.a`。`prepare`（107–150）只是检查输入、独占创建目录、硬链接并记录 SHA/大小；不是四 profile 发行构建器。`compiler_library`（153）硬链接本机 LLVM 动态库。
- `target/debug/runtime/linux-x86_64/manifest.json` 当前 runtime.a 为 **191253848 bytes**，摘要 `7002224967cfbc3609b304f964e911d2f03fe204d5b58c07e90dc35d71c4b84c`。源码 archive 的 stat 相符；本次未重复计算其 SHA。此 archive 为三硬链接之一，严禁原地 strip，否则会修改现有 fixture。
- `crates/dever-backend-bridge/Cargo.toml:9` 输出 rlib/staticlib；17–25 已有独立 features，无需新 runtime crate。
- `crates/dever-backend-bridge/build.rs:1` 只有 embedded feature 才编 C++/链接 LLVM；runtime-only archive 必须禁用 embedded。私有 SDK 位于 `target/backend-sdk/usr/lib/llvm-18`（12），LLD 静态库、LLVM18/z 动态库、zstd 静态库是 compiler 侧依赖（39–44）。
- `crates/dever-runtime/Cargo.toml:11` 已有 api/external/sqlite/postgres 独立 feature owner；SQLite bundled engine 固定于 workspace dependency，不能依赖宿主动态 SQLite。
- `Cargo.toml:46` release 为 fat LTO、codegen-units=1；dev/test debug=0、incremental=false。现有 debug archive 不能靠 strip 冒充 release 优化。

### 四 profile 的最小 feature 映射

| profile | bridge features（每次只构建 bridge lib） |
| --- | --- |
| base | runtime-abi,runtime-api,runtime-external |
| sqlite | 上述 + runtime-sqlite |
| postgres | 上述 + runtime-postgres |
| both | 上述 + runtime-sqlite,runtime-postgres |

profile 的区别只在真实数据库 driver。base 不是仅 runtime-abi：否则缺失 API/external 应用能力。证据：ffi.rs:4319 的 Worker 实现由 runtime-external 门控，ffi.rs:5196/5333 等应用路径由 runtime-api 门控，ffi.rs:5476–5477 按两个数据库 feature 声明 driver 能力。不要在 workspace-wide build 中无意通过 CLI dev-dependencies 合并 embedded/全数据库 features。

建议作者构建步骤（未执行）：用显式 Cargo/Rust 工具路径，`cargo build --offline --locked -p dever-backend-bridge --lib --release --no-default-features --features <mapping> --target x86_64-unknown-linux-gnu --target-dir <shared-build-dir>`，每次将结果复制为独立 profile archive 后再构建下一个。四个 profile 共用一个 Cargo target，避免四份依赖树；不用环境变量/PATH作为产品配置。

### 裁剪与可重复制作的最小交付

1. 作者工具接受显式输入：目标、四份真实 release archive、CRT/static lib 清单及工具路径；记录工具版本、输入摘要与制作参数。将 profile 映射保存在单一配置表，manifest 仍使用既有 v1 协议。
2. 对**新副本**进行确定性 archive debug 裁剪；本机 GNU strip 支持 `--strip-debug --enable-deterministic-archives -o <new-output>`。不要使用 `--strip-all`，它移除重定位/符号，会损坏可继续链接的 archive。不要凭文件名任意删除 Rust/C ABI object member；真正 driver 裁剪由 Cargo features 完成。
3. 保留 ABI 导出、链接符号及故障来源字符串；检查普通 archive、目标架构与输入完整性，生成每个 profile 独立 runtime 路径。可共享真正相同的 CRT/libs，但不可拿同一 all-features archive 冒充四包。
4. 对同一批输入重复制作两次并比对文件/manifest 摘要，证明“pack 制作确定性”。若要声称从源码可重复，还须固定完整 Rust/CRT/SDK 版本、构建路径映射等并实际重建比较；仅重复复制不证明源码 bit-reproducibility。
5. 将完成后的字节集合交给既有 release 签名 owner（主线程负责），签名之后不得再 strip。定向验收应覆盖四档链接/执行、driver 分离、缺失/篡改及安装后消费；没有实际 PG 服务只能证明 postgres 构建/链接，不能声称真实 PG 执行通过。

复用点：`sha256_file` 已为流式摘要 owner（llvm_pack.rs:5）；现有 runtime v1 manifest/校验及 release v1 签名闭包由主线程复用，不新造协议；fixture 的 CRT 顺序与 archive 清单可作为 Linux GNU 输入模板，但其绝对路径与硬链接行为不能直接当通用发布接口。

### 对作者工具是否亲自调用 Cargo 的建议

**不必让签名/制作器亲自调 Cargo 才算完整交付**。更小且边界清楚的链路是：一个有真实执行证据的统一 `runtime-pack` Cargo profile + 四条固定 feature 构建命令，输出四份真实 archive；作者 maker 只读 `<author-root>/config/setting.json` 的明确输入、验证/裁剪副本、生成 manifest 并交既有签名 owner。如此制作器不依赖源码树、Cargo 缓存或编译工具，不把“签名已准备资产”与“编译源码”混成一个职责。构建配方与制作配置共同构成可重复链，文档应区分两层，不能仅交付 maker 而省略真实四 archive 构建。

若当前验收明确要求“一条作者命令从源码生成四包”，才给 maker 增加独立 build 子流程；不要通过扫描 PATH 或环境变量选择工具。现有需求是可重复制作/裁剪/签名，未见必须单命令从源码构建的约束。显式 Cargo/Rust/C 工具输入可属于作者构建配方；避免在产品 setting 中新增这些实现细节。

统一 runtime-pack profile 应继承 release 的 fat LTO/单 codegen unit，保持 debug 信息策略明确。先实际构建一次检查 archive/link/尺寸，再决定是否需要额外 strip 步骤；不为缩小时间或磁盘而悄悄降低既有生产优化合同。共享 target + 顺序构建四档即可，不需要泛化跨平台构建框架。

### 本机现有资源（只读检查）

- `/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/{cargo,rustc}` 存在；rustlib 只发现 x86_64-unknown-linux-gnu 标准库，没有其他目标 std。
- `/usr/bin/{ar,strip}` 是 GNU Binutils 2.42；另有 `/usr/bin/x86_64-linux-gnu-objcopy`。其 strip help 列出确定性 archive 支持；未验证对新 release archive 的实际处理结果。
- `/usr/bin/llvm-ar*`、`llvm-strip*`、`llvm-objcopy*` 和 `/usr/lib/llvm-18/bin` 不存在；私有 SDK 有 LLVM/LLD headers/static libraries，不能由此推定已安装完整跨平台裁剪工具。
- `/usr/lib/llvm-18/lib/libLLVM.so.1` 为 123215144 bytes；当前 fixture 指定 glibc 2.39、GCC13。正式交付须明确该 Linux GNU 工具链基线，不能声称兼容所有 Linux。
- `target/native-runtime-abi` 约1.3 GiB；文件系统可用718 MiB。只有主线程可构建/清理；研究未移动或删除任何产物。

### 相关规范 / 外部参考

- `.trellis/spec/backend/index.md`、`directory-structure.md`、`error-handling.md`、`quality-guidelines.md:5`（共享 Cargo target、release优化、定向验收）。
- `.trellis/spec/backend/toolchain-and-library.md:7–19`（Rust≥1.95、feature边界、runtime v1与不可伪称六目标发行）。
- 当前任务 `implement.md` 的 2026-10-02 节明确下一步是正式可重复裁剪 packs，现有签名 fixture 不代表正式资产。
- 外部网页未访问；工具选项证据直接来自本机 GNU strip 2.42 `--help` 和 ar/strip `--version`。

## Caveats / Not Found

- 子代理运行 task.py current --source 返回 none；输出路径使用父代理明确给出的 Active task，未猜测/改动任务指针。
- 检查了 shemic-dever SKILL.md；实际仓库 AGENTS.md 明确这是 Rust 语言编译器，不适用 Go Model/Page/Service 规则。
- 未找到正式四 profile maker；现有入口清楚标记 fixture。本结论不覆盖主线程正在编辑的文件。
- 本次没有 Cargo/build/test、下载、全局安装、git 操作及真实发布；没有验证 release archive 大小、跨平台运行、签名真实性或源码重建一致性。

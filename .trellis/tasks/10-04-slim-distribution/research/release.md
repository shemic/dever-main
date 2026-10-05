# Research: 精简发行、签名扩展与机器更新

- Query: 核对现有签名发行、bootstrap、共享机器资源与更新契约，给出最小完整拆分方案。
- Scope: mixed；源码只读，外部仅查解码 API 文档。
- Date: 2026-10-04

## Findings

### 现状与文件归属

| 文件 | 归属与证据 |
| --- | --- |
| `crates/dever-cli/src/toolchain/release.rs` | `ReleaseManifest`（74）只有 format/version/platform/artifacts；`MachineManager` 安装（293）、更新（310）、只读 core（437）、编译租约（470）、签名验证（579）、文件验证（616）、安装权限（968）；稳定 `install.lock` inode（713）。 |
| `crates/dever-cli/src/toolchain/release_source.rs` | 官方 URL/重定向白名单（16、119）、元数据先验签（184）、同版本清单字节不变（213）、有界 gzip 下载（223）、catalog（250）、严格 USTAR 文件白名单（280）、NOREPLACE 发布（343）。 |
| `crates/dever-cli/src/toolchain/packaging.rs` | 固定 author 输入与签名；assemble（141）将 native、生态、sandbox、build 全部加入同一清单（202–234）；必须在此按语义分类，不在压缩脚本根据随机路径猜测归属。 |
| `toolchain/packaging/{ecosystems,builds,sandbox,bootstrap}.rs` | 各类资源本身的语义校验与打包复用点；bootstrap（18）已有独立 ELF loader closure。 |
| `sdk/release-assets.py` | 所有清单 artifact 输出一个 deterministic gzip USTAR（create）；目录发布使用 renameat2 NOREPLACE（publish）。 |
| `skills/scripts/install.py` | Python/OpenSSL 独立锚定公钥；authenticate（79）先验签，copy_artifact（129）精确字节/SHA 校验，payload（148）严格 tar 流，最后执行 bootstrap/dever。 |
| `toolchain/bootstrap.rs` 与 `bootstrap/transaction.rs` | 固定 bootstrap 系统落点、可信 key pin、崩溃恢复；SERVICE（19）仅允许写 state/cache/run，不允许服务写 versions 或新顶层目录。 |
| `toolchain/service.rs` | IPC Operation（100）只有 Status/Clean/ArtifactPut/ArtifactGet/Compile；内核 UID 认证（440），外 UID 可编译/资源缓存，不能 Clean；socket 0666、8 workers（34–36、154）。 |
| `toolchain/service/compile.rs` | compilation 租约、cache identity（74–86）、verify_pack（120）逐项验证所选 runtime 文件属于签名 manifest，cache hit 前也检查。 |
| `toolchain/runtime_pack.rs` | load_manifest（99）接受资源根目录，内部追加 runtime/<platform>；公开 validate（142）、RuntimePack::load（173）。 |
| `crates/dever-cli/src/compile.rs` | daemon 子进程依然在 worker（149）用 compiler_directory 加载 runtime；仅改 daemon 的 verify_pack 不够。 |
| `crates/dever-cli/src/libs.rs` | InstalledRegistry（709）；tools（726）、runtime（797）、signed_artifact（823）、sandbox_resources（841）直接读版本根与根清单。 |
| `toolchain/cache.rs` | CacheStore 根实际为 cache/native（118），其中 ArtifactPut 单文件上限 64 MiB（81）；不能用此 opaque artifact 上传接口安装 256 MiB 运行包。 |
| `toolchain/launcher.rs` | install/update 先下载后调用 MachineManager（27）；当前 update 仅 core+skill，target 命令尚不存在。 |

现有清单并非全目录严格白名单：verify_artifacts 验证声明文件，core_libraries 对 lib 目录另有精确 closure 检查。拆分后不能把“目录存在”或只读 root metadata 当作签名证明。所有消费者仍需验证所属声明和文件字节。

### 建议的数据契约：一个签名根清单

将现有根清单明确升级新格式，保留 `version/platform/artifacts`，增加有界 `extensions`。`artifacts` 仅基础必需项；`extensions` 每项为 `{kind, target, artifacts}`。enum `ExtensionKind = Runtime(Ecosystem) | Build(Ecosystem) | Target`；身份为 `(version, host_platform, kind, deployment_target, signed_catalog_sha256)`，内容身份可对该项规范化完整描述计算 SHA-256。根签名覆盖所有扩展文件完整 path/bytes/sha256，不需要第二套扩展密钥或可变 unsigned receipt 作为信任来源。

基础内容：core、LLVM 完整 private closure、本机四 profile runtime、host sandbox、bootstrap、skill/templates。Target(ARM) 包含 ARM native 四 profile/CRT 与 ARM sandbox。Runtime(pip/npm/go,target) 与 Build(pip/npm,target) 独立；Go runtime 已包含 host 工具和 target stdlib，不强造 build/go。无基础文件与扩展间路径重叠，也无扩展互相路径重叠。

根清单 validator 统一校验版本/host、枚举目标组合、重复 ID、规范路径、lowercase SHA、数量/字节溢出、每包上限与全部描述上限。按固定类型限制 logical path：runtime/<ecosystem>/<target>、build/<ecosystem>/<target>、Target 对应 runtime/<target> 与 sandbox/<target>。防止扩展声明覆盖 core、lib、bootstrap、skill 或另一个 target。元数据顶层需 deny_unknown_fields；Python 同步语义，不维持兼容分叉。

外部 asset 名从固定 enum 计算；客户端与 manifest 不传任意 URL。每包一个 tar.zst；基础清单和签名单独发布；bootstrap 解码器作为原始文件单独发布并由根清单绑定。先签名固定原始内容，再压缩；确定性 USTAR 排序、mtime/uid/gid 和完整文件集继续成立。

### 扩展存储、接口与权限

推荐 `/opt/dever/cache/extensions/<version>/<catalog-sha>/<kind-target>/`；位于当前 service 可写范围内，和 `cache/native` 的可清理缓存分开。扩展先在专用 owner-private staging 中完全解压/验签/校验语义，fsync 文件和子目录，再 NOREPLACE rename 到最终路径。终态目录 0755、数据 0644，root-owned、不可被组/其他用户写；无需把 /opt/dever/cache 或 state 改成 0700。终态存在时重验内容而不是静默覆盖；失败仅清理自己 staging。manifest 摘要不同但版本相同必须失败，不创建并行“修正版”身份。

```rust
enum Operation {
    // 现有操作……
    EnsureExtension { version: Version, extension: ExtensionId },
}
struct ExtensionId { kind: ExtensionKind, target: BuildTarget }
// 网络/落盘仅 daemon 或拥有机器写权限的更新 owner 调用。
fn ensure_extension(layout: &Layout, version: &Version, id: &ExtensionId) -> Result<(), String>;
// 只读；不得打开 root-only install.lock，也不得下载。
fn resolve_extension(layout: &Layout, version: &Version, id: &ExtensionId) -> Result<VerifiedExtension, String>;
```

IPC 只接受已安装版本与其已验签目录声明的有限扩展，返回成功/内容身份而非用户指定路径。复用内核 UID/admission 和严格官方 HTTPS 下载，按请求限时、压缩/解压字节和数量限额；拒绝 caller 提供 URL、任意 filesystem path、payload 或 trust key。没有资源时 readonly resolve 返回准确 `dever lib install ...` 或 `dever target add linux-aarch64` 准备命令；run/build/check 不隐式进入 ensure。

采用一个 `SignedResources` 只读 owner，负责 root catalog、路径归属和已安装 extension 定位，供 InstalledRegistry 与 native pack 共用。`read_artifact(logical_path)` 与 `artifacts(prefix)` 只看到选定基础/扩展文件；避免构造 unsigned merged ReleaseManifest。RuntimePack::load 接收其实际资源根（base root 或 Target extension root），保留 logical path 到签名 artifacts 的逐项绑定。

SignedResources 需同时接入 libs.rs runtime/tools/sandbox_resources、service/compile.rs verify_pack、compile.rs worker RuntimePack::load。LLVM 动态库仍从 core 根加载。daemon BuildIdentity 必须包含所选 target pack 的签名/内容身份；不要让未使用扩展改变本机编译 cache identity。cache hit 仍重验所选 target pack。

### 更新与原子性

在同一个安装事务 owner 下读取 active 版本和其已安装扩展 ID 集合，验证新根清单有对应完整定义，下载并准备新 core+skill+这些扩展，最终重验后才 append active-version。失败保持旧 journal、旧 core 和扩展；已完整准备的新 immutable bytes 可留待重试复用，不声称更新成功。不自动安装新清单里所有扩展。

扩展选择直接由严格校验后的已发布目录集合导出，无需增加一个独立可漂移 selection pointer。若更新期间允许并行 ensure，必须在提交前锁内重新读取旧 active 扩展集合并补齐；更简单是将 ensure 与 update 的集合采样/发布串行化，避免丢失更新期间新增的扩展。沿用现有持久 inode lock，但不能在持有 exclusive lock 时递归调用再次 acquire 的公用 API；拆 locked 内部路径。当前锁为 try_lock，冲突会明确失败；如需“并发两项目都成功”应有限等待并在获锁后重验，不引入无限等待。

私有 staging 与 release.cleanup_staging 的全目录清扫不能冲突：若沿用 cache/staging，所有操作必须持同一安装锁；独立下载阶段则必须独占子树并禁止被旧 cleanup 删除。cache clean 目前只清 cache/native，保持其不删除 installed extensions。卸载版本时同安装锁删除对应扩展命名空间，active 保护不变。

当前 `dever update` 不更新机器 `/opt/dever/bin/dever[d]`，只更新版本目录/active。新资源 IPC 必須在首装 bootstrap launcher/daemon 中就存在；后续若改变 IPC，旧 daemon 兼容性是单独的 bootstrap 生命周期问题，不能假设 active core 更新自动重启了新服务。

### Zstandard：统一 Rust 解码，Python 验证后执行已有 launcher

推荐 zstd crate 的安全 streaming wrapper，静态编入发行时所需 decoder library；不要手写 C FFI 或 ctypes 依赖宿主 libzstd。Rust release downloader 与 launcher 隐藏 extraction 入口复用一个 codec/archive 模块：固定 `window_log_max(27)` 对应 128 MiB、压缩输入上限、解压输出上限、精确 frame 结束与 EOF 检查。不得仅 tar 遍历到结束标记就成功，必须排空/校验 zstd frame 尾部，拒绝截断与尾随额外数据；默认 Decoder 会连续解多个 frame，单 frame 策略需显式实施。

首装 Python 保留 OpenSSL 签名锚：下载/验证根 manifest → 从基础 artifacts 的固定 bootstrap/dever 与 bootstrap/lib/* 集合，按 digest 命名的 raw assets 下载全部 launcher closure → 精确大小与 SHA 验证、权限落盘 → 以空环境、绝对路径执行隐藏 `--dever-extract <owned-config-root>` → launcher 再验证根清单签名并调用共享 Rust USTAR 解包 → 完整已校验后执行 bootstrap 安装。配置与 trust key 仅放 root-owned scratch，不从应用读取。raw helper 文件与 payload 目标必须是两个目录，避免 create_new 冲突。需要 subprocess timeout 与失败后 kill/wait，不能把解包失败当成可继续安装。

现有 staged manifest 声明 `bootstrap/lib/libgcc_s.so.1`（`target/release-0.1.0/assets/dever-linux-x86_64.manifest.json:27`）。因此直接下载裸 launcher 不成立；但按主 agent 收敛方案，额外发布已签名 raw launcher + 完整小型私有 lib closure 后，复用 launcher 优于新 decoder bin：没有额外执行文件/第二套 tar validator。raw asset 文件名由既有 SHA 推导，不能由远端任意 URL 或目录控制。作者阶段仍需验证 ELF DT_NEEDED、RPATH、完整私有 closure，确保 zstd 为静态且 LLVM 不被引入 launcher/daemon；Linux libc/loader 为已声明 OS ABI。Python 在执行前只能信赖独立签名和已固定路径规则，签名发行 maker 对 closure 的检查仍是必要发行门槛。

Python SDK maker 可以使用作者提供的 zstd 工具或编码库完成 deterministic streaming tar.zst；这属于 author 工具链，不把系统 zstd 加成最终用户前提。公共资产总大小应单独计入 raw bootstrap closure，而不是只报告主 tar.zst。

### External references

- [zstd Decoder API](https://docs.rs/zstd/latest/zstd/stream/read/struct.Decoder.html)，本次页面为 0.14.0：Read streaming、single_frame、window_log_max；finish_frame 错误需要显式处理。版本选型应核对仓库 MSRV 并固定 Cargo.lock，本次没有安装或锁定依赖。
- 本地 SDK `target/backend-sdk/usr/include/zstd.h:107` 为 1.5.5；`:606` 说明 streaming window 上限控制、`:648` 提供 DCtx 参数 API。这是本地已有头文件证据，不代表最终打包采用此版本。
- GitHub zstd-sys Cargo.toml 页面读取失败；静态链接需在实施中核对选定 crate build 配置与产物 ELF，不能将本研究当作已验证链接证据。

### Related specs

- `.trellis/spec/backend/toolchain-and-library.md:23` compiler-relative runtime 与 daemon 签名闭包；`:27` bootstrap；`:84–96` release/skill/author/ELF closure；`:337–343` onboarding、native release、shared toolchain 定向验收。
- `.trellis/spec/backend/index.md`：本仓库是 Rust 语言编译器，不采用 Go Dever Model/Page/Service 规则。
- `.trellis/tasks/10-04-slim-distribution/prd.md`：离线 run/build、多用户共享、按需自动生态、target 命令、core+已装扩展更新、不改宿主服务。

## Caveats / Not Found

- 子 agent `task.py current --source` 返回 none；主 agent 已明确指定此任务与写入位置，未修改任务状态。
- 本次未下载发行/依赖、未运行构建/测试、未改产品文件/服务，也未读取私钥或授权凭据。仅查看公开解码 API 文档。
- 120 MiB 目标是已有压缩估算；codec 与新正式优化 core 会改变最终数字，尚无本次测量。
- 精简需同时调整 package/Lib 显式准备策略：另一研究分支已发现 `source_workers_with_packages` 在 resolve_project 前读 runtime；不能只在 resolver 内 ensure。remove/doctor/check/run/build 必须 readonly。
- 更新携带范围建议 active 版本已安装选择；若产品要求机器所有历史 pinned 版本的 union，应在设计明确，不应默默给新版本安装历史上任何扩展。
- 大包首次安装、损坏重试、多 UID 同时 ensure、更新失败仍运行旧版、ARM cache hit 校验、decoder 超大 window/截断/尾随数据均需定向验收；本研究没有执行验证。

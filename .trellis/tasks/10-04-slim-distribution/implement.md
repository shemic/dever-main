# 实施记录

## 已确认

- 用户已批准完整五项方案及任务创建/继续实施，无需重复确认相同范围。
- [x] 工作区起点核查和备份，`target/distribution-before.10dsis`，gzip 校验通过。
- [x] PRD 记录用户要求、范围、回滚和验收，非语言迁移任务。
- [x] research/release.md 和 research/libs.md 完成，按实际 owner 完成 API 对接。
- [x] 实施上下文配置、任务进入 in_progress。

## 实施顺序与责任

1. 发行/扩展 worker：根 catalog schema、Zstandard Rust owner、资源视图、共享扩展安装 IPC、target 命令、installed-extension update、Rust 隐藏 extraction 入口；拥有 toolchain 下相关模块、launcher binary、CLI Cargo dependency 和对应定向测试。不要修改 author packaging、SDK Python、skills 或 Lib resolver owner。
2. Lib worker：锁/收据当前格式与精确恢复、add/update/install/Package preparation scope、InstalledRegistry 资源读取接线、CLI lib help 与相关测试；拥有 libs/**、必要 workers 资源消费/Package 路径及 main.rs。与发行 worker对接 typed extension API，不重复实现下载/安装。
3. 主会话：author packaging 接 catalog、SDK asset publisher、Python 首装、skill/docs、最终构建及真实发行验收。三个 owner 先固定接口，避免同文件改动；若遇所有权交叉先沟通。
4. trellis-check 独立检查整个改动范围并修复；主会话核验关键结果，更新 canonical spec 和记录，然后按已有授权处理 Git 与可用的发行发布。

## 验证清单

- [x] codec/manifest：10 定向通过；bootstrap执行位另有断言。
- [x] install/cache/update：4 定向通过（并发/损坏/更新失败/已装集合/closedIPC/typedmiss）；实际 UID65533/65534 共享扩展通过。
- [x] lib install：4 cold restore、2 npm cache/projection、1 Package exact closure、PEP517 nested replay、npm optional replay 定向通过；实际安装版三生态无 Lib Worker 的锁恢复/运行/构建通过。
- [x] 运行/构建缺资源只提示，不联网；target add 支持 ARM，实际 ARM64 ELF 交叉构建通过。
- [x] SDK/Python bootstrap 10/10、skill quick_validate、全部修改/新增 Rust 文件 rustfmt check、CLI lib/bins/author examples clippy -D warnings/type check 通过。
- [x] 正式 release 构建、core/launcher/daemon ELF closure、保留 runtime 档案链接符号。
- [x] 实际签名包基础首装：普通/Markdown new/fmt/check/test/run/build；不含可选生态和 ARM 文件。
- [x] 实际扩展准备/锁恢复/跨项目复用、ARM 构建、已准备资源离线和独立应用执行。
- [x] 安装/更新失败恢复和成功更新：定向回归通过；真实负载本地后继 catalog 的 4 阶段更新事务验收通过。公开网络单独阻塞，不混同。
- [x] 清理本次 owned 临时产物，保留 durable assets、hash、验收 JSON 和源码备份。
- [ ] 最终 diff/self review、任务进度、Git 提交/push 和 Release 状态准确记录。

## 构建资源

起始磁盘约 0.78 GiB 空闲，/dev/shm 约 1.4 GiB 空闲。必要时可回收上一轮由本会话创建的隔离安装镜像（正式包与验收记录已持久保存且先复核），不可删除其他项目、源码、keys、未知 cache 或实验。使用显式 Rust toolchain 路径，保留现有共享 target，Cargo 串行 -j1；不因磁盘不足跳过必要验收或宣称完成。

## 当前证据

- 首轮 SDK/Python installer 定向 10/10，通过基础/扩展资产分离、真实 Ed25519 验签、raw bootstrap 摘要/长度/私有库、发布竞态、流量边界。Rust 接管的归档拒绝测试由发行 worker 补齐，未以 Python mock 代替实际安装验收。
- Cargo 新增静态 zstd 0.13.3；工作区版本 0.1.1，release strip=symbols，runtime-pack 仍显式 strip=debuginfo。正式串行构建目录 `/dev/shm/dever-slim-cargo.PvjPZZ`，未运行全量 CI。
- 旧 v0.1.0 完整 archive SHA-256 重新核对为 `7b56e825fb2b0468a271276bffbd84775e9dd9ae219f7e16097d1ff3df22b1cc` 后回收 owned 1.5 GiB 临时镜像。正式旧包/验收记录/源码备份均保留。
- Cargo 自有 `clean -p dever-cli -p dever-core --profile dev --target-dir target` 回收 898.3 MiB 旧编译产物；仅 native-acceptance-init 验收助手另存 `target/release-0.1.1/tools/`。源码/依赖缓存/其它项目/全局服务未清理。
- 真实本地 sandbox PEP517 nested replay 142.15s、npm optional replay 61.70s，均通过。原生maker 25/25默认定向通过，10个独立重验收未在这条命令启动。旧cross sandbox fixture已补齐同组native input；不是删除失败检查。
- trellis-check 独立审查未发现其他Rust合同问题；主会话修复其SDK bounded metadata/path长度/namespace/hostTarget差异，补文件/目录重叠拒绝。主会话另修复解压bootstrap执行位。clippy唯一is_multiple_of诊断已修复并复验通过。
- 原正式build先完成了全部依赖和CLI类型检查；在最终ELF链接前因上述代码修正主动停止（exit130）。最终源码 release 构建已成功（6m39s），产物保存到 `target/release-0.1.1/tools/`。core 16,951,872 字节，launcher 3,330,288 字节，daemon 3,310,296 字节；ELF 已 strip，launcher/daemon 仅需私有 libgcc_s 和 OS ABI，core 的 LLVM/private closure 随包提供。

## 真实 0.1.1 产物与验收

- 已制作实际签名 v2 目录与全部 split assets。基础 `.tar.zst` 为 117,481,208 字节（112.04 MiB），SHA-256 `fbf04562012b40038c8d845dcf528070606b4fd8374edec34c6516ea4afdf167`；基础文件解压合计约 573 MiB。首装另有 raw helper 3,513,312 字节（3.35 MiB）和约 22 KiB 元数据。全部可选资产合计约 596.59 MiB，不代表默认下载这些资源。
- `target/release-0.1.1/sizes.json` 记录每个资产的字节数/摘要；`provenance.json` 记录显式输入及未修改 runtime 档案复用。host glibc 最低 2.39，ARM64 为应用交叉目标。
- 首次实包验收到普通/Markdown 项目和缺 ARM 资源提示均通过；跨 UID 步骤因 outer bwrap 单 UID 映射不能 setgid65534 而失败。修复的是验收夹具：使用既有测试模式，在私有 machine 启动独立 daemon，以真实 UID65533/65534 检查共享资源。产品代码/签名资产未改；重新从 durable assets 验收。
- 更新验收另用真实负载签署仅供本地测试的后继版本，传输指向本地真实扩展。Ed25519 PKCS#8 v2 由与 maker 相同的 ring 签署，不再使用不支持该作者密钥格式的 OpenSSL 私钥读取；正式安装的 OpenSSL 公钥验证未变。最终 `update-acceptance.json` 为 passed，4 阶段通过：安装/扩展基线、失败保留旧活动版本且项目可运行、成功激活核心/skill/恰好已装扩展、切回真实版本后项目继续运行。仅供验收的 0.1.2 catalog 不能冒充 0.1.2 编译器，托管编译器严格拒绝目录版本与编译版本不同；没有关闭这项产品校验，也不声称虚拟后继版本执行通过。
- 实包从 durable assets 复验最终 `acceptance.json` 为 passed，13 阶段全部通过：签名首装、可选资源排除、公共入口/版本、普通与 Markdown 各自完整新项目流程、skill、离线缺资源、ARM64、三生态、双 UID、五个程序脱离源码/机器目录且清空 Worker 缓存后执行。全程未启动全局服务、未替换全局命令；本地传输不表示线上更新通过。
- 独立 trellis-check 对新增验收夹具/作者助手再次限定复核，无阻塞。产品验证合计 Rust 48 项、Python 10 项；正式实包 13 阶段、更新事务 4 阶段通过。fmt/clippy/skill 检查通过；未运行无关全量 CI/压力测试。
- 仅为回收本轮已完成的测试产物，Cargo clean CLI dev profile 再释放 376.8 MiB；正式包、作者输入、源码备份保留。
- 所有本轮隔离安装镜像均已卸载，自有 daemon/namespace 已退出；空临时目录已移除。正式构建临时目录核对仅含本轮 Rust 缓存/产物，保留工具与正式资产后清理约 556 MiB tmpfs。该目录没有新版 Cargo clean 要求的 CACHEDIR.TAG，未补造标记，核对归属后按精确目录移除；可从源码重建。
- GitHub SSH 源码访问可用；GitHub Release API 尚未登录。不能把源码/tag push 或本地模拟传输当作公开发布及线上更新。

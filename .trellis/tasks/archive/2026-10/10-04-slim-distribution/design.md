# 精简发行设计

## 修改边界与复用

现有 `packaging` 统一签名全部资源，`release_source` 下载单一 gzip，`MachineManager` 原子安装版本，daemon 保护共享缓存，`InstalledRegistry` 从安装版本取工具。改动必须落在这些 owner：不向 App/业务配置加入安装概念，不重复实现每种生态的下载器/进程监督器，不把 Rust 源码编译要求转给使用者。

保留源码签名、路径/类型/摘要校验、四种数据库 profile、系统服务权限范围、离线编译、失败前置和版本 journal 的既有语义。CLI/安装/缓存/SDK/skill 的改变属于已确认五项方案；语言语法、模型、业务运行时与其他宿主平台不变。

## 发行目录与认证

- 升级当前发行 schema：签名根 manifest 包含基础 `artifacts` 和封闭类型的扩展目录 `extensions`。每个扩展绑定 kind、target 和完整 artifact 列表，身份由根 manifest 精确字节摘要与扩展 ID 共同确定。单一根签名认证全部描述，不能信任单独下载的无绑定元数据。
- 基础包包含 host native runtime 四 profile、core/lib、bootstrap、host sandbox、skill；Runtime(pip/npm/go)、Build(pip/npm)、Target(linux-aarch64 native+target sandbox) 分开发布。纯 Python/npm 包不触发 Build。
- author maker 继续只复制/校验/签名显式输入。预备发行目录可同时包含待发布扩展，SDK publisher 输出 base `.tar.zst` 与每种扩展 `.tar.zst`，文件列表来自同一签名目录。既有 v0.1.0 tag 不覆盖，正式新产物使用新版本。
- Zstandard 固定长窗口上限 27、流式解压、压缩/解压/文件数/单文件/尾部边界仍受限；不以签名成功替代归档检查。Rust 使用维护中的 codec 库安全接口，不手写 codec 或新增 workspace unsafe。
- Python 首装沿用独立发行公钥+OpenSSL 认证根目录，再下载并逐字节校验 raw bootstrap launcher 及其已声明私有动态库到独立 scratch。使用已认证 launcher 的隐藏 extraction 入口解压 `.tar.zst`，然后运行现有 bootstrap；使用者无需系统 zstd。完整解压/路径规则复用 Rust owner，避免保留两套 TAR 安全实现。SDK 发布上述 raw blob 供引导，所有可执行字节必须在执行前经独立密钥和摘要验证。

## 扩展安装、读取与更新

- 资源存储位于现有 service 可写的 `cache/extensions` 下，按已安装签名 manifest 身份与扩展 ID 建立不可变目录；不扩大 systemd 写范围，不写用户家目录，不修改版本 core 树或签名数据。
- 新 IPC 只接收 `version + ExtensionId(kind,target)` 封闭枚举，不接受 URL、路径、脚本或解压参数。官方 HTTPS origin、redirect allowlist、字节/时限/并发限额由工具链固定。外部用户复用现有 peer authentication/admission，只能准备已安装可信版本目录里声明的扩展。
- 复用持久 install 锁，下载在私有 staging，验证完整列表和字节后原子 no-replace 发布；竞态仅在完整同身份校验通过后复用。缺失是准备提示，损坏必须拒绝，不能悄悄掩盖成 cache miss。
- 单一只读资源视图统一 base 与已安装扩展路径，所有 worker runtime/build/sandbox 和目标 runtime 读取共用它；编译和缓存命中不得触发下载。源 manifest 不被拼接改写成伪造签名数据。
- `target add linux-aarch64` 对当前 active 版本显式准备 target 扩展；Lib preparation 额外准备实际使用生态对应 target。目标是应用 ARM，不要求 ARM 宿主编译器。
- `update` 准备新 base，并把当前 active 版本已经安装的扩展选择全部准备到新版本身份下，再写同一个 active journal。任何前置失败保留旧 active/core/skill/扩展，已安装资源不重复下载；未使用扩展不被顺带安装。

## Lib 恢复

- 显式 add/update/install 及 Package 中真正准备 Lib 的动作进入同一 preparation scope。先取得未绑定 Worker 声明，按需准备 Runtime，然后通过现有 owner 绑定和解析；普通 check/doctor/remove/run/build 保持只读或既有离线语义。
- Build 仅在 resolver 已确认 sdist 或 npm hook 必需后由统一 build-input owner 准备，不能根据生态名称一律下载。
- `lib install` 消费当前格式准确锁：验证项目、Package 传递依赖与 checked Worker binding；缓存命中验证完整 bytes/hash；缺失时按锁定精确版本/源定位下载，不选择新版本。完成后 setting/lock 字节保持一致。
- 为原生构建收据补全准确源 locator/文件名、固定 build requirement closure、原始 npm 执行图和编译输入身份；当前格式一并迁移仓库 fixtures，不引入旧格式 fallback。重放只能使用锁定输入；最终产物必须符合原收据 bytes/hash，不能用不同产物改写原锁。不可重现明确失败。

## 质量与操作边界

主任务串行协调 Cargo，worker 不并发创建新 target。测试只选择发行/资源/Lib/安装路径，真实验收使用 owned 镜像和进程，不修改宿主服务、安全策略或真实数据库。正式优化构建与四种 runtime 档案来源分开记录。压缩收益不与重复内容字节直接相减；实际发布前报告 base download/install、raw bootstrap 开销及扩展增量。

公开 Release 和线上更新独立于本地签名包验收。若缺 GitHub API 登录，先完成可审查的源码与产物，再报告唯一外部阻塞，不能以 tag/源码 push 代替 Release。

# Linux ARM64 应用交叉构建设计

## 边界与复用

只有应用部署目标变化，编译器/daemon/build tools 仍在当前 x86 宿主运行。复用 LLVM bridge 的 Target、现有 pack 清单/签名验证、NativeProgram 发布、Lib resolver/lock/Worker 以及 CacheStore，不建立平行流水线。主机平台身份继续用于 launcher/daemon 验证；新增独立、封闭的构建目标值，不能全局替换 platform_identity。

## 目标数据流

CLI build 参数 → 明确的 BuildTarget → 目标 Lib 资源准备 → CompileRequest → daemon 验证目标 pack 与缓存 → worker 加载目标 runtime → LLVM object/LLD → 独立程序。私有编译走同一目标与 pack owner。run/test 在入口固定使用 host；应用配置不保存编译器状态。

toolchain/target.rs 拥有 BuildTarget（LinuxX86_64、LinuxAarch64），严格解析 linux-x86_64/linux-aarch64，提供 host/platform/triple 映射。共享库不引用 bridge，避免 launcher/deverd 引入 LLVM。原始六目标 bridge 保持独立。

每个 runtime/<target>/manifest.json 验证实际目标与 compiler_version/ABI；所选目标的整个输入闭包必须纳入安装签名，缓存键包含目标和该 pack 身份。pack 制作器接受显式准备的目标运行输入，同时只验证/打包 x86 宿主核心，不要求构建 ARM 核心。

## Lib 与宿主执行

资源路径、runtime/构建收据和 ELF 校验使用部署目标；实际启动的编译工具使用构建宿主。Go 交叉编译工具必须在 x86 执行且输出 ARM，标准库/importcfg 和源码 tags 选择 ARM。Python wheel 与 npm addon 遵守目标 ABI；跨目标源码构建不能把 ARM 工具直接作为 x86 可执行文件启动，也不能无条件接受宿主生成的收据。共用现有显式 Lib 准备入口传递目标，离线 build 只消费完整、已校验的目标材料。lock 模式如需变化直接迁移仓库合同/fixture，不增加旧格式回退。

源项目与 Dever Package 的业务声明不增加架构分支。只有明确的部署目标选择和作者工具输入区分架构。缺少目标资产给出明确准备错误，不能退回宿主资源或无沙箱执行。

## 验证与运维

作者资源/模拟器解包到本任务专用目录，使用绝对工具路径与校验摘要，不全局安装。所有 Cargo/原生构建由主代理串行执行，优先复用现有缓存，开工前检查磁盘与内存预算。跨编译测试验证 ELF、执行结果及坏输入拒绝；共享编译验证目标隔离和命中前完整性校验。模拟 ARM 内核只在任务自有进程/镜像内运行，不修改主机 AppArmor 或已有服务。

ARM 真机吞吐和不同硬件/内核兼容性不从模拟结果推导。本次不重新跑既有全量矩阵或长压测。

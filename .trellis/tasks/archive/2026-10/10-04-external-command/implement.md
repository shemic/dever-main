# 实施与验证

## 执行顺序

1. [x] 读取规范、调查编译/运行/沙箱/打包，固定用户已确认范围和合同。
2. [x] 备份当前源码；并行实施编译器合同及runtime进程执行，主线程实现CLI资源打包。
3. [x] 串行运行必要offline Cargo检查和定向测试；修复真实失败。
4. [x] 用独立ELF与真实Dever源码验证check/run/build、移除源码运行、动态库、取消和完整性；验证ARM目标。
5. [x] 独立Trellis复核、清理本任务冗余、更新语言规范及验证证据，归档；不提交Git。

## 所有权

- 编译代理：core、library/dever/process.dever、runtime/wire.rs、bridge ffi/runtime.h、对应编译/codec测试。
- runtime代理：runtime component/command/external/config、sandbox及进程测试，不改wire.rs和CLI。
- 主代理：CLI workers/libs/ELF闭包/Package必要集成及测试、任务/文档、全部Cargo和重资源验证。

代理不并行运行Cargo，不覆盖其他代理或用户改动。涉及共同合同先沟通。

## 验证原则

仅本任务所需定向检查，无全量测试/业务服务/长测；提前说明自有沙箱进程测试。先check与轻量测试，后更新实际runtime ABI再运行真实源码，不能用旧archive验证新codec。开始可用磁盘约3.3GiB，复用现有目标目录，串行构建。

预期目标：port_adapter、command专用runtime/CLI测试、external_workers/resources、llvm_external_source中command案例；原Worker回归按实际共享代码影响选择。fmt/clippy只覆盖改动所属目标。错误、跳过和未运行独立记录。

## 最终证据

- 改动前备份：`target/external-command-before.lBL9wU/source.tar.gz`。
- CLI/core/runtime 初次 offline check 已通过；command 编译/codec/format/fake 5项通过。
- external_workers 当前13项通过，10项需特定资产的旧真实生态测试未运行。command 打包4项通过，后续新增越界 RPATH 回归需复验。
- runtime 首轮3过5失败为未优化debug哈希耗时超过fixture启动等待；作者构建为sha2/miniz_oxide/flate2指定opt-level=3，测试等待与命令deadline分离。随后即时文件锁断言揭示真实namespace清理竞态：管道EOF早于内核后代回收。最终改为info-fd + block-fd门禁，在放行guard前pin namespace pidfd并核验PPid，取消等待pidfd readiness后回收wrapper。9/9定向测试通过（17.86s），包括启动错误、无FD泄露、非零1/127、输入输出、限额、取消/超时/root close/queued/explicit shutdown。
- 独立复核发现启动失败与非零退出混淆、RPATH越出程序树两项问题；已修复，复核确认无剩余阻塞。新增RPATH回归4/4及共用Python ELF闭包2/2通过。
- 旧 Port 原生测试先因测试作者 rustc 路径缺失中断；指定作者工具路径后相关测试运行，期间新增sandbox直接依赖导致 --locked 拒绝。Cargo offline check已增量更新锁，不升级第三方包；最终21/21复验已关闭这些失败。
- 新runtime ABI archive已离线重建（runtime-api/external/sqlite/postgres，3m25s），真实LLVM/CLI验收使用该新档案；未连接任何数据库。改动Rust格式检查及C fixture语法检查通过。
- LLVM command对象+真实调用2/2通过（102.33s）；六目标对象生成，预热+64轮每轮3次普通程序调用，二进制/EOF/退出7和allocation ledger全通过。初次60s fixture预算仅完成42轮，给command probe单独180s预算后保留全部65轮与相同断言；原Worker预算不变。
- native_runtime_link 的Bytes wire C ABI验收1/1通过（7.18s）。
- signed_cli_runs_and_packages_ordinary_command_with_private_library 1/1通过（172.02s）：当前核心和新ABI、真实C动态工具+私有.so、check/run/build、无Lib锁、Package锁定资源、删除项目/作者/机器目录后两种产物独立运行、篡改.so拒绝。所有daemon/程序均为自有临时fixture，未安装全局命令。
- 最终旧Worker生命周期1/1、external_resources 8/8通过；1项旧Rust bootstrap打包用例维持ignored，本次已有新的真实LLVM/签名CLI独立打包证据。
- 最终Port/Adapter（包含普通/fake实际原生执行）21/21通过（103.41s），此前作者路径/锁文件导致的失败已全部关闭。
- Linux ARM64 runtime-api/runtime-external桥接库、runtime、sandbox/guard 交叉cargo check通过（65s）；结合六目标LLVM对象生成与CLI跨目标打包/拒绝错架构验证。未重发四个ARM发行profile、未运行ARM实机/虚拟机验收，不能将这些检查表述为ARM执行通过。
- 所有改动所属库、guard及6个相关测试target的clippy `-D warnings`通过（45.21s）；Rust定向fmt和C语法检查通过。git diff --check通过但本仓库核心文件为untracked，实际差异还通过改动前tar对比、逐文件检查与独立复核覆盖。

## 最终检查入口

Cargo均为offline、单并发，不使用全量测试。作者工具路径仅影响构建/验证进程，未加入应用环境配置。作者测试使用`profile.dev.package.{sha2,miniz_oxide,flate2}.opt-level=3`减少大量重复资源校验耗时。

- `dever-tests --features external --test port_adapter`：21通过。
- `dever-tests --features external --test external_command`：9通过。
- `dever-tests --features external --test external_component --test external_resources`：1+8通过，1旧bootstrap用例ignored。
- `dever-cli --test external_workers`：13通过、10个旧真实生态fixture ignored；最终command过滤4通过。
- `dever-cli --test external_libs wheel_tests::native_`：2通过。
- `dever-backend-bridge --features embedded,runtime-external --test llvm_external_source command_ -- --include-ignored`：2通过。
- `dever-backend-bridge --features embedded,runtime-external --test native_runtime_link wire_codecs_validate_slots_before_callback_and_release_partial_values -- --ignored --exact`：1通过。
- `dever-cli --test native_release signed_cli_runs_and_packages_ordinary_command_with_private_library -- --ignored`：1通过。
- ARM交叉check使用现有`target/arm64-cross`作者sysroot/工具，profile runtime-pack，target aarch64-unknown-linux-gnu；检查sandbox和bridge runtime-api/runtime-external。

未运行：全量测试、长压测、真实数据库、ARM目标执行及Windows；未替换全局安装或修改主机策略。当前无已知本任务阻塞。原有两篇tracked文档删除及其他既存任务保持原状。

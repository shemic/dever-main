# 执行记录

## 已确认范围

用户已批准 x86 构建 ARM 应用及创建任务继续实施。最终方案沿用已确认目标，没有 ARM 开发工具链/公开发行/长压测。旧生产化父任务的其他平台延期范围，仅为本任务的 Linux ARM 应用部署这一项解除延期。

## 收口结果

本任务范围全部通过。最终汇总为 `target/arm64-cross/acceptance.json`，逐项绑定构建日志、运行日志和当前产物SHA。最终完整ARM内核模拟日志为 `target/arm64-cross/logs/guest-1791082810005965465.log`：Python、JavaScript、Go各自成功/故障/恢复/清理全部通过，HTTP返回`arm-http`，guest退出0，222.57s，无超时。四profile、私有/共享编译、缓存隔离/篡改拒绝和SQLite真实CRUD由此前独立验收覆盖。

本机构建、41项初始定向回归、Lib 73项、Worker 9项、迁移后的真实Go构建运行1项、Python/JavaScript SDK合同、作者Python输入2项及相关Clippy/格式检查通过；这些测试集有交集，不相加成独立总数。最新Python签名构建201.28s通过，包已包含最终reader及completion清理修复。

没有ARM真机；未运行ARM真机性能/长测、真实PostgreSQL服务器或全工作区CI。模拟器验证功能，不推导真机吞吐。Python sdist/npm hook跨目标仍需事先准备匹配目标的输出和完整收据；纯Python/npm与Go交叉构建已实际覆盖。产品仍不隐式联网或从环境变量/PATH读取语言工具。未安装/替换全局命令、修改主机AppArmor/sysctl或业务服务，未提交Git。

## 实施

1. [x] 调查 CLI、runtime pack、daemon/cache、Lib/Worker 目标链路，建立可审查任务。
2. [x] 编译链路：BuildTarget、build 参数、CompileRequest、runtime/签名/缓存及打包者，配套回归。
3. [x] Lib 链路：显式目标准备、lock/resource/runtime/Worker/build pack 的宿主与目标分离，错架构拒绝与回归。
4. [x] 准备可复现 ARM 运行输入与模拟器；构建四 runtime profile 和所需生态资源。
5. [x] 串行定向检查，执行 ARM 真实应用/三生态模拟验收及 x86 最小回归。
6. [x] 独立复核、修复实际问题，更新规范/CLI 文档/执行证据，清理自有进程与临时配置，不提交。

## 所有权

- 编译实施代理：CLI main/compile/worker、toolchain target/runtime_pack/compilation/service/release/cache、packaging.rs 主文件与编译相关根 test。
- Lib 实施代理：libs.rs 与 libs/**、workers.rs 与 workers/**、packages 的传递 Lib 必需接线、packaging/{ecosystems,builds,sandbox}.rs、生态相关根 test。CLI 参数集成通过主代理/编译代理协调，禁止重写对方文件。
- 主代理：任务/规范/SDK/README、作者 ARM 资源和模拟验收 owner、所有 Cargo 和重资源命令、最终集成。

## 检查原则

先参数/目标映射/清单/缓存/锁的定向测试，再用真实 ARM 产物执行。编译命令明确 offline/locked 和单作业；不让子代理并行 Cargo。真实业务服务仅在自有临时项目/端口或模拟机里启动，提前说明。失败保存证据，不放宽断言、不能将未准备目标或模拟器缺失标成通过。

初始磁盘可用约8.2GiB；现有 debug 3.1GiB、参考 runtime 948MiB 均保留。准备资源前重新核算，不删除用户不明文件、不恢复旧长测。

## 过程与证据（按执行顺序保留）

- 编译与 Lib 两条接线已实施，新增 BuildTarget、请求/缓存目标绑定、显式目标 Lib 准备及 ELF/Go archive 检查；尚待 Cargo 与真实产物验收，未将代理报告记成通过。
- 作者工具均在 `target/arm64-cross/`：Rust 1.98 ARM std、GCC 13 ARM cross、QEMU user/system、同版本私有 rustfmt/clippy。未安装全局工具。Ubuntu 包按现有受信 metadata 的 SHA256 验证，ARM OS 库索引另外通过 Ubuntu InRelease 签名验证。
- ARM base 和 sqlite 已使用 `runtime-pack` 串行构建成功，保存到 `archives/`；postgres/both 仍在构建。初次失败是私有 cross as 找不到同目录 libopcodes，已仅在作者 Cargo 子进程设置私有库路径修复，未加入产品环境变量。
- Python 3.12.14 ARM standalone 和 Node 24.15.0 ARM 已按上游发布摘要下载并生成目标 pack；真实 six/is-number 包及 Google UUID 现有官方校验证据供后续 Worker 验收。
- Alpine 3.22.6 ARM 内核完整系统模拟启动成功，日志 `target/arm64-cross/logs/guest-1791077299508695065.log`：`aarch64`、user namespace 存在、`DEVER_ARM_GUEST_EXIT=0`。这只是模拟机准备，不是 Dever/Worker 验收。
- author native release helper 增加目标参数复用现有 Python/npm选取/manifest；Go author fixture通过私有 Go overlay 固定交叉工具默认目标（源码未改全局SDK），待实际构建与执行。

- 四份 ARM runtime 全部通过：base 311.91s/81,418,028B、sqlite 362.84s/96,034,088B、postgres 338.03s/102,287,968B、both 237.84s/109,661,934B。总作者构建20m52s，峰值1.9GiB、无swap/OOM；每份SHA和命令在对应log JSON。`target/debug/runtime/linux-aarch64`已按真实输入准备，未改原x86 pack。
- 新host工具构建通过；定向测试39项通过：CLI参数3、native maker24、请求目标2、LLVM CLI5、共享缓存1、resolver目标1、exec架构1、managed runtime目标2。未执行的ignored项未算通过。作者Python输入测试2项通过。
- `trellis-check`独立复核修复1处多余借用，17个生产Rust文件格式检查通过；未发现目标数据流/签名/缓存的功能性阻塞。主代理继续负责实际测试，不以复核替代执行。
- 实际签名安装/shared+private ARM构建验收1/1通过（177.32s）：4共享profile + 私有base均在删除源码与机器目录后由QEMU执行成功；SQLite真实CRUD、ARM/x86缓存隔离、重复命中、命中前篡改拒绝通过。结果 `target/arm64-cross/compile-acceptance.json`，HTTP应用已导出但尚待完整guest运行。未运行真实PG服务器。
- ARM guard已按私有sysroot成功链接，sandbox OS assets准备完成；前两次作者链接失败（缺libgcc_s、GNU脚本绝对sysroot）均保留日志，修复只在私有作者输入/链接参数，不改变产品配置。Go交叉pack正在构建。

- Go交叉pack已成功（144.86s）；签名安装后的三生态ARM应用离线构建1/1通过（453.51s），各应用导出后删除源码和机器目录。完整guest运行尚未通过，构建成功不算Worker执行成功。
- 完整模拟定位两个独立问题：fixture直接从initramfs rootfs运行导致bwrap `pivot_root: EINVAL`，已改为switch_root到正常tmpfs根；随后日志 `guest-1791080132703634001.log`明确显示ARM loader为0600，guard启动Permission denied。
- 根因属隐含宿主假设/跨层传播与测试缺口：`asset_executable`只识别宿主loader，ELF架构校验正确但打包和嵌入权限错误；首轮复核未覆盖这条权限数据流。已在共享owner精确识别两种目标loader，并增加真实pack权限回归（输入0600→loader/guard/bwrap0755、库0644）和负例。目标架构校验、guard和沙箱策略未放宽；规范已补齐。主代理正在重建host工具及全部三生态产物复验，旧产物不能作为修复证据。
- loader修复后host工具重建通过，定向测试41/41通过（原39项加sandbox默认2项）；三生态应用重新生成通过（461.13s，日志 `ecosystems-1791080644145693595.log`）。旧失败应用保存在 `target/arm64-cross/failed-loader-mode/`，当前guest/apps均是修复后产物。
- 完整系统验收脚本固化在根test下 `test/ecosystem-release/arm64/{simulate.py,init.sh,bootstrap.sh,guest.sh}`；独立复核补上源码定位的预期错误、每次调用后 `/worker/*` 进程检查、流水线子进程回收，语法检查通过。完整guest正在运行。
- 四ARM静态runtime/guard沿用前述已验证构建：本次最后修复的`asset_executable`仅由宿主作者/CLI打包与嵌入路径调用，ARM运行期不调用它；重新构建host core并重生成应用资源标记即覆盖实际修复，不重复20分钟runtime构建。
- `guest-1791081118958747964.log`已得到Python `data:true`，随后Worker以134退出，验收正确判失败。宿主SDK合同用例保持stdin打开后稳定复现 `_enter_buffered_busy`：daemon读线程持有BufferedReader锁，之前测试先关stdin掩盖了退出竞争。SDK已改用asyncio管道与有界队列，finally取消并等待reader、关闭transport；修改后的整个Python SDK握手/调用/业务错误/取消/关闭/拒绝检查通过。只需重嵌入Python SDK，ARM runtime/Node/Go源字节未变；共用原签名构建fixture增加Python定向入口，不复制构建流程。
- SDK复核补齐有界队列中completion通知任务的取消/等待，`test/external-sdk/check.py`新增Python队列满时关闭、保留stdin等待退出且stderr为空的回归。Python完整合同与JavaScript原合同检查均通过。该保留stdin回归针对CPython退出竞争；JavaScript继续遵循原测试/真实host在shutdown应答后关闭stdin的流程。
- `guest-1791081776953594061.log`完整通过Python成功/故障/恢复/清理，但整个三生态模拟在180s总上限时被停止，记为未通过。模拟器需反复读取校验大型独立程序，后续有限总预算改为600s并增加每步marker；这是9次调用加HTTP的功能验收，不是长压测。
- 旧host Go fixture缺少新增的`build.host`，已校验原SHA及358个leaf，保留工具/stdlib原字节和mode，仅重封装descriptor并同步6个作者文件、锁及prepared摘要；原文件完整备份在 `target/arm64-cross/host-go/backup`。新固定SHA `8818a1eccb510948a55f63088c01678391a5e0c2236e6eacadfe45d0a5edbb43`，prepare.py和SDK文档同步，未改其他语言收据和UUID/sumdb。实际host Go重复编译/移除源码后运行回归1/1通过（18.06s）。
- 静态检查修复沙箱条件嵌套、Go编译参数过多（组合现有构建输入）、共享ELF测试fixture在两个新消费者中的未用辅助方法告警；CLI相关bin/test及sandbox全部target的Clippy `-D warnings`通过。受影响Lib默认73/73、Worker默认9/9通过；各10项ignored未算通过。最后改动Rust格式检查通过，作者Python输入2/2通过。

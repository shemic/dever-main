# Linux ARM64 应用交叉构建

## 目标

在现有 Linux x86_64 开发机执行 Dever build，得到能在 Linux ARM64 独立运行的应用。部署端不安装 Dever 编译器、Rust、Cargo 或 LLVM。用户已确认上述范围及创建任务、继续实施；不重复要求实施确认。

## 实施前基线

- LLVM bridge 已有 LinuxAarch64 对象生成和链接；CLI compile.rs、toolchain/runtime_pack.rs 仍按 host 选 pack。
- toolchain/compilation.rs 的请求未携带产物目标；service/compile.rs 的缓存目标固定为 host。
- libs.rs、workers.rs 与 Go/build pack 的验证多处把运行目标视为开发机；不能仅改最终主程序而混入 x86 资源。
- 启动本任务时没有 ARM 真机及模拟器。准备作者工具与 ARM 资源属于本任务，模拟通过不等于真机性能验收。

## 要求

1. `dever build <root> --target linux-aarch64 --output <new-file>` 选择产物目标；省略目标保持本机构建。run/test 继续本机执行，未知/重复参数明确失败。
2. 私有 CLI 与机器共享编译均传递同一目标；验证正确版本、ABI、目标 pack 和签名闭包，缓存按目标隔离。不能用宿主 pack 替代缺失 ARM pack。
3. 制作并验证 ARM64 的 base/sqlite/postgres/both 运行库及必要 CRT/静态库；构建端保持 x86_64。
4. Lib 准备/锁定/离线打包区分构建宿主与部署目标，Python/Node 解释器、Go/exec Worker、原生扩展和沙箱资产不得混入错误架构。复用原协议、resolver 和资源 owner，不另建跨编译框架。
5. 产品配置只用 config/setting.json；run/build 不联网、不读取环境变量或系统 PATH 获取语言工具。准备目标依赖必须显式进行。
6. 用模拟环境运行生成的真实应用与目标 Worker，验证输出、失败和清理；涉及 namespace/seccomp 的验收使用带 ARM 内核的完整系统模拟，不能拿用户态模拟结果替代沙箱证据。

## 验收

- [x] 默认本机 check/run/build 行为及参数拒绝定向回归通过。
- [x] x86 构建的应用 ELF 标记 AArch64，模拟器中执行实际 CMD/HTTP/数据库路径并得到预期输出。
- [x] 四 profile 都使用真实 ARM 输入；私有与共享构建命中正确目标缓存，缺失/错误/篡改目标输入均拒绝。
- [x] pip/npm/go 与 exec 的目标资源选择、锁定和错架构拒绝有回归；准备的 ARM 三生态应用能在模拟环境独立运行。
- [x] 产物不依赖 ARM 编译器或源项目，构建不隐式联网；最终日志明确记录通过、未运行和阻塞。

## 不在范围

ARM 上开发/编译 Dever、macOS/Windows、公开发行/新签名体系、真机性能及长压测。保留现有 x86 验收证据，不重复全量 CI；不改宿主 AppArmor、业务服务或全局工具。

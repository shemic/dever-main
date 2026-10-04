# 普通二进制调用设计

## 源码合同

Adapter：`external command "bin/tool" { allow file }`，路径沿用领域相对资源路径，allow 沿用现有能力名。不接受 setting/lib；部署配置和授权沿用已有 adapter 节点。

每个 command Adapter 对应一个只有一个操作的 Port，操作名按业务自定：`execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) fails dever.process.Error`。第二个 stdin 参数允许整项省略（仅 args），传 null 表示立即关闭 stdin。固定字段名防止增加映射 DSL。标准 Output 是 `{code: Int, stdout: Bytes, stderr: Bytes}`；标准 Error 是 `error Failed(message: Text)`，满足现有 Port failure 合同。命令非零退出按沙箱传播的退出码返回，不猜测 signal；启动/超时/超限继续作为源位置 runtime fault。

## 编译器与进程边界

新增 ExternalEcosystem::Command，保留现有 external 合成函数、Port fake、effect 和 suspends 链路。共享 wire 增加仅 Command policy 可用的 Bytes/base64；API/Job/真实 Worker 原有 wire policy 不变。两个后端仍用同一 external start/call/reply ABI，只给 Bytes wire 增加最小桥接函数。

HIR 共用合同构造实现，但 external_worker_contracts 只返回 exec/pip/npm/go，external_command_contracts 返回 command，避免 Lib lock/resolver 散落 command 特判。command 不需要 dever.lock；有真实生态依赖的同项目仍保持原 lock 行为。

## 运行时

复用 invocation Session、Managed、registry、bounded requests 和 structured Scope。command supervisor 启动不执行二进制或协议握手；收到每个请求才启动一个独立沙箱进程，并发写 stdin、读 stdout/stderr 和等待退出。不追加 --dever-component，不重放命令。超时、调用取消和关闭均 kill/wait，再返回结果；空闲 supervisor 可继续服务后续请求，不为正常多次调用套用 Worker crash restart 次数。

复用 verified extraction、launch manifest owner 和 sandbox Launch。内部资源 argv 继续映射，动态调用 argv 作为普通数据追加，不当宿主路径重写。保留只读程序树、grant、namespace/seccomp 与空环境。沙箱 PID namespace 负责后代收束，测试必须观察真实后代清理。

限额来自 config/setting.json 的 `adapter.<port>.command`：timeout_ms（默认30000，最大3600000）、output_limit（默认1048576，最大8388608，两路输出合计）。argv 总量最多65536字节，stdin最多8388608字节；拒绝NUL。base64膨胀及JSON包装不得超过已有16MiB wire上限。Worker Adapter 不使用这组命令限额。

## 离线打包

CLI 专门 command packager 接受一个 ELF 文件及同目录 lib/ 中的普通库文件；只采集所属项目/Package 的显式资源，不扫描宿主或调用 ldd。静态 ELF 可直接运行。动态 ELF 复用签名 sandbox GNU loader/OS库，递归解析 DT_NEEDED、验证目标、SONAME和版本需求，私有库按随附 lib/ 查找，缺少依赖构建期报错。共享已有 Python native ELF 闭包校验算法，不复制第三套解析器。使用固定私有 loader 参数关闭系统 cache，库搜索受打包树与sandbox库限制。

资源沿用SHA、私有原子提取、缓存/签名和Package归属验证；每次启动重新校验。具体 launch manifest 字段按既有owner扩展，不能从运行配置接受任意可执行路径。

## 改动边界

- core syntax/parser/check/ports/hir/wire/native/llvm，library/dever/process.dever，以及 wire runtime/bridge：一条类型与调用合同。
- runtime component/command/external/config 与 sandbox：一次调用的进程和资源生命周期。
- CLI workers command/ELF依赖、libs资源准备：离线可执行资源闭包。
- 根test、LANGUAGE/README和已有规范：真实证据和可用示例。

不增加通用执行注册中心、新依赖、第二套SDK/JSON引擎，不改CMS业务。回滚按上述拥有者整体撤销本任务差异，保留开始前源码备份与用户已有改动。

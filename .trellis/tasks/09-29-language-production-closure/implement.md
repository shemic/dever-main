# Dever 语言生产化收口执行计划

## 2026-10-02 继续：Linux质量门与非root Worker

最新范围只推进完整CI/长压测和受限AppArmor非root Worker，其他平台及公开发行暂缓。复用现有任务，不新建平行实现或重新设计语言。

2026-10-03用户因耗时明确允许不做剩余长测，要求尽快收口。已停止长测队列；此决定替代下文全部18个子场景必须跑完的本轮要求。保留CMS64/128和HTTP2-64正式通过证据，HTTP2-128记主动中止，recovery/live四阶段未执行；不标为通过。用户仍保留完整CI要求，默认/全功能矩阵已经通过，不再追加长测。随后用户明确回复“可以”，授权临时部署可信helper、加载AppArmor策略、启用严格转换限制并在验收后恢复；授权已经取得，不再重复询问。

1. [x] 核对workspace/feature/ignored检查清单，复用现有测试与性能owner建立可重复Linux质量门。
2. [x] 实测定位AppArmor启动边界，完成不放宽隔离的修复和专用部署策略；已在授权的严格宿主策略下完成验收并恢复原状。
3. [x] 串行完整检查并修复实际失败；磁盘/内存预算、源版本、未运行项与日志一并保留。
4. [x] 按2026-10-03最新范围完成长测收口：CMS64/128、HTTP2-64通过，其余长测经用户允许停止/免跑，未冒充全矩阵通过。
5. [x] 独立复核两项交付、清理自有测试资源，更新当前结果；不以旧定向结果替代新质量门。

实施边界：CI/长测逻辑归根test与已有performance runner；Worker启动归dever-sandbox，可信发行资产及可选部署接线归现有CLI toolchain owner。不改App/API/CMS业务语法；最初只读宿主的限制已由上述明确授权替代，仅允许本轮临时部署与严格策略验收，结束后恢复。

### 2026-10-03 最终结果：本轮 Linux CI 与受限 AppArmor 非 root Worker 已收口

- **最终源码与证据边界**：源码 `7310a568ebe3fbb3cc747e343ab2fd17ff1eada8da32d54e98bb143c3dca6cba`，固定完整输入 `e40b7f16cf6f7a84d256210024a3569143d5716f3cc6b34152162f4d4049e873`。本次接续只修改测试 owner：修复嵌套 libtest 汇总重复计数及标准库 TLS 缓存造成的分配账本误报，新增确定性回归。恢复 `target/ci-harness-before-37kQnY` 的四个原测试文件并排除两个新增回归文件，精确重现旧源码指纹 `90245dd9737bfed7ebe13240a05958e5eba9b088457ba1e90de093d8fa3fa379`；编译器、runtime、SDK 和产品输入均未改变。因此保留原矩阵证据，不重写其指纹、不重复长时间矩阵，也不声称所有阶段在同一新指纹下重跑。
- **最终专项**：`target/linux-ci/native/1791040913590327459/result.json` **185/185**，`target/linux-ci/sandbox/1791040885539823828/result.json` **9/9**，`target/linux-ci/python/1791040504277745773/result.json` **92/92** 全部通过。inventory `1791040538867110700` 覆盖108目标/217项分类；原生原失败用例固定重复 `target/linux-ci-parallel-repro/1791040439295737989/result.json` **20/20**。真实泄漏和错误返回的负面回归均仍被拒绝；没有放宽断言或重试到通过。
- **完整矩阵及其他检查**：上述产品字节一致的旧指纹下，default `1791029064673984868` **651/651**、all-features `1791031757319396825` **873/873** 通过；全局格式/默认与全功能全target Clippy、19 feature组合与依赖树、三语言SDK、真实PG **7/7**、有界性能 **3/3** 均保留原正式通过证据。阶段报告在 `target/linux-ci/` 对应目录；不把用户免跑的长测或不同指纹报告拼成严格 `run.py summary` 全阶段通过。
- **严格宿主与真实非 root**：`target/linux-ci-apparmor/1791040870463419164/result.json` 验收通过且回滚完成。四种普通/立即 `aa-exec`、有/无 `--no-new-privs` 组合均实际运行探针并由内核拒绝 `uid_map`（errno1），未以启动失败代替隔离证据。`target/closure-sandbox-ecosystem-release-acceptance.json` 验证签名安装、三生态受管check/run/build、隔离独立程序和 **UID65534 的 pip/npm/go 全部通过**，包含源码原生依赖与离线构建收据重放。
- **预算与清理**：最终队列日志 `target/linux-ci-final-ledger.log`，用时1小时3分28秒；父cgroup峰值3,565,158,400字节，另预留32MiB，总计小于4GiB，OOM/oom_kill均0。自有unit已停止，cgroup和PG已退出；临时helper目录 `/opt/dever`、策略文件及两个已加载profile均移除，无残余附着进程。sysctl恢复原值 `apparmor_restrict_unprivileged_unconfined=0`、`apparmor_restrict_unprivileged_userns=1`，后者始终未关闭。根临时 `config/setting.json` 校验SHA `4758568903fd1233d565cd381372353b598f994517fede3d117c6508c948e2bb` 后移至私有 `target/linux-ci-session-BrZ5t3/setting-final.json`（0600），恢复根配置原先不存在的状态；日志、固定输入与备份可恢复保留。
- **长测与父任务**：CMS64/128及HTTP2-64正式长测通过，均0错误/0 OOM；HTTP2-128主动中止，recovery/live的64/128四阶段未运行，按用户最新要求免跑且不记通过。其他平台适配与正式公开发行继续延期，父任务保持 `in_progress`；本轮两项 Linux 交付无剩余阻塞，不提交、不归档。

### 以下为历史执行记录

记录中的“进行中”“未授权”“尚未通过”均描述当时状态，最终结论以上节为准；失败和中止证据原样保留。

- **分配账本误报定位与修复**：native `1791038231061444951` 的前137项通过，在 `native::synchronous_file_stream_parallel_failure_releases_workers_and_rows` 报+1；宿主报告 `target/linux-ci-apparmor/1791038179604022584/result.json` 确认恢复完成。固定20次复现 `target/linux-ci-parallel-repro/1791039680829850445` 为14通过/6失败。临时指针追踪六次均余留48字节；保存ELF `target/parallel-trace-kernel`，`addr2line -f -i -C ... 0xb0ff6` 明确为 `Arc<std::sync::mpmc::context::Inner>::new -> Context::new:69`，不是业务owner丢失。根因类别E（隐含假设）：一次warmup不保证触发满队列producer的TLS缓存，主线程未退出时缓存仍正常存活。共享managed-driver改为每次入口经独立pthread并join后计数，保持进程初始化、warmup1+重复64和严格delta=0；不改产品队列/ABI、不增加容忍值或概率预热。新增确定性C fixture覆盖warmup后TLS初始化、故意泄漏和错误返回；19项CI单元通过，原driver对合法late TLS稳定失败，修复后通过且两种真错误仍被拒绝。临时trace已移除，备份原driver SHA `3050becbd464c1640739dd1db5df7dcdbe1cb41095e28f7faf4053089a7ca46b`。只读并发专家复核通过；观测边界是入口及宿主线程退出，未声称证明长寿命宿主TLS无增长。当前 `7310a568...` 正进行固定20次复验以及Python/inventory/sandbox/native完整接续；原生门尚未宣布通过。

- **2026-10-03已授权宿主验收续跑**：首次部署报告 `target/linux-ci-apparmor/1791037194864425474/result.json`，四种aa-exec转换组合均实际执行探针并被内核拒绝；真实UID65534的 `enforces_unprivileged_namespace` 及子探针均成功退出。沙箱gate误将父/子两条libtest结果相加为2，导致精确单用例门禁失败；宿主已完整恢复（strict=0、userns=1、helper/策略移除、无残余附着进程）。本次只修改CI Python owner：直接单harness验收取最终父汇总，仍要求1通过/0失败/0忽略；保留普通Cargo多汇总计数。17项定向回归通过，包含子测试成功但父测试空跑/忽略/失败的拒绝。当前串行重跑Python、当前inventory、完整sandbox/native；不重跑已通过的Rust默认/全功能矩阵，不制备新作者输入、不追加长测。其旧证据保留原源码指纹，不伪装为这次Python改动后的新证据。

- **2026-10-03本轮最终状态**：当前源码 `90245dd9737bfed7ebe13240a05958e5eba9b088457ba1e90de093d8fa3fa379` / 验收输入 `e40b7f16cf6f7a84d256210024a3569143d5716f3cc6b34152162f4d4049e873` 的default `1791029064673984868` **651/651**、all-features `1791031757319396825` **873/873**正式通过，覆盖78/113目标。合计1小时56分17秒，峰值3,565,158,400字节（另预留32MiB），memory.max回收2489次、OOM/oom_kill均0。当前静态、19组合+依赖树、Python88、三SDK、PG7、性能3和3项长测均有同版正式通过证据。完整native185/sandbox9门仍缺受限AppArmor非root授权验收，不称整个CI全绿。
- **清理/交付**：压测与CI unit已退出，owned cgroup和PG进程不存在，最后复核无自有验收进程；测试可执行文件按既有gate规则回收。原先不存在的根 `config/setting.json` 已确认仍为本轮SHA `4758568903fd1233d565cd381372353b598f994517fede3d117c6508c948e2bb`，移动到私有 `target/linux-ci-session-BrZ5t3/setting.json` 留作复现，恢复根目录无临时测试配置的状态。日志/固定输入/编译缓存保留；将来复验须显式恢复该配置后再运行。宿主AppArmor未改，profile/helper未安装。父任务保持in_progress，无commit/发布/归档。
- 取消剩余长测后的当前版本 default `1791029064673984868` **651/651通过**，source `90245dd9...` / inputs `e40b7f16...` 起止稳定，覆盖78个目标；all-features `1791031757319396825` 已接续运行。此轮不重制author输入、不追加长测，也不将缺授权的native/sandbox完整正式门标为通过。
- 已精确向本轮 `http2-128` gate PID1085911发送SIGINT，运行器保存 `KeyboardInterrupt`（命令3002.8秒），自有服务、cgroup、PG均已退出，无残留压测进程。整轮4小时13分钟，峰值3,052,568,576字节、OOM/max事件0；保留 `target/linux-ci-cms-recovery-final.log` 和原始中断报告。宿主strict仍0/userns限制1，helper/profile均未安装。
- 只读独立审计确认旧651/873完整矩阵仍是历史通过证据，当前source `90245dd9...` / inputs `e40b7f16...` 不可冒用其正式标签。当前88项Python、静态/features/SDK/PG7/bounded3及3项正式长测已通过；`target/linux-ci-final-matrix-without-soaks.log` 现只安排default→all-features。native185/sandbox9完整正式门受非root宿主授权阻塞；旧184/8诊断保留其对应版本，不做无效重跑。
- HTTP2-64 `1791022159074052799` **通过**：明文/TLS各1800秒、计划各9,000,000请求，成功8,996,761 / 8,996,074（4998.20 / 4997.82次/秒），均0错误、0 OOM、断连FD差0；请求P99 1.328 / 1.376ms，含客户端调度的P99 2.848 / 2.912ms，RSS峰值4.90 / 5.67MiB。3/8长测阶段已完成，HTTP2-128 `1791025811330571248` 进行中；不把固定5000目标负载当饱和吞吐。
- CMS128 `1791018511743113541` **通过**：双源码各1800秒，Dever/MD为1,760,530 / 1,744,559次成功鉴权读、978.05 / 969.18次/秒，0错误/0 OOM；RSS峰值29.10 / 29.04MiB、后半段增量8 / 12KiB，FD合同通过。64/128MiB四个CMS场景合计7,020,593次成功请求。HTTP2-64 `1791022159074052799` 正在执行，CMS之后其余6阶段长测及最终矩阵未完成。
- 正式长测首阶段 CMS64 `1791014862780230485` **通过**（source `90245dd9...` / inputs `e40b7f16...`）：Dever/MD各1800秒，1,754,220 / 1,761,284次逐回复校验的鉴权读，974.56 / 978.46次/秒，均0错误、0 OOM，RSS峰值29.37 / 29.22MiB，后半段增量16 / 28KiB，非DB句柄恢复且数据库句柄在池上限内。吞吐来自2CPU总预算下8并发闭环，并非服务饱和上限。CMS128 `1791018511743113541` 继续；其余长测与最终矩阵仍待完成。
- 最终CMS修订版源码 `90245dd9...` 已完成 inventory `1791013634029656866`、author `1791013959047970369`（6/6）、static `1791014156347447546`、features `1791014186612471905`、Python `1791014213163412921`、SDK `1791014240370055980`、PG `1791014295204024541`（7/7）及bounded-perf `1791014699485900727`（3/3）。作者完成后完整输入固定为 `e40b7f16cf6f7a84d256210024a3569143d5716f3cc6b34152162f4d4049e873`；5内核比0.707/1.004/0.538/1.014/1.069，原1.15门槛不变。`target/linux-ci-cms-recovery-final.log` 中正式CMS64 `1791014862780230485` 已启动，后接其余7个长测阶段、完整default/all-features及独立native/sandbox诊断；当前仅记进行中，owned PG已停止，非root宿主策略授权仍待回复。
- CMS句柄检查修订后的完整短矩阵 `target/linux-ci-soak-smoke/1791013117430967380` **8/8通过**：64/128MiB各覆盖双源码CMS、HTTP/2明文/TLS、两种HTTP/2连接恢复及TCP/WS/SSE。源码 `90245dd9737bfed7ebe13240a05958e5eba9b088457ba1e90de093d8fa3fa379`、输入 `1649db21a5bd7fb2224f4120d191fad7c034edba9d28bc8072907035ba380b75` 起止一致，6分47秒、峰值126,705,664字节、OOM0；这是短诊断，正式30分钟各场景仍全部待重新执行。下一轮先完成author再冻结输入，串行重跑完整质量门与长矩阵。以下保留历史尝试及其对应版本，不能把其中的进行中描述当作当前状态。
- 首个真实30分钟CMS64长测 `1791010360837130398` 在1801.7秒后因总FD差断言失败，后续阶段自动停止；运行中RSS约28.9→29.1MiB、OOM0，但失败不能记通过。75秒诊断 `target/linux-ci-cms-fd-diagnostics/1791012321412052347` 确认17→28 FD由SQLite双池各扩3连接（12个主/WAL句柄）与控制socket空闲关闭（-1）组成，压测8连接均关闭；原报告丢失case详细结果。
- 只修测试owner：CMS关闭控制client后取完整FD目标快照，非DB目标Counter精确恢复；已知SQLite主/WAL各≤配置4、SHM≤1（与锁定bundled SQLite实现及实际句柄一致），CI复用同一校验，不忽略任意文件增长；发布/基线/读负载逐步写入case结果，失败也保存raw/预算/错误并进入总报告。50项runner及15项gate单测通过，覆盖同数socket替换/dup、超池上限及两类失败证据保留；真实75秒复验 `1791012808869143017` 通过，16→28仅为受限数据库池增长，非DB全部恢复、OOM0。当前测试源码已变，准备先完成短协议检查，再冻结新版本重跑正式长测；不把75秒当30分钟。
- `be785533...` / `1649db21...` SDK `1791009652252648482`、真实PG `1791009708083087094` **7/7**、bounded-perf `1791010181020595614` **3/3正式通过**，材料起止一致。固定25进程×9样本的5内核比值依次0.710/0.998/0.546/1.014/1.053，保留原1.15阈值和全部分组日志，不把参考Rust后端的微基准当LLVM服务吞吐。长测已实际开始：CMS64阶段 `1791010360837130398`，Dever服务PID944306，当前只记running，完成后还需MD与其余7阶段。owned PG已停止。
- 固定25组采样版本源码 `be785533e4ee767df5d3500570868533c40f2489a0e42c4ff3790fa80d226f8f` 已通过inventory `1791009039923269723`（217项ignored分类）、author `1791009367171058428`（6/6）、static `1791009565839581267`、features `1791009598346719330` 和Python `1791009624921947548`；完整输入为 `1649db21a5bd7fb2224f4120d191fad7c034edba9d28bc8072907035ba380b75`。`target/linux-ci-stable-sampling-closure.log` 串行安排SDK、PG、固定采样性能门、8个长测阶段、default/all-features及不依赖待授权宿主策略的native/sandbox诊断。先完成author再冻结长测输入，后续不重制材料使长测证据失效。长测尚未开始，非root宿主授权仍待回复。
- 正式PG `1791007358946731625` **7/7通过**。随后bounded-perf `1791007764657474746` **2/3**：Map比值1.421超过1.15，其他4内核及allocation/任务粒度通过，后续长测未启动。固定同一对ELF的诊断保存在 `target/linux-ci-map-diagnostic`、`target/linux-ci-map-diagnostic.log` 和 `target/linux-ci-map-affinity.log`；实际机器码两侧都执行SipHash/Map覆盖写，handler已内联，没有每步Result调用或Map克隆。相同ELF在各CPU仍出现约1.6/2.2–2.3ms成簇耗时，具体宿主/种子根因未定；CPU0前5组比1.370、后5组1.072，合10组1.073，说明原5进程门有采样偏差风险。只将固定进程数5→25并永久保留每组9个原始样本，算法/优化参数/1.15阈值不变，不动态补样直到通过；临时Rust诊断测试已移除。无绑定25组诊断比1.069只是证据，不能替代新正式门；长压测、最终矩阵和待授权非root验收继续。
- 最新源码仍为 `d39a1108...`。正式inventory `1791006147903077573`、author `1791006502867605785`（6/6）、static `1791006705602191313`（fmt及默认/全功能全target Clippy）、features `1791007065359258253`（19组合+依赖树）、Python和SDK `1791007302833243256` 已全部通过。author重建材料后完整输入更新为 `1acd35250dbdb0872e68744cf9c434ae160a3caf60a5328e65bfcc965009d566`；旧 `c6aebfab...` 的184/8/3诊断不冒充这批新材料的正式native/sandbox证据。`target/linux-ci-final-acceptance-soak.log` 已串行排入新版本PG/有界性能、8阶段约9小时持续测试，以及最后default/all-features；当前PG `1791007358946731625` 进行中。压力阶段不并行Cargo。
- `d39a1108...` / `c6aebfab...` 独立验收全部完成：原生 `1791003377055500069` **184/184**、沙箱 `1791005949517983914` **8/8**、有界性能 `1791003223649311165` **3/3**，source/input起止一致。三轮在同一2CPU/3400MiB unit共45分52秒，峰值2,988,834,816字节，无max/OOM事件。覆盖此前SIGPIPE、Node大程序链接、Package全链、真实180秒deadline与PID清理、跨UID共享、四profile签名/exec移源、bootstrap228秒双项目双UID和root真实沙箱。**明确排除**待宿主授权的 `enforces_unprivileged_namespace` 及签名三生态非root独立执行；不把这三份诊断记录替代完整native/sandbox正式门。宿主strict仍0/userns限制1，profile/helper均未安装。
- 新测试源码 `d39a1108526fc593ea192e17f80191ecad7aaec5b26b083195f81bedf24acd54` 的有界性能诊断 `1791003223649311165` **3/3通过**，source/input起止一致（input仍 `c6aebfab...`）。含任务粒度、分配/记录/列表/JSON/Map原生3进程和5内核对匹配runtime基线；不是正式bounded-perf门或LLVM长测。随后 `1791003377055500069` 执行184项不依赖待授权宿主规则的原生用例；三生态签名独立非root链明确未选，待AppArmor策略授权后单独完成，不将184项当185项全通过。
- 分配基准夹具已迁移为 `main.dever` → `benchmark/allocation/app.dever` 的run入口，删除旧package/exposes和strict Main不允许的public标记，并将7处错误Choice消费改为显式result。保留14轮倍增/16,384字节、每进程text/list/json各6次和map1次、原计时边界及3进程断言。诊断 `1791002981413191937` **2/3通过**，allocation首轮仍被public标记拒绝（已修）；热路径5内核原阈值1.15下实际比值0.710/0.998/0.537/1.012/1.062通过。这是Rust参考后端对匹配runtime基线，不能冒充LLVM HTTP/CMS长测。当前再次复验分配基准；测试源码已变化，之前正式门仍保留对应版本身份。
- 根盘可用空间另行回升至约12GiB（超出本轮明确清理155MiB，不归功于本任务）。在owned 2CPU/4GiB unit内持Cargo锁，将确切RAM缓存逐文件SHA/类型/权限验证后恢复为普通 `target/native-runtime` 目录，删除本轮 `/dev/shm/dever-ci-reference-cache-7Qn1JD`，释放962,711,552字节内存盘；3971项校验及报告见 `target/linux-ci-reference-restoration.json`，无dangling链接，OOM为0。源码、固定输入及验证摘要未因此改变。
- 当前 `93971614...` / `c6aebfab...` 的正式PostgreSQL `1791002131949151822` **7/7通过**：私有REST身份/权限/事务、Model组合/stream/schema升级、两类错误回滚、权限目录、CMS双物理租户和ORM池/取消/关闭/TLS拒绝。临时PG34889已停止。紧接的bounded-perf `1791002657426882136` 1/3通过后失败：allocation fixture的根app.dever仍沿用旧package exposes/目录规则，计时前C003拒绝；热路径基准尚未运行，不记性能通过。正在只迁移测试夹具，保留算法、样本规模和断言。该unit共9分15秒，峰值2,433,675,264字节加参考缓存预留996,265,984字节低于4GiB，max/OOM事件均0。
- 最新完整矩阵正式完成：default `1790994992208925145` **651/651**（78目标）、all-features `1790997779069364253` **873/873**（113目标），源码均为 `93971614df839969267bce14e98a2400c3a68fee80ad78b9ec9fcbc061e7a507`、完整输入 `c6aebfabf6103f85758aa9d1c8145dbd7b019738df4cc541c68616a260ca0bc1`，起止摘要一致。两阶段共1小时57分19秒；OOM/oom_kill为0，memory.max回收2817次，进程峰值3,358,203,904字节加参考缓存预留936,763,392字节达到总4GiB限额。静态与最小feature等正式准备门沿用同版通过证据；当前版本真实PG、显式native/sandbox、长压测继续，尚非完整CI全部收口。
- 矩阵完全退出后，独立artifact联合图与当前工具fingerprint依赖闭包核对11个旧编译缓存；主线程持Cargo锁重查最新artifact、单链接、固定输入排除及进程maps/fd后回收162,013,184字节。明细 `target/linux-ci-reviewed-libraries-cleanup.json`；仅可重建库/元数据，源码、当前工具、固定归档和报告保留。临时目录只读审计没有其他确定可回收的大型残留，归属不明的旧SQLite目录保留。
- all-features正式尝试 `1790997779069364253` 中，runtime_entry_cleanup 4/4、完整durable_jobs 18/18均通过；后者277秒，覆盖先前栈膨胀相关的SQLite状态机、信号退出和api/worker/all模式。全矩阵仍执行中，不能以这两组结果代替整组通过。
- 正式default `1790994992208925145` **651项全部通过**，source `93971614...`，输入摘要由正式门验证稳定；同一受限unit已接续all-features `1790997779069364253`。default末段出现memory.max回收事件（当前202次），OOM/oom_kill仍0；不称“未触顶”。全feature、显式native/PG/sandbox和真实长测仍未完成。
- 冻结测试源码 `93971614df839969267bce14e98a2400c3a68fee80ad78b9ec9fcbc061e7a507` 后，正式inventory `1790993660614204359`、author `1790993991491934824`（6/6）、static `1790994189999992691`（fmt+两组全target Clippy）、features `1790994532647168020`（19组合+依赖树）、Python `1790994744317023527`（85项）、SDK `1790994771348473444`（三语言）全部通过。19分27秒，进程峰值3,077,398,528字节+参考缓存预留936,763,392字节低于4GiB，OOM/max事件均0。完整default/all-feature矩阵继续，不以准备门替代。
- 准备门停止且确认无编译/测试活动后，复用元数据回收规则删除555个无配套rlib/a/so的单链接检查元数据，323,555,328字节；`target/linux-ci-fixture-metadata-cleanup.json`保留明细。仅可重建缓存，固定输入/源码/报告保留。全矩阵的native-artifacts使用unit私有768MiB tmpfs并计入总内存限额，退出自动释放；根盘已有缓存保留，测试在独立挂载中使用新缓存，不更改产品配置。
- `1790993368772328336` 两项新fixture exact均通过：取消/断连/真实180秒期限回收，以及CMD/App缓存与接口基线拒绝。源码/输入摘要前后一致；开始最终fixture版本正式inventory/author/static/features/Python/SDK门。受限AppArmor宿主策略仍未获用户回复，不修改sysctl或安装profile/helper；尚不能记非root通过。
- 补验 `1790992602334552404` 9/12通过，包括Package check/test/run/build/独立执行、真实跨UID共享编译、官方Python wheel/source、签名四profile及新增exec移源独立运行。三个失败分别是共用wait超时、Node参考backend链接Bus error和native_cache旧main入口。取消fixture已拒绝空/非法PID并区分mode/phase（保留10秒）；cache fixture已迁移CMD/App并保留篡改基线拒绝check/run/build与不生成产物。Node用自有/tmp内存盘复测 `1790993196374065418` exact通过48.4秒；同轮测试源码有其他fixture编辑，因此整个诊断记录仍为failed，不能记正式门通过。两处新fixture独立复验进行中。
- 剩余native诊断 `1790990565345235019` 的TLS exact及共享suite均通过；累计89项通过，未完成整组。两个接线失败已修：external_workers参考backend fixture遗漏sandbox资产，复用prepare_sandboxed_worker与真实pip用例；llvm_cli旧unsigned真实Lib正向场景迁至既有签名四profile/daemon验收，保留run/build/删整个源项目后独立data7，原私有路径改精确拒绝，新增第5条编译缓存断言。三生态signed在npm run因ENOSPC退出，未到非root；Package完整链在test阶段失败但旧断言丢失stdout/status，现补诊断。随后根盘仅12MiB，主代理SIGINT停止全部自有unit，当前managed_compile exact记中断，不把systemd的success终止摘要算CI通过。
- 完全idle复核后清理精确中断目录 `/tmp/dever-core-test-699057-0`（内容匹配managed_compile source_project/签名sleeper）及121个无活动maps/fd、单链接、完整三文件native-artifacts条目，273,723,392字节缓存；`target/linux-ci-idle-cache-cleanup.json`保留明细。源码/固定原料/报告未删除，缓存可重编。根盘约109→643MiB。运行期间提出的旧PID缓存回收在PID退出检查时被取消，没有执行。下一轮先做Package/三处fixture迁移与此前未执行的native定向验收。
- 正式native `1790989136780529193` 86项通过后在TLS HTTP2上传下载被SIGPIPE终止，18分12秒、无OOM；不是完整通过。根因为公共C测试驱动绕过产品process main却漏调用已有 `dever_rt_v1_process_init`。产品进程入口已有正确初始化，可调用root依法不改嵌入宿主信号策略。仅在 `test/native-runtime-abi/managed-driver.c` 首次执行前复用canonical header/初始化并检查返回值，indexed suite自动共用；独立复核通过。既有ABI驱动已有SIG_DFL→init→raise的确定性断言，保留TLS真实网络和allocation ledger复验。继续收集剩余native失败，再冻结测试源码重跑正式门；产品runtime/CLI/压力产物字节未变。
- 正式 PostgreSQL `1790988576909213458` **7/7通过**，source `6515b6d8...` / input `6a7b4ee8...` 前后一致；涵盖原生Model组合、两类失败回滚、私有REST权限事务、站点角色目录、CMS双租户数据库和ORM连接池/取消/断开/TLS拒绝。耗时8分47秒，进程峰值1,407,135,744字节，无OOM/预算触顶；owned PG端口34889已停止，postmaster.pid不存在，server日志已保存。全矩阵、原生发行和长测继续，非root宿主策略授权仍待用户回复。
- 2026-10-03 source `6515b6d8...` / input `6a7b4ee8...` 正式准备门通过：inventory `1790987099128193601`（108目标/217分类），author `1790987506507180413`（6/6），static `1790987706540255061`（fmt及两组全target Clippy），features `1790988062781869337`（19组合+依赖树），Python `1790988269722882336`（85项），SDK `1790988297034468817`（三语言）。20分54秒，无OOM；memory.events max=336是限额内回收，不能称未触顶。正式PG、全矩阵、native/sandbox和长测仍待运行。
- 准备检查结束根盘仅196MiB；持Cargo锁确认无编译器后，回收568个无配套rlib/a/so的单链接检查元数据，323,489,792字节，排除固定inputs。明细 `target/linux-ci-check-metadata-cleanup.json`；可重建，未删除源文件/报告/固定资产。随后以独立loopback端口34889启动本轮owned PostgreSQL，日志 `target/linux-ci-runtime-postgres.log`，退出由托管器停止。
- 2026-10-03 四runtime、CLI/guard/maker/init、3 peer和双CMS/协议产物全部重建成功；`target/linux-ci-author-runtime-closure/provenance.json` 绑定 source `6515b6d8...`。耗时29分10秒，进程峰值3,414,069,248字节+已预留参考缓存876,298,240字节，合计低于4GiB，无OOM。双CMS为8,325,896/8,328,136字节，sha分别1bdabcf5.../e528684e...。component-fixture固定副本同步刷新为f89f6af7...；临时setting指向新产物（自身SHA47585689...）。本轮临时runtime编译缓存已回收。
- 重建后磁盘一度仅35MiB，独立审查确认只回收早期Rust backend完整缓存，不能仅凭源码mtime宣称全部失效。主代理持Cargo锁、排除活跃writer/maps/fd和固定inputs，逐项复核mtime/inode/单链接/完整三文件后原子移出并删除287条目，397,664,256字节；`target/linux-ci-native-cache-cleanup.json`保留明细。较新118条目、CLI LLVM cache和所有报告/固定原料保留，删除项可重编恢复。空闲现534MiB，准备启动新源码/新资产的正式门。
- `1790981698051341418` 最终67目标/605项实际通过，source前后同为 `6515b6d8243fa1ac9fb925510ab2e0b191a644243c9075ec88b745737bdabeeb`，耗时50分34秒、进程峰值1,213,997,056字节、无OOM。包括入口清理4、完整Job18、借用/non-Send新回归及剩余all-feature目标。这是修复充分性诊断，不替代完整正式门；下一步重编四runtime/CLI/guard/maker/peer以及双CMS和协议程序，再冻结资产完成正式验收。
- 入口/Job scope 修复后 `target/linux-ci-job-probe/1790981398350130546` 三 exact 全通过：SQLite状态机、signal三种退出场景、API/worker/all模式；无OOM，进程峰值933,769,216字节。独立只读复核未见新增类型限制或清理/错误语义回归。新增借用Rc跨await的根入口及cleanup回归；`target/linux-ci-diagnostics/1790981698051341418` 入口清理4/4已通过，继续完整Job及剩余all-feature目标，尚非正式全矩阵通过。
- 最新 all-features `1790978249784275058` 在 durable_jobs 失败：此前271项通过，本组15通过/3失败，完整门未通过。两个真实问题是根 Future 在入口/task-local 包装间按值搬运导致栈膨胀：信号用例根 Future 294,544字节，入口 block_on poll 单帧3,535,480字节。入口改为首次 Box 后进入共享执行owner，SQLite状态机 exact 已通过（`1790980718162628429`）；信号用例仍溢出，继续修正 Job scope 的同源包装，保持默认栈、身份惰性、清理与错误语义。模式用例旧API body迁移为声明+App+CMD，使用全局Model保持原非租户三模式检查。第二轮复验进行中，不记通过；临时尺寸输出已删除。运行时改动意味着四profile归档和验收程序必须重新生成，之前静态/feature/SDK通过不能算新源码最终证据。
- 源码备份 `/tmp/dever-linux-quality-backup-hI3zdG/source.tar.gz`（SHA256 `096aa0755e79b88acd5d1acc19f008b7b81feb38460ba750eae86c97d5c6ce20`）；全仓机械格式化完成。初始格式门1174处Diff位置/132文件。初查Clippy修复CLI旧测试多余clone、network peer手工clamp；完整正式门尚未通过。
- 静态bwrap 0.11.0制作并验证无INTERP/动态依赖；只替换自有作者输入，未安装宿主helper/profile。root沙箱7项实际隔离、2项默认合同、helper不安全路径拒绝1项通过；日志 `target/linux-ci-sandbox-*.log`。非root旧fixture在root构造Launch后才降UID导致仍走私有路径，已修为先降UID清groups再构造Launch，待重编验收。
- CLI native_release默认18/18通过（6项明确ignored），日志 `target/linux-ci-bootstrap-default.log`；这是增加安全宿主前置检查之前的定向结果，不代表新的签名安装/非root通过。
- CI入口与持续CMS/FD/RSS门已落地并完成独立复核；gate13、CMS12、runner48项Python定向通过。补齐最新尝试、完整ignored owner和实际长测预算核验。实际完整Cargo矩阵、作者输入、真实PG/SDK和约9小时长测矩阵待完成。
- 安全独立复核发现 `aa-exec` 可直接进入新父profile；已在产品启动和实际宿主安装增加严格profile切换前置检查，并通过代码复审与定向回归。已发起异步授权询问，待用户批准临时strict=1、安装指定摘要helper和加载专用规则后真实测试；验收后先卸载新规则再恢复strict原值。当前没有获得该操作授权，也未更改任何宿主策略。
- 完整all-target/all-feature Clippy `-D warnings`通过（`target/linux-ci-all-features-clippy-fixed.log`）。修复旧测试遗漏External声明分支和lint，更新live peer装箱后的关闭调用。默认全workspace矩阵开始预检；最终阶段仍须绑定准备后的输入摘要重跑。磁盘不足已清除三个过时编译快照（bootstrap-cli-snapshot-s5i92v、final-cli-snapshot-jiqbfz、final-native-fixture-snapshot-xw7bGT），约回收300MiB；源码备份、日志、当前工具和固定原料保留，删去的编译快照可重建。
- 默认预检 `target/linux-ci/default/1790958354110646233` 在 Port target 失败（15通过、1失败），不是完整通过。此前双 CMS 6/6、native_entry 16/16、REST 12/12及CLI常规目标已通过；进程峰值约2.8GiB、无OOM。失败原因为旧测试直接编译无资源的external入口，未启用受验证bundle运行时。保留Port生成/清理检查，真实typed调用与每次shutdown验证合入既有external_resources打包用例，不增加邻接Worker回退。修正后的Port exact 1/1通过（`target/linux-ci-port-fix.log`）；实际打包用例待复验。
- 当前源码四profile归档、CLI/guard/静态init/压力客户端重建中，作者过程与源码摘要写 `target/linux-ci-author-build/`；临时内存盘仅缓存编译产物。自有PG16集群 `/tmp/dever-linux-ci-pg-AqF2Cz/cluster` 初始化通过，尚未启动；根临时setting声明loopback端口34889与专用库模板，结束后恢复原先不存在状态。
- 为保留本次编译空间，进一步移除过时closure-current-compiler的runtime/cache/可执行文件、closure-verified-tools-ksA2Hn快照、未引用native-runtime/runtime-sandbox.a，以及回收站内六个旧ledger/LLVM诊断可执行文件。仅删除可重建编译产物，保留源码备份、固定原料、历史报告和摘要；回收站内这六项为永久删除，需重编恢复。
- 修正CI配置接线：运行时严格拒绝未知顶层键，因此验收工具改用既有 `performance.linux_ci`，只把 `performance.network_peer/live_peer` 解析为peer路径，未放宽产品配置。gate Python14/14、Rust application_config6/6通过。此前失败清理疑点经metadata入口和文件时间核对属重跑生成文件，现有清理正常，没有额外修改。
- 四优化runtime归档、当前CLI/guard/静态init及3个压力客户端最终构建确认通过，源码SHA `8c699b703e5ef689c076678f2d5ea9ed0cca5c986a5aa8ac245fb567d70e9716`，完整归档/编译器/peer摘要在 `target/linux-ci-author-final/provenance.json`。首轮因CI源码中途修正而拒绝最终通过，旧日志保留。`target/performance/cms-linux-ci-10-02`双CMS检查/构建成功，`protocols-linux-ci-10-02`协议产物构建成功；尚无本轮长测结果。
- `target/linux-ci-preflight-closure/result.json` 定向通过：配置6/6、external_resources常规8/8及真实打包exact1/1（107.37秒），覆盖清空环境、两次typed返回7、每次新的shutdown确认和资源篡改拒绝。不是完整CI通过，也不是受限非root验收。构建后删除自有1.3GiB内存盘临时编译缓存；磁盘紧张继续删除未使用的 `target/go-cache` 与 `target/go-managed/cache` 旧可重建Go缓存，原runtime.pack/依赖/报告保留。
- 正式inventory首轮因backend完整静态归档写满磁盘失败，日志 `target/linux-ci/inventory/1790962709158372077`。将整个可重建参考编译缓存逐文件核对后迁到 `/dev/shm/dever-ci-reference-cache-7Qn1JD/native-runtime`，原 `target/native-runtime` 暂时为指向它的链接，不修改编译器。每阶段从总4GiB额度扣除已有缓存页并保留32MiB开销，进程额度最高3400MiB。结束时须回收该链接/临时缓存。重跑inventory `1790963141083952733` 通过：108个实际target、217个ignored逐一分类，无缺项；期间无OOM。
- 正式author首轮5/6通过，npm第6项因临时构建目录写满磁盘失败（`target/linux-ci/author/1790963610753874259`）。重跑只给本轮systemd临时unit配置私有 `/tmp` tmpfs（2GiB上限、仍计入总内存额度），只读映入托管脚本与PG原料；主机 `/tmp` 未变。`1790963918501049513` 6/6通过，含真实Go/Python依赖验证、工具pack制作、隔离PEP517及npm原生addon构建，输入/输出摘要一致，进程内存峰值1,963,769,856字节、无OOM。作者结果不是非root AppArmor验收；宿主策略仍未获得授权且未改动。
- 正式static通过（`1790964267132478061`）：全仓fmt及default/all-feature全target Clippy零警告。features通过（`1790964373302707758`）：19个最小组合加CLI依赖树；Python85项（gate14/performance69/ecosystem2）通过；SDK `1790964628129189173` 三语言真实协议全部通过。新default矩阵 `1790964733321613886` 正在执行，随后all-features；这两个完整矩阵及真实PG/native/sandbox/长测仍未记为通过。
- 上述default正式尝试最终在 `035.log` 的async_composition失败（6通过、1失败）：旧fixture把零输出sleep赋给变量，当前checker正确报C005；此前Port16/16、REST12/12及CMS6/6已通过。进程峰值1,719,320,576字节、无OOM。仅将两处无用赋值改为action后，实际原生exact1/1通过（22.60秒，`target/linux-ci-async-composition-fix.log`）。
- 同源测试漂移扫描修正async_stream/http_library/live_library/pooled_library/performance/differential/structured_concurrency七文件：按当前成功值与显式result捕获合同迁移调用，Read/Receive使用Done(state)，保留独立行为及拒绝断言。七文件rustfmt通过；`target/linux-ci-diagnostics/1790966028853786940` 串行执行剩余all-feature目标并收集失败，仅为诊断。测试源码已改变，之前正式门不能视为当前版本全部通过；产品和固定runtime/peer字节未改。
- 上述62目标诊断完成：20组失败、无OOM，进程峰值1,138,159,616字节。修复一处真实checker崩溃：结构类型深度超限时contract阶段未生成summaries，auth仍继续索引；改为返回致命C015，主检查和Port case均停止依赖阶段，普通合同诊断仍聚合。独立只读复核通过，256层原回归未修改，实际复验待完成。
- 其余失败fixture迁移到当前App/Model目录、公开类型、显式result及main命名；差分仍精确比较原生/参考输出和完整错误JSON，ORM保留查询/迁移/回滚断言，HTTP非法response按现行请求边界500并验证后续请求健康，H2/时间codec补正式请求scope/metadata提交。sqlite_orm有一次磁盘不足，不修改产品规避。当前 `target/linux-ci-diagnostics/1790968164204229608` 复跑20失败组及Port，使用独立2GiB临时/tmp，仍受总4GiB预算约束；尚不记为正式CI通过。
- 该定向复验已确认Port16/16、contract_semantics14/14（含原256层崩溃用例）、contract_execution11/11、原生SQL7/7通过；当前唯一再次失败组secure_contracts是fixture缺部署site绑定，已恢复原source-only合同检查并独立验证platform数据库，保留global及原匿名/事务断言，待再次执行。复验期间移除5份本次core修复前的过时rlib/rmeta（约238MiB）；均为可重建缓存，保留报告/原料。
- `1790968164204229608` 最终19/21组通过、无OOM（进程峰值973,602,816字节）；含SQLite5/5、迁移10/10、差分13/13、HTTP/H2/live/入口/time codec。`1790969758624751556` 安全合同14/14通过，并发12/13失败。进一步保留精确liveness断言定位真实遗漏：CaptureResult未参与挂起点判定，且同源inline-blocking分支也只匹配Call。`1790970195428604455` 修改前11/13，实际证明捕获async sleep漏liveness、捕获sync sleep可绕过worker边界。现两处共用Call|CaptureResult原判定，加入同步capture正向及诊断span；独立只读复核确认根因，实际修复后复验正在进行。未重建验收程序或重跑正式质量门，前述正式结果仍为旧版证据。
- CaptureResult修复后 `1790970325577979930` structured_concurrency 13/13实际通过，source前后一致、无OOM（峰值704,716,800字节）；完整保留三个挂起点的精确liveness span、同步capture正向及捕获同步sleep的worker边界负向。继续评估直接受影响的既有fixture后冻结源码重跑正式门。
- CaptureResult调用方独立有界审查未发现需迁移的正向用例；async_native及LLVM async/stream checker由后续正式门覆盖。当前 `/tmp/dever-linux-ci-refresh.py` 托管重建CLI/maker与双CMS/协议产物；四runtime归档和三个压力客户端源码未改，逐字节摘要核对后复用原 `linux-ci-author-final/provenance.json` 来源，新的provenance会明确记录复用，不冒充归档重编。
- 新编译器/打包工具及双CMS/协议产物刷新4/4通过、无OOM（峰值1,046,528,000字节）。冻结源码 `900043ab2c819372ac22b1c8c44bf905d23a115c2f7a936eab0e38dbb04c86d0`，CLI SHA `c22b9fc622322d0b2456cd929533b9f0414c2a9ce48cb1a547fdf6582c078b61`；`target/linux-ci-author-current/provenance.json` 明确保留runtime/peer原构建来源。新 `cms-linux-ci-closure` 两源码分别47.71/47.90秒构建，8,324,776/8,327,016字节，SHA与原CMS一致。临时setting已指向新产物，准备后input SHA `cbdb39c8284c4a3685567c7417fb54eb0dadfe3ff92d7603f3d00b05347b9fc3`。`target/linux-ci-frozen-matrix.log` 串行启动inventory/author/static/features/python/sdk/default/all-features；仍未记为全部通过。宿主strict仍0/userns限制1，未安装profile/helper。

- 冻结版inventory `1790970867807152470` 在backend静态归档生成阶段因ENOSPC失败，尚未执行完整测试；峰值785,174,528字节、无OOM。停机后清理4个遗留测试可执行文件、3个旧CLI可执行文件及失败静态归档，约406MiB，均为可重建产物；当前CLI/输入原料/报告未删除。

- 冻结版inventory重跑 `1790971446868474214` 通过（108目标、217分类用例）；author `1790971823437290546` 6/6通过，准备后完整input为 `d8657e37f743d4758bbcc6fc5c045dac032df62eedeea58556f824798b0eae87`；static `1790972018127138063` 格式/default与all-feature Clippy均通过，随后继续features/Python/SDK及完整矩阵。
- 在同一受限control cgroup内逐字节核对旧performance ELF，将58个完全一致的单链接程序合并为硬链接，释放311,181,312字节；历史路径/字节/报告全部保留，当前两个closure输入目录未改。明细 `target/linux-ci-cache-dedup.json`。未删除其他项目或旧数据库。

- 冻结版features `1790972235115826233` 20命令通过（19最小组合及CLI树），Python `1790972375205452463` 85/85、SDK `1790972401999175653` 三生态协议通过。default `1790972457820456527` 在CMS5/6后因测试工具hard_link遇私有/tmp跨文件系统失败；CMS真实check/test/run/build/package链已通过，非产品错误。后续default/all-feature改用原盘临时目录，不改源码规避。另复核当前artifact后回收6个过期runtime/embedded bridge文件约141MiB；原候选中的fe5c5dcd75301ecf已被default重新使用，明确保留。

- 缓存清理纠正：上述bridge `9735e2773af0aacd` 虽旧时间戳，default仍复用，不应算过期；其约51MiB被误回收后由Cargo自动重建，源码和固定输入未受影响。其余4项约90MiB才是本次实际旧缓存回收，不将暂时消失的有效缓存计为净节省。

- 同盘/tmp的完整矩阵 `target/linux-ci-full-tests.log` 已启动；当前default目录 `1790972808956136476`，已通过CMS6/6、原生入口16/16、REST12/12、Port/异步/连接池/合同语义，仍在运行，尚不记为全门通过。
- 根据实际inventory/default artifact联合图、当前作者工具fingerprint依赖闭包及固定inputs排除，独立核对377个单链接rmeta。主代理持有 `target/debug/.cargo-lock`、确认无writer，再复核最新artifact与dev/inode/size/mtime/blocks后回收361,652,224字节；明细 `target/linux-ci-rmeta-cleanup.json`。都是可重建元数据（含旧rlib配套metadata），不把它们一概称为check-only；未删除任何当前登记库、固定输入或源码。

- default `1790972808956136476` 全78目标/650测试实际通过，但阶段结束因固定input漂移拒绝通过；source仍为900043...、无OOM（进程峰值1,167,564,800字节）。精确重算确认唯一变化是 `target/debug/component-fixture`：其Cargo功能组合重链接由66743d...变为089a2d...；只替换该摘要即重现原input d8657e...。未放宽质量门、未将失效证据标pass。
- 修正验收输入边界：两处显式LLVM Worker fixture改读 `target/native-release-inputs/component-fixture`，普通Worker测试仍读Cargo各自构建的bin；README/spec说明先准备并记录独立副本。临时setting同步固定路径；产品代码/编译器/runtime/已建CMS与协议程序未改。`target/linux-ci-immutable-fixture-matrix.log` 托管重新准备fixture、inventory/author/static/features/Python/SDK，随后须用同盘/tmp重跑default/all-feature。原650结果保留为诊断，未冒充新正式证据。
- 新冻结源码 `fbe4a92970850e29b3561cc22af437bc1dbe2819d173af5ad32a4848145d4461` 的固定fixture准备通过，程序SHA `58a5a120104baabac29cc712358174b0a1573ca726744fab14168edf5e9bb802`，独立副本/provenance写在 `target/native-release-inputs`。inventory `1790975682052927294`（108目标/217分类）、author `1790976010020603166`（6用例）、static `1790976202296015153`、features `1790976561899757466`（20命令）、Python `1790976771445471337`（85项）、SDK `1790976798389126261` 均通过；准备后input为 `7d5ef87ea08c9ed8803b8c32733578d87b5b2fcc054c9f527fe27aa106077ce3`。受限进程峰值1,638,834,176字节、无OOM。`target/linux-ci-immutable-full-tests.log` 已按all-features→default启动，同盘/tmp；完整矩阵、PG/native/长测仍未记为通过。
- 发行测试空间预审确认 optimized 安装时根盘同时保留两份约603MiB payload，当前余量不足。为避免必然ENOSPC，主动SIGINT结束all-features `1790976909411656631`，报告如实为KeyboardInterrupt，未记完成。复用已有 `machine_temporary_root` 配置，将 optimized machine、signed_core release/isolated、bootstrap downloaded 四处统一接入既有临时目录owner；author仍留同盘支持原料hardlink。独立审查确认所有hardlink同文件系统、安装仍真实copy、回收边界不变；两文件rustfmt通过，未改产品或验收断言。新源码 `3ca3f74e4696973e5dbde34efa188dda6ad103861c98a8576def3d6d31223254` 正在 `target/linux-ci-release-storage-matrix.log` 重跑inventory/author/static/features/Python/SDK，前版证据不算新门通过。
- 3ca3版准备门全部通过：inventory `1790977493517800171`、author `1790977818442118586`、static `1790978011370964455`、features `1790978044513313008`、Python `1790978070742789549`、SDK `1790978097831556704`；准备后input为 `02c363bd03e50772acfefe2b59d2493a82665faa74616ef2d502b7d0bd4700da`，无OOM。再次基于新artifact/作者闭包清单、持锁复核368个旧rmeta并回收334,819,328字节（`target/linux-ci-rmeta-storage-cleanup.log`），根盘余量约702MiB。正式all-features→default运行日志为 `target/linux-ci-release-storage-full-tests.log`；此次复用未变的fixture/runtime/CMS及其原provenance，不将测试目录接线改动冒充产品重编。

## 2026-09-30 用户批准的最终交付顺序

范围固定为以下六个闭环，不再重新设计已稳定的语法、API、目录角色或 CMS 业务。单个模块或 fixture 通过不等于整个闭环完成；代码、正式资产、真实平台验收分别记录。

1. [x] Dever Package：版本/传递依赖、共享摘要缓存、安全安装/更新/移除/doctor、源码加载和传递 Lib 统一锁已完成。2026-10-02 已通过签名安装 → LLVM daemon 编译 → check/test/run/build → 停止 daemon 后独立执行的自有完整 fixture，没有私有缓存旁路。公开 registry 仍未发布，归入第 5 项正式发行。
2. [ ] 正式 runtime/build packs 与 OS sandbox：可重复制作/签名/版本绑定，完整标准库与构建资源，以及真实拒绝未授权文件/网络/子进程操作的验收；不把 capability 回显当沙箱。
3. [x] 完整 Lib（当前 Linux x86_64 实现）：Go 自动 Worker 编译/链接与 sumdb、Python extras/sdist/PEP517 原生构建、npm 高级依赖与受控 hooks；官方 UUID、simplejson C 扩展及 bufferutil 原生 addon 已通过实际隔离构建、锁定重放与离线 Worker 运行。正式多平台包仍归第 2/5 项。
4. [ ] 正式原生后端：完整 HIR 语义到 LLVM/LLD、稳定运行时 ABI、六目标资源，干净机器离线 check/run/build；不以隐藏 rustc 或子进程重命名替代。
5. [ ] 共享编译与发行：deverd 编译 IPC、跨用户产物、签名目录/下载、平台权限与服务、升级回滚卸载，六宿主到六目标的真实验收。
6. [ ] 最终质量：完整测试/CI、双源码与 PG/HTTP2/Job/上传组合、持续并发/故障恢复、体积/冷启动/延迟/RSS/64/128 MiB 容量报告。

验证继续从必要的最小定向检查开始。全量、服务级和正式发布步骤先说明并遵守用户授权；未经授权不提交、发布、安装全局服务或使用真实签名身份。每阶段完成后复核本节及原始验收合同，未完成项不得归档。

已接通 Package 配置/解析/锁/cache/source/exec、受管 Go 构建与 LLVM/LLD 连接桥。后续实现了同步强类型函数、受管值/集合、业务失败/Result、Bytes/File/同步 Stream 与真实协程的 Task/Group/Channel/sleep/blocking/parallel、AsyncStream/TCP/timeout/race/ParallelEach HIR→LLVM 和 runtime ABI；2026-10-01 又完成 CMD/wire/Dever Adapter 应用根，以及非租户 Model/双数据库 SQL/事务/DbError/Related/RowStream/schema/Seed 切片，证据分别见对应复验节。2026-09-30 用户批准的 unsafe 例外仍只在独立 `dever-backend-bridge` 的私有 `ffi.rs`，workspace 继续 forbid；runtime 本体、core、CLI 和 tests 不扩大例外，不开放 Dever 语言 FFI。私有默认 CLI 与 Linux 可信 daemon 已接 LLVM/LLD，不再调用 rustc；正式目标 runtime packs 和发行验收仍未完成。Go 保留环境变量禁令，采用固定目标受管工具路线，不引入 GOOS/GOARCH/GOCACHE/GOPATH 运行配置。

当前已完成 Linux 可信 daemon 编译、跨用户编译缓存及取消/断连/真实期限回收，真实三生态高级依赖和 Linux root 下的 OS sandbox 已通过定向验收。四个优化 runtime archive 已刷新；双 CMS/真实 PG/Job/API 与 LLVM 性能、64/128 MiB、HTTP/2 恢复矩阵均有独立报告。Linux签名首装、双项目/真实双UID共享使用、升级/篡改拒绝和移源执行通过。三生态最终签名安装、受管check/run/build、移除源码及机器目录后的无系统语言独立执行亦已通过，含Python/Node源码构建原生依赖及离线收据重放。2026-10-03已补齐受限AppArmor真实非root入口、三生态非root独立执行与本轮Linux质量门，证据及版本边界见顶部最终结果。仍缺其他平台产品实现和资产及正式公开发行，按用户要求暂缓；剩余长测免跑，尚未公开发布或安装全局服务。

以下保留原始任务的历史阶段编号；当前交付顺序以上述六个闭环为准。

## 阶段 0：任务基线与旧记录收口

- [x] 核对既有子任务实现与最新源码，标出“已实现但任务未归档”和“文档中已过时的未完成项”。
- [x] 为每个子任务补充真实验证命令和阻塞项，不把历史记录当作当前缺口。
- [x] 保持源码配置只走 `config/setting.json`，清理旧环境变量说明。

## 阶段 1：Production API 与真实 PostgreSQL

- [x] 补真实 PostgreSQL fixture/配置入口，默认无配置不连接数据库。
- [x] 完成 PostgreSQL 授权 Store 的权限目录、角色、跨站点、多角色和撤销验收，以及 CMS Owner HTTP/两个物理租户数据库验收。
- [x] 完成同一真实 PostgreSQL HTTP 链路的非 Owner 多角色、跨站同名角色及撤销立即失效验收；使用仅注入临时 CMS 副本的成员 fixture，生产 CMS/Owner 保留角色规则未改。
- [x] 完成请求、响应、Cookie/Header/Context、上传、取消、事务、HTTP/2 的已有定向回归；不代表所有 PostgreSQL/双源码组合已运行。
- [x] 更新 `09-20-production-api` 和父任务；共享 ORM fixture 已迁移并通过真实 PostgreSQL，未完成的任务不归档。

## 阶段 2：大型 CMS

- [x] 按当前组件目录约束实现账户/会话/权限、内容/版本/状态、媒体、发布、审计和清理 Job。
- [x] 完成 Dever 与 Markdown 双源码行为合同和等价测试。
- [x] 完成 `check/test/run/build` 与 `api/worker/all` 验收；CMS isolated `check/test/run/build/package/run` 双源码长链路已通过。
- [x] 删除旧 demo-only 路径，更新 CMS 文档和示例；新增 `system.health.ping` 作为无数据库运行烟囱，不恢复旧 `news.article.demo` 兼容入口。

## 阶段 3：External Lib 生态

- [x] 完成离线 lock/resolver/cache/artifact 边界、Adapter 精确 Lib 请求接线，以及真实 PyPI wheel/npm registry/Go proxy 解析器；显式联网入口依赖已验证的安装版本及签名 runtime 描述，不访问宿主语言工具。
- [x] 完成 Python、JavaScript、Go SDK 协议库和显式宿主验收。
- [x] 支持 external pip/npm/go，三 SDK 共用 checked wire manifest；Python/JS 自动同名 entry 绑定、离线运行树和受控启动已有实现及定向 fixture 验收。
- [x] 完成锁定 host-target build pack 的 Go compile/link、build tags、module ZIP、go:embed 和静态操作绑定，并验收源码移除后的独立 Worker。
- [x] 完成 Dever Package 版本/传递依赖、源码加载、共用缓存与 Lib 锁、离线移除、失败回滚，以及真实安装到独立应用运行的自有 fixture 链路。
- [x] 完成 Python extras/wheel/sdist/PEP517 原生构建、npm 高级安装/受控生命周期/native 输出和 Go sumdb；默认合同及真实第三方包定向验收通过，见 10-02 记录。
- [ ] 交付正式签名语言 runtime/build packs，并在无宿主语言环境验收；复制宿主工具的自有 fixture 不代替发行验收。
- [x] 完成 fixture run/build preparation、Worker 资源完整性、取消/超时/中断和并发协议回归。
- [x] 完成 Linux x86_64 三生态程序的无宿主语言环境独立执行，包含Python/Node源码构建原生依赖和离线收据重放；Go官方UUID另有隔离构建/移源Worker证据。正式公开与跨平台交付仍属于上述未完成项。

## 阶段 4：共享工具链与发行版

- [x] 完成 Linux 开发态 `deverd` 认证 IPC、租约、并发发布、完整性校验和缓存状态/清理命令；跨平台 peer credential 与系统服务注册仍阻塞于发行层。
- [ ] 完成 Linux/macOS/Windows 权限、服务注册、安装、升级、卸载和安全检查（跨平台发行层阻塞）。
- [ ] 实现公开 HIR→LLVM/LLD 后端与六 target runtime ABI packs，再验证干净机器无 cargo/rustc；Rust bootstrap 不代替此项。
- [x] 实现隔离的 LLVM 18/LLD 连接桥，验收六目标 object/link、输入拒绝、失败恢复、并发独占和私有输出目录；不是完整 HIR 后端或发行 SDK。
- [x] 实现同步强类型函数 HIR→LLVM 内核及源位置/调用链，验证真实对象、本机执行、静态 handler、复合值和数值边界；不代表资源/异步/应用语义完成。
- [x] 实现独立 runtime-abi staticlib 的数值/Text/Bytes 缓冲边界，并通过 canonical C header 和 LLVM object 的实际调用验收；复合资源/异步 ABI 和正式 target packs 未完成。
- [x] 实现 Linux 内核 peer credential、跨用户 Lib artifact IPC、配额与有界 admission；真实不同 UID 的自有 fixture 已通过。
- [x] 实现 Linux 可信 daemon build IPC、签名核心/pack 校验、跨用户编译缓存与取消/超时回收，恢复机器管理的 Package 完整执行合同。
- [ ] 实现 macOS/Windows peer adapters、系统服务与正式发行；Linux 自有 fixture 不代替全平台验收。

## 阶段 5：最终质量门

- [x] 完成本轮 Linux 全量 workspace 矩阵和阶段定向测试；版本边界见顶部最终结果，其他平台延期。
- [x] 完成本轮真实 PostgreSQL、CMS 服务级、External Lib 真实运行时和 Linux 签名发行验收；公开发行另行延期。
- [x] 完成 LLVM 四 profile/双 CMS 体积、启动、并发、RSS、64/128 MiB 与共享编译缓存报告；HTTP/2 含一分钟稳态及 100 轮恢复，不把短测或私有 debug 编译耗时当生产饱和上限。
- [x] 在私有临时目录准备同版本 rustfmt/clippy；core/runtime 与 CLI libs/bins 的 Clippy `-D warnings` 通过，不安装或替换全局命令。
- [x] 完成源码全局格式及本轮 Linux CI 质量门；剩余长测按用户要求免跑，不计通过。
- [ ] 更新规范、归档完成子任务和总任务（本轮规范和执行记录已更新；总任务因跨平台/正式公开发行延期继续in_progress，不归档未完成任务）。

## 2026-09-29 收口复验

- 通过：`cargo check --offline --locked --workspace`；API declarations 16/16；SQLite authorization 1/1；CMS parity 1/1；External Lib 6/6；shared toolchain 7/7；CLI Lib 单测 4/4；Dever CMS `check`（Dever/Markdown）及 Dever native build/package/run。
- 通过：真实本机 PostgreSQL authorization ignored 1/1（记录在 `09-20-production-api/implement.md`）。
- 修复：无租户 Job 的 tenant migration 未定义符号；CMS 打包验收的工作目录配置缺口；恢复 CMS 双源码测试入口。
- 追加修复：无数据库的 application-test native 编译不再生成引用 `dever_runtime::database` 的 tenant Job 空实现；`application_testing` 8/8 已通过。
- 追加修复：`deverc run` 在启动临时 native 程序前将项目 `config/` 安全复制到受保护的 build staging 目录，并拒绝配置目录符号链接；因此运行配置仍只来自 `config/setting.json`，不引入环境变量。
- 阻塞/未运行：旧 `postgres_orm` fixture 在连接前因当前 Model 规则失败；真实 pip/npm/Go registry、受管 runtime/正式 SDK、跨平台服务注册与 peer credential、全量 workspace、64/128 MiB 压测、真实持久服务；rustfmt/clippy 组件未安装。

## 阶段 5 定向质量门记录

- 通过：workspace `cargo check --offline --locked`；`api_declarations` 16/16、`application_config` 6/6、`application_testing` 8/8、CMS parity 1/1、External Lib 6/6、shared toolchain 7/7、SQLite authorization 1/1。
- 通过：CMS isolated 长链路 `cms_project_checks_runs_and_builds_from_an_isolated_copy` 1/1（Dever/Markdown 的 check、test、run、build、package、package-run）；native stripped 产物约 6.3 MiB。
- 通过：项目 formatter 对 Dever/Markdown CMS 的 `fmt --check`；`cargo fmt`/Clippy 仍因组件未安装不可用。
- 保留阻塞：全量 workspace 测试、真实 registry/runtime、跨平台发行、旧 ORM fixture 迁移及性能/RSS/64/128 MiB 压测未纳入本地完成证据。

## 首个实施目标

优先启动阶段 1，使用现有 production-api 代码和测试作为基线，只补真实 PostgreSQL fixture/配置及最终组合验收；不先扩展新语法。

## 2026-09-30 实施与当前源码证据

任务仍为 `in_progress`，没有把五个阶段记为完成。上面的 09-29 数据是历史定向证据，不能代替当前源码的运行级/发行验收。

已实现并复验：

- 共享 SQLite/PostgreSQL ORM fixture 迁移到当前 App/Model 结构，保留事务、CRUD、分页/游标/流、错误恢复及回滚断言。
- Lib 配置统一为 `config/setting.json.lib`，拒绝旧独立配置文件；canonical v2 lock 绑定 checked Worker schema/操作/capability/Adapter/依赖闭包，允许不同 Worker 独立锁版本，拒绝同一环境冲突和完整 provider 元数据漂移。
- run/build 自动嵌入全部 checked exec 候选，原子提取、Unix 私有权限/所有权和每次 Worker 启动的精确清单/摘要复验；不扫描其他 bundle。清空环境、不带源码 Worker 的重复运行与篡改拒绝实际通过。
- 三语言 SDK 的实际协议、i64/JSON 预算、错误/取消/关闭；不合作取消一秒终止。显式宿主验收通过，不代表无宿主发行。
- `deverd` 同所有者 token 校验、OS singleton 锁、断连/坏帧容错和固定线程/队列；缺 token、256 字节长度差、慢/坏客户端及重复实例/进程退出恢复回归通过。
- base-only CMD/Job 不再引用未启用 database/tenant 模块；当前四 profile 均实际编译通过。
- CMS 性能入口改为 stock 双源码的真实 bootstrap、tenant migrate/Owner、admin/front 登录、REST 创建/替换、发布、前台完整查询和重复发布 409；删除旧 `READY|3` fixture，不用无关记录数作为业务验收。

当前通过的定向检查：

- `cargo check --offline --locked -p dever-core -p dever-cli -p dever-runtime --features 'dever-runtime/external dever-runtime/postgres dever-runtime/api'`。
- `external_resources` 6/6；`native_entry` 15/15；`port_adapter` 15/15；CLI `external_libs` 14/14；`shared_toolchain` 10/10；`application_config` 6/6；`application_testing` 8/8；`cms_acceptance` 1/1。
- `test/external-sdk/check.py` 的显式 Python/Node/Go 宿主协议验收通过。
- 真实 PostgreSQL 16.15：`postgres_authorization_preserves_site_scoped_roles_and_catalog` 1/1、`postgres_orm_uses_the_configured_isolated_database` 1/1，后者约 398 秒。服务端 deb 仅解压到自有临时目录，使用现有低权限账号和专用 loopback 端口，不安装系统服务。配置写入临时根 `config/setting.json` 的 `postgres_test` URL 模板，全部结束后恢复原先文件不存在状态，关闭自有实例并删除测试库/下载包；服务日志保留在 `target/performance/postgres-acceptance-09-30-server.log`。此证据不是 PostgreSQL HTTP/租户完整组合验收。
- CMS stock 双源码实际 `check/build`；产物 Dever 6,594,936 字节、Markdown 6,595,576 字节；每项 SHA 与构建日志在 `target/performance/cms-publish-09-30-v3/manifest.json`。
- CMS 两套源码在 64 和 128 MiB 自有 cgroup 内各完成 16 篇顺序发布及完整输出核对，无 OOM；专属 cgroup 和服务进程全部清理。当前报告 `target/performance/cms-publish-64-09-30-v1/report.json`、`target/performance/cms-publish-128-09-30-v2/report.json`。峰值 RSS 约 33.3–33.7 MiB；64 MiB 下创建中位约 1.9–2.1 ms、发布约 2.2–2.4 ms，启动约 45–51 ms。这是 loopback/暖页、小规模顺序业务基线，非并发上限、冷缓存总内存或生产 p99。
- 四 profile 体积：base 919,720、SQLite 2,901,400、PostgreSQL 2,920,488、both 4,884,760 字节，生产 fat LTO/strip 未放宽。报告 `target/performance/profile-cmd-09-30-v2/manifest.json` 与 `profile-report.json`，只构建不连接数据库。

失败及后续修复证据：首次 native 检查的开发 rustc PATH 缺失与旧测试合同已修正；发现真正的 base CMD database/tenant 引用缺陷并修复后复验通过。首次临时 PG 配置缺少必需 database.default，补正完整配置后两个实际测试通过。CMS v2 构建的 180 秒等待超时，v3 使用有界 900 秒构建并保留超时输出/关闭自有进程组，实际双源码构建通过。旧报告和失败目录未覆盖。

仍未完成的产品代码与验收，不混为“缺环境”：

- Go 受管 compile/link、Python extras/sdist、npm 高级安装语义、Go sumdb、正式签名语言 runtime/build packs、无宿主发行验收，以及 Dever Package 安装与传递 Lib 集成；真实三生态 resolver、三 SDK typed manifest 和 Python/JS 自动 Worker 代码已补，见本轮追加。
- OS capability sandbox；当前仅编译 effect 与握手 admission。
- daemon build 服务、macOS/Windows peer adapters 和三平台正式安装/服务注册/升级/卸载；Linux 跨用户 artifact IPC 已补并通过实际不同 UID 验收。
- HIR→LLVM/LLD 公开编译器和六 target ABI packs；无 Cargo/Rust 的公开项目构建仍未交付。
- Markdown PostgreSQL/HTTP2 全组合、持续并发 CMS/全性能矩阵、全量质量门。当前相关的定向测试不代替这些；Dever CMS 的 PostgreSQL 非 Owner HTTP 授权组合已追加通过。
- 全局工具未安装/替换；私有 rustfmt/Clippy 已可用且以下定向静态门通过。没有默认运行全量 workspace 测试。

### 2026-09-30 追加运行与质量复验

- PostgreSQL 16.15 的 `postgres_cms_http_isolates_two_tenant_databases` ignored 定向验收 1/1 通过：真实 CMS 原生程序、两个租户独立数据库、admin/front 登录、每租户两篇创建/替换/发布/查询、相同 slug 不同标题、重复发布 409、跨站 Cookie 和跨租户登录 401。报告 `target/performance/postgres-http-09-30-v1/report.json`；不包含非 Owner 授权撤销。Rust fixture 只复制 stock CMS 到自有临时项目，数据库清理失败也会使验收失败，配置 URL/密钥不写摘要。
- 清理独立复核：PG 仅残留原有验收基库和 postgres，测试控制库/两个租户库均删除；停止本轮自有实例，临时根 `config/setting.json` 恢复为原先不存在，专属 cgroup 和服务进程回收。服务器日志 `target/performance/postgres-http-09-30-v1/server.log`；本轮 PG 临时目录移入回收站，可恢复，未注册系统服务。
- PG 首次失败为工具在原位私有配置上使用独占创建，已修复为同目录原子替换并验证序列化失败不破坏原文件；第二次为 Rust wrapper 对含 `..` 的 workspace 路径判定错误，已 canonicalize 后第三次通过。独立复核进一步指出公开工具入口原位修改调用者配置，现已改为工具自身复制到 TemporaryDirectory，构建失败也保持源配置字节不变，副本自动清理。
- SQLite ORM 实际 HTTP：64 MiB 自有 cgroup，128 行、短窗口 50 目标 QPS，25/25 正确响应、无错误/丢弃，实际约 49.91 QPS、请求 p50 约 1.056 ms、峰值 RSS 约 7.54 MiB；单连接事务断连后约 5.965 ms 回收、计数仍为 0。`target/performance/orm-runtime-64-09-30-v1/report.json`。仅短时合同基线，不是饱和吞吐或 p99。
- typed App→Port→Adapter live TCP：64 MiB 下 8 次实际 echo 全部正确、RSS 约 4.77 MiB、无 OOM，`target/performance/live-tcp-64-09-30-v1/report.json`。不声称 TLS/WebSocket/SSE 压力已复验。
- rustfmt/Clippy 使用 Rust 1.98.0 同版本的校验下载包，临时解压，不安装全局工具。core/runtime 与 api/postgres/external 组合 Clippy `-D warnings` 通过；CLI libs/bins 同样通过，日志 `target/performance/quality-09-30-v1/clippy-core-runtime-v3.log` 与 `clippy-cli-v3.log`。运行时数据库 open 改为具名 ConnectionOptions 并迁移所有调用点；不改变 setting.json 格式或连接语义。
- 当前追加定向通过：`external_resources` 6/6、formatter 13/13、ORM performance fixture 4/4、ORM query 9/9、Port performance fixture 3/3、syntax/format 22/22、CLI external_libs 14/14、shared_toolchain 10/10、Python CMS/runner 55/55。
- ORM query 的过时断言已迁移：事务内密码哈希保留 C014 拒绝，合法路径先哈希再进入短事务；空 create_many 从 owning-domain App 调用，公开名称不带 app 段；where 外 contains 仍拒绝，断言按当前 C004 诊断。没有放宽 checker 合同。formatter 测试补齐新 AST 声明，完整 13/13 通过。

### PostgreSQL 非 Owner HTTP 最终补验

- `postgres_api` 同一 exact ignored 目标当前 1/1 通过，约 41.29 秒；新摘要 `target/performance/postgres-http-09-30-v1/report-1968019-1790739069361983512.json`，日志 `test-rbac-final.log`，旧 Owner-only 报告保留。stock CMS 副本只追加 `test/dever-tests/fixtures/postgres_api/` 的普通成员 CMD/Reader membership，不修改生产源码或 Owner 不可撤销规则。
- 真实会话成员 user_id=3，与两个 Owner 不同，三个权限来自各站点实际目录而非硬编码 key：授权前三个 GET 均 403；两个 admin 角色授权形成并集，front 同名角色独立；撤销首个 admin 角色后原会话对应 GET 立即 403，另一 admin GET 及 front GET 仍 200；继续撤销剩余角色后均 403。Rust 独立核对 ID/互异权限键/状态，且在数据库清理成功后才保存通过摘要，不记录密码、邮箱或 Cookie。
- 两轮工具失败保留：首轮误从 admin 目录找 front 权限，现按当前 site 过滤合同分别读取；次轮 DELETE 发 JSON body，返回 400，现统一在 CmsClient 的 GET/DELETE 路径编码 query 且不发 body。没有放宽目录隔离或 API 输入规则。新增负向/编码回归；Python CMS/runner 最终 57/57，Rust 非 ignored 3/3，私有 rustfmt 对 postgres_api.rs 检查通过，CLI Clippy 最终复验 exit0（`clippy-cli-final.log`）。
- 最终清理独立复核：仅保留本来就有的验收基库/postgres，client backend 数量 1 是核对自身；已停止本轮 PG，临时根配置再次恢复不存在，PG 临时目录再次移入回收站。`server-rbac.log` 保留完整服务器日志。
- 真实 provider 诊断也同步事实：当前 resolver 只接 FixtureRegistry，错误明确说 provider integration 未实现，不再暗示安装一个尚未接线的 pack 就能解决。该诊断修正不算 pip/npm/Go resolver 完成；CLI external_libs 再次 14/14。
- 私有 rustfmt/Clippy 下载与解压目录在复验后也移入回收站，可恢复；未改全局 Rust 工具链。PG 临时目录本次回收名称为 `dever-pg-http-JbyaSX.2`，最终根配置不存在、42209 无监听、自有 CMS/PG 进程及 cgroup 无残留。工作区原有 untracked 源码和用户删除未回退，未创建 commit。

### 2026-09-30 缺实现补齐：真实 resolver、typed Worker 与 Linux 共享资产

本轮实际产品代码：

- 三生态共享 `RegistryResolver`/transport/lock/cache owner，实现 PyPI PEP 508/440 wheel、npm semver/SRI、Go proxy/MVS；只从当前已验证发行 core 的签名 runtime 元数据取 pack。拒绝代理、非协议允许的重定向、网络错误回退旧版本、坏归档、身份/工具链漂移，解析预算有界。Go ZIP 最多两跳手工转入固定 HTTPS GCS bucket，规范化目标检查在请求前完成，总 30 秒 deadline、循环/异常 Location 拒绝、签名 URL 日志脱敏；自有 loopback mirror 不成为项目配置。
- Adapter contextual pip/npm/go 语法、生态/零 Lib runtime 锁定与 checked SDK manifest 接线。三 SDK 复用 setting/input/output/error 的 wire DAG；Python/JS runner 自动绑定 Port 同名导出，不增加业务注册表或第二套动态 App 调用。
- Python/JS 离线展开 runtime/dependencies/owned source，生成 launch manifest，启动只接受精确资源树中的解释器/argv/cwd。Python 固定 `-I -S -B` 且先导入可信 SDK；Node 兼容 ESM/CommonJS。普通 Dever tests 继续 fake，不启动 Lib。
- Linux SO_PEERCRED 跨用户 artifact 上传/下载、owner-only token/clean、metadata-only status (`verified:false`)；64 MiB 单资产、2 GiB/4096 条总缓存配额，外部 UID 两连接/总量六连接，保留管理员容量。流式 exact-length/SHA staging、原子发布、操作租约和传输 deadline。
- 原生资源使用独立二进制编译输入并按实际 SHA 共享一个静态 payload，编译前拒绝伪摘要，缓存身份仍含路径/摘要/长度/可执行位。移除 managed-only raw pack/archive 的重复嵌入；资源校验按 64 KiB 分块，不再额外读取整份解释器到堆。

已验收：CLI external_libs 最终 27/27（新增 Go ZIP 正常跳转、非法目标/异常 Location/循环/跳数、其他端点禁止跳转三项；主进程另独立复验 redirect 两项 2/2）；shared_toolchain 14/14，另 actual UID=65534 ignored 1/1（含 token、clean、二连接 admission 和未校验摘要）；external_resources 9/9（含独立 exec、重复资源、篡改和大文件）；native_entry 资源生成定向 2/2；Port/Adapter 16/16、Worker protocol/lifecycle 1/1。Python/JS 默认 Worker 6 项通过/6 个宿主专项默认 ignored，其中最终 Native managed Node exact 用例单独执行 1/1（285.45 秒测试总墙钟，包含打包/编译，不是启动延迟）。三语言 SDK 协议已分别通过显式宿主路径验收，不等同于正式无宿主环境。最终 core/runtime api+postgres+external 与 CLI libs/bins/三项变更测试的 Clippy `-D warnings` 通过；Go 追加变更后的 CLI Clippy 也再次通过，修改的 Rust 文件私有 rustfmt check 通过。未安装全局工具、未默认全量测试。

失败及修复：独立资源生成测试首轮固定 32 KiB 总源码上限错误，改为相对不含资源的基线增量，1 MiB payload 不增长为数组且伪摘要拒绝通过。Python 隔离参数下的 SDK 导入顺序和业务同名模块遮蔽已修；Native managed npm 初次已编译但临时 fixture 缺 setting.json，补自有 Adapter 配置后实际独立产物返回 code=0/data=8（已移除原 entry、lock、raw pack 并清空继承环境）。复制宿主 Node 的 pack 只证明 bootstrap 链路，复制 Python 仍依赖宿主 stdlib；不能记成正式发行。最终协议复核发现全面禁跳会阻断 Go ZIP 的官方存储跳转，已按精确 bucket 增加受限跳转；规范化 URL 会抹掉空 userinfo，故同时检查原始 URI authority，负向测试确认不发第二次请求。

根因复盘（Trellis break-loop）：

1. 类别 E/D：小 exec/fake runtime 夹具隐含假设了大 runtime 也能作为数字数组生成，以及 Python script 目录会出现在隔离 sys.path；默认测试缺少真正的隔离启动和 SDK 遮蔽证据。
2. 表面修复风险：仅减少 emitter 临时字符串不会消除重复 binary payload；只调整 Python source 路径而不先导入受信 SDK仍可能被同名模块替代。
3. P0 防复发已实施：唯一内容输入/静态引用、差分源码大小/伪摘要回归、真实独立资源执行、固定隔离参数/SDK 导入优先级/同名模块用例，以及 actual UID 与缓存 quota/summary 测试。
4. 扩展范围只覆盖同一资源打包/启动和 daemon 入口；没有改业务 API、真实配置或其他项目服务。
5. 已同步 README、LANGUAGE、IMPLEMENTATION、compiler/toolchain specs 与三个相关任务的当前边界，不新增平行文档、commit 或归档；总任务保持 in_progress。

该轮结束时待做的 Package 集成和 Go compile/link 已在后续实现并复验，见下一节。当前仍未完成：高级生态安装规则/Go sumdb、OS sandbox、daemon build IPC、macOS/Windows peer/service adapters、完整 HIR→LLVM 与六 target runtime ABI packs、正式签名语言包/无宿主发行、持续并发/完整性能矩阵和全量质量门。连接桥已经有产品代码，不能再把它记为完全未实现，也不能把它记为完整编译器。

## 2026-09-30 Package、Go 与编译器连接桥复验

本轮已完成的实现边界：

- `packages.rs` 复用现有 semver、ArtifactStore、Lib resolver 和原子锁：单版本传递闭包、精确归档/manifest 摘要、组件所有权、Package 源码注入普通 check/test/run/build、统一 Lib 锁、离线 remove 保留剩余版本、配置失败恢复原始字节。无本地 module 目录的 Package-only 项目也能加载。
- exec 和受管 Worker 共用由 setting.json + canonical lock 决定的 Package 归属，不根据归档中是否有同名目录决定是否回退。真实 checker 的入口是 `module/<component>/<domain>/<entry>`；原有组件碰撞已阻止本地同路径替换。新增实际 prepare 回归验证锁定入口缺失拒绝，以及本地组件碰撞拒绝，不声称证明了可利用漏洞。
- Go 仅使用锁定 build pack 的 compile/link/analyzer/stdlib/importcfg，工具以绝对路径和空环境运行；生成 main 静态检查同名 handler 的签名，并复用 SDK typed manifest。编译后仅嵌入 Worker 与合同，删除展开后的 raw build pack/module archive。显式作者工具制作的 host-target fixture 压缩 55,785,046 字节，展开 203,288,946 字节/358 项，未放宽原有 64 MiB 单资产与 256 MiB 展开预算。
- 新增 `dever-backend-bridge`，安全 API 显式选择 Linux/macOS/Windows 的 x86_64/aarch64 目标；LLVM 在进程内 parse/verify/O2/object，LLD 在进程内串行链接。所有 foreign buffer 同一 allocator 释放，输出只在当前用户私有且祖先可信的目录独占预留；失败清理不覆盖已有文件。不可恢复的 LLD 状态终止私有 compiler worker。私有 LLVM 18 开发 SDK 不做全局安装，当前 Linux 作者 SDK仍使用宿主 LLVM 动态库，不能记成自包含发行包。

主代理最终验证证据（不是仅采用子代理报告）：

- bridge `--features embedded --test backend_bridge`：5/5；六种真实对象和链接输出、IR/SSA/target/layout 拒绝、失败后重试、并发同输出唯一胜者、共享目录/可写祖先拒绝，Linux freestanding LLVM→LLD 程序在空环境实际退出 42。日志 `target/backend-bridge-test.log`。
- Go ignored 专项单独执行：1/1，44.46 秒（整项编译+执行，不是启动延迟）；包含 module ZIP、目标 tags、go:embed、typed call 与源码移除/空环境独立执行。日志 `target/closure-go-managed-final.log`。
- 最终 Lib 27/27、默认 Worker 6 项通过/7 项 ignored、Package 10 项通过/1 个 signed child 入口按设计 ignored/1 个既有慢链路过滤；signed child 由父测试实际执行，覆盖真实 `workers::prepare`。日志 `target/closure-package-worker-final.log`。
- 本轮之前已由主代理独立执行完整 Package 集合 9/9，其中安装→check/test/run/build→停止私有 daemon→独立运行链路通过，总 221.17 秒；最后的锁归属加强只重跑相关集合，没有重复这个慢链路。日志 `target/closure-package-go-regression.log`。
- 配置/Port/源码回归：application_config 6/6、Port/Adapter 16/16、source_architecture 12/12、source_visibility 10/10。日志 `target/closure-source-package-regression.log`。
- 最终 CLI/core/runtime/bridge offline locked check（runtime external/postgres/api）通过；CLI libs/bins 与三项变更测试、bridge embedded all-targets Clippy `-D warnings` 通过；本轮修改的 Rust 文件私有 rustfmt check 通过。日志分别为 `target/closure-current-cargo-check.log`、`target/closure-cli-final-clippy.log`、`target/backend-bridge-clippy.log`、`target/closure-final-rustfmt.log`。

没有新增应用环境变量、默认联网或宿主语言回退；未运行全量、真实数据库/CMS 服务级或新性能压测，未使用真实发行签名身份，未提交或归档。自有测试 daemon 已退出，无遗留服务；磁盘约 7.3 GiB 可用。总任务继续 `in_progress`：完整 HIR lowerer/稳定 runtime ABI、正式 packs/OS sandbox、高级生态安装、daemon build/跨平台发行和最终质量门仍需完成。

## 2026-09-30 强类型 LLVM 内核与首段 runtime ABI 复验

实现：`dever-core::llvm::emit_kernel` 复用 checked HIR、分句 Domain 和静态 specialization，直接产生 typed LLVM IR。当前覆盖 Int/Float/Bool/Nullable、普通 Record/Choice、多输出、分句绑定投影、值拷贝/字段修改、直接调用/静态 handler、左到右求值、短路和 checked 数值错误；故障保留 origin span 和逐层 caller spans，容量由无递归的可达实例图界定。所有可达但未支持的类型、资源、异步、失败、API/CMD/REST/Job 或 intrinsic 都明确带源码位置拒绝，不改默认 CLI 后端、不引入动态 Value 或 Rust fallback。

首段 runtime ABI 采用固定宽度标量/u32 status/显式输出指针，Decimal 使用原 runtime 的 LE 双 u64 位表示；Text/Bytes 返回带所有者的 Buffer，错误保留原数值/UTF-8 消息。`runtime-abi` feature 和 `embedded` 分离，前者独立构建的 staticlib 不含 LLVM/C++ SDK；所有 C 导出/原始指针操作仍在唯一私有 ffi.rs，runtime 本体保持 forbid。canonical 签名、借用、对齐、非重叠和释放合同位于 include/runtime.h。C++ compiler Reply 和 Rust Box ABI Buffer 分配器不混用。

主代理最终证据：

- `llvm_lowering` 4/4：typed IR、源位置拒绝、完整调用链，以及仅嵌在表达式中的 nominal type 定义。日志 `target/closure-llvm-lowering-tests.log`。
- `llvm_native` 9/9：简单内核六目标 object/link、复杂聚合六目标 object、本机分句/Nullable/Choice/record 值语义、静态 handler、Float NaN、多输出、短路、Int 极值/除零/溢出及实际 origin/caller span；其中 Windows 未提供 target runtime 时的显式拒绝是负向验收，不是 Windows 复杂链接成功。日志 `target/closure-llvm-runtime-tests.log`。
- 原 bridge 回归 5/5、ABI safe 合同 2/2；同上日志。
- 独立 `--no-default-features --features runtime-abi` archive 在 `target/native-runtime-abi` 离线制作；C header→实际库和 LLVM object→实际库 opt-in 2/2，owned 子进程 env_clear 并按 5 秒期限回收。开发 C/Rust 作者工具只用于夹具，不是产品启动依赖。日志 `target/closure-runtime-link-tests.log`。
- core/runtime/CLI/bridge（runtime external/api/postgres）offline locked check、bridge/new tests 严格 Clippy（runtime/api feature）、core/IR tests 严格 Clippy及私有 rustfmt check通过。ABI-only 构建成功，但它单独的严格 Clippy 仍遇到旧 HTTP client_address 的 dead_code 警告；未改无关 HTTP 或添加抑制。
- 双方独立只读审查没有剩余的当前 kernel/ABI milestone 阻断；不代替六目标运行或完整后端验收。

实测发现并处理：错误帧数组 GEP 原多一个索引，已在 owning emitter 修复；收集类型只看签名/locals 会漏临时 nominal 表达式，已复用 `check::visit` 全遍历并补最小回归。类别 D/E（IR 字符串通过不证明 LLVM 验证/执行、隐含类型会落地到 local 的假设），P0 防复发是当前真实对象/执行链及嵌套表达式回归。扩到 Windows 复杂聚合时，实际链接暴露 `__chkstk`/`_fltused` target runtime 缺失；没有提供 fixture 替代符号，此项仍归正式 target packs 未完成，已有负向失败/自有输出清理证据。已同步 canonical compiler/toolchain specs，仓库没有模板副本，无 commit/归档。

剩余：完整资源/集合/Decimal/Text/Bytes 的 HIR lowering、业务错误/async/Model/Port/Adapter/API/Job、全部 runtime ABI、六目标 CRT/stdlib/runtime assets和正式签名/无宿主发行；总任务及六阶段未完成项继续 in_progress。未运行全量、真实 PostgreSQL、CMS 服务级或新性能压测，未安装全局工具、改应用环境变量或使用真实发行身份。

## 2026-09-30 下一段：同步受管值与集合

范围：沿用当前任务推进 Decimal/Text/Id、基础 Bytes、List/Map 的 checked HIR→LLVM 与真实 runtime ABI 调用。不修改语法、默认 CLI 后端或配置方式；业务失败/捕获、Bytes Result-choice、资源、异步、Model/Port/Adapter/API/Job、正式目标 packs 仍未完成。

- [x] 复用现有 Text/Bytes/数值及 List/Map COW；固定 typed element 的布局、静态生命周期回调与消费式更新合同，原始指针操作仍只在私有 ffi.rs。
- [x] LLVM 为受管局部、临时、嵌套 Record/Choice/Nullable 建立具体类型所有权；正常、短路和故障出口统一清理，成功输出移交调用者。
- [x] runtime 原始错误 Buffer 移交 fault 尾槽，保留 source origin/caller stack，并提供调用者释放；不混用 C++ Reply 分配器。
- [x] 从真实 checked Dever 源码生成对象、链接实际 runtime archive，定向验收集合/静态 handler、别名与字段替换、提前失败、根输出及重复执行分配平衡。
- [x] 最小定向检查、格式/Clippy、独立审查并同步 canonical 合同；分别记录支持范围和仍明确拒绝的 HIR，不把该里程碑称为完整后端。

实现边界：

- `runtime.h` 的 Text/Bytes/List/Map opaque owner、process-static DeverRtType 回调及消费式 cursor 接入现有 runtime。元素存储用对齐的 MaybeUninit，不复制/读取 padding；普通 Element 保留 PartialEq，只有合法 MapKey 包装实现 Eq/Hash。Text split 使用调用者的同一 Text descriptor。
- LLVM 拆分到 abi/ownership/intrinsics/collections/traversal，复用 native last-use/字段替换分析。typed guard 在入口初始化，正常/失败统一清理；根输出与原始错误分别通过 `dever_outputs_release`/`dever_fault_release` 释放。无动态 Value/解释器或 Rust fallback。
- 支持 List/Bytes 的静态 each/filter/find/reduce/reduce_until/sum；C ABI 已有 Bytes 转换/切片，但其 BytesToText/BytesFromInts/BytesSlice Result-choice 源码 lowering 仍明确拒绝，不能记为完整 Bytes 已交付。
- 本轮所有永久测试都在根 test/。共享 checked-source→IR 辅助函数复用同一 SourceMap/check/emit 路径，不另建测试编译器。

主线程独立复验（全部离线，2026-09-30）：

- runtime-only staticlib 明确构建：`cargo build --offline --locked -p dever-backend-bridge --no-default-features --features runtime-abi --lib --target-dir target/native-runtime-abi`，通过；不含 embedded LLVM/C++ SDK。
- `cargo test --offline --locked -p dever-backend-bridge --features embedded,runtime-abi --test llvm_managed_native --test llvm_native --test native_runtime_abi --test native_runtime_link -- --include-ignored`：**25/25**。其中 managed 10/10（一个六目标 object 用例，九个 Linux 实际执行用例）、原有 LLVM 9/9、运行时 ABI 4/4、真实 C/LLVM→archive 2/2。日志 `target/closure-managed-final-tests.log`。
- `cargo test --offline --locked -p dever-tests --test llvm_lowering`：**5/5**，日志 `target/closure-managed-final-ir.log`。
- core/runtime/CLI/bridge 的 offline cargo check（runtime api/external/postgres、bridge embedded/runtime-abi）通过，仅编译、不连接数据库；日志 `target/closure-managed-final-check.log`。
- core/bridge 及上述四个 bridge 测试 target 的私有严格 Clippy `-D warnings` 通过，日志 `target/closure-managed-final-clippy.log`；root `llvm_lowering` 的严格 Clippy 也通过，日志 `target/closure-managed-final-ir-clippy.log`；本次改动 Rust 文件私有 rustfmt check 通过，日志 `target/closure-managed-final-fmt.log`。没有安装/替换全局组件。
- canonical C driver 与 allocation driver 的语法检查通过。九个 managed 执行用例预热后各重复 64 次，结果与存活分配计数保持一致；这只是有界所有权回归，不是性能/RSS/64/128 MiB 容量报告。
- 独立只读审查确认当前修复范围无剩余阻断；主线程验证关键执行证据，不以审查报告替代测试。

本轮实际失败与防复发：新增 `List<Text?>`/`Map<Text,Text?>` 边界执行发现 First/Get 原按 nullable 结果的内层值分配 ABI out，而 ABI 写入完整 nullable 行；同时外层 present 覆盖内部 null。此前只读审查和普通集合用例漏掉该合同。修复统一在 `llvm/abi.rs::runtime_optional_row`：按真实 row_type 分配，status 成功后才读取 present，无行不读 out，有可空行整行转交而非双包装。First/Get 共用此 owner，Find 同型路径也覆盖；新增 IR 与真实 native 回归后整组通过。根因是把“行存在”与“值非 null”混为一层的布局假设（类别 D/E），防复发合同已写入现有 compiler/toolchain specs，不创建重复规范或模板。

剩余先补业务失败/Result-choice，再接资源/异步及应用语义；正式 CRT/stdlib/runtime assets、签名 packs、无宿主发行与父任务其他未勾选闭环继续 `in_progress`。测试仅执行自有有限生命周期进程，不启动数据库/CMS 服务，不增加应用环境变量或默认联网；没有运行全量测试、新性能压测或六目标实际运行，没有 commit/归档。

## 2026-09-30 下一段：业务失败与 Bytes 结果

沿用已有 Rust 后端合同补同步 LLVM 缺口，不修改语言语法、Checker、默认 CLI 后端或应用配置。业务失败保持 nominal identity 和 typed payload，`result` 仅捕获已检查的业务错误；溢出、除零和 ABI 故障继续传播。复用 specialize/capture 的失败集合及匹配规则、已有 runtime Bytes 接口和受管值清理，不建第二套错误系统。

- [x] 具体类型的业务失败载荷、默认传播、根释放和源位置/调用链；捕获前后不泄漏或重复释放。
- [x] `result(named_call)` 的零/单/多输出成功、direct/wrapped error、显式 outer identity 及 static handler 传播。
- [x] BytesToText/BytesFromInts/BytesSlice 的成功/Failed choice；普通标准库调用和显式 result 共用同一通道，非法 ABI 不伪装为业务错误。
- [x] 真实 checked-source→LLVM→实际 archive 执行，包含受管载荷/提前失败、捕获后再次失败、运行故障不可捕获、Bytes 边界和重复分配平衡；保留上轮回归。
- [x] 最小 offline check、私有 fmt/strict Clippy、独立检查及 canonical specs/任务证据。

实现代理负责 core LLVM 与 IR 回归；主线程负责根 test 的实际执行验收、整合及规范。资源/异步/应用入口/正式 packs 仍不在此段完成范围；仅执行自有有限生命周期进程，不启动数据库/CMS/HTTP 服务或进行全量/性能测试。

实现与复用：`llvm/failures.rs` 统一 ordinary invoke/fail/capture/write_variant，复用 `specialize::failures`、`capture::targets` 与已有 TypeOwner；不改 Checker、runtime/bridge ABI 源码或默认 CLI。fault 前五字段不变，追加 type_id+1（0 为无载荷）、variant、按可达错误 Choice 最大布局对齐的内联 typed payload；Business code=4，匹配捕获后转交并清空整个 fault，根释放静态分派原类型。Bytes 三项只将 ABI status1 转为标准 Failed(Text)，status2 与数值/其他 runtime fault 继续传播。没有 Value/Any、错误字符串匹配、Rust fallback 或新 unsafe 边界。

主线程最终独立复验（离线，2026-09-30）：

- `cargo test --offline --locked -p dever-backend-bridge --features embedded,runtime-abi --test llvm_result_native --test llvm_managed_native --test llvm_native --test native_runtime_abi --test native_runtime_link -- --include-ignored`：**37/37**。新 Result target 12/12（1 checked-source/IR 合同检查、11 actual Linux LLVM native），旧 managed 10/10、原 LLVM 9/9、runtime ABI 4/4、真实 C/LLVM 链接 2/2。日志 `target/closure-results-final-native.log`。复用此前显式准备的 runtime-only archive，未在测试中递归构建。
- `cargo test --offline --locked -p dever-tests --test llvm_lowering`：**7/7**，日志 `target/closure-results-final-ir.log`。
- core/runtime/CLI/bridge offline check（api/external/postgres、embedded/runtime-abi）通过，日志 `target/closure-results-final-check.log`，仅编译不连接数据库。
- core/bridge 及上述 targets、root llvm_lowering 的私有严格 Clippy `-D warnings` 通过，日志 `target/closure-results-final-clippy.log`；改动与相关 LLVM 文件私有 rustfmt check 通过，日志 `target/closure-results-final-fmt.log`。未安装或修改全局工具。
- 新旧 managed native 共用 `test/dever-tests/tests/support/llvm_managed.rs` 原有 runner 与 C allocation driver：每次实际执行保持明确 output/fault 释放，预热后 64 次结果与存活分配平衡。新测试覆盖错误载荷 COW、原始 Bytes 文案、捕获后重失败、第二 variant 的根释放及精确 origin/两 caller spans。这是有界所有权回归，不是性能或容量报告。
- 独立 trellis-check 只读对照 Rust native/capture 合同，无当前 milestone 阻断；主线程重新核验关键运行证据，不以子代理报告替代验收。README、LANGUAGE、IMPLEMENTATION 与现有 compiler/toolchain specs 已同步，不建重复设计文档。

质量修正：原验收 helper 曾同时认可“任意成功或失败”分支，零输出捕获可能被错误 Done 误判为通过。主线程读用例后加入逐次 wanted_success/预期内层错误标签，成功/失败及 Missing/Conflict 互斥；加强后整组复验通过。记录的是测试判定缺口，不是已观测到的 LLVM 实现故障。中途其余失败是 fixture 语法/类型上下文错误，均按现有合同修正，没有放宽 Checker。防复发要求写入 compiler-contracts 的 Required Tests/Wrong vs Correct。

下一段继续资源生命周期与异步语义，再接 Model/Port/Adapter/API/Job 应用入口和正式目标资源。父任务与六个闭环的未完成项仍 `in_progress`；没有全量/数据库服务/CMS/性能/跨平台运行验收，没有 commit 或归档。

## 2026-09-30 下一段：资源生命周期与真实协程

当前用户已确认继续；沿用父任务，不切换默认 CLI 后端，也不改变语言或配置语法。

- [x] File 的 create_new/open/read/write/close、共享别名关闭与最后 owner 释放；原始 I/O 失败进入标准 choice/fail 通道。
- [x] 同步 Stream 的共享游标、文件 chunks、内部 pull/close 与静态 handler 遍历，EOF、最后错误项和提前释放保持 runtime 合同；不新增源码 pull/first(Stream)。
- [x] LLVM18 switched-resume coroutine + runtime ForeignFuture：真实 Pending/恢复、owned 输入/局部量/输出与 typed fault，初始/pending/final destroy 配对清理；不是每函数 block_on。
- [x] 基于现有单 runtime Scope/supervisor 的 Task、Group、Channel、sleep、checked 同步 blocking/parallel；取消和失败先排空后代，保留业务错误身份与来源。
- [x] checked-source → LLVM object → 实际 runtime archive 的有界自有资源/进程验收、旧定向回归、offline check、私有 fmt/Clippy、独立检查与 canonical 规范同步。

归属：bridge 只在原 private ffi.rs/header 增加资源和 future ABI；core 在 llvm/ 中完成 typed lowering，复用现有 specialization、liveness、TypeOwner 与失败通道；测试仍在根 test。异步输入、输出和 fault 通过静态描述符的 typed move/drop 转交，禁止解释器/Any/Value、错误 String 化、raw padding copy 或同步线程冒充 async。LLVM 自有协程帧使用配对 runtime allocator，借用的 poll Context 只在当次 resume 有效，final suspend 后绝不再次 resume。

此段不启动数据库/CMS/HTTP 服务，不做全量或压力测试，不安装全局工具；AsyncStream、网络/系统族、Model/Port/Adapter/API/Job 入口及正式跨平台 packs 仍需后续接入，不能以 Task/Group/Channel 的定向验收代替完整应用后端。

### 2026-10-01 最终复验

- 显式离线构建 runtime-only archive（`target/native-runtime-abi/debug/libdever_backend_bridge.a`），含 File/Stream、异步与同步 time_sleep ABI；与 embedded LLVM/LLD 分离。测试没有递归 cargo 构建、宿主语言回退、联网或数据库连接。
- `cargo test --offline --locked -p dever-backend-bridge --features embedded,runtime-abi --test llvm_async_source --test llvm_resource_native --test llvm_async_native --test llvm_result_native --test llvm_managed_native --test llvm_native --test native_runtime_abi --test native_runtime_link -- --include-ignored`：**70/70**。async_source 21/21（17 actual archive 执行、六目标 object、源码 Checker/两个原规则拒绝），resource 10/10、async ABI 2/2、旧 Result/managed/kernel/ABI/link 37/37。日志 `target/closure-resources-async-final-native.log`。
- 源码覆盖大记录及 Text/List/Map/多输出跨 Pending、异步 each/reduce 静态 handler、同步/异步 Task、Group 容量和 Unit child、Channel 背压/Nullable/close/End、stop 与嵌套后代释放、blocking/parallel 的 owned 值和业务错误、capture/refail、默认业务错误的精确 origin/caller、负数 sleep 原始消息、数字 code2 与 ABI code3 原始 Buffer 不可捕获。受管运行 runner 用并发安全的原子 allocator ledger，一次预热后 64 次重复核验；不是 RSS/体积/性能测试。
- 资源验证覆盖同次第二次 create_new 必须失败、不同内容不能覆盖、File 别名 close、read EOF、Stream 共享进度/提前关闭、reduce_until 未访问行、一次末尾 Failed 后 End、each 的 owned 业务错误、运行故障清理及 root File owner 移交。C 层无效输出在 I/O/cursor advance 前拒绝。
- `llvm_lowering` **7/7**，日志 `target/closure-resources-async-ir.log`；原 runtime Stream 定向 **3/3**、File 定向 **1/1**，分别见 `target/closure-resource-reference-stream.log`、`target/closure-resource-reference-file.log`。
- core/runtime/CLI/bridge 的 offline locked check（runtime api/external/postgres，bridge embedded/runtime-abi）通过，日志 `target/closure-resources-async-final-check.log`。postgres 仅编译，没有真实 PostgreSQL。
- 私有工具对 core lib 与 bridge lib/上述 8 targets 的严格 `Clippy --no-deps -D warnings` 通过，日志 `target/closure-resources-async-core-clippy.log`、`target/closure-resources-async-bridge-clippy.log`。改动和相关 LLVM/ffi/根测试的 rustfmt check 通过，日志 `target/closure-resources-async-final-fmt.log`；没有安装全局工具，不代表全仓库质量门。
- 独立只读检查已完成：发现并修正内部 Unit/Outputs ABI transferability、同步 sleep 符号、可达 async 临时值的 Send 验证；fresh review 未发现 typed fault、initial/pending/final cleanup 的剩余阻塞。主线程用实际最终日志确认关键证据。
- 已同步 README/IMPLEMENTATION/LANGUAGE 与 compiler/toolchain canonical specs；父任务仍 `in_progress`，未 commit、未归档。磁盘约 4.9 GiB 可用，此段没有清理用户数据或旧目录。

质量复盘：内部 Unit/Outputs 的源码属性不可直接充当 ABI row 属性（跨层合同/隐含假设）；修复归属在唯一 descriptor 生成器，不改 Checker，真实 sync Task/多输出/blocking 用例防回归。SyncFunction.invoke 只借用 runtime-owned input，避免双 drop。测试判定另有两处缺口：create_new 相同内容掩盖错误覆盖，以及以 RuntimeAbi 字符串 oracle 验证数字除零；已改互斥结果/不同内容、精确 code2/origin，并另验 code3 原始 Buffer。规范已记录字段转移、Scope/Send、状态映射和 exact oracle 要求；没有新增重复框架/错误系统或伪造成功分支。

后续仍需 AsyncStream/网络及其余系统 intrinsic（含 timeout/race）、Model/Port/Adapter/API/Job 的 LLVM 应用入口、默认 CLI 后端整合、正式六目标 runtime packs，以及父任务其余真实生态/发行/性能质量门。资源/协程这一段已验收，不以此标记整个阶段四或全部语言完成。

## 2026-10-01 下一段：AsyncStream、TCP 与 timeout/race

用户已确认继续，沿用原 LLVM 后端阶段；本段不改变语法、Checker、默认 Rust bootstrap CLI 或配置协议。

- [x] AsyncStream 的 List/Bytes/Channel 来源、ticks、共享游标/close、静态 handler 遍历及当前合同允许的并行分派。
- [x] Socket/Listener typed owner 和 TCP connect/connect_timeout/listen/accept/port/read/write/timeout/close/chunks/connections；复用 runtime net 的方向锁、期限及取消合同。
- [x] timeout/race 消费同签名 Task，超时/胜出结果返回前排空被取消的工作；fallback 只在真正超时后运行，业务和 runtime fault 保持原始 typed 身份与来源。
- [x] checked-source → LLVM object → runtime-only archive 的实际运行、六目标 object、旧定向回归、offline check、私有 fmt/Clippy 与独立合同检查。

改动归属在 core llvm/ 和 bridge 私有 ffi.rs/canonical runtime.h；永久用例仍在根 test。复用现有协程、typed move/drop、recoverable choice/fault 和单 runtime Scope，不增加第二套异步引擎、动态值或 fallback。验证只使用自有短生命周期 loopback、系统分配端口和有界等待，不接触真实服务或外部网络。HTTP/TLS/WebSocket、其余系统族及 Model/Port/Adapter/API/Job 应用入口不计为此段已完成；正式 packs、共享编译和最终质量门仍按父任务推进。

### 本段最终复验

- 主线程显式 offline/locked 构建 runtime-only staticlib，最终 archive 晚于最终 ffi 修改，不含 embedded LLVM SDK；日志 `target/closure-stream-network-final-archive.log`。
- 9 个相关 LLVM/ABI targets 首次最终集合 **92/92**（旧 70 + 新 22），日志 `target/closure-stream-network-final-native.log`。随后追加「并行 handler 故障打断空闲 Channel producer，同时只关闭流别名而不关闭独立 Channel」精确回归；最新新 target **23/23**（21 个实际 archive 执行项、Checker、六目标 object），日志 `target/closure-stream-network-final-new-native.log`。当前独立覆盖共 **93 项通过**，不是将重复执行相加。
- 新源码执行覆盖 nullable 流、Channel 背压/挂起 handler、ticks 和提前关闭别名、四类序列及同步/挂起 handler 的合法并行路径、同步 File Stream worker 的 owned 业务错误、timeout/race 受管记录及多输出/null/Unit、真正超时的 async fallback、业务错误不走 fallback、数值 code2 的原始来源、ticks/timeout 非法期限的原始 code3 文案。TCP 自有 loopback 验收 short read/完整 chunks/EOF/共享关闭、原始输入错误、chunks/connections 一次末尾 Failed 后 End；超时返回后对端 EOF 证明 pending 后代 Socket 已释放。
- 沿用同一 atomic allocator ledger，实际 native 每个程序预热一次后重复 64 次，输出/fault 与资源释放平衡；这不是体积/RSS/延迟/64或128 MiB 性能报告。六目标只证明对象生成，本机 Linux x86_64 证明实际运行，没有六 OS 执行验收。
- 原 runtime/IR 定向复验：`llvm_lowering` **7/7**、`async_network` **9/9**、`async_composition` **6/6**（明确过滤原 Rust native 慢链路）；日志 `target/closure-stream-network-reference-ir.log`。覆盖原背压/空闲 producer 故障、读超时后继续使用、部分写取消先关闭、阻塞后代排空和规则拒绝；不启动现有服务。
- 异步 poll 的非法输入状态最小修正为既有 canonical `4`，不再与 typed fault `2` 冲突；C driver 新断言实际 archive 执行 **2/2**，日志 `target/closure-stream-network-final-async-abi.log`，严格 C11 语法检查通过。
- core/runtime/CLI/bridge offline locked check（runtime api/external/postgres、bridge embedded/runtime-abi）通过，日志 `target/closure-stream-network-final-check.log`；postgres 仅编译。core lib 与 bridge lib/9 targets 私有严格 `Clippy --no-deps -D warnings` 通过，日志 `target/closure-stream-network-core-clippy.log`、`target/closure-stream-network-bridge-clippy.log`；追加新 fixture 后再次 Clippy 通过，日志 `target/closure-stream-network-final-new-clippy.log`。相关 LLVM/ffi/新根测试的 rustfmt check 通过，日志 `target/closure-stream-network-final-fmt.log`。没有安装全局工具。
- 按 Trellis 2.2 进行双向非本人代码审查：core 作者独立对照 bridge ABI，bridge 作者独立对照 core 与原 native/runtime；新 reviewer 线程额度不足时复用已完成线程，没有启动额外进程。最终未发现本段剩余阻断；主线程独立执行关键验收，不以作者 compile 或审查报告代替运行证据。
- 已同步 README/IMPLEMENTATION/LANGUAGE 和现有 compiler/toolchain/directory specs。父任务仍 `in_progress`，六个闭环的未完成项不勾选；无全量测试、真实 PostgreSQL、CMS 服务、压力测试、正式签名发布、全局安装、commit 或归档。磁盘约 4.7 GiB 可用，未清理用户数据。

首轮失败均明确保留为未通过的迭代证据：fixture 的空比较缺类型上下文、named Outputs 跨 suspension、保留词 `other` 当局部名、Result 的 timer failure 集合不匹配；均按现有语言合同改 fixture，没有改 Checker 或降级错误类型。桥接中途一次 Result error 类型推断失败已加具体类型，最终 check/Clippy 通过。质量收尾还保证 race null 创建路径保留 caller owners、遍历关闭责任在 state/context 求值后建立，以及 ParallelHandler 只校验布局而不额外分配临时三份存储。

下一段仍需 HTTP/TLS/WebSocket 与其余系统 intrinsic，再接 Model/Port/Adapter/API/Job 应用入口、默认 CLI 后端与正式六目标 runtime packs；不得将这 93 项内核/ABI 定向证据记为整个阶段四或完整语言交付。

## 2026-10-01 LLVM HTTP/TLS/WebSocket 与系统 intrinsic（本段已验收）

沿用当前 checked HIR、静态 specialization、强类型 fault/owner/coroutine 描述符及既有 runtime；不改语言语法、配置或默认 CLI 后端，不把本段内核验收计为完整应用/正式目标包验收。

- [x] 补 HTTP 单次/池化/流式请求与静态 server handler 的 typed ABI/lowering，复用 Hyper、原 HTTP/2 和连接/任务 Scope。
- [x] 补 TLS client/server、live reply/SSE/WebSocket 与对应资源 retain/drop，保持背压、原始可恢复错误、取消和有界排空。
- [x] 补剩余非应用系统 intrinsic（时间、进程参数、日志/stdout、UUID/Secret/crypto），保持敏感类型不可观察与既有阻塞工作边界；鉴权/上传/Job/Model 归属后续应用 lowering，不擅自合并。
- [x] 根目录 checked-source→LLVM→真实 runtime archive 定向执行、无网络 Checker/六目标对象生成及必要旧覆盖复验；网络仅使用自有临时文件/有界 loopback peer，TLS 复用根目录 fixture。
- [x] offline locked check、相关 fmt/Clippy、独立边界复核，并更新现有 canonical 合同与本记录。记录精确已验收范围和剩余项，父任务保持 in_progress。

实现所有权：bridge 工作线仅修改私有 `ffi.rs`、canonical `runtime.h` 及必要 runtime feature 接线；core 工作线仅修改 `llvm.rs`/`llvm/**`。主线程负责根测试、target 注册、验收和合同/任务记录；不互相覆盖，不运行全量或现有服务。

### 实现与复用

- 新 `llvm/http.rs`、`http_values.rs`、`http_handlers.rs` 复用原 Hyper/rustls/tungstenite 与 Scope，使用固定命名字段、opaque owner、静态 typed sync/async handler/context；不经过 JSON/Value。响应 owned 字段先注册再做可能失败的 header 转换。
- `llvm/system.rs` 接日历/时钟、argv、stdout/log、Uuid/Secret/crypto，密码计算保留 blocking。Uuid 使用真实 UUID owner；敏感类型不新增 Render。`llvm/render.rs` 只在请求边界渲染可观察的终端 fault，保留原有日志+500/中止回复+服务器继续处理的行为。
- 协议 async poll 在 I/O 前检查 output/presence/align；非法 metadata 返回 4，不改 sentinels 或操作状态，修正后可重试。原非协议 async 合同不扩大。
- 纯日志探针定位到 runtime Logger 后，主线程明确扩展 bridge 的 owner 范围到 `log.rs`：单一预分配 128 槽队列配合 Mutex/Condvar，保留原过滤、截断、JSON、FIFO、低级别丢弃/高级别背压；串行固定 ACK，异常退出释放积压并唤醒等待者。没有新依赖或第二套 logger/队列。
- 根 test 保留源码执行、C ABI 与队列回归；实际 native helper 共用原严格 allocation ledger，只增加明确的昂贵用例次数/期限与文件输出捕获，不建另一个执行器。

### 最终证据

- 显式 offline/locked 重建 runtime-only archive，最终 `2026-10-01 03:44:10 UTC` 晚于最终 ffi 与 log 修改，不含 embedded LLVM/C++ SDK；`target/closure-http-system-final-archive.log`。
- 新 `llvm_http_system_source` 唯一覆盖 **25/25**：23 项 Linux checked-source→LLVM→实际 archive 执行；21 份源码 Checker；21 份源码×六目标 object。末批输出 fixture 的 Map 分隔修正后单独复验，未重复相加：`closure-http-system-native-final.log` 的 22 项已通过，修正后的三个检查见 `closure-http-system-output-final.log`、`closure-http-system-checker-final.log`、`closure-http-system-objects-final.log`。六目标 object 不等于六 OS 运行。
- 实际执行覆盖独立 chunked wire/repeated headers、H1/H2/TLS 池化与流式上下行、live Reply/SSE、WS/WSS 文本/二进制/控制帧与共享关闭、owned context 最初挂起前克隆、owned handler 失败后 500 再正常处理、原始诊断、时间/UUID/Secret/密码/argv、stdout 与四级精确 JSON 日志。一般 ledger 每进程 warm 1+64 次；密码是 warm 1+2 次，45 秒边界。
- C/header/archive **4/4**，含非法协议目的槽→原样保留→有效重试→完成后拒绝 repoll，以及纯 log flush 的严格 ledger；`closure-http-system-c-abi-final.log`。C 探针不假造成功的 WsReceive row，真实消息路径由源码执行覆盖。另同一纯 flush 探针在 **40 个独立进程**中全部通过。
- 原 8 个相关资源/异步/集合/Result/ABI targets **91/91**；`closure-http-system-old-regression.log`。root IR **8/8**；`closure-http-system-ir-final.log`。原 Rust bootstrap 的 source logging **1/1**；`closure-http-system-bootstrap-log.log`。Logger 的 FIFO/dropcount、Warn/Error 背压、并发 flush、writer panic 四项 **4/4**；`closure-http-system-log-unit.log`。
- core/runtime/CLI/bridge offline locked check（runtime api/external/postgres，bridge embedded/runtime-abi）通过；`closure-http-system-check-final.log`，postgres 仅编译。core lib、bridge lib/10 定向 targets 与 root IR 的私有严格 `Clippy --no-deps -D warnings` 通过；`closure-http-system-core-clippy.log`、`closure-http-system-bridge-clippy.log`、`closure-http-system-ir-clippy.log`。实际 `log.rs` 与根队列测试的独立 strict Clippy 通过；`closure-http-system-log-clippy.log`。相关文件私有 rustfmt check 与 C11 strict syntax 通过；`closure-http-system-fmt-final.log`。没有安装全局工具；不是全仓库 Clippy 门。

### 根因与防复发

首轮 native 为 15 通过、6 项 ledger 失败（+2/+1），返回值 oracle 没失败。只调用 flush 的 C 探针同样出现 +2，证明 owner 在 Logger，不能据此断言 LLVM/HTTP 泄漏。固定 ACK 通道仍失败；分配栈最终确认是 std mpsc receiver 首次阻塞的 48B Context 与 96B waiter Vec 持久初始化，发生在 ACK 后。最终修正整个单一队列等待 owner，而不是增加 warm、sleep 或放宽 ledger。此前关于「仅临时 ACK 析构 race 就解释全部失败」的初判过强，已纠正。纯探针、全部协议源码和旧回归共同复验通过；独立检查确认无丢唤醒、串 ACK、队列 owner 遗漏或新阻断。约束已写入 logging/toolchain/compiler canonical specs。

另修正 C async poll 的边界检查时机：先 poll 后检查目的槽会推进有副作用的操作，必须先验证 metadata，根 C fixture 固化可重试合同。测试维护同步迁移原「stdout 尚未支持」断言为真实支持检查，保留合法 App 内鉴权 intrinsic 的源码定位拒绝；没有放宽 Checker。Clippy 曾命中失败的 rustc 探测缓存，保留到 `target/closure-http-system-rustc-probe-failed.json` 后只重建该元数据，最终通过。

README/LANGUAGE/IMPLEMENTATION 与 backend compiler/toolchain/directory/logging/index 已同步。未运行全量、数据库连接、CMS 服务、新性能/RSS/64–128 MiB 压测或跨 OS 实际执行；没有 commit/归档。下一段继续 Model/Port/Adapter/API/CMD/Job 与鉴权/上传等 LLVM 应用语义，再接默认 CLI 和正式六目标 runtime packs；父任务其余生态/发行/最终质量门未因此变为完成。

## 2026-10-01 LLVM CMD/App/Dever Adapter 应用执行链（本批已验收）

继续原生产化任务，不重定语言语法，不切换默认 CLI。应用合同不能只靠解除 kernel 拒绝：原 Rust 生成入口还拥有配置、事务、权限、租户、服务与清理。

本批先交付独立 `llvm::emit_application`：真实 CMD 参数 → shared checked wire schema → App → 配置选中的 Dever Port/Adapter → typed 输出 → 固定 JSON envelope。Wire 的 JSON 只留在输入/配置/输出边界，复用 runtime `wire::parse/Node/Encoder`；业务仍是具体 LLVM 类型和静态函数。选中 Setting 在应用 entry 生命周期内初始化并清理；未选中 Adapter 不要求其配置。同步/挂起 CMD 都必须保持原 source fault、typed business failure 和所有权。

主线程负责根测试、实际 archive 链接、文档和串行定向验证；原 core/bridge implement 线程分持 LLVM lowerer 和 canonical C ABI。普通 `emit_kernel` 不替代生成应用根。实际执行使用测试自有临时目录、明确离线 runtime archive 和严格 allocation ledger；不启动数据库、CMS、外部 Worker 或服务。

已完成：

- [x] 独立 `llvm::emit_application` 与 compiler-owned CMD 根，标准 `dever_application_entry`/output/fault release；内部 run 不公开。完整 CMD 名与单个 JSON 对象使用原参数合同，同步/推导挂起 App 共用已有 Scope 和 typed fault。
- [x] `llvm/wire.rs` 消费原 checked schema DAG，具体 scalar/record/List/nullable codec 复用 runtime Node/Encoder，不把 JSON/Value 引入业务。CMD 顶层复用原 `Inputs`，保留缺失/未知字段文案、重复键检测和字段错误优先级；嵌套 wire policy 不混用。
- [x] `llvm/ports.rs` 的 closed Dever Adapter 选择与 selected Setting 初始化。配置仍只读 `config/setting.json`，未选中的 Adapter 不要求配置，单个无 Setting 的 Adapter 不要求配置文件。选中槽为模块私有，非阻塞原子 gate 拒绝重入/重叠，根 Scope 排空后清理/复位并释放 gate；部分初始化失败走同一清理。
- [x] 私有 canonical wire/CMD/settings ABI，回调借用不越过同步调用，输出 metadata 校验在解析/取字段/选择之前。成功解码后若 trailing CMD input 或 Buffer 不合法，桥接 drop 完整 owner；失败回调由编译后的部分初始化 guard 清理。复用原 runtime wire/config owner，`runtime-abi` 只增加 wire feature，不依赖 LLVM/C++ SDK。
- [x] 新应用根拒绝尚未接通的 Model/API/REST/Job/auth/permission/tenant 和 external Adapter，不按某个 CMD 当前未调用就跳过启动合同；普通 kernel 仍拒绝应用入口。
- [x] 公共 native runner 下移到根测试 support，kernel/application 共用实际 archive、超时、owned process 和原 allocation ledger；没有平行测试执行器或放宽既有 fault oracle。

实际验收：

- runtime-only archive 显式 offline/locked 重建通过，最终文件时间 `2026-10-01 05:12:48 UTC`，晚于 ffi/header 修改；不含 embedded SDK。core 后续 failure metadata 修正只改变 generated object，不要求重新构建 runtime archive。
- 新 `llvm_application_source` **19/19**：15 项 Linux x86_64 checked-source→LLVM object→实际 archive 执行，另含原 Checker、六目标 object、kernel/app 边界及 source-located unsupported metadata 检查。实际执行保持 warm 1 + 64 次、严格原分配平衡与每次精确 JSON/fault oracle；覆盖 owned Unicode/大 Int/全部受支持 wire scalar/原始 Json、可空顶层/列表、异步输入、业务失败/result/数值错误、配置选择、部分 Secret/多 Adapter 初始化失败及 Task 读取 selected Setting。日志 `target/closure-llvm-application-source-final.log`。六目标 object 不是六 OS 运行。
- 旧受影响回归 **43/43**：managed 10、Result 12、async source 21；runtime ABI **4/4**，C/LLVM 实际链接与 ledger **5/5**（含 wire/CMD callback/非法槽/成功后 finish 失败清理），日志 `target/closure-llvm-application-regressions.log`。IR **8/8**，日志 `target/closure-llvm-application-lowering.log`；没有重复相加早期运行。
- core/runtime/CLI/bridge offline locked check（runtime api/external/postgres、bridge embedded/runtime-abi）通过，日志 `target/closure-llvm-application-check.log`；PostgreSQL 仅编译，未连接。
- core lib 与 bridge lib/本批六个相关 test targets 私有 `Clippy --no-deps -D warnings` 通过，日志 `target/closure-llvm-application-core-clippy.log`、`target/closure-llvm-application-bridge-clippy.log`。相关文件私有 rustfmt check 通过，日志 `target/closure-llvm-application-fmt.log`；两个 C fixture 的 C11 `-Wall -Wextra -Werror` 检查通过。没有全局工具安装。
- 本批独立 ABI/所有权交叉检查及最后的 failure metadata/runner 有界复核无阻断。主要风险分别由实际 fault、partial owner、重复 gate 复位和严格 ledger 用例覆盖；不把审查声明当作执行证据。

发现并修复的真实缺陷：扩展到 19 项时首轮为 18 通过、1 项 lowering panic。无配置同步 Adapter 声明了 `SleepResult`，实现并未实际构造它，但 fault 布局/drop 仍消费 `specialize::failures`；旧统一收集点遗漏仅声明的 failure nominal/owned 字段。现于 `validate_function`/`Module::new` 对同一 reachable failure 集做校验及递归类型收集，不注册无关类型、不加 fallback。单用例与最终 19 项、43 项旧回归均通过，canonical compiler spec 已记录此 invariant。最初关于「CMD 输入行缺描述符」的猜测已纠正。此前源 fixture 的字段换行、Int `//` 与 Port fails 合同，以及 C fixture 的 mandatory equal callback 修正是测试 fixture 修正，不是放宽语言/ABI。

README/LANGUAGE/IMPLEMENTATION 与 backend compiler/toolchain/directory specs 已同步。没有全量测试、数据库/CMS 服务、真实外部 Worker、新性能/RSS/64–128 MiB 压测或跨 OS 执行；没有 commit/归档。父任务仍为 `in_progress`。

完整 LLVM 应用阶段仍按依赖收口：① 本批入口/wire/CMD/Dever Adapter；② Model/双数据库 SQL plan/事务/DbError/Related/RowStream；③ API/auth/tenant/component/permission/REST/Upload；④ Job 与 compiler-owned Test；⑤ external Adapter 与准备好的资源集成。后续未支持族继续 source-located 硬拒绝，不用 Rust fallback、成功 stub 或放宽 checker。只有这些全部通过，才进入默认 CLI 和正式六目标 packs。

## 2026-10-01 LLVM Model/ORM 与事务（实现与定向验收通过）

承接已验收的 CMD 应用根，本批实现静态 Model 操作、SQLite/PostgreSQL SQL/bind/row codecs、nested transaction 与精确 DbError、Related 批量加载、typed SQL、RowStream，以及 schema/migration/Seed 启动。沿用 checked QueryPlan 和原 driver，不修改语言语法、业务 CMS、默认 CLI，API/auth/tenant/Job/external 仍为后续边界。

代码归属：core implement 负责 `llvm/**` 和必要纯 SQL/schema helper 共享提取；bridge implement 负责私有 `ffi.rs`/`runtime.h`、独立数据库 ABI profiles，以及 runtime 数据库生命周期薄适配；主线程负责根测试、所有 Cargo/实际验收、canonical specs 和本记录。两个 implement 不运行 Cargo/真实数据库，不回滚对方或既有修改。

已经确认的结构风险：原 `config::bootstrap_scoped`、SQLite/PostgreSQL registry 都是 OnceLock，shutdown 后不能重复初始化或复用已关闭池。因此新 LLVM 根需要 owned `database::Session`，原 Rust native 仍保留现有 bootstrap；两个路径共享实际 driver 构造。事务普通/顺序调用继承隐藏 context，并发边界显式 detached；迁移和 SQL 仍使用原 owner。Session 在根 Scope 排空后、同一 runtime 销毁前关闭；必要 `task::run_entry_with_typed_cleanup` 只扩展这个生命周期，原入口委托它保持原语义。禁止新建池栈/SQL builder/反射业务 Value、成功 stub、tenant-to-global fallback，禁止新增环境变量配置。

验收先离线 Checker/objects/最小新根 target，再实际自有 SQLite 与严格所有权检查；PostgreSQL 只使用显式隔离 fixture，任何临时服务启动先说明。共享 helper/driver/task 改动必须复验受影响的原 Rust native/ORM/async 合同，不能只测新实现。未通过前不标记本批或父任务完成。

本批实现：

- checked Model 的 CRUD、bulk/upsert、offset/cursor、nullable/scalar/private/choice codecs、typed SQL 和 bounded RowStream 均由 LLVM 降级到真实 runtime archive；ToOne nullable join 与 ToMany 按 998 个父键分批，保留每父条数限制。schema/migration/data migration/Seed 按实际数据库分组，先建全表再建外键。SQL/DDL 和 SQLite/PostgreSQL driver 复用原 owner，不复制 ORM 引擎、不通过 Rust fallback 执行。
- 根 `database::Session` 持有本次 Settings/pools，根 Scope 排空后在同一 runtime 关闭，支持重复进入；没有给全局 OnceLock 重绑新池。数据库配置仍只来自程序旁 `config/setting.json`。本切片只支持普通非租户 Model，真实 tenant 配置硬拒绝，不回落平台数据库。
- 顺序调用/handler 继承隐藏事务 context，Task/并发边界 detached；嵌套 transaction 共享 owner，不引入 savepoint。CMD 顺序为输入解码→开启事务→App→输出编码→commit→stdout；数值、业务、数据库或输出编码失败均 rollback，commit 前已初始化的输出有明确 typed drop。清理失败只追加 cause，不覆盖原故障。
- `runtime-abi`、`runtime-sqlite`、`runtime-postgres` profiles 独立选择实际 driver，canonical C header 中的 DbError/Row/Rows/Related/RowStream ABI 只使用固定持久化边界值，业务字段保持具体类型。非法输出槽在 I/O、pull/next 或 clone 前拒绝。

实际验收（未将重复复验相加）：

- 最终双 driver runtime-only archive 重建通过，时间 `2026-10-01 08:56:03 UTC`，晚于最终 ffi/运行时机械修正；日志 `target/closure-llvm-database-archive-final.log`。
- 最终 `llvm_database_source` **23/23**：19 项自有 SQLite 原生执行、1 项全部 fixture Checker、1 项六目标 object，以及共享 PostgreSQL fixture 的 2 项纯校验。每个原生 probe 为 checked source→LLVM object→实际 archive，warm 1 + 64 次，清空子进程环境，检查精确 stdout/fault 与原分配平衡；覆盖两版迁移、nullable/所有存储 scalar、cursor、ToOne/ToMany、stream full/early-close/cancel、单连接嵌套/顺序事务和 constraint/numeric/encoding rollback。日志 `target/closure-llvm-database-sqlite-final.log`；六目标 object 不等于六 OS 运行。
- 真实自有 PostgreSQL **3/3 组**（共 19 个逻辑场景，迁移包含两版）：CRUD/codec/query/relations/stream/transaction/schema/Seed，以及 encoding 和 uncaught constraint 回滚后重试/读取。日志 `target/closure-llvm-database-postgres-final.log`，运行 271.58 秒；这是依赖升级与最后 MSRV 机械修正前的实际验收，不声称之后又启动服务。仅连接自有 loopback fixture、随机 schema，未接触部署数据库。检查残留 schema 为 0 后停止服务，临时 `config/setting.json` 删除恢复原先不存在状态；PG fixture 移入可恢复目录 `/root/.local/share/Trash/files/dever-llvm-pg-ZQQmWO`，日志保留 `target/closure-llvm-database-postgres-server.log`。
- 依赖修复后受影响旧应用/async/managed/Result 回归 **62/62**（19 + 21 + 10 + 12），日志 `target/closure-llvm-database-native-regressions.log`；最后 MSRV/ffi 机械修正后重跑 task-group 取消及后代 frame 释放关键用例 **1/1**，日志 `target/closure-llvm-database-async-critical-final.log`。
- 最终原 runtime 定向回归 **24/24**：async composition 5、ORM migration 7、Seed 3、entry cleanup 3、SQLite runtime 6，日志 `target/closure-llvm-database-runtime-regressions-final.log`。覆盖 deferred constraint commit rollback、pool 未知状态丢弃、全部四种 decoder 的非法 UTF-8 列索引及后续连接替换。
- 最终 C ABI probe **1/1**，十种 DbError/borrowed Text cause、Related Loaded(null)/Unloaded、非法/错位槽和不消费原 fault 的 rollback 校验通过；日志 `target/closure-llvm-database-c-abi-final.log`。C11 `-Wall -Wextra -Werror` 语法检查通过，日志 `target/closure-llvm-database-c-format-final.log`。
- base ABI / SQLite / PostgreSQL / 双 driver 四 profile offline locked lib check 通过；core/runtime/CLI libs/bins、bridge lib/本批根 tests 与 cleanup/SQLite runtime tests 的严格 Clippy `-D warnings` 全通过，日志 `target/closure-llvm-database-profile-*.log`、`target/closure-llvm-database-clippy-{product,bridge,tests}-final.log`。本批 LLVM/bridge/database/task/config/driver/根测试相关文件私有 rustfmt check 通过，日志 `target/closure-llvm-database-rustfmt-final.log`；未声称原 `native/orm.rs` 等历史基线的全局格式门通过。工具私有运行，不安装/替换全局命令。

真实故障与根因修复：首轮 SQLite 21 项中 stream 用例超时，之后单用例重复暴露。两个 blocking-worker 栈定位到旧 bundled SQLite 3.51.1 的 WAL close/replacement-open VFS 锁反转（global→inode / inode→global），不是 Dever Scope 未清理。已升级既有精确依赖 `deadpool-sqlite` 0.13.0→0.14.0，锁定 `rusqlite` 0.40.2 / `libsqlite3-sys` 0.38.2 / `hashlink` 0.12.2，使用 bundled SQLite 3.53.2；没有改 registry 缓存、私有 SQLite fork、超时阈值或未知连接复用规则。四条 SQLite decoder 适配真实列索引，新增非法 UTF-8 回归。依赖要求作者 workspace 最低 Rust **1.95**（本机实际使用 1.98.0，未单独运行 1.95 toolchain）；对应新 MSRV 触发的 let-chain/is_multiple_of/as_chunks 写法仅机械修正，独立复核短路/资源 drain/对齐/偶数长度 invariant 后严格 lint 与上述关键复验通过。临时栈探针和 thread-trace 源已移除，原 `managed-driver.c` 恢复，诊断日志保留。

另外纠正了测试 oracle：cursor `next = null` 传作 after 表示重新开始，不表示终点；PostgreSQL constraint 会使当前事务 aborted，不能沿用 SQLite 捕获 constraint 后继续 SQL 的测试预期。现 PostgreSQL 捕获 NotFound 独立验证错误身份，uncaught constraint 则验证整笔回滚和下一次入口读取。没有放宽 checker 或改变既有 driver 事务语义。

README/LANGUAGE/IMPLEMENTATION 与 compiler/toolchain/directory/database canonical specs 已同步，数据库 spec 保存锁顺序回归及清理 invariant。未运行全量 workspace、CMS/HTTP 服务、性能/RSS/64–128 MiB、其他 OS 或正式 packs。ToMany 的 998 分批静态路径已复核，实际 fixture 只覆盖少量父键；新 LLVM commit/Session-close 人为失败注入未做，原 Seed deferred-commit 回归不冒充新 ABI 注入。完整 log 配置、API/REST/auth/tenant/component/permission/Upload、Job/Test、external Adapter 与默认 CLI/正式发行仍在后续闭环，本批不重新设计这些已约定合同。父任务保持 `in_progress`，没有 commit 或归档。

## 2026-10-01 LLVM API/REST 与可信请求上下文（本切片已完成）

用户在上一切片验收后批准继续本阶段。范围为已有 API/REST、站点目录映射、可信身份、自动权限、database-only 租户/组件状态、Cookie/Header/Upload 与 log 配置的 LLVM 应用接线；不改语法、权限 key 或 CMS 业务，不提前切换默认 CLI，也不加入 Job/Test/external/正式 packs。

最小行为差距：原 native 已生成完整 HTTP 路由与认证/授权链，LLVM 只生成 CMD root 并拒绝这些元数据。compiler 需统一所有 API/CMD/REST/verify/字段绑定执行根的 reachability、typed input/output/failure layouts；runtime 需让已存在的 config/auth/store/tenant/API/log owner 服务 invocation-owned Session，而不是重绑 OnceLock 或实现第二套 HTTP/RBAC/数据库。

分工：`llvm_api_core` 只改 core `llvm/**` 与确需复用的 native 纯 helper；`llvm_api_runtime` 只改 runtime 及 bridge ffi/header/features；主线程拥有根 tests、文档、所有串行 Cargo/实际验收。代理按项目 `.codex/agents/trellis-implement.toml` 的可用 `gpt-6-astra/high` 配置运行，因本环境内建角色的 `gpt-6.1-sol` 无法启动，显式上下文加载作为 native injection 的 fallback。双方不回滚彼此或既有修改。

实现约束：静态 route descriptor + 具体输入 decoder + compiled typed wrapper；runtime 只复用既有 request/auth/tenant scope、dispatch、envelope/metadata，App 无动态业务 Value。verify 使用固定 Claims/Identity pack/unpack，business failure 按精确已编译身份映射。REST 复用原 Model SQL/codecs；POST/PUT/DELETE 仍输入解码后开始事务，输出编码成功后提交。每 root 只加载一次 Settings；身份/租户/响应 metadata 的并发传播按原 caller 合同，不能把事务 detached 规则泛化到全部隐式 context。部分启动失败和最终清理保留原 fault，根任务与资源在同一 runtime 排空。

主要风险为：route-only/mixed root 选择、REST 字段绑定漏执行根、global settings/tenant OnceLock 串 owner、请求间身份或响应 metadata 串用、权限目录/多角色/跨站及禁用组件绕过、跨租户 fallback、Upload affine owner 取消泄漏，以及 shutdown 时先销毁 runtime。首轮先以现有 native 合同做 oracle，之后静态检查与本机 LLVM 实际对象/owned loopback 验收；服务或数据库启动在执行前说明，不使用部署服务。

资源准备：改动前已备份 `crates/`、`library/`、Cargo manifest/lock 到 `/tmp/dever-llvm-api-backup-AC4QeV`。磁盘约 430 MiB 不足后续构建，已核对无 Cargo/rustc 进程后仅删除 `target/debug/deps/` 内 9 个旧 bridge 静态 archive（旧 profile/hash）；保留最后两版及 `target/native-runtime-abi/debug/libdever_backend_bridge.a`，后者摘要 `994ed15beb13da2856723ce73cc988bd688e931cd7857dfae8baca7111652703` 未改。可重建缓存回收约 0.9 GiB，剩余约 1.3 GiB；未删除源码、数据库、日志、SDK 或配置。

本批实现与复用边界：

- `llvm/api.rs`、`api_context.rs`、`rest.rs` 和 `application_commands.rs` 接通 HTTP/CMD 混合根、REST 字段来源与 owner 过滤、Claims/Identity、Cookie/Header/Upload 及管理命令。仍由 checked HIR 生成具体输入、输出与业务失败布局，API/CMD 不经过 Rust fallback，也不引入动态业务 Value。SQL assignments、tenant fingerprint 与 wire query kind 从既有 owner 共享，原 Rust 后端继续使用同一合同。
- runtime `application::Session` 统一持有本次配置、生命周期、平台库、tenant manager 和迁移池；Settings 每根加载一次，log-only CMD 也消费配置并在返回前 flush。根任务排空后在同一 runtime 关闭资源；部分迁移与租户池先登记 owner、验证成功才 ready，取消不遗失 pool。
- `runtime-api` profile 与 canonical C ABI 接通真实 request/auth/store/tenant/upload owner。public 接口允许匿名并保留原 Bearer 校验规则；受保护接口按原配置选择站点/provider，执行可信 verify→租户/组件检查→精确角色权限→输入及业务。权限键仍为 component.domain.site.action，REST 保留 read/create/replace/delete；缺失认证或权限元数据在 lowerer/ABI 拒绝。禁用组件对 Tenant Owner 同样生效，不回落控制库。
- POST/PUT/DELETE 保持输入解码→事务→App/Model→输出编码→commit→响应 metadata。public/private 错误映射、隐藏 500、跨站/过期会话、多角色撤销、租户物理隔离均沿用原合同。配置只在 `config/setting.json`，无环境变量入口。

实际发现并修复的根因：

- 私有 HTTP 首轮 SIGSEGV 来自调试构建的嵌套 future poll 栈过大，发生在 verify 已成功之后。只在 compiled handler 的共同边界 `Box::pin(self.call(...))`，保留原 auth/tenant/cancel scope；没有扩大线程栈、到处装箱或重写 Identity ABI。
- borrowed receiver clone 原先保存了新句柄却返回原 SSA 值。现在从真正 clone guard 取值后再释放来源，REST search 复用同一路径；新增 Channel 记录跨 Task 返回的实际回归，覆盖 clone 与原 owner 销毁顺序。
- REST DELETE 的 `null` 响应原先由两个 guard 同时持有。改成只交给响应 wrapper 的唯一 owner，跨 commit 保留并正确转交。
- 同时纠正测试 oracle：嵌套输入仍需 payload 对象、owner 不可见行是 404、禁用角色不会由 save 重新启用、max_page_size=4 时必须显式 size、PostgreSQL TLS 使用 disabled 且必须配置 default。没有为测试放宽语言或数据库合同。
- 原 Rust `rest_model` 的三个编译 fixture 仍对无 settings 的 HIR 生成应用，权限注册为空却调用组件辅助函数，导致 E0425；相关 product 逻辑与本批备份一致。现复用一个本地 fixture，向实际编译阶段补齐 config/setting.json、站点/provider 和名义身份，通过 `check_with_settings` 绑定。原 SQL/owner/字段来源断言保留；没有禁用组件授权或补匿名 fallback。

验收证据（不把重复复验累计为不同用例）：

- `target/closure-llvm-api-final-native.log`：API 根 **14/14**，其中 6 项 Checker/六目标 objects/fixture 校验，8 项实际原生执行；HTTP、私有 REST、禁用组件、上传、混合 CMD、无认证 metadata、启动失败、log-only CMD 均检查 warm 1 + 64 次资源平衡。六目标 objects 不代表六 OS 实际运行。
- `target/closure-llvm-api-postgres-native.log`：真实 PostgreSQL **1/1 组**，复用同一 private REST 场景，执行 3 次根；隔离的控制库与两个租户物理库覆盖可信身份、多角色/跨站权限、租户隔离、创建/更新/删除、字段绑定、owner 行过滤、并发身份、Cookie 与失败回滚。该验收在最后无行为变化的 Clippy 修正前执行，未声称修正后再次启动 PG。
- `target/closure-llvm-api-upload-final.log`：最后 runtime-only archive 重建后，包含新增上传中途断开的用例 **1/1**，65 次根均确认文件实际开始写入，再断开连接并等待临时文件清零；正常存储与解码失败同时覆盖。
- 原应用入口 **19/19**（`target/closure-llvm-api-owner-regressions.log`），最终异步 **22/22**、受管值 **10/10**（`target/closure-llvm-api-owner-regressions-final.log`）。首轮后 9 个异步用例在启动 cc/ELF 时遭遇 ENOENT，未进入业务断言；/bin/bash 同期也暂时无法启动。只读确认工具恢复后，22 项全量定向重跑通过，未更改全局工具或掩盖失败记录。
- 原配置/请求上下文/鉴权/权限/声明 **36/36**，日志 `target/closure-llvm-api-runtime-regressions-final.log`；该日志保留三个旧 REST fixture 的失败。修正 fixture 后，REST **12/12**（含三条实际 Rust AOT 编译）、entry cleanup **3/3**、tenant database **3/3** 均通过，日志 `target/closure-llvm-api-runtime-rest-final.log`。共 54 项定向结果包含各 target 自带的 PostgreSQL fixture 纯校验，不代表这些 target 启动了 PG。
- 最终双 driver/API archive 下，原 LLVM Model bulk/upsert 与单连接 RowStream 取消 **2/2**、canonical C header 实际链接运行 **1/1** 通过，日志 `target/closure-llvm-api-database-abi-final.log`。验证共享 SQL 及 API profile 没有破坏先前 Model 根和基础 ABI。
- 最终 archive 重建通过：`target/closure-llvm-api-archive-final.log`；runtime-api base/SQLite/PostgreSQL/双 driver 的四 profile check 见 `target/closure-llvm-api-profiles.log`。本批文件 rustfmt、C11 `-Wall -Wextra -Werror` 与相对源码备份的 whitespace 检查通过，见 `target/closure-llvm-api-{rustfmt,c-header}-final.log`、`target/closure-llvm-api-diff-check.log`；历史 native 源码整体格式差异未全库重排。
- 最终 core/runtime/CLI/bridge libs、三个 CLI bins、本批 API/async/REST tests 严格 Clippy `-D warnings` 通过，包含原 runtime external feature 的编译检查，见 `target/closure-llvm-api-clippy-final.log`。旧 REST diagnostic 的 `.err().expect()` 改为等价 `expect_err()` 后，9 项非 AOT 合同再验通过（`target/closure-llvm-api-rest-diagnostics-final.log`）；三个 AOT 不为此机械测试写法重复构建。新 fixture 块 rustfmt check 通过，保留文件其余历史格式。

资源清理：真实 PostgreSQL 验收结束后核对随机库已由共享 fixture 清理、无其他连接，只删除本批创建的 `dever_llvm_llvm_api` 基础库，保留此前 `dever_llvm_llvm_database`；停止自有 loopback PostgreSQL，资产回到 `/root/.local/share/Trash/files/dever-llvm-pg-ZQQmWO`，服务日志在 `target/closure-llvm-api-postgres-server.log`。临时仓库 `config/setting.json` 已移除，恢复原先不存在；诊断用失败 fixture、临时 signal/backtrace/debug 标记已移除，原 C allocation ledger 恢复。源码备份保留。磁盘清理只针对已核实的旧 target archive，可重新构建；未删除源码、业务数据或 SDK。

最终复核没有本切片未解决的功能失败。README/LANGUAGE/IMPLEMENTATION 和 compiler/database/toolchain/directory/logging specs 已同步；共享 clone 值、DELETE 单 owner、请求 Future 栈边界、pool 取消归属及配置式 REST fixture 的规则已记录。没有运行全量 workspace、CMS 服务、性能/RSS/64–128 MiB 或其他 OS；新 LLVM commit/Session-close 人为故障注入、完整协议组合和容量仍属于最终验收矩阵，不由重复入口分配平衡代替。

父任务继续 `in_progress`，不提交、不归档。下一步按原合同完成 LLVM Job/Test 执行根及持久任务的身份重验、取消/重试/事务资源清理，再接 external Adapter、默认 CLI 和正式发行；不重设计语法或缩减既定外部生态/平台范围。

## 2026-10-01 LLVM Job/Test 执行根（本切片已完成）

用户在 API/REST 切片完成后明确继续。当前边界是把现有持久 Job 与应用 Test 合同接到 LLVM；语法、默认 CLI、external Adapter 和发行工作保持各自原定阶段。复用 checked Job/Payload/Test metadata、case-local Port fake、队列 store/worker、Clock、可信身份和 application Session，不另建任务队列、测试解释器或全局可重绑状态。

编译器接通入队、调度、typed Job dispatch、User 每次执行的身份/精确权限重验、System 编译器入口、Job 数据库初始化/租户迁移及 api/worker/all 组合；Test 接通 filename entry、断言、每 case fake/数据库/虚拟时钟与资源清理。所有业务值保持具体类型，序列化仅用于既有持久 payload 和 ABI 边界；失败、取消、重试和 lease fencing 由现有 owner 承担。

分工：core implement 负责 `crates/dever-core/src/llvm*` 与必要纯 helper 共享；runtime implement 负责 runtime、bridge ffi/header/features。两者先协调共同 ABI，再分别修改；主线程负责根 tests、文档和串行 Cargo/实际运行，第三方独立复核后再收口。不得回滚彼此或既有用户改动。没有新的产品决策或任务批准等待。

改动前源码备份：`/tmp/dever-llvm-job-test-backup-spJueT`（crates、library、Cargo manifest/lock）。磁盘起始约 381 MiB，先核对无编译进程后清理必要的旧可重建产物；不清业务数据库、源码或私有工具。

验收：最小静态/六目标 object → 实际 archive + 隔离文件/SQLite → 持久 Job、Test fake/虚拟时钟、重复根/取消/错误清理 → 受影响旧 API/Model/task/Job/Test 合同。真实 PG/自有 worker/HTTP 的运行前说明；不访问部署服务，不用环境变量配置，不默认全量/性能测试。

### 实现与根因修复

- LLVM `jobs.rs` 接入具体类型 payload、入队/定时、handler 事务、Job-only 与 API/worker/all、租户队列迁移及每次执行的 User 身份/精确权限/组件重验。Session 持有本次 bindings/Clock，复用 store/worker/cron/lease fencing；同步 handler 复用 bounded blocking，异步 handler 复用既有协程。
- `llvm::emit_test_suite` 复用 case-local `test_program`，单 IR/可执行文件按稳定 index 派发；导出 indexed fault size/render/release。只给已声明的 LLVM 内部符号加 case 前缀，不改字符串和 runtime ABI；不同 fake 的效果、失败类型、数据库与时钟互不串用。普通测试不启动真实 Lib 或服务；无数据库 case 不创建无关 Model。
- 修复服务配置无条件校验：只有 `service_start` 才检查 Worker/HTTP 启动策略，普通入队 CMD/管理命令无需这些配置。修复无 tenant 配置的普通 Job 被要求 tenant ID：两后端共享 `job::require_components`，由实际部署配置决定是否隔离。
- 实际 API User 入队首次报 500，根因是 LLVM 复用的 `auth::authorize_permission` 成功后未绑定权限，而原 native 已有 `job::bind_permission`。现在授权成功后由该 runtime owner 绑定精确 key，native 也委托同一实现；拒绝/错误不绑定，不伪造 System 身份。
- 校正身份测试 oracle：空 subject、篡改物理归属会触发 store 的早期 fail-closed 错误，不是 dispatch-time blocked。现使用格式合法的失效 subject/session、已删除权限、与执行租户不符的 tenant claim 来验证重新授权，保持原存储合同。Job 不允许调用 HTTP/auth getter，fixture 的业务写入只读取 typed payload。
- 更新原 LLVM 应用测试中过期的“Job 必须拒绝”断言，使用新 Job-only/CMD/服务回归证明支持；沿用原 C allocation ledger（warmup + 64 次），仅修正零次重复时的无符号循环警告。新增 case driver 和 SQLite 检查均在根 `test/`，复用原链接/子进程工具。

### 最终证据与边界

- 新增 Job/Test target **9/9**：2 个静态/六目标 object 项，7 个显式实际 archive 执行组。覆盖 4 个纯断言/业务失败 case、7 个 Job fake/虚拟时钟/重试/cron/事务 case、2 个同步 blocking/异步 fake case，以及入队 CMD、启动前策略拒绝、schema/payload/policy 阻断、超时子任务排空和回滚。纯用例与重复入口沿用 warmup + 64 次分配平衡，未降低 oracle。日志 `target/closure-llvm-job-suite-static.log`、`target/closure-llvm-job-test-native-final.log`；最终纯 suite 加入业务失败和字符串不被 namespace 改写的断言后，六目标复验见 `target/closure-llvm-job-api-suite-final.log`。
- 新增 API User Job **2/2**：六目标 object（`target/closure-llvm-job-api-static.log`）及真实自有 loopback/两租户 SQLite。实际验证 api 入队、all 执行、worker-only、精确权限引用、撤权/会话失效/权限删除/账号失效/租户不匹配的 `blocked + identity_rejected + attempt=1`、组件禁用的 `component_disabled`，正常业务记录只落入各自租户库。最后与既有私有 REST（65 次根）共同复验 **2/2**，见 `target/closure-llvm-job-api-suite-final.log`。
- 原后端 `durable_jobs` 选定 **4/4**：case-local SQLite/Clock/fake、撤权后禁止业务派发、SQLite durable state machine、租户业务/队列同库与仅恢复 tenant scope。日志 `target/closure-llvm-job-original-regression.log`。
- 受影响 LLVM 应用/异步/受管值 **19 + 22 + 10 全通过**，日志 `target/closure-llvm-job-shared-regression.log`。基础 ABI **4/4**，canonical C header 与实际 archive 链接执行 **1/1**，日志 `target/closure-llvm-job-abi-final.log`。重复执行不重复累计新覆盖。
- 最终 runtime archive 重建通过（`target/closure-llvm-job-permission-archive.log`）；runtime-api base/SQLite/PostgreSQL/双 driver 四 profile offline/locked lib check 全通过（`target/closure-llvm-job-profiles.log`）。core/runtime/CLI libs/bins 与 bridge/lib/本批 tests 严格 Clippy `--no-deps -D warnings` 通过（`target/closure-llvm-job-clippy-product.log`、`target/closure-llvm-job-clippy-bridge.log`）。修改范围私有 rustfmt、C11 `-Wall -Wextra -Werror`、相对源码备份的 whitespace 检查通过（`target/closure-llvm-job-rustfmt.log`、`target/closure-llvm-job-c-driver.log`、`target/closure-llvm-job-diff-check.log`）；没有全局安装工具或重排历史 native 源文件。
- 独立 review 复核了 typed ABI、资源清理、原 backend 鉴权等价性与两次 shared-owner 修复，没有剩余 P1/P2。早期失败日志保留，最终通过证据如上；测试子进程和临时 SQLite 由 fixture 清理，仓库未新增 `config/setting.json`，源码备份保留。README/LANGUAGE/IMPLEMENTATION、相关 specs 和日志已同步。

本轮未运行真实 PostgreSQL Job、全量 workspace/CMS、性能/RSS/64–128 MiB 压测或其他 OS 实际运行；四 profile 和六目标 object 不代表这些已验收。默认 CLI 仍使用 Rust 引导后端。下一切片接现有 external Adapter/Worker 的 LLVM 执行链，再完成默认 CLI、正式 packs/全平台发行与最终容量门。父任务保持 `in_progress`，不提交、不归档。

## 2026-10-01 LLVM external Adapter/Worker 与内嵌资源（本切片已完成）

- 本轮从现有 Worker 协议、生态解析和打包实现接入 LLVM；缺口是 LLVM 当前拒绝 external 函数，尚无 typed wire 调用及内嵌资源入口。备份：`/tmp/dever-llvm-external-backup-kI9I74`。
- Core 负责 checked Adapter 候选、Setting 校验后启动、typed 输入/输出/声明业务错误、资源元数据与应用入口；runtime/bridge 复用现有 component supervisor、external 安全提取和 Scope，补 canonical ABI。原 Rust backend 的活跃调用者同步迁移，不能另写协议、解析器或进程管理器。
- Worker 与资源归属于一次应用调用，重复进入不能复用已关闭实例或泄漏元数据。启动失败、取消、超时、未声明错误及清理失败保留原来源/故障语义；普通 Test 仍只走 fake。共享资源 hash/dedupe 算法只有一个 owner。
- 验证覆盖六目标 IR/object、真实自有 fixture Worker 成功/业务错误/畸形响应/启动失败、连续 root 清理、独立可执行程序内嵌 Worker 与受管 launch 资源，并复验原 Worker、资源及受影响 LLVM 入口。主 agent 串行运行定向 Cargo/链接测试；实现 agent 不执行 Cargo，不清理缓存。当前磁盘约 733 MiB，构建前持续核对容量。
- 本轮不切换默认 CLI 后端，不重做 pip/npm/go resolver，不访问生产服务、不下载语言环境、不增加配置环境变量；最终发行/容量门仍在父任务内。

### 实现与复用

- 新增 `llvm/external.rs` 和 `llvm::emit_application_with_resources`：复用 checked Port/Adapter、具名 wire codec、声明错误身份、原 native 摘要/资源去重和 runtime 安全提取。选中的 Setting 校验成功后才启动 Worker；缺失字段（包括可空字段）、额外字段、错类型与未声明错误保持来源位置并作为运行故障，不能伪装成可恢复业务错误。
- `runtime-external` 独立于编译器 SDK。canonical ABI 明确元数据借用、输入复制、Reply 转移和资源清理；无新的协议、生态解析器、进程管理器或宿主环境查找。
- 将进程级 Worker 注册表/资源缓存迁入 invocation-owned `component::Session`；显式传播到 Task、blocking 与 API。supervisor 属于根 Scope，短命子任务结束不关闭共享 Worker；启动前注册 owner，取消启动或 shutdown 仍由入口继续收尾。根先关闭 Worker，再排空子任务及应用/数据库 owner；保留原故障并附加清理原因。
- 普通 Test 沿用 case fake 图，不启动真实 Lib；原 Rust backend 同步使用 owned Definition 和同一 Session。源码备份保留，未提交或归档父任务。

### 根因修复

- 共享 C 分配账本补计 `realpath(NULL)`：glibc 内部 malloc 绕过链接器包装，Rust 随后的 free 会被计数，原计数因此出现 -2/-1。独立探针验证修正前 -1、修正后 0（`target/closure-llvm-external-ledger-probe.log`）；未放宽 warmup + 64 根的精确余额断言。临时探针与诊断源码已删除。
- 原 Worker 生命周期测试暴露新嵌套 Future 的调试栈溢出。没有挂起点的 `task::run_in` 改为同步返回 Task，`component::scope_resources` 直接返回 task-local scope Future，去掉重复状态机；不增加线程栈或 Box，不吞错。最终原测试和共享异步回归均通过。
- 修正新测试中 CMD 绑定多个/零 App 输出的非法 fixture：Port 仍测多/零输出，App 按既有规则返回单个记录/Bool。未修改语言合同。内嵌资源验收保留每根 SHA 校验，仅将外层进程限时调整到能覆盖 65 根的 120 秒；测试墙钟包含编译/链接/反复校验，不作为性能数据。

### 最终证据与边界

- 新 LLVM external 共 8 项均已有通过证据。四种 exec/pip/npm/go 内嵌 launch 组在 `target/closure-llvm-external-verified.log` 通过：只迁移可执行程序和 setting.json、删除构建目录、清空继承环境，各跑 65 根。该轮另外一项 CMD fixture 失败已修复，见 `target/closure-llvm-external-typed-final.log`。最后两个无挂起 helper 修正后重建 archive（`target/closure-llvm-external-archive-final.log`），其余 7 项全部复验通过（`target/closure-llvm-external-final.log`）；四生态内嵌大组未为等价 helper 修正重复运行。
- 最终原 Worker 生命周期/资源/入口清理 1+9+3 全通过，含真实启动/关闭取消检查点、短命 Task 调用和原 Rust backend 独立 exec 打包运行（`target/closure-llvm-external-original-final.log`）。
- 最终共享 application 19、async 22、ABI 4 全通过（`target/closure-llvm-external-shared.log`）；fake-only Test、HTTP 请求连续入口、canonical C 实际链接分别 1/1（`target/closure-llvm-external-{fake,http,c-link}-final.log`）。删除旧 external 拒绝断言后的 application 元数据测试 1/1（`target/closure-llvm-external-application-static-final.log`）。
- 六 runtime profile 的 offline/locked lib check 通过（`target/closure-llvm-external-profile-*.log`）；base ABI 保留备份中已存在的 `append_fault_cause` dead_code warning，不声称全 profile 无警告。修改范围 rustfmt、C11 strict syntax、相对备份 whitespace 检查通过（`target/closure-llvm-external-{rustfmt,header,diff-check}-final.log`）。
- core/runtime/CLI libs/bins、bridge lib/受影响 LLVM tests 与原 Worker/资源/清理 tests、fixture binary 的私有严格 Clippy `--no-deps -D warnings` 全通过（`target/closure-llvm-external-clippy-{product,bridge,tests}-final.log`）。未安装或替换全局工具，未将定向检查称为全仓库质量门。
- 本批原失败日志保留供追溯，以上述最后证据为准。受管 launch 使用自有合成 fixture，不能代替真实 Python/Node/Go SDK 的正式发行验收；六目标 object 也不等于六 OS/架构实际执行。本轮没有全量 workspace/CMS、真实 PostgreSQL、性能/RSS/64–128 MiB 或其他 OS 验收。
- 独立 trellis-check 最终确认无剩余 P1/P2；复核了资源账本与 Future 层数修正，结论与上述准确测试范围一致。测试子进程无残留，父任务和用户原有工作区变更保留。

下一切片接默认 CLI 的 LLVM 编译/链接与 runtime pack 选择，再完成正式 packs、真实生态/共享发行及最终容量门；父任务保持 `in_progress`。

## 2026-10-01 LLVM 私有默认 CLI 与 runtime pack 接入（本切片已完成）

- 差距：`deverc run/build/test` 仍读取 RUSTC 并生成 Rust；已有 LLVM 只导出 callable application/suite，还缺真实进程入口、pack 选择及编译/链接编排。继续现有父任务，不重新设计语言或 API。
- 改动 owner：core 的 LLVM 入口/故障呈现与 native artifact 临时目录/cache；CLI 的 run/build/test、runtime pack 验证/选择；已有 LLVM/LLD bridge 只补实际链接所需的封闭选项。复用 checked HIR、测试逐case隔离、现有资源pack与输出create-new规则，不另写协议/进程管理或文件清理实现。
- 默认应用命令不能调用 Rust/Cargo/cc 或通过环境变量/PATH选择后端；依赖显式准备且校验过的目标 runtime/CRT/library，缺失明确失败，不增加 Rust fallback。私有作者准备工具与产品执行严格区分；签名发行继续复用现有 release v1。
- 源码核对发现 deverd 只有 artifact IPC，没有受信任编译入口。按既有共享缓存安全合同，本切片完成私有 `deverc` 默认 LLVM 并保留其作者缓存；managed `dever-core` 编译命令明确报告缺少共享编译入口，不能把失败降级成本地/无缓存编译。该部分留在已排定的共享编译/发行切片，不新增隐式旁路或用户环境变量。
- 备份：`/tmp/dever-llvm-cli-backup-6mojiN`（crates/library/Cargo manifests）。磁盘低至153MiB，已删除逐个确认的旧 first-party rlib/LLVM test binary；所有源码、备份、设置、数据、SDK及当前runtime archive保留。主线程独占所有Cargo/链接验证，agent不运行构建或清缓存。
- 本切片验证实际离线 CLI check/run/build/test、错误/失败继续、pack缺失/篡改、输出防覆盖/资源清理、cache身份与必要旧调用者；包括plain/Markdown、fake和最小数据库/外部Worker路径。六目标metadata/object与当前Linux实际执行分开记录，不将自有pack fixture声称为正式签名多平台发行。
- 之后仍需正式packs、真实生态/共享编译发行、跨平台和最终性能容量；不在本次默认跑全量测试、CMS长链路或生产服务，不安装全局命令，不提交。

### 实现与根因修复

- `deverc` 私有 `compile.rs`/`compile/runtime.rs` 接通 checked HIR→LLVM→object→进程内 LLD。复用 `native::compile_artifact` 的临时目录、缓存完整性和 create-new 发布；旧 Rust 作者回归也委托同一 owner，不留产品 fallback。
- pack 使用编译器同级 `runtime/<platform>/manifest.json`，绑定 compiler version/ABI/target 与 base/sqlite/postgres/both。路径、大小、SHA 和 profile 引用均校验；缓存命中也复核选中输入。链接前复制并重验，仅接受显式 CRT object/普通静态 archive，拒绝 symlink、脚本、thin archive。缓存身份含编译器字节、完整 manifest/profile、IR 与资源。
- `llvm/process_entry.rs` 在 callable application/suite 外生成 C main，复用 typed 故障呈现、原 logger 和资源释放。suite 参数严格检查单个十进制 index，size/run/render/drop 共用同一 index。实际 closed-pipe 探针发现原入口缺 Rust 启动的 SIGPIPE 策略（退出 -13、stderr 空），现在仅进程 main 调用统一 ABI 初始化；callable 根保持宿主策略，实际回归要求 code=1 与源码错误。
- 新编译模块最初在共享 CLI lib，readelf 发现 launcher/daemon 因此误链接 LLVM。将 compile/pack 收回 binary 私有模块，最终 `dever`/`deverd` 不再有 LLVM/C++ DT_NEEDED，空环境真实入口回归通过。`deverc` 使用自身 `lib/` 的 LLVM；未安装或修改全局命令/服务。
- debug 大 archive 的 SHA 校验造成每次 CLI 数十秒等待。共享 `sha256_file` 复用已有 ring 流式 SHA-256，测试 pack 删除重复摘要实现；每次完整校验仍保留，既有签名发行测试用独立 sha2 oracle 复验。没有新增密码库、缓存信任豁免或性能达标声称。
- 首轮 CLI 5/6 通过，Test fixture 的损坏 JSON 在 Package 元数据读取时被拒绝。改用合法项目 JSON + 无法作为数据库打开的目录，验证测试执行只使用生成的临时 SQLite；缺失真实 Worker 仍由 fake 隔离。旧 RUSTC 失败注入用例已迁移为无 pack 的独占编译器，保留全部用例失败汇总和清理断言。

### 最终证据与边界

- 实际 CLI **7/7**：`target/closure-llvm-cli-execution-final.log`。覆盖无宿主编译器调用、Unicode/异步、typed 与运行故障/来源、build 防覆盖/搬迁、closed stdout、源码/manifest 缓存身份、坏缓存/命中后的 pack 篡改、Markdown CMD、SQLite/fake 逐 case 隔离、失败继续、非法 suite index，以及内嵌 exec Worker 在源码删除后独立运行。真实 Worker 只是协议 fixture，不代替 Python/npm/Go 生态发行。
- 进程入口 **3/3** 与既有 bridge **5/5**：`target/closure-llvm-cli-entry-link-final.log`，含六目标 object、原简单内核六目标 link、失败后恢复/并发输出保护。当前实际独立应用仅验收 Linux x86_64。
- canonical C header 与最终 runtime archive 实际链接 **1/1**：`target/closure-llvm-cli-c-abi-pack.log`，含新增进程参数/错误 ABI 与 SIGPIPE 初始化。最终 ABI version 改为复用同一个常量后 archive 再建、此项重验；其余实际 CLI 已在相同语义的 archive 上通过，没有重复计数。
- artifact cache 新用例 **2/2**，原 Rust cache 编译回归 **1/1**：`target/closure-llvm-cli-cache.log`、`target/closure-llvm-cli-bootstrap-cache.log`。旧 CLI 编译失败汇总 **1/1**；签名版本安装/回滚、空环境多项目 launcher、自有 daemon IPC 各 **1/1**：`target/closure-llvm-cli-{test-failure,release,launcher,daemon}-final.log`。本切片合计 23 项不同定向用例通过。
- CLI lib/bins/相关 tests/example、core lib、完整 bridge profile/相关 tests 和 cache test 严格 Clippy 均通过：`target/closure-llvm-cli-clippy-{product,core,bridge,cache}.log`。修改范围 rustfmt/C11/whitespace 检查通过；历史 native 文件未整库重排。base ABI 编译仍有此前已有的 `append_fault_cause` unused warning，不声称所有 profile 无警告。
- 私有作者 pack 已准备于 `target/debug/runtime/linux-x86_64`，复用 root `test/native-runtime-pack.rs`/共享 fixture，日志 `target/closure-llvm-cli-author-pack.log`；重复准备明确拒绝，原 manifest 摘要未变。四 profile 在此夹具中复用完整 archive，不是四套正式裁剪包。最终动态依赖见 `target/closure-llvm-cli-dynamic-dependencies.log`。两套 CMS 仅在空环境做 check，通过并保留原 W001 建议，日志 `target/closure-llvm-cli-cms-check.log`。
- 清理仅涉及逐个确认的旧可重建 target 产物，源码备份保留；临时 Worker/SQLite/daemon 由 fixture 回收，仓库未增加应用 setting.json。没有运行全量、完整 CMS、真实 PG、其他 OS、体积/性能/RSS/64–128 MiB 压测；没有提交、发布或安装全局工具。

父任务仍为 `in_progress`。下一步接可信 `deverd` 编译与正式签名 packs；managed `dever-core` 目前明确拒绝编译，因此此前 Package 的受管完整执行合同也需在该阶段复验，不能用私有旁路把它标为完成。真实生态高级能力、OS sandbox、跨平台发行及最终容量门仍按父计划实施。旧全 CMS mixed-suite fixture 的坏 JSON/历史用例计数未作为本切片通过证据，留在完整 CMS 验收中同步。

## 2026-10-02 可信共享编译与签名 pack 引用闭包（本切片完成）

- 原始差距：managed core 拒绝 run/build/test，deverd 只有不透明 artifact IPC，不能把上传字节当作可信编译结果。本轮复用现有前端、LLVM、签名发行和 CacheStore 补齐了可信编译链路。
- caller 保留 Package/Lib 准备、完整部署配置、API baseline 及最终程序运行；提交规范逻辑源码、无秘密编译绑定和资源字节。daemon 自行计算身份，只启动已验签 core 的编译 worker，不执行 App/Test/Lib Worker，不接受客户端 IR、cache key 或输出路径。
- 改动 owner：core source/check/model 及 runtime config 的无秘密编译投影；binary-private compile/worker 与 NativeProgram 接收产物；toolchain service/cache/release 的 IPC、生命周期、版本共享锁及签名 runtime 引用闭包。LLVM 动态依赖仍不得进入 launcher/daemon。
- 请求、响应、并发和编译时间有界；断连/取消/失败回收 child 与 staging。缓存命中仍验证签名资源，完整请求才可读编译产物，不能通过 artifact digest 读取编译缓存。复用现有 operation/read lease，安装/卸载持久独占锁与编译共享锁互斥。
- 主线程独占 Cargo/native 验证。采用自有临时 daemon/签名 fixture，恢复受管 Package 最小完整链路；不启动现有服务，不做全量/CMS压测、全局安装、正式签名发布或其他平台执行。
- 备份：`/tmp/dever-shared-compile-backup-udVyQC`（crates/library/test/Cargo manifests/父任务）。已确认无构建进程，清理旧可重建 `target/native-artifacts` 和 `target/native-runtime/inputs`，空闲由约417MiB恢复至1.1GiB；未删除源码、备份、SDK、当前LLVM archive/pack或数据库。
- 本切片签名 fixture 证明安装/引用绑定，不把四 profile 共用 debug archive 当作正式裁剪六平台资产，不把受限编译子进程当作完整 OS sandbox。

### 实现与边界

- `CompilationBindings` 只携带数据库 driver/选择名、Provider verify 方法及 site path/auth；URL、密码、JWT/Cookie 等部署配置不进 daemon。普通配置和远端编译复用同一验证/数据库选择 owner。Test 不携带部署绑定，继续由 caller 逐例检查并执行隔离 SQLite/fake。
- 规范请求限制为 64 MiB、4096/16 MiB 源码、4096/48 MiB 资源（单个32MiB），拒绝未知字段、重复/非规范编码、目录逃逸及伪摘要。严格 SourceMap/checker 在已验签核心里重建 HIR，缓存身份绑定完整请求及签名 manifest。
- daemon 同时最多2个编译，180秒总期限；客户端190秒总接收期限。私有 worker 限制 CPU/地址空间/文件大小/FD/core dump，不运行应用或依赖脚本。取消、断连、失败、超时均 wait/reap 并回收 staging；缓存与安装版本租约持续到传输结束。编译缓存独立限制2GiB/4096条/单产物64MiB，artifact 上传及摘要读取不能访问它。
- 每次请求包括命中缓存都复验签名、native manifest、全部 profile 的 CRT/archive 与 LLVM 库；安装/卸载用持久独占锁、编译用共享锁。worker 只接收自有0700目录中的0600请求，state/cache/socket祖先仍允许其他用户安全遍历。
- 共享 runtime pack 校验不引用 bridge，LLVM 只由私有 binary 编译模块调用。修正安装锁误用公开权限验证、worker 对公开祖先误要求0700，以及相对 `--root .` 下切换 cwd 后找不到核心程序的问题；核心路径在验签及持锁后转换为绝对路径。

### 定向验收

- `packages --ignored --skip installed_managed_package_worker_guard --test-threads=1`：4/4，355.62秒。包含签名安装的 Package check/test/run/build/独立运行、两个实际 UID 跨目录复用、错误源码与缓存恢复、未签名 CRT/篡改 pack 命中拒绝，以及实际180秒超时、取消/断连回收与版本锁互斥。日志 `target/closure-shared-compile-managed-final.log`。
- 最后将测试 daemon 改为 `--root .` 后，Package 完整执行及跨UID两项复验2/2，88.83秒（`closure-shared-compile-relative-root.log`）。Package Worker 锁定归属/本地碰撞父用例1/1，9秒（`closure-shared-compile-package-worker.log`），由它正确调用签名安装里的 child-only helper。
- 无秘密源码快照 1/1；`shared_toolchain` 16/16（原独立 artifact 跨 UID 项保持默认 ignored，本轮编译跨 UID 已显式执行）；私有 `llvm_cli` 含 ignored 7/7。日志分别为 `closure-shared-compile-snapshot.log`、`closure-shared-compile-toolchain.log`、`closure-shared-compile-private.log`。
- `api_declarations` 16/16、`application_config` 6/6，日志 `target/closure-shared-compile-bindings.log`。上述共51个不同定向用例，不把重复复验或嵌套 helper 计入总数。
- 双源码 CMS 仅清空环境 `check` 通过，既有W001建议保留；没有启动 CMS/数据库或压测。日志 `closure-shared-compile-cms-dever.log`、`closure-shared-compile-cms-md.log`。
- CLI lib/bins/相关三个测试 target 与 core/runtime lib 的 Clippy `-D warnings` 均通过；本次独立/已格式化 Rust 文件的 rustfmt check、备份对比 whitespace 检查通过。历史大文件未做无关全局格式化。证据 `closure-shared-compile-clippy.log`、`closure-shared-compile-core-clippy.log`、`closure-shared-compile-rustfmt.log`。
- `readelf -d` 确认 dever/deverd 不依赖 LLVM/libstdc++；deverc 保留固定 LLVM18依赖及 `$ORIGIN/lib`。证据 `target/closure-shared-compile-dynamic-dependencies.log`，不能只凭本机装有LLVM时能启动得出无依赖结论。
- 首轮失败记录保留：fixture 配置字段/保留命名空间断言修正、祖先权限修复、跨UID fixture外层目录需0755，以及 child-only ignored helper 不应从仓库根独立运行。最终通过日志覆盖对应问题，未放宽语言或权限合同。

父任务保持 `in_progress`。下一步制作正式、可重复、裁剪 profile 的 native/runtime/build packs；六目标正式资产、真实生态高级能力、OS sandbox、跨平台发行以及完整性能/容量门仍未完成。本轮未运行全量、真实PG、全CMS、低内存/性能或其他OS验收，未安装全局服务或发布真实签名版本。

## 2026-10-02 原生发行包制作（本切片完成）

- 实施前只有 `test/native-runtime-pack.rs` 的作者 fixture，四 profile 复用 debug archive；签名安装/运行时验证已存在，缺可重复的制作与签名入口。本轮补 Linux native release 的作者制作工具、四个真实优化 archive 和对应安装/LLVM链接验收。
- 复用 `toolchain/runtime_pack` 的 manifest/引用/格式/摘要校验、`ReleaseManifest` 与 ring Ed25519；作者工具仅从自己根下 `config/setting.json` 读取显式文件/摘要/签名key路径，输出全新发行目录。禁止私钥进入产物、硬链接导致源文件变化污染产物、隐式联网、环境变量/宿主工具查找或覆盖已有发行目录。
- 作者侧构建与应用 run/build 分离：共用一个 Cargo target 构建 base/sqlite/postgres/both，独立 feature 组合；发布材料记录实际 archive、编译版本/目标/选项和摘要。测试两次制作字节一致、错误输入无半成品、验签安装后实际编译/运行，并检查 profile 裁剪和应用产物体积。重复打包一致不冒充跨机器源码重建一致。
- 代码 owner：CLI `toolchain` 制作/签名模块及私有 SDK 作者入口，根 Cargo 的 runtime pack profile/作者构建配置，root test 的定向制作/安装验收。产品编译与签名安装协议保持不变；改动原始 fixture 时必须复用同一个制作 owner。
- 本轮不发布真实密钥签名版本、不安装全局服务；Python/Node/Go正式pack、OS sandbox、其他宿主实际运行与容量门仍按父任务另行验收。Linux本机构建不能充当六平台完成证据。
- 修改前备份 `/tmp/dever-native-release-backup-u48cQl`，包含 crates/sdk/test/Cargo manifests/父任务。主线程独占 Cargo、实际构建、清理和运行验证；子代理负责边界内实现及检查。

### 实现与实际产物

- 新增 `toolchain::packaging::create` 和私有 `sdk/native-release.rs` 作者命令；使用现有 v1 manifest/验签合同，无新依赖和 lock 漂移。配置拒绝未知字段/路径逃逸/符号链接/错误摘要，私钥必须 owner-private，拒绝把相同私钥字节作为 payload。先复制再复核摘要，固定清单顺序并签名，以 Linux `RENAME_NOREPLACE` 发布；失败只清理自有 staging，并发不能覆盖输出。
- 根 `runtime-pack` profile 保留链接符号，四次离线 locked 构建共用 `target/native-runtime-abi`。均启用 `runtime-api,runtime-external`，按 profile 分别启用 SQLite/PostgreSQL；未启用 `embedded`。作者工具与应用 run/build 分离，不引入产品环境变量、联网或宿主编译器查找。
- 实际保存于 `target/native-release-inputs/`：base 78,568,584 字节，SQLite 93,357,820，PostgreSQL 99,502,804，both 106,926,902；全部 SHA-256 见 `target/closure-native-release-acceptance.json`。`nm` 检查证实 SQLite/PostgreSQL 驱动只在对应 archive 中出现，未包含 LLVMContextCreate。这些是供链接的库体积，不是应用体积。
- 作者工具链为 rustc 1.98.0（88d9e12ae，内置 LLVM 22.1.8）/cargo 1.98.0（797e8a9bc）；Dever 编译核心使用独立 LLVM 18/LLD，Linux x86_64 输入使用本机 glibc 2.39/GCC 13 CRT。未证明其他 libc/宿主/目标可用或跨机器源码构建一致。
- 删除旧 `test/native-runtime-pack.rs` 作者入口（上述备份可恢复）。测试共享显式 SDK 输入及 daemon 生命周期，旧私有 pack fixture 保留作回归；没有复制第二套签名/manifest 校验。作者配置、制作与验收用法记录在 `sdk/native-release.md`。

### 定向验收

- 四个优化 archive 构建全部通过；日志 `target/closure-native-release-{base,sqlite,postgres,both}-build.log`，分别约 2m52s、4m47s、3m40s、3m35s。作者 executable 构建通过，日志 `closure-native-release-maker-build.log`。
- `cargo test --offline --locked -p dever-cli --test native_release`：默认 5/5，通过重复制作/签名/独立副本、坏输入及 staging 回收、私钥和 symlink 拒绝、已有目录保护和并发单次发布。真实资产用例默认 ignored；日志 `target/closure-native-release-tests.log`。
- 显式 `native_release acceptance::optimized_profiles_make_reproducible_signed_release_and_run_through_daemon -- --exact --ignored --nocapture`：1/1，97.42 秒。先库调用、再实际作者 CLI 制作，清单和签名字节一致；验签安装后四 profile 的 `check/run/build` 均通过，缓存恰好四条。关闭自有 daemon 并删除源码后，四个程序独立运行通过，SQLite/both 实际创建、计数和删除通过。日志 `target/closure-native-release-acceptance-final.log`，摘要/体积/边界见同前缀 JSON。
- 该最小 CMD 的程序体积：base 2,088,488 字节，SQLite 5,082,504，PostgreSQL 2,269,832，both 7,592,744。base/postgres 执行异步 sleep，sqlite/both 执行 CRUD，不能直接作为同一业务的四 profile 性能对照；PostgreSQL 配置指向未使用地址，未连接服务，不宣称真实 PG 验收或驱动全部代码都进入最终程序。
- 验收初轮 fixture 错把单输出 Bool 预期成对象，随后发现 `value/model.dever` 必须声明 `Value`；仅修正这两个测试合同，语言/输出规则未放宽。首次失败日志 `target/closure-native-release-acceptance.log` 保留；最终完整重跑通过。
- 回归：`llvm_cli` 默认 4/4（3 个历史显式用例保持 ignored），签名版本安装/切换/回滚 1/1；日志 `closure-native-release-llvm-regression.log`、`closure-native-release-signature-regression.log`。连同制作器 5 项及实际发行 1 项，本轮共 11 个不同定向用例通过，不重复计算四 profile 和重复运行。
- CLI lib/bins、作者 example 及 native_release/llvm_cli/packages/shared_toolchain test targets 的严格 Clippy `-D warnings` 通过，日志 `closure-native-release-clippy-final.log`。初轮暴露测试重复载入 temp 模块及新 Clippy 的定长切片建议，改为复用父级 TemporaryDirectory 和 `as_chunks`，不加 suppress；默认制作测试复验 5/5，日志 `closure-native-release-tests-final.log`。修改范围 rustfmt、备份对比 whitespace 通过，独立审核无确认阻塞。
- `readelf -d` 复核 dever/deverd/作者制作器均无 LLVM/libstdc++ 依赖；deverc 保留 LLVM18 与 `$ORIGIN/lib`。日志 `target/closure-native-release-dynamic-dependencies.log`。这不证明核心及 LLVM 的其他系统库都已随发行打包，干净机器验收仍未完成。
- 清理仅使用 Cargo clean 回收已确认的可重建旧 native cache/debug ABI/重复 runtime-pack bridge 输出；当前私有 debug archive 链接、四个优化输入、源码及所有备份保留。测试临时发行目录、密钥、daemon 与 SQLite 由 fixture 回收。收尾磁盘约 1.2 GiB 空闲。

父任务保持 `in_progress`。本切片证明 Linux x86_64 原生制作、安装和执行；签名密钥是测试临时身份，核心程序是当前开发构建，没有发布正式版本。Python/Node/Go packs、完整标准库/构建输入、OS sandbox、其他平台发行以及完整性能/容量门仍未完成。本轮未运行全量、真实 PG、CMS 服务、低内存/性能或其他 OS 验收，未安装全局服务或提交。

## 2026-10-02 外部生态发行包制作（Linux 定向验收完成）

- 现有 Registry/Worker 已消费签名 `runtime/<ecosystem>/<target>/manifest.json` 与 `runtime.pack`，缺制作 owner；本轮扩展现有原生作者制作器，按显式文件/摘要生成确定性压缩包及 registry 描述，将 Python/Node/Go 材料统一纳入发行签名。Worker 与作者共用 runtime manifest/闭包验证，不再各写一份字段合同。
- Python 必须携带标准库与扩展，Node 使用明确发行输入，Go 携带 compile/link/analyzer 与 stdlib importcfg。只在作者准备阶段获取/整理固定输入；产品 run/build 不联网、不读取环境变量或宿主工具。实际脱离源码/输入目录的执行用于证明迁移能力，临时签名不代表公开发行。
- 已确认 Node 解释器约 118 MiB，超过上一轮共享编译的 32 MiB 单资源、48 MiB 总资源及64 MiB输出限制；本轮在实际编译协议/缓存 owner 修正相互冲突的有界预算，保持不透明 artifact 与可信编译产物隔离、2 GiB 总缓存限制和生命周期校验。避免只让制作器通过、受管 run/build 仍不可用。
- owner：`toolchain/packaging` 与 Worker/runtime 描述共享校验、registry metadata；共享编译的资源/输出预算；根 test 的三生态签名制作及独立执行验收。语言语法、Provider、API/CMS 业务不变，extras/sdist/npm高级安装/Go sumdb 仍按父计划后续实现。
- 备份 `/tmp/dever-ecosystem-release-backup-7jv8nj` 包含 crates/sdk/test/Cargo manifests/父任务。实现/审核子代理遵循明确文件边界；主线程独占 Cargo、输入准备和实际验收。

### 已落地与验证过程

- 继续追踪后确认，旧 LLVM emitter 把每个资源字节编码为三个字符，122,889,056 字节的 Node 会生成约 351.6 MiB 的 IR，不能通过 8 MiB IR 边界。本轮复用 `native::resource_inputs` 摘要去重，把 payload 作为有界二进制附件交给私有 LLVM bridge，在 verify/优化之前填入已声明的常量；IR 仍绑定路径、摘要、长度和执行位，8 MiB IR 限制保留。runtime C ABI 未变，因此不重建上一轮四个优化 archive。
- 共享编译传输 384 MiB，资源最多 4096 项、单项128 MiB/总256 MiB，可信产物320 MiB；bridge保持对应附件边界。不透明 artifact 上传仍64 MiB，编译与 artifact 缓存分别2 GiB/4096条。canonical编码只排序引用，daemon及worker尽早释放已不需要的请求副本，保留原180/190秒编译与客户端期限。
- 固定作者输入：python-build-standalone 20260901 CPython3.12.14完整可搬移标准库发行、Node24.15.0官方Linux发行，以及先前保存的Go1.26.3构建fixture。Python/Node下载分别核对GitHub资产摘要和官方SHASUMS；确切SHA及路径保存在 `test/ecosystem-release/prepare.py`，生成材料位于 `target/ecosystem-release-inputs/prepared/`。Go不是独立验证的公开上游pack，临时签名不替代供应链来源证明。
- 默认 `native_release` 9/9、`external_workers` 6/6；bridge 7/7及LLVM external默认3/3，包括超旧限制的二进制附件实际链接与逐字节核对。共享编译输入/输出配额2/2，canonical无秘密快照1/1。日志前缀 `target/closure-ecosystem-release-`，实际三生态验收仍单独执行，默认ignored不算通过。
- 实际验收初轮因debug作者准备完整语言材料超过测试夹具190秒而终止，未进入应用编译；日志 `closure-ecosystem-release-acceptance-author-timeout.log`。作者制作步骤独立设为900秒，其他命令及产品编译期限保持不变。
- 新 fixture 曾遗漏 Port 必须声明 `fails Choice` 的合同，语法/合同检查先后拒绝了缺失body与普通空body；已复用现有Port测试的明确失败Choice声明，并把三生态源码预检查移至制作之前。没有放宽语言合同。失败证据为 `closure-ecosystem-release-acceptance-fixture.log` 与 `closure-ecosystem-release-acceptance-port-contract.log`。
- 调试版客户端准备完整Python资源另触发一次190秒fixture超时，保留 `closure-ecosystem-release-acceptance-preparation-timeout.log`。只优化压缩依赖后仍慢；对自有客户端3秒CPU采样（297样本）定位约七成开销在未优化sha2软件实现，非共享Worker死锁。诊断后仅终止已确认PID的自有客户端，daemon/项目由fixture回收；证据 `closure-ecosystem-release-preparation-perf.log` 及 `closure-ecosystem-release-acceptance-sha-diagnostic.log`。最终作者构建及显式验收统一使用Cargo命令级 `profile.dev.package.{miniz_oxide,flate2,sha2}.opt-level=3`，不改产品Settings、校验或期限。运行性能/内存仍需另行验收。
- 仅以 `cargo clean -p dever-tests` 回收确认可重建的旧测试产物约1.0 GiB；保留源码、备份、四个优化archive和本次发行输入。独立审核暂未确认其他阻塞；格式/whitespace及依赖锁不变检查通过。

### Go 缓存身份问题

- 根因属于跨层合同与覆盖缺口：Go编译器把每次随机stage目录写入调试/源位置，两个相同锁定输入产生不同Worker字节，进而改变Dever编译请求及缓存身份。完整三生态验收第一次跑通全部 `check/run/build` 后，明确得到4条缓存而非3条；日志 `closure-ecosystem-release-acceptance-go-cache.log`，未把它标为通过。
- 既有Go用例只构建一次并验证移除源码后可运行，不能证明重复构建一致。共享缓存没有误判：不同字节应得到不同身份，因此修正owner为Go `Builder::compile`，SDK/Adapter/依赖共用 `-trimpath` 删除stage前缀，保留逻辑文件名/行号；不按生态绕过哈希或放宽缓存断言。依据为Go官方compile文档的trimpath合同。
- 将现有真实Go fixture改为在两个新stage重复准备并比较全部资源身份，再执行无源码Worker；定向1/1通过（10.10秒），日志 `closure-ecosystem-release-go-determinism.log`。规范已固定该断言及三生态共享缓存恰好三条要求；最终完整验收已通过，不新增独立构建/缓存实现。

### 最终结果与未完成边界

- `closure-ecosystem-release-acceptance-final.log`：实际三生态验收1/1通过，573.56秒。签名制作/安装、runtime锁定、三个受管 `check/run/build`、共享缓存恰好三条均通过；停止自有daemon，删除整个机器安装与源码目录后，三程序在只有显式Linux OS ABI库及私有proc的chroot中执行成功，没有系统Python/Node/Go或包管理器。Python另核对全部导入路径在内嵌目录，并执行SQLite、SSL、压缩等标准库能力。
- 产物证据 `target/closure-ecosystem-release-acceptance.json`：pip/npm/go压缩pack分别41,778,610 / 43,996,315 / 54,494,714字节；测试程序分别94,754,600 / 125,792,424 / 6,960,616字节，完整SHA-256见JSON。Python/Node程序包含解释器与标准库，Go只内嵌编译后的Worker。这是三个探针程序的大小，不代表CMS大小或吞吐/RSS性能。
- 本轮共30个不同定向用例通过：bridge/LLVM资源10、默认制作/Worker合同15、共享编译配额2、canonical快照1、真实Go重复构建1、完整三生态1。最终默认复验日志 `closure-ecosystem-release-contracts-final.log`；历史ignored未运行项不计入通过。
- CLI/core/bridge严格Clippy均通过，日志 `closure-ecosystem-release-{clippy-final,core-clippy,bridge-clippy}.log`；修改范围rustfmt、备份对比whitespace、依赖清单/锁不变检查通过。独立审核复核Go统一入口与真实日志，未发现确认阻塞。
- 最终 `readelf` 日志 `closure-ecosystem-release-dynamic-dependencies-final.log`：launcher/daemon/maker没有LLVM或libstdc++依赖，deverc仍依赖LLVM18及现有系统库。自有进程、签名密钥、安装/项目目录与mount已随fixture回收；磁盘恢复约1.9 GiB，备份和输入保留。

父任务仍为 `in_progress`。本切片完成三生态pack制作与Linux无系统语言环境的执行链路；临时签名不等于公开发行，Go输入的正式来源闭环、完整核心动态依赖交付、其他平台资产/执行、sdist/extras/npm高级安装/Go sumdb、OS sandbox及最终性能/低内存门仍未完成。本轮未运行全量、真实PG、CMS服务、第三方包安装、低内存/吞吐或其他OS验收，未安装全局服务或提交。下一步按父任务继续真实生态高级解析/构建能力，之后处理发行平台与容量门。

## 2026-10-02 五项剩余收口（实施中）

用户已明确授权实施高级Lib、OS沙箱、发行/跨平台、最终组合与性能容量五项；沿用本任务，不创建重复任务。源码备份 `/tmp/dever-closure-remaining-backup-EFQlXQ`，保留此前备份和无关删除。主线程串行构建/测试，独立研究与实现遵循不重叠归属。

执行依赖：先补不执行第三方代码的精确extras/依赖解析与sumdb；并行建立可复用的Linux隔离启动边界；再接源码包/受控hooks及正式构建输入；之后统一发行验收和质量/性能矩阵。源码包/hook不能靠删掉拒绝分支完成，必须有锁定工具、受控网络/文件/进程及真实包验收。

- 研究 `research/ecosystem-closure.md` 明确每生态缺口和锁/Worker隔离合同。Python extras沿用规范化Lib请求身份，不把一个Worker选择的extras泄漏到另一个Worker。主线程负责Worker归档路径与重复wheel去重接线。
- Linux隔离设计复用显式签名打包的bubblewrap与safe Rust seccomp guard，共享namespace/PID生命周期、路径与固定syscall策略。保持workspace unsafe禁止，不在Tokio线程设置进程级限制，不调用宿主PATH工具，不缺资产裸跑。文件授权由setting.json明确授予，配置内容不发给Worker；未支持的平台必须明确拒绝。非特权用户、真实三生态和后代回收均需执行证据，方案不是完成证据。
- 最终CMS夹具基线失败于Package元数据读取前的非法JSON。改为合法JSON但不可打开的部署数据库目录，继续证明应用Test使用隔离SQLite；混合用例保留bootstrap，准确要求4项。复用根process owner为所有夹具命令增加180秒期限、文件捕获与空环境，避免输出管道阻塞。`closure-final-cms-project.log` 6/6通过（143.32秒），涵盖双源码check/test/run/build/移除源码后的独立健康入口；服务、PG与性能组合另行验收。
- Python extras已接解析、精确变体图、同包自激活/回溯、Worker隔离、单归档安装与大小统计。空extra被pep508规范化接受的问题已修；`closure-python-extras-final.log`中external_libs 32/32、`closure-python-extras-install.log`安装7/7与Package11/11通过，显式宿主/发行fixture未被计入。这是extras证据，不包含sdist/native wheel。
- 沙箱新增`dever-sandbox`共享namespace启动/ELF闭包验证/seccomp helper，发行maker及运行时所有exec/managed启动正在整合；Go工具也改走相同隔离owner，源码只读、build目录独立可写。`closure-sandbox-isolation.log`、`closure-sandbox-missing-library.log`、`closure-sandbox-descendants.log`分别验证文件/网络/进程权限与线程、缺动态库拒绝宿主回退、杀死namespace后无存活后代；`closure-sandbox-component.log`真实协议/生命周期1/1通过（2.28秒，沿用命令级SHA优化）。独立审核指出ioctl参数宽度、DNS输入和动态库回退问题，均已修代码；ioctl高32位实际回归、完整三生态/ABI/发行复验仍待补。
- `closure-sandbox-unprivileged.log`失败：宿主启用AppArmor非特权user namespace限制，临时签名资产的bwrap不能配置隔离loopback。保留fail-closed；未更改主机sysctl/profile，不能把root下通过说成非root验收通过。GPU/其他平台隔离仍待支持。
- Go sumdb已接固定官方key、h1、MVS元数据、签名树/收录/一致性与lock v3证据；首轮`closure-sumdb-first.log`6个sumdb测试通过，含官方66321505叶树的离线vector。独立审核的ZIP目录条目、无Go更新锚保留、并发更新锚回退与整锁预算四项正在修复；尚未宣布sumdb收口。新sandbox要求导致的两个exec fixture预期正在同步。

本轮后续复验与修正：

- sumdb四项审查修复和exec准备夹具已完成，`closure-sumdb-final.log` **45/45**通过（1项作者联网准备默认忽略）。真实官方`github.com/google/uuid@1.6.0`及sumdb证明已由显式作者入口获取、验证并存入`target/go-managed/dependency`，日志`closure-official-go-fixture-prepare.log`；这一步不等于实际Go构建/执行，后者正在接线。npm嵌套依赖/peer环境图继续实施，拟直接更新锁格式，不保留旧格式回退。
- 沙箱独立安全复核的五项已修并复核闭合：执行缓存写授权、目录替换竞争、native配置快照漏接、宿主loader preload、Go输出符号链接。共享mount使用`openat2`和`command-fds`安全传递FD，不新增项目unsafe。`closure-sandbox-kernel-final.log` **6/6**真实kernel通过（含ioctl截断、FD目录替换、读写/网络/进程/线程与后代终止）；`closure-sandbox-config.log` **1/1**；`closure-sandbox-runtime-final.log`协议/生命周期 **1/1**、资源 **8/8**（Rust bootstrap显式fixture1项未运行）；`closure-sandbox-clippy.log`严格lint通过。
- 当前ABI已重建。LLVM首次9项并发验收因磁盘不足出现5项链接失败，保留失败日志`closure-sandbox-llvm-external.log`；以`--test-threads=1`复验 **9/9通过**（774.66秒），日志`closure-sandbox-llvm-external-serial.log`。真实Python/Node在只读沙箱内完成标准库与线程操作，`closure-sandbox-python-node.log` **1/1通过**；runtime external/api/postgres严格lint通过`closure-sandbox-runtime-clippy-final.log`。这些不是三生态签名release与第三方包完整验收。
- 已按Cargo包范围清理可恢复编译缓存，保留源码、备份、输入、报告以及`target/native-runtime/runtime-sandbox.a`当前归档；没有清理其他项目。所有大型LLVM夹具后续串行执行，避免低磁盘下并发链接。
- 官方`google/uuid@1.6.0`已完成真实沙箱内Go编译两次、资源身份相同及移走适配源码后协议执行，`closure-sandbox-official-go-worker.log` **1/1通过**（13.52秒）。工具、依赖源码与importcfg只读，输出独立授权；编译与执行均使用共享隔离入口。
- npm安装图迁移到lock v4（不保留旧格式回退）。首轮`closure-npm-graph-tests.log` 50项通过、1项失败、1项作者准备忽略；失败是手工夹具漏填精确npm环境，已迁移。bundled父来源、同名依赖优先级、循环孤儿删除、安装膨胀预算四项均修复并经独立复核关闭。`closure-npm-graph-final.log` 解析 **56/56**、Worker安装 **7/7**、Package **11/11**通过，另13项显式环境fixture未在此组运行。npm hooks/native仍另行实施，不混计为依赖图完成。
- 当前沙箱四profile优化archive串行重建中；编译缓存再次仅按`dever-cli/dever-core/dever-runtime`包范围清理896.9MiB，日志`closure-core-dependency-cache-clean.log`。源码、备份、author SDK/生态输入与验收报告保留。
- 发行实查确认此前仅复制LLVM不足：Linux编译器仍依赖14个非GNU OS ABI动态库，且核心RUNPATH不会向传递依赖继承。正在将author单`llvm`字段改为`core_libraries`完整声明、核心继承RPATH及制作/签名native发行验证共用闭包检查；追加仅声明GNU OS ABI的自有chroot内真实编译验收，不把原有仅独立程序chroot成功替代核心编译证据。
- 六平台复核：Linux ARM64作者SDK multiarch路径硬编码已修源码，尚无ARM资产/宿主验收；macOS/Windows仍缺原生可执行链接与runtime协议、发行发布、跨用户认证服务及沙箱实现，不能只称“缺机器”。本轮不把六目标对象文件生成说成六平台产品交付。跨平台实际输入及运行宿主当前均不可用，Linux后续实施继续。

后续已完成的定向证据：

- Linux核心动态库闭包已完成：maker和native发行安装/解析共用校验，核心改用可继承RPATH，严格拒绝unsigned lib/hwcaps目录、缺传递库及宿主preload。`closure-core-contracts-final.log` **12/12**，`closure-core-loader-acceptance.log` 最小OS根真实check/build/移除源码运行 **1/1**（18.62秒）。不是正式公开签名或其他平台交付。
- wheel三项独立审查修复已闭合：候选阶段使用选定wheel的已验证METADATA，统一Python prefix/安装scheme/脚本，准确模拟RUNPATH和扩展祖先RPATH。`closure-wheel-contracts-reviewed.log` external_libs **63/63**、workers **7/7**、native_release **12/12**、packages **11/11**；18项显式fixture未计入。`closure-official-python-native-wheel-reviewed.log` 官方simplejson3.20.1真实扩展、同prefix资源和独立脚本导入、沙箱执行 **1/1**（16.55秒）。sdist/PEP517/npm生命周期仍在实现。
- 四个sandbox runtime-pack archive已构建，随后sandbox抽出了已有的preload检查供核心闭包复用；最终构建沙箱修改稳定后仍需刷新四个归档。`target/closure-current-compiler/provenance.json`保存当前私有测量编译器、四archive、系统SDK和14库的准确摘要；不是签名发行。
- 当前LLVM双CMS构建通过，Dever **8,323,880**字节、Markdown **8,326,120**字节。`cms-llvm-128-10-02`及`cms-llvm-64-10-02`各自双源码、顺序16篇真实登录/租户/CRUD/发布均通过且无OOM，RSS峰值28.20–28.33MiB、cgroup峰值23.48–23.91MiB、启动44.6–51.9ms。顺序发布约53.6–55.6篇/秒是完整登录及发布链基线，不是并发饱和QPS或p99。
- 发现runtime/http/live性能构建器仍调用旧Rust backend；已迁到公开LLVM项目/CMD入口并移除无其他调用方的native_fixture_builder。每组构建一个多CMD应用，报告明确共享产物和生产默认调度，拒绝旧manifest充当当前证据。5项运行器定向合同已通过，真实协议测量尚未完成。

最终矩阵继续：

- `profiles-llvm-10-02/profile-report.json`四profile同CMD实际大小base **2,083,880**、SQLite **4,139,976**、PostgreSQL **2,265,224**、both **4,238,568**字节；无数据库连接。`protocols-llvm-10-02`已通过公开LLVM构建runtime/http/live，各组共享多CMD产物，三个Rust参考由当前源码runtime-pack优化构建。
- `memory-llvm-64-10-02`五种空闲/挂起服务和`async-llvm-64-10-02`六种任务/Channel/取消路径完成，均无OOM。`http-llvm-{64,128}-10-02`共24组2秒HTTP/HTTPS测量无请求错误或OOM；目标5000req/s下实际约4889–4980req/s，p99约0.7–3.2ms，少量负载端未派发请求保留在dropped，不能称持续饱和上限。新server owner统一检查OOM，HTTP warmup/measurement复用请求错误与成功计数门；runner合同 **47/47**通过，`closure-performance-runner-contracts.log`。
- 当前CMS LLVM/PG验收 **1/1**（56.31秒），`closure-postgres-cms-configured.log`：Owner及非Owner多角色、跨站同名角色、原会话即时撤权、两个物理租户库与同slug数据隔离通过。原始首次fixture失败是根Settings缺default，不是产品失败；仅补临时合法default后重跑。专属PG16实例只监听owned 51183端口，结束已删除本轮两基准库、停止并移回原fixture位置，root config恢复原先不存在。测试内部创建的control/tenant库均由共享fixture清理，无其他库被删除。
- LLVM API首轮17项中16通过，上传取消fixture失败：4KiB正文在头部独立TCP frame时不足以推动16KiB分块，等待写盘不成立。改夹具发送32KiB（声明64KiB，仍不完整），保留真实断连、temp文件与affine owner零残留断言；`closure-upload-cancellation-llvm-final.log` exact **1/1**（65轮生命周期），无运行时限制改动。组合证据覆盖17项，含真实PG权限/事务与排队Job重新鉴权；另9项Job suite单独继续，未因前target失败误记通过。
- 共享build Stage与signed rootfs边界迁移后，官方Go UUID双构建/移源执行 **1/1**（13.58秒），`closure-shared-build-go-exact.log`。`closure-shared-build-go-regression.log`最初过滤名错误只运行0项，不计通过。独立只读复核固定RO/usr、network=false、普通Worker权限不变、kill/wait与工具依赖闭包，无确认阻塞；实际PEP517/npm工具执行仍待验证。
- 作者工具叶准备 `test/ecosystem-release/build_inputs.py` 复用已验SHA的Python/Node提取owner，固定GNU工具/sysroot与同版本headers/npm；明确记录两份linker script的受控前缀转换，来源仅用于私有作者fixture，未冒充官方发行。源码包/receipt锁v5正在接线。
- 又清理已保存archive对应的bridge optimized缓存622MiB、已完CLI验证缓存548.4MiB，并删除四profile manifest已不引用的旧191,253,848字节debug/runtime.a（SHA70022249…，可重建）；当前四archive、私有compiler、SDK、报告与备份均保留。
- Job独立LLVM定向 **9/9**（185.22秒），`closure-job-cancellation-llvm-final.log`：fake clock/cron、重试、事务、入参策略、CMD enqueue及超时后子任务回收与回滚通过，没有因之前API target失败漏跑却记通过。PEP517作者签名工具pack准备 **1/1**（26.50秒），`closure-pep517-author-tools-fixed.log`；实际sdist/native构建正在验收。
- CMS增加显式可选并发读取负载，沿用真实登录cookie、查询与资源采样owner，每次校验完整列表，错误立即停止同组客户端并关闭连接；合同 **11/11**，`closure-cms-load-contracts.log`。`cms-read-llvm-{64,128}-10-02/report.json`双源码共四case，8并发、每组10秒，约 **1210.6–1322.2 QPS**、p99 **11.48–13.59ms**、峰值RSS **30.81–31.04MiB**，请求错误和OOM均零。负载端为Python闭环客户端，服务未绑单核，不能称生产容量上限或单核成绩。
- HTTP/2单核校准 `http2-scan-llvm-64-10-02`：h2c/TLS、4物理连接×16流、3秒/档、1000/5000/15000目标。六case零请求错误/OOM且断连FD差均0；15000目标下h2c完整成功44741/45000，TLS44349/45000，后者不足99%派发成功门，因此不能直接用15k宣称持续容量，后续稳态降至10k。
- HTTP/2正式一分钟稳态 `http2-steady-llvm-{64,128}-10-02`：单核、4连接×16流、目标10k，四case实际 **9971.6–9979.2 QPS**、成功/计划 **99.718–99.794%**、p99 **2.912–3.424ms**、峰值RSS **4.88–5.72MiB**，错误/OOM/断连FD差全部0，达到99%门。源码/编译器/产物摘要沿用当前protocols manifest；不是历史20k报告或跨主机生产容量。
- 100轮恢复首次失败于runner每轮复制同一peer导致磁盘耗尽，保留`closure-http2-llvm-recovery.log`和未完成目录，不计通过。`stage_peer`改为同文件系统硬链接（EXDEV才独占复制），配置/日志仍独立；**48/48**合同通过。仅把确认同字节的本轮重复peer替换为共享硬链接，释放约0.8GiB，日志与报告未删；另清理完成后的Cargo缓存261.5+531.9+128.4MiB，已留存四archive/参考程序/SDK。PEP复验的磁盘失败与后续copytree/xattr真实失败均留日志，不能混记为通过。
- HTTP/2恢复v2两种传输各100轮 **通过**，19,887完整成功、200次FD回落为0、错误/OOM为0；`closure-http2-llvm-recovery-shared.log`。TCP/WS/SSE单核64/128MiB、32/128连接、各10轮共 **12case通过**，120次断开FD差均0、完整消息及关闭计数一致、错误/OOM为0，`closure-live-llvm-capacity.log`。未运行小时级soak或外部网络故障。
- PEP隔离hook回归 **1/1通过**（`closure-pep517-metadata-execution.log`），官方simplejson仍因原生扩展未生成失败。成功hook现在沿现有Inputs诊断边界输出有界stdout/stderr，定位到作者linker脚本将已有`/usr/lib`二次变成`/usr/usr/lib`；已修token改写且 **2/2**回归通过，已更新两生态工具叶摘要，等待重制pack及真实原生验收。只读xattr与资产执行位共享owner经独立定向复核关闭，实际C syscall probe已重建，kernel复验待运行。
- 修正工具pack后，官方simplejson3.20.1源码经真实隔离PEP517/GCC构建出 `_speedups.cpython-312-x86_64-linux-gnu.so`，resolver/doctor与同session重放 **1/1通过**（23.55秒），`closure-pep517-native-corrected.log`。作者pack重制1/1通过27.28秒；构建C扩展不是Worker运行证明，后者及npm receipt字段最终冻结后复验仍待完成。Node runtime作者fixture已复用同一已验叶→确定性归档owner生成，`closure-npm-runtime-fixture.log`，未重取上游包或安装系统Node。
- lock v5新字段接线后，默认四target分别 **68/7/12/11通过**（`closure-source-build-contracts.log`）；显式官方源码解析 **1/1**（25秒）及源码构建产物实际沙箱Worker **1/1**（16.58秒）通过，见`closure-pep517-{lock,worker}-final.log`。只读xattr实际kernel probe **1/1**及资产路径/执行位合同 **2/2**通过，见`closure-sandbox-{metadata-kernel,asset-modes}.log`。
- 共享编译跨真实UID和并发缓存 **2/2通过**（`closure-managed-compilation-final.log`），两个同步客户端相同新源码收敛为同一产物。取消夹具清空环境后cc找不到ld，在开始产品验证前失败；已补作者显式`-B/usr/bin/`，待重编复验。`closure-shared-compile-measurement.json`记录私有debug核心、热OS缓存下冷请求7.318秒、命中5.564秒、并发新请求7.463/7.838秒；包括签名输入校验和IPC，不能当纯编译速度。两份4,239,368字节产物占缓存8,478,736字节，零损坏条目。
- npm作者pack **1/1通过**（34.68秒），修正主线程fixture缺空`python_wheel_tags`，见`closure-npm-author-tools-fixed.log`。首次真实bufferutil4.0.9构建在node-gyp配置之后、make运行printf时EPERM，`closure-npm-native-first.log`；尚不算native通过。独立审查另外确认可选依赖环失败传播及预构建addon入口完整性两项，正在同一owner修复。
- npm失败实际strace确认是make的`POSIX_SPAWN_RESETIDS`重设有效身份被拒绝，clone3原有ENOSYS回退正常。共享filter仅在process授权时允许`setres{u,g}id(-1,当前真实身份,-1)`，三个参数按内核低32位比较；其他有效身份、real/saved修改及无process情况仍拒绝。更新C probe两个capability分支 **1/1通过**（0.36秒），`closure-sandbox-spawn-identity.log`；sandbox严格Clippy通过。作者SDK另补glibc的pthread/dl/rt/util链接占位文件，修复真实链接缺`-lpthread`，没有使用宿主工具回退。
- node-gyp输出的内部硬链接经同一output owner按(dev,ino)核对完整别名数后转为独立字节，输出外别名仍拒绝；实际正反fixture **1/1通过**（38.27秒），`closure-npm-output-alias-final.log`。可选循环失败固定点、native成功/拒绝完整分区与Worker物理裁剪均经独立复核关闭；不是JS层可绕过的dlopen补丁。Stage增加250ms周期的可见scratch目录512MiB/65536项监控，不声称内核磁盘quota。
- 最新默认四target **70/7/12/11通过**，`closure-native-build-contracts-final.log`。官方bufferutil4.0.9源码构建/锁定/重放 **1/1**（33.60秒），`closure-npm-native-final.log`：新编译Release及内部obj.target别名、现成Linux-x64 prebuild都经runtime-only实际加载；三个Darwin/Windows变体被分类拒绝。实际hooks/optional环/bin与离线remove **1/1**（38.38秒），scratch超限终止与清理 **1/1**（18.81秒），分别见`closure-npm-{hooks,scratch}-final.log`。
- 官方npm产物离线Worker与worker_threads **1/1通过**（19.78秒），`closure-npm-worker-final.log`；直接调用生成addon的mask/unmask，移除原adapter后从资源恢复，两次prepare身份一致，所有拒绝项都未进入运行目录。不是纯JS fallback。源码收据到最终LLVM程序的集成仍在追加验收，暂不以Worker测试代替公开run/build。
- base优化archive刷新完成（5分03秒），通过副本+rename替换作者输入，保留历史测量compiler的原hardlink字节及SHA。仅清理已保存的base桥接缓存与debug下7条已完成测试的原生编译缓存（约91MiB，可重建）；报告/源码/备份/四profile输入未删除。其余三个archive及最终签名集成继续。
- 最终v5收据下Python作者pack **1/1**（27.26秒）、源码解析/原生构建 **1/1**（23.97秒）、源码Worker **1/1**（16.24秒）通过，`closure-pep517-{final-author-pack,native-lock-v5-final,source-worker-v5-final}.log`。共享Stage新增budget后官方Go UUID双构建一致、移除源码与Go环境运行 **1/1**（13.81秒），`closure-go-final-build-budget.log`。
- 共享编译取消/断连/版本租约/真实180秒deadline最终 **1/1通过**（196.27秒），`closure-managed-cancellation-final.log`；原夹具ld路径失败已经关闭，不缩短产品期限。结合先前跨UID及并发缓存两项，三项实际共享编译验收全部覆盖。
- SQLite及PostgreSQL优化archive分别5分09秒、5分01秒刷新，both继续。CLI严格Clippy首次发现pack返回类型复杂度及npm可折叠分支，已在owner修正、等待复验，不使用allow。源码输出归档在managed Worker展开后重复嵌入的问题已在原prune owner修正，保留声明/exec/投影环境共享使用；独立复核通过，默认及最终签名运行证据继续。
- 最终签名生态夹具已接入simplejson `_speedups.scanstring`和bufferutil原生mask/unmask，仍保留Python标准库路径、Go、三缓存条目及移源chroot。测试作者签名的是实际源码构建绑定的原runtime pack字节，未篡改收据SHA；依赖与npm输出通过公开artifact owner导入，run/build不重建或联网。此时只完成源码/独立复核，尚不计运行通过。
- both 最终优化 archive 已完成（3分39秒），四份 SHA/大小均已核对；私有 debug runtime 清单同步刷新，历史测量 compiler 的硬链接输入保持原字节。最终默认合同 `closure-final-consumer-contracts-retry.log` **71/7/12/11通过**（101项）；新 npm 收据输出裁剪覆盖 managed、declared、exec 和共享环境，不重复嵌入已展开归档。
- 严格 CLI Clippy 最终 `closure-final-cli-clippy-clean.log` 通过；前两次暴露测试中三处多余分配和一处多余借用，已直接修复，不加 allow。最终 author example 重新构建通过。保留最终CLI/测试/SDK到 `target/final-cli-snapshot-jiqbfz` 硬链接快照后，仅清理已完成 dev 编译缓存（Cargo报告2.4GiB，实际磁盘可用由1.2增至2.8GiB），恢复原精确可执行路径；源码、日志、历史测量产物、作者输入和备份保留。
- 最新四profile签名/确定性/受管及移源独立执行 **1/1**（126.43秒），`closure-final-signed-profiles.log`；最小OS核心编译 **1/1**（18.16秒），`closure-final-signed-core.log`。增强三生态最终链首轮 **失败**（752.34秒，`closure-final-signed-native-ecosystems.log`）：managed三组均完成，移源后pip进入Worker前EOF，不能记为无宿主通过。
- 根因复盘（D/E：组合覆盖缺口/隐含假设）：旧fixture把chroot当成完整namespace根，Linux会拒绝在其中创建CLONE_NEWUSER；与simplejson本身无直接证据关联。静态C最小探针实际确认chroot→EPERM、真正pivot→成功，显式作者bwrap也成功，日志`closure-{owned-chroot,owned-pivot,prepared-pivot}-ns-probe.log`。没有尝试放宽产品seccomp/namespace限制。一次性C源码已删，证据保留。
- P0防复发：core与生态验收复用`isolated_os_command`，仅挂载自有OS根、proc与synthetic设备，安装器验收也使用同一入口；已更新toolchain/quality规范。全链复验待重编，不以探针成功替代三生态运行。仓库无对应spec模板副本；遵守用户约束，不提交或归档。
- 正式Linux bootstrap另有实际产品缺口：原maker没有公开launcher/daemon，二者还依赖宿主libgcc_s，原最小OS core验收不覆盖它们。已开始复用既有签名与机器安装owner补独立bootstrap闭包和首次安装事务，备份`/tmp/dever-bootstrap-backup-YRnulM`。只允许owned系统根验收，不安装/重启本机全局服务。
- Linux bootstrap实现已接入独立签名的launcher/daemon/私有库、固定systemd资产、原子信任锚与五目标恢复journal。五项独立审查修复闭合：首次失败的服务恢复顺序、最终权限/目录树同步、旧签名模板兼容、绝对可信根、公钥中断恢复；额外覆盖通用旧版本`resolve_core`消费者，保留项目固定旧编译器的能力。`closure-bootstrap-contracts-clean.log`默认 **16/16通过**，`closure-bootstrap-trust-recovery-final.log`root信任/恢复 **2/2通过**，`closure-bootstrap-clippy-clean.log`严格Clippy通过。
- 最终maker已重编；保存精确bin/test/lib/runtime到`target/bootstrap-cli-snapshot-s5i92v`后清理可重建dev缓存，日志`closure-bootstrap-dev-cache-clean.log`（Cargo报告1.5GiB，物理可用1.1→1.8GiB）。首装实际验收`closure-bootstrap-actual-first.log` **失败**（107.92秒）：安装、旧模板升级和篡改包拒绝通过后，独立创建的客户端/daemon PID namespace令SO_PEERCRED的PID不可见。此为新夹具隔离边界不一致，产品认证不放宽；正在让客户端进入同一个owned namespace后复验。三生态pivot复验独立继续。
- 三生态pivot首轮`closure-final-signed-native-ecosystems-pivot.log` **失败**（526.20秒）：Python受管run/build通过，npm在复制已编译程序进入缓存时遇到ENOSPC；尚未进入最终移源阶段。根盘其后仅约1.5GiB空闲。验收临时机器目录改由显式作者`target/native-release-inputs/config/setting.json`选择`/dev/shm`，输入hardlink仍留在原文件系统，产品无改动/无TMPDIR变量。下一轮复用既有performance进程/cgroup owner，以4GiB上限运行自有测试，避免挤占根盘；结果另记。
- 临时目录合同 **17/17默认通过**（`closure-bootstrap-fixture-contracts-final.log`），严格Clippy和scoped rustfmt通过；产品dever/deverc/deverd摘要与此前冻结版本完全一致。受4GiB预算的bootstrap实际复验`closure-bootstrap-actual-final/` **失败**（184.97秒）：隔离根双项目check/run/build及缓存恰好1条通过，阶段切换时wrapper已退出而内部daemon尚持锁。峰值1,875,550,208字节、OOM为0；不是容量失败。夹具已用启动时固定的pidfd停止所属namespace，等待内部退出及同一daemon锁释放后再继续；显式停止失败终止验收，Drop不二次panic。产品认证/锁未修改，独立定向复核通过，实际重跑待完成。

- 三生态受4GiB预算的复验 `closure-signed-native-ecosystems-bounded/` **失败**（742.77秒）：三种生态的受管check/run/build及3条缓存断言全部通过，移除机器目录和项目源码后，Python独立程序启动Worker收到协议early EOF。预算峰值2,804,113,408字节、OOM/预算触顶均0，磁盘无耗尽；不能把此前chroot修正记为最终通过。正在对真实namespace内层启动做最小探针定位，避免反复整链构建。
- 最小探针确认上述EOF来自内层bwrap `Can't mount proc ... Operation not permitted`：外层bwrap的只读proc子挂载令内核拒绝嵌套user namespace重建proc。新增root/test下显式静态`native-acceptance-init`，只在已经pivot的owned namespace内覆盖完整private proc后exec；保留外层原能力，内层产品cap-drop/guard不变，不绑定宿主proc。实际Rust init嵌套Python/guard探针 `closure-static-init-nested-probe.log` 通过；所有可见PID的root inode均为owned根的独立探针亦通过。修正rustix信号常量后默认 **17/17** 通过（`closure-final-namespace-contracts.log`），新静态helper实际构建通过。最新fixture Cargo产物hash已改变；误选旧hash的 `closure-bootstrap-actual-private-proc/` 主动中止，不计证据，其5个owned临时目录和cgroup已清理；复验使用当前 `native_release-c3060e189a48ed83`。

- Linux首次安装完整验收 `closure-bootstrap-actual-current-init/` **1/1通过**（231.51秒），报告 `target/closure-linux-bootstrap-acceptance.json`：独立信任签名首装、旧签名模板升级、篡改拒绝不损坏原入口、最小OS根双项目check/run/build、单条共享编译缓存、真实UID65533/65534使用和管理拒绝、移除项目/机器目录后的两个独立程序均通过。仅自有image目录的systemd资产，未激活全局服务；4GiB预算峰值1,512,423,424字节，无OOM或预算触顶；所有自有daemon/namespace/cgroup已退出。不是公开发行或其他平台交付。

- 三生态最终 `closure-signed-native-ecosystems-current-init/` **1/1通过**（755.81秒），报告 `target/closure-sandbox-ecosystem-release-acceptance.json`：三种签名runtime身份、受管check/run/build、3条编译缓存、离线源码构建收据重放，以及删除全部项目源码/机器目录后的三个最小OS独立程序全部通过。Python实际加载simplejson3.20.1 C扩展，Node加载bufferutil4.0.9 addon；本项Go是标准库协议Worker，官方UUID第三方依赖另有前述真实隔离构建/移源执行证据，不混记。4GiB验收预算峰值3,276,374,016字节，OOM/预算触顶均0；这是编译和临时文件总预算，不是应用RSS。自有进程、namespace、临时机器目录与cgroup已清理。失败日志均保留，没有改动产品sandbox来绕过夹具失败。

- 最终静态质量：`closure-final-namespace-clippy.log` CLI lib/bins/native_release及静态test-init **Clippy -D warnings通过**，scoped rustfmt通过。没有运行或冒充全量workspace/完整CI；独立复核确认新init仅属于测试入口，未绑定宿主proc或修改产品sandbox。
- 结束时将当前已验收bin/test/example、私有lib和四profile runtime以硬链接保存到 `target/closure-verified-tools-ksA2Hn`，清理本轮可重建dev缓存后恢复原路径；当前native_release为 `c3060e189a48ed83`，静态init亦保留。`closure-final-dev-cache-clean.log`记录2822文件/1.8GiB逻辑容量，实际根盘可用约1.1→2.1GiB。首装报告中的launcher/daemon SHA与恢复后完全一致；源码、SDK/作者输入、报告、既有备份和无关项目未清理。

## 风险文件/边界

- `crates/dever-runtime/**`、`crates/dever-cli/**`：API、数据库和运行时集成，需避免破坏已有协议。
- `examples/cms/**`：双源码行为一致性，删除旧流程前先保留可恢复验证点。
- `crates/dever-build/**`、`crates/dever-cli/**`：External Lib 和共享缓存构建边界。
- `crates/dever-backend-bridge/**`：唯一授权的底层连接 unsafe 边界、foreign buffer 所有权、LLD 串行/恢复与私有输出目录。
- `config/setting.json` 与测试 fixture：不得引入环境变量秘密或真实凭据。

## 验证命令

优先使用离线和定向检查，例如：

```text
cargo test --offline --locked -p dever-tests --test application_testing
cargo test --offline --locked -p dever-tests --test native_cache
cargo test --offline --locked -p dever-cli --test cms_project
deverc check examples/cms/dever
deverc check examples/cms/md
```

真实 PostgreSQL、跨平台发行和性能命令必须在条件具备后单独执行并记录结果；不默认运行全量 build 或集成测试。

2026-10-04 范围更新：用户另行授权的 Linux x86_64 → ARM64 **应用交叉构建**已在 `linux-arm64-cross-build` 任务完成，最终证据为 `target/arm64-cross/acceptance.json`，包括四profile/缓存/SQLite与完整ARM内核下的三生态Worker及HTTP。此项完成不表示ARM原生编译器、其他平台或公开发行完成，原有延期范围保留。

# Dever 持久化 Job

## Goal

为 Dever 提供数据库持久化、事务一致、可恢复且有界的后台任务和 UTC 定时任务，使 API 与 worker 可使用同一编译产物独立或组合部署，禁止用脱离结构化作用域的异步任务代替可靠 Job。

## Dependencies

- `time-typed-codec`：DateTime、payload codec 和 schema fingerprint。
- `port-adapter`：Job 调用的 App 可以通过 Port 执行外部副作用。
- 复用既有 SQLite/PostgreSQL ORM transaction、pool、settings 和 shutdown。

## Requirements

- 新增 Job source role；允许 `job.dever` 或平铺 `job/*.dever`，角色目录遵守现有 file/directory exclusivity 和 no-single-topic 规则。
- Job entry 使用明确 contextual declaration，零输入或一个 App-owned wire-safe record 输入、零输出，只能调用同领域 App，不能直接访问 Domain/Model/Port/Adapter/API；Job payload 支持保留 Model identity 的 ModelId。
- Job entry 不可被普通 source call；App 只能通过 compiler-owned enqueue/enqueue_at target reference 创建持久化任务。
- 每个 enqueue 必须提供业务 idempotency key；同 Job identity + key 的未终结任务不重复插入。
- Job 声明绑定一个逻辑 database connection，并声明有界 timeout/max attempts；`retry(n)` 表示包含首次执行在内最多执行 n 次，不允许零/负/越界值。
- 立即和 run-at Job 持久化 target identity、payload、schema fingerprint、key、run_at、attempt/state、lease 和错误摘要。
- 周期 Job 使用编译期验证的五字段 UTC cron declaration且只能绑定零输入Job；持久 cursor、occurrence insert 和推进在同一 transaction，每个计划窗口生成确定性key，多进程及已完成任务不能重复创建同一occurrence。带payload的业务计划使用enqueue_at。
- transaction App 内 enqueue复用隐藏 transaction context；Model effect与Job connection不同则编译失败。非 transaction enqueue使用自身短事务。
- SQLite/PostgreSQL使用runtime private versioned schema；初始化、claim、lease renew/expiry、success、retry、dead-letter均有明确事务边界。
- worker采用至少一次执行；attempt 在 claim 时原子增加，每次 claim 使用独立 fencing token；失败按有界指数退避重试，达到上限进入 dead-letter，不无限循环或静默丢弃。
- payload schema不兼容、未知 Job target和解码失败进入dead-letter/blocked状态并记录脱敏原因，不best-effort执行。
- `dever.job.serve()`显式启动worker。combined main使用现有Group并发启动API/Job；不增加隐式后台线程。
- `config/setting.json`提供严格 `runtime.mode=api|worker|all` 和 job worker/lease/poll/shutdown上限；不接受环境变量或CLI覆盖。
- SIGTERM/Ctrl-C停止API accept和Job claim，排空在途工作到deadline，再停止、释放lease/连接并执行数据库/log shutdown。
- 应用测试的Job数据库由runner生成隔离SQLite；不读取部署setting或自动连接PostgreSQL。

## Acceptance Criteria

- [x] Job合法路径/签名可检查和格式化；ModelId payload 保持 nominal identity；普通调用、非法输出/类型、跨域或直接Model/Port访问被拒绝。
- [x] 业务Model写入与Job入队在同一transaction一起commit/rollback；跨连接组合在编译期拒绝。
- [x] 相同identity+key并发入队只保留一个可执行任务，不丢失已提交业务状态。
- [x] 到期前不执行，UTC run-at到期执行；周期任务在多个worker下每个window最多创建一次。
- [x] worker崩溃/取消会消耗 attempt，lease 到期可由另一 worker 恢复；活跃 lease 不被重复 claim，旧 fencing token 不能提交迟到结果。
- [x] handler失败按策略重试，成功终结，达到attempt上限进入dead-letter；错误信息不含Secret/payload全文。
- [x] schema mismatch、unknown target、corrupt payload不会执行App并保留可诊断记录。
- [x] api/worker/all三种mode分别只启动应有服务；缺少相应配置在监听/claim前失败。
- [x] SIGTERM/Ctrl-C定向runtime test验证停止新工作、排空或超时取消以及最终数据库/log关闭顺序。
- [x] SQLite端到端通过；PostgreSQL feature编译通过，真实行为只在隔离 `postgres_test` 明确存在时运行。

## Out Of Scope

- Exactly-once外部副作用、分布式事务和跨数据库原子提交。
- Redis/Kafka/RabbitMQ/SQS和第三方队列。
- 非UTC cron、DST和动态用户时区调度。
- Job管理front；dead-letter运维界面后续由业务API/front实现。
- 无界任务fan-out、优先级抢占和动态代码任务。

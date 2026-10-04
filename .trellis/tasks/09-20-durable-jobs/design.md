# Dever 持久化 Job 设计

## Source Contract

Job role 使用 `job.dever` 或平铺的 `job/*.dever`，继续遵守角色文件与目录互斥、单 topic 必须合并的规则。Job 只是持久任务入口，不是可调用的普通函数：

```dever
database default

job publish_due(input: app.PublishInput) () retry(5) timeout(30000) {
  app.publish_due(input.article_id)
}

job cleanup_sessions() () retry(3) timeout(30000) {
  app.cleanup_expired()
}

schedule cleanup_sessions = "0 * * * *"
```

Payload record 由同领域 App 声明为公开契约，Job 不新增第二套 DTO 可见性。Job 只能有零个输入，或一个 wire-safe record 输入；输出必须为空；`retry(n)` 表示包含首次执行在内最多执行 `n` 次。`timeout(ms)` 和 `retry` 都是必填、有硬上限的正整数。

共享 wire schema 增加 nominal ModelId 节点。编码仍使用既有 ModelId 数值表示，但 fingerprint 包含所属 Model identity，不能把两个 Model 的 id 降级为同一种 Int/Id。Job payload 仍拒绝 Secret、private Model data、Choice、handler 和不稳定类型。

App 通过 compiler-owned 静态 target 入队：

```dever
id = dever.job.enqueue(job.publish_due, input, key)
id = dever.job.enqueue_at(job.publish_due, input, key, run_at)
id = dever.job.enqueue(job.cleanup_sessions, key)
id = dever.job.enqueue_at(job.cleanup_sessions, key, run_at)
```

Target 不是普通值，不能存储、转发或用于普通 call/run/group。enqueue 使用专用 HIR，不把 Job handler 加入同步调用/effect/递归图；Job handler 作为 worker 独立 root 生成并检查。

## Effect And Transaction Integration

Model 和 Job 都映射到一个统一 database binding。effect 系统记录有类型的 database owner，并通过唯一 accessor 解析为现有 `ConnectionSelector`；Model CRUD 仍保留 Model schema owner，不制造虚假 Model，也不增加第二套 transaction context。

enqueue 只传播队列连接的 database/suspension/failure effect，不同步传播 Job handler 的 Model、Port 或业务 failure。transaction App 的 enqueue 使用现有隐藏 `_database: Option<&Transaction>` 和 `Executor::new`，与同连接 Model 写入共享物理 transaction；无 ambient transaction 时开启并提交短 transaction。显式不同连接在语义检查拒绝，依赖 default/package fallback 的冲突在 settings-aware run/build bootstrap 拒绝。

## Private Schema And States

每个使用 Job 的逻辑 database 初始化 runtime-owned versioned schema，独立于 Model history、snapshot、seed 和 migration。至少包含：

- Job：id、target identity、schema fingerprint、payload、dedupe key、state、run_at、attempt、max attempts、timeout、claim token、lease deadline、安全错误分类/摘要和时间戳。
- Schedule cursor：稳定 schedule identity、cron fingerprint、last processed UTC minute 和更新时间。

状态为 pending、running、succeeded、dead、blocked。业务 key 只在 pending/running 上保证 `(target, key)` 唯一，终结后可重用；schedule occurrence 由持久 cursor 保证永久单窗口物化一次，不能依赖 active-only 唯一索引。

未知 target、schema mismatch 和 corrupt payload 不执行 App，保留原记录并进入 blocked。历史记录的实际 timeout 也必须满足当前 SQLite lease 约束，否则以固定 `invalid_policy` 进入 blocked。attempt 耗尽进入 dead，并由独立有界清理处理，不占用领取可执行任务的槽。运行时不保存 Secret、完整 payload、backtrace 或未经控制的 `AppError::to_string()`；只保存固定类别和有界安全摘要。

## Claim, Lease And Retry

PostgreSQL 使用短 transaction、bounded batch 和 `FOR UPDATE SKIP LOCKED`；SQLite 使用短 `BEGIN IMMEDIATE` 与条件 update。只按当前空闲执行槽 claim，不能预占后在无界内存队列等待。

每次成功 claim 时原子增加 attempt，并生成独立随机 claim token。这样进程在 handler 返回前崩溃也会消耗一次 attempt。过期 running 在 attempt 未达上限时可重领，已达上限时转 dead。

renew、success、retry、blocked/dead 更新均使用 `id + claim token + running state + 未过期 lease` CAS。claim 在获得写锁后读取时钟并建立租约；renew/complete 先锁定 claim 行，再读取同一 Clock，不能用锁等待前的时间续租或确认已过期任务。CAS 失败表示已失去所有权，旧执行者不得覆盖新 owner 状态。worker 在 lease 周期内续租；handler timeout 或安全业务失败按 checked exponential backoff 重新 pending，且不超过配置上限。语义为 at-least-once，外部副作用仍由业务 idempotency 保证。

## UTC Schedule

Cron 固定为五字段 UTC：minute、hour、day-of-month、month、day-of-week。支持 `*`、单值、逗号列表、闭区间和步长；范围分别是 `0..59`、`0..23`、`1..31`、`1..12`、`0..6`，其中 Sunday 为 0。day-of-month 与 day-of-week 同时受限时采用 cron 的 OR 语义，否则匹配受限字段。

首次启用从当前 UTC minute 建 cursor，不回放历史。以后在同一 transaction 锁定 cursor、插入 occurrence、推进 cursor；每轮最多扫描并推进 256 个分钟槽，只对匹配 cron 的槽生成任务，剩余分钟后续继续补。slot key 包含稳定 schedule identity 和 UTC minute，与业务 key 分离。相同 schedule identity 的 cron fingerprint 不一致时启动失败关闭，禁止多版本互相改写 cursor。

## Runtime Topology And Shutdown

严格配置只来自 `config/setting.json`：

```json
{
  "runtime": { "mode": "all", "shutdown_ms": 30000 },
  "job": {
    "workers": 4,
    "poll_ms": 250,
    "lease_ms": 60000,
    "retry_base_ms": 1000,
    "retry_max_ms": 300000
  }
}
```

`runtime.mode` 只能为 api、worker、all。generated entry 在 listen、schema materialize 或 claim 前统一校验 source capabilities、HTTP/Job 配置、driver 和所有上限。api 模式中的 `dever.job.serve()`、worker 模式中的 `dever.api.serve()`安全 no-op；同一 binary 保留两套静态 dispatch roots，不按运行模式裁剪。

没有 worker capability 的纯 API 应用可省略 runtime，默认 api 和 30000 ms；有 worker capability 时必须显式配置。worker 连接池必须比 worker 槽至少多一个连接，供 claim/heartbeat 使用；不引入独立控制连接池。SQLite 只有一个写者，额外连接无法绕过业务写事务，因此启动前还必须验证 `lease_ms > Job timeout_ms`，让租约覆盖 handler 的最长执行时间；PostgreSQL 可继续使用较短租约并续租。

generated entry 拥有唯一 lifecycle：结构化等待应用 root 与 SIGTERM/Ctrl-C。收到信号后广播 shutdown；API 关闭 listener 并复用 HTTP connection drain，Job 停止 schedule/claim 后等待 active handlers。到 `runtime.shutdown_ms` 才取消剩余作用域；未确认 success 的 Job 保留/恢复 lease。应用 root 汇合后执行 database shutdown，再 flush log。Tokio signal 是既有依赖的 feature 扩展，不引入 signal/queue/cron 框架或 unsafe handler。

Job timeout 上限为 3,600,000 ms，lease 的有界范围为 100..7,200,000 ms。测试使用 lease 上限，保证所有合法 Job timeout 都能在隔离 SQLite 中执行，仍复用生产 claim/dispatch/finish 和策略检查。

## Application Testing

每个 test case 保持独立 native module、进程、临时目录、Port fake graph 和 SQLite connections。闭包同时包含：test 直接 enqueue 的 Job、Job handler 调用的 App/Port，以及 handler 的 Model connections；生产 Adapter 和部署 settings 不得回退进入 case。

测试专用 compiler-owned 操作提供 case-local clock advance 和 bounded due-job drain。它们复用生产 claim/dispatch/finish 状态机，不启动永久 `serve()`，不真实 sleep，也不允许测试直接调用 Job entry。无 database/Job effect 的纯测试仍不加载 settings。

## Explicit Non-Goals

不增加动态 target string、通用 DI/queue framework、外部 broker、非 UTC schedule、exactly-once 外部副作用、payload 自动迁移或 Job 管理 API/front。真实 PostgreSQL 并发、锁与取消仍需隔离 fixture 验收；feature check 不能替代运行证据。

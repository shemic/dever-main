# Dever Minimal Core 0.1 Implementation Plan

## HTTP/2 Ceiling And Soak (2026-09-18, Complete)

子任务 `09-13-http2-ceiling-soak` 复用既有负载器，将客户端 worker/CPU 集合参数贯通 HTTP/1、HTTP/2 和长连接入口。34 项运行器定向测试及最小 loopback 回归通过；生产运行时未改。1 CPU、128 MiB 下 h2c/TLS+h2 的 16x16、30k/45k/60k/90k 双次扫描表明本机吞吐约 36k-40k req/s 时服务端接近 1 核，客户端仍占 1.4-2 核；请求槽丢弃使它不是纯服务端上限。28k 的首轮稳态未达 99% 成功率，故改用 20k：128/64 MiB 各 60 秒的四组成功率为 99.77%/99.92%/99.94%/99.80%，无请求错误、OOM 或 FD 残留。h2c/TLS+h2 各 100 轮重连无错误或 FD 残留；短时 RSS 数据不证明小时级无泄漏。原始报告、命令和限制见 `test/performance/README.md`。

HTTP/2 有界验收和 typed ORM 子任务均已完成。根任务保持进行中：历史计划的编译器发行/跨平台与自举仍未交付，小时级稳定性等扩展也没有在本轮获批或验证；不得凭子任务完成数宣称整个语言路线图结束。

## N5 HTTP/2 (2026-09-12, Complete)

子任务 `09-12-http2` 完成：Hyper/h2 buffered/live 服务端、single-use/pooled 客户端、TLS ALPN、固定流控窗口与每连接流配额、逐流 handler/上传/下载/SSE、RST 隔离及 GOAWAY 排空。官方 `Limits.http2: Http2Limits?` 与检查器一起直接演进，静态原生桥接复用既有 Nullable/Record；没有新 intrinsic 或兼容层。

Hyper Executor 的协议任务接入 Dever 子 Scope，关闭 admission 与登记串行，故障上报/注销在 completion 之前；不可取消 supervisor 清理协议任务和已启动 blocking 后代后才释放物理许可。池连接归持久 driver；上传/下载共享流租约。只有 Hyper 结构化交回的未发送请求可在原期限重排，已发请求不重放。

88 个不同定向用例通过：HTTP/2 client/server 各6，HTTP engine12、HTTP library7、live16、pooled network14、pooled library6、async runtime10、structured concurrency11。独立 h2 对端覆盖流控、取消、容量、ALPN、语义与故障；原生程序覆盖单连接 TLS 嵌套代理、流式上传、SSE、run/await 和原有 HTTP/1/WSS。修复传输取消误升为整服故障、响应发布锁重入、GOAWAY 未发请求卡住旧租约和 Response TE 静默删除；测试端句柄和 WSS 独立连接预算也已修正。

最终 CLI release 构建、backend check/fmt-check、network_bench/reference 编译及源码检查通过；独立审查闭环。Rustfmt/Clippy 不可用，官方库 fmt-check 的原有 net.dever 差异未改。临时构建目录和暂存约378 MiB可再生缓存已清理，旧编译器与性能报告保留。不提交、不自动归档；N5 实现没有运行 HTTP/2 性能矩阵。后续仍有 HTTP/2 压测、小时级稳定性、入口调度优化、RFC 8441/协议扩展、数据库、发行和自举。

## P2 Async Comparison And Long Connections (2026-09-12, Complete)

子任务 `09-12-runtime-performance-p2` 继续复用 P1 工具，完成同语义 Rust run/wait 对照、perf 调度定位、HTTP/HTTPS 高负载和 TCP/WS/SSE 分阶段回收测量。12 个运行器检查、4 个独立对端检查、11 个原生入口及新 Rust examples 的离线构建通过；未修改生产编译器/运行时。

1 CPU/128 MiB 下，Dever run/await root约15.50 µs、实验worker约2.003 µs，与Rust15.59/1.984 µs接近；任务入口跨线程切换是明确优化方向，根任务配额、Send和取消语义需保留。54组HTTP高负载发出1500830请求全部成功、299170计划请求未发出；压测槽位已限制结果，不是服务上限。

最终27组长连接覆盖32/128/512条，每组3轮；18144连接、377061消息均成功，81次FD回基线。另各协议512连接十轮，共15360连接、471207消息，无错误/OOM且30次FD回基线；SSE RSS在约25.3 MiB趋稳。修正采样退出与阶段交接竞态、SSE无心跳误通过和客户端清理；完整证据与限制集中在 `test/performance/README.md`。

P2 结束时尚未启用http2/h2依赖；当时规划的TLS ALPN、独立流/连接预算、流控、取消隔离和GOAWAY已由后续N5交付。小时级soak与入口调度优化另行落地，总语言路线图保持未完成。

## P1 Standalone Runtime Performance (2026-09-12, Complete)

用户批准建立常驻内存、异步调度和 HTTP/HTTPS 三组基准，取代旧网络实施阶段的局部“不压测”限制。子任务 `09-12-runtime-performance` 完成：真实原生二进制、Rust runtime/Hyper 对照、有界固定速率负载、Linux RSS/PSS/CPU 和专属 cgroup 预算、原始 JSON 报告。生产语言/运行时未改动。

7 个运行器与 2 个独立 HTTP 对端检查通过；离线原生/release 构建和格式检查通过，Rustfmt/Clippy 不可用。最终 78 组主测、18 组新连接、15 组 64 MiB 空闲基线均完成且无 OOM；主测网络发出 107962 请求全部成功，38 个计划请求未发出并按原因记录。当前只是热缓存、短时、指定负载结果，不是吞吐极限、冷启动整机内存或长期稳定性结论。

运行命令、确切统计口径和首轮数据以 `test/performance/README.md` 为准，原始产物/报告保存在 `target/performance/`。后续候选：同语义 Rust 异步对照与调度剖析、高负载扫描、TCP/WS/SSE 长连接；其余语言/后端路线图仍未完成。

## N4 Concurrent Composition And Reusable Clients (2026-09-12, Complete)

- [x] Implement source runtime configuration, typed handler context, timeout/race and pull-driven stream adapters; extend async memory traversal.
- [x] Add verified TLS transport and bounded reusable HTTP clients with streamed uploads/downloads; integrate HTTPS/WSS and TLS servers.
- [x] Wire compiler contracts, official source, native conversion and examples together.
- [x] Run focused compiler/native and independent loopback protocol/lifecycle tests; review final ownership and limits.
- [x] Update canonical language/spec docs and record evidence without a commit or archive.

### N4 Evidence

83 distinct focused cases passed: async_composition 7, pooled_network 14, pooled_library 5, structured_concurrency 11, async_network 9, http_engine 12, http_library 6 and live_network 16. Reruns are not counted twice. The old structured-concurrency case that rejected async List handlers was migrated to the newly accepted contract and checked for a concurrency effect.

New coverage includes source runtime bounds/start-once, timeout cleanup of started blocking children, race loser cleanup/fault propagation, Channel null/drain/close and periodic stream timing, static handler/output checks, C012 stream failure obligations, async memory traversal/fusion and concrete HTTP Context transfer. Native programs exercise HTTPS, pooled proxy calls through shared Context, streamed uploads/downloads, SSE, WSS and graceful shutdown.

Independent peers verify physical connection reuse, repeated headers, early chunks beyond the response-head deadline, chunked upload, slow/abandoned/truncated bodies, upload failure/cancellation, queue/body/idle deadlines, pool close wakeups, unpolled-task cleanup, certificate CA/name rejection and WSS frames. A reproduced lifetime defect allowed a pool's unpolled driver to escape its creator; constructing the driver owner guard before task creation fixes it. Native checks also exposed missing Channel Debug/Render for shared records and async handlers entering synchronous fusion; both are corrected. Final bounded cleanup removed an infallible Result from the private lease-completion path; all 14 pool cases passed afterward.

Actual CLI backend example check and fmt-check passed without warnings. Offline build produced a standalone binary; running with an empty environment and PATH restricted to /usr/bin:/bin printed exactly `Hello /Dever\nrequests = 1\n` and exited successfully. Official source formatting, optional-reference core/runtime/CLI cargo check, whitespace/conflict-marker and final ownership/contract inspection passed. Rustfmt/Clippy are not installed, so their checks were unavailable; Rust layout was reviewed manually.

Only initial dependency preparation fetched locked packages; validation/builds were offline. All network peers used owned loopback random ports and static test certificates under test/. No full suite, prior performance baseline, existing service, persistent example launch, global install or commit. No throughput/RSS improvement is inferred from functional checks. Temporary build, certificate-generation, formatting and demo directories are removed at handoff; workspace native caches remain. N4 is complete; HTTP2, protocol extensions/close metadata, database, distribution/cross-platform and self-hosting keep the umbrella task active.

## N3 Streaming HTTP, SSE And WebSocket (2026-09-12, Complete)

- [x] Add `serve_live`, HttpReply and LiveLimits while keeping ordinary serve direct. Reuse request/response codecs and connection configuration; one-slot body channel and one-handler Group per live connection.
- [x] Add SSE field encoding, bounded chunks, Last-Event-ID access and body-driven heartbeat timers; no event history or extra heartbeat task.
- [x] Reuse tokio-tungstenite 0.30.0 handshake/protocol for server/client Text/Binary/Ping/Pong, AsyncStream messages and close handshakes. Shared Endpoint lifecycle, 4 KiB read buffer, bounded messages/write buffers and independent directions.
- [x] Integrate resource types, exact nominal records/choices, async effects, static handlers, native conversion, API/Markdown names and Render. No new syntax, compatibility layer, dynamic interpreter or generic transport abstraction.
- [x] Verify owned wire peers and real native programs; update canonical docs/example and manually record the session without a commit or archive.

### N3 Evidence

46 distinct focused cases passed: live_network 16, live_library 3, http_engine 12, http_library 6, async_network 9. Repeated checks after corrections are not counted again. All final checks passed; the earlier slow-peer test assumed normal return and was corrected to observe cleanup on either return or cancellation.

Wire evidence covers early chunk delivery, bounded slow-peer output, SSE exact encoding/heartbeats past the ordinary deadline, idle failure isolation, HEAD cleanup, handler faults, upgrade capacity/preread bytes, independent client masks, fragmentation/UTF-8/control frames, close, partial writes, waiting-writer cancellation and started blocking descendant drain. A reproduced regression showed duplicate response publication closed the existing body; cancellation ownership now starts only after publication is accepted, and the regression passes.

The native source program exercised all four routes (buffered, stream, SSE, WebSocket), including client connect/send/receive/close and static async message-stream handlers. Existing native HTTP clients also matched independent wire peers. Final optional-reference cargo check, actual CLI example check (no warnings), CLI fmt-check, source format roundtrips and explicit whitespace/owner inspection passed. Rustfmt/Clippy are not installed and were not run. No throughput/RSS claims or old baseline.

Only a one-time dependency fetch accessed the registry; checks/builds ran offline. No full suite, existing service, persistent example launch, global install or commit. N3 is complete; TLS/WSS, HTTP2, client pools, protocol extensions, close metadata, database/distribution and other roadmap work keep the umbrella task active.

## N2 HTTP Engine (2026-09-12, Complete)

The user approved continuing after N1. Execute inline; no further planning approval. Reuse Hyper HTTP/1 on the shared Tokio runtime. No legacy compatibility, old baseline, full suite, service checks or global installation.

- [x] Replace the custom HTTP parser/transport with Hyper server/client primitives and migrate all source callers.
- [x] Preserve repeated headers as `List<Header>` with byte values; buffer bodies within explicit limits. Reuse byte storage across the engine boundary.
- [x] Add `serve(async handler, Listener, Limits) -> ()` with bounded structured connections, server keep-alive, chunked input and cancellation cleanup. Configuration/accept/handler faults propagate; peer protocol failures terminate only that connection.
- [x] Add one-shot `send(address, port, Request, Limits) -> ResponseResult`. Poll its driver in the request future; no detached tasks, implicit retries or client pool.
- [x] Check static handler contracts/effects and native record conversion. Replace obsolete parser tests with independent loopback wire cases and focused compiler/native tests.
- [x] Update canonical docs and record evidence. TLS, HTTP/2, client pooling, streaming response bodies, WebSocket and SSE remain later work.

Limits: header buffer at least 8192 bytes (Hyper requirement), nonnegative body bytes, positive timeout, bounded connections. Defaults: 8192 / 1048576 / 5000 ms / 32. Runtime owns HTTP framing; outgoing Content-Length/Transfer-Encoding are rejected. Request header/body/handler and stalled write deadlines prevent one idle peer retaining a connection indefinitely. Static Dever handlers specialize directly; no dynamic interpreter dispatch.

### N2 Evidence

- 44 distinct focused cases passed: http_engine 12, http_library 6, async_network 9, structured_concurrency 11, native_ownership byte cases 3, runtime_foundations bytes case 1, contract_execution selected example checks 2. Repeats after local corrections are not counted again.
- Independent wire cases cover persistence/pipelining, HEAD and 204/205/304, fragmented/chunked/binary bodies, repeated and non-UTF8 headers, conflicting lengths, header/body limits, unsupported trailers/upgrades, truncated responses, all deadlines, connection capacity and descendant cleanup. Header timeout closes directly; body/handler timeouts produce 408/504. Native tests execute real compiled send/serve and compare client bytes to an independent peer.
- Bytes tests retain unique Text storage transfer, shared-alias behavior, shared slices and consuming iteration. Connection setup now configures TCP_NODELAY once through one helper. No old-performance baseline, throughput or RSS claims were derived from correctness tests.
- Core compiled offline with the optional reference feature. Actual CLI HTTP example check/fmt-check and formatting roundtrips passed; the persistent example was not started. Final owning-layer, dependency, obsolete-reference and whitespace review passed. Rustfmt and Clippy were unavailable (components not installed), not reported as passing.
- Hyper 1.11.1, hyper-util 0.1.20, http-body-util 0.1.5 and bytes 1.12.1 are pinned. Missing transitive development dependencies were fetched once; builds/tests remained offline. No Axum, TLS or HTTP2 features were added.
- No full suite, existing service check, load test, global install, commit or task archive. N2 is complete; the umbrella task remains active for N3 and other unshipped roadmap work.

## N1 Async Stream And TCP (2026-09-12, Complete)

- [x] Persist the approved new-language/no-compatibility policy; use existing inline execution routing.
- [x] Add bounded, cancellable async stream consumption and Tokio TCP resources to the shared runtime.
- [x] Integrate AsyncStream types, async handlers, effects/liveness and concrete native code generation.
- [x] Replace synchronous TCP source contracts and migrate official HTTP transport, examples and tests.
- [x] Verify stream backpressure, close/stop, duplex I/O, timeout and compiler/native behavior with focused tests.
- [x] Update public contracts, inspect the final changed flows and record evidence. N2 HTTP engine and N3 WS/SSE remain deferred.

### N1 Evidence

- 62 distinct focused cases passed: async_network 9, async_stream 6, structured_concurrency 11, async_native 6, async_runtime 10, http_library 11, core_native `official_` 5, runtime_foundations `stream` 3, and the reference/native file-stream differential case 1. The HTTP benchmark stayed ignored. Repeated individual checks after local fixes are not counted again.
- Network tests owned only loopback port-0 sockets. A one-worker runtime proved duplex progress, pending close wakeups, read timeout reuse, partial-write cancellation closure, pre-pull backpressure, failure while the producer is idle, child cleanup and listener release. The persistent server example was checked without starting it.
- Native execution exposed missing Render implementations for new resource types; those were fixed at the runtime owner and the affected native checks passed. Migrated TCP differential cases now assert independent native expectations; the synchronous evaluator no longer carries a duplicate TCP implementation.
- Compiler/default and optional reference builds passed offline. Source formatting round trips, explicit whitespace checks and final owning-layer inspection passed. Rustfmt/Clippy were unavailable because those components are not installed; no claim of passing them.
- No workspace-wide tests, service checks, old-performance baseline, network load test, global install or commit. The task remains active for N2 HTTP engine and N3 WebSocket/SSE, not for unfinished N1 work.

## Structured Concurrency Implementation (2026-09-12, Complete)

The user approved this preceding implementation. Networking was paused during it; N1 above resumes AsyncStream/TCP. Each phase uses only focused offline tests under root `test/`; do not run the workspace-wide suite, service checks, old-performance baselines or network load tests.

### Phase A: Freeze Syntax And Semantic Model

- [x] Add the canonical `async name(...)` modifier while retaining post-signature `pure`/`recover`; update lexer/parser/formatter and Markdown signature model together.
- [x] Add async to function identity metadata and HandlerSignature compatibility without changing package/name/arity lookup.
- [x] Reject `async` together with `pure` in the first release and classify every concurrency operator through the existing transitive effect owner.
- [x] Add contextual `await`, `run`, `stop`, `group`, `parallel`, `blocking` and `channel` syntax; reject async calls as ordinary stored values.
- [x] Add HIR task/group/channel operations and compiler-owned affine local metadata; do not add a user-visible Future or generic evaluator value.
- [x] Extend recursive type properties with thread transferability and explicit resource opt-in.
- [x] Add focused syntax/format/check cases for clause agreement, sync-to-async rejection, handler mismatch, forbidden Task escape and transfer failures.

Rollback point: syntax/HIR-only changes must leave all existing synchronous HIR and formatted output unchanged.

### Phase B: Await And One Shared Runtime

- [x] Add Tokio with the smallest required features and one runtime owner in `dever-runtime`.
- [x] Emit async functions as concrete Rust futures while retaining direct sync emission for wholly synchronous entry graphs.
- [x] Implement direct `await(async_call(...))` and async entry execution; preserve zero/single/named multi-output and source-located failures.
- [x] Implement async timer sleep as the first real suspension boundary; do not use TCP/HTTP as the proof fixture.
- [x] Verify multiple sleeping tasks can make progress without one OS thread per task and sync binaries do not initialize the runtime.

Rollback point: runtime dependency and native entry changes are isolated from Task ownership and can be removed while keeping Phase A diagnostics.

### Phase C: Structured Task And Group Ownership

- [x] Implement `run(async_call(...)) -> Task`, `await(task)`, and exactly-once affine consumption across local writes and every clause exit.
- [x] Implement zero-output `stop(task)` as cooperative cancel-and-wait; reject result-bearing stop and repeated consume.
- [x] Implement positive bounded `group(limit)`, backpressured `run(group, call)`, `await(group)` and `stop(group)`.
- [x] Ensure normal and abnormal parent completion cleans all children; no detached path or swallowed expected failure.
- [x] Verify start-before-await concurrency, output equivalence, cancellation cleanup, capacity backpressure and bounded task ownership.

Rollback point: Task and Group runtime modules remain separate from scheduler construction and do not change direct await.

### Phase D: Multi-thread And Blocking Work

- [x] Implement `parallel(sync_call(...))` for pure transferable calls using bounded execution; preserve existing numerical/error semantics.
- [x] Implement the compiler-owned `blocking` boundary only for calls with a transitive compiler-known blocking effect; bound active workers plus queued submissions.
- [x] Route async use of `parallel_each` away from Tokio workers while retaining the current synchronous contract and shared worker validation.
- [x] Configure conservative defaults from available parallelism with explicit hard maxima; do not create per-call runtimes or unconstrained pools.
- [x] Verify multi-core execution, no scheduler starvation under bounded blocking work, exact output/error behavior and small-worker configurations.

Rollback point: retain the existing synchronous `parallel_each` implementation until the common executor passes its focused compatibility cases.

### Phase E: Bounded Channels And Contract Integration

- [x] Add compiler-contextual `channel(Type, capacity)` and the runtime Channel resource.
- [x] Implement async `send`/`receive`, idempotent `close`, wakeup behavior and explicit source-defined result choices.
- [x] Extend effect, failure, dependency, redundancy, API snapshot and Markdown checks for every new signature/operation.
- [x] Add documentation only for implemented behavior and a runnable non-network producer/consumer example.
- [x] Run the available focused syntax/semantic/native/runtime/concurrency, reference-feature and whitespace checks; record unavailable rustfmt/Clippy components explicitly.

Final gate: inspect generated native code for concrete futures/direct static handlers and absence of generic Value/handler dispatch. Record passed, failed and unrun checks without claiming network readiness.

### Final Evidence

- `structured_concurrency`: 11 passed; syntax, formatter shape, affine ownership, transferability, effects/failures, handler boundaries and Channel contracts.
- `async_runtime`: 10 passed; recursive cleanup, cancel-and-wait, Group backpressure, bounded channels, shared blocking limits and async `parallel_each` peak concurrency.
- `async_native`: 6 passed; real native execution of await/Task/Group/Channel/parallel/blocking/timer, sync-entry isolation and nullable Channel flattening.
- Existing synchronous `concurrency`: 5 passed and its explicit benchmark remained ignored; the async example and nullable-result native regressions each passed their exact case.
- `dever-core` with the optional `reference` feature compiled offline. `git diff --check` and explicit trailing-whitespace scans passed. Dependency inspection confirmed Tokio uses only runtime, sync and time features in this phase.
- The installed Rust toolchain has no rustfmt or Clippy component, so those two checks were unavailable rather than passed. No full suite, build, service, network, old-performance baseline, global installation or commit was run.
- Independent final review found no remaining blocking defect after nullable Channel emission, synchronous collection-handler enforcement and transitive async effect boundaries were corrected.

### Deferred Follow-through

- [x] Design and implement AsyncStream/Tokio TCP on the shared runtime in N1. HTTP engine and WebSocket/SSE integration remain later phases.
- [ ] Measure the new packaged binary and live network behavior only after network integration; the user explicitly waived an old-runtime baseline.

## Active Performance Checklist (2026-09-08)

- [x] Capture bounded compiler, record/text/list/map/JSON/HTTP and concurrency baselines.
- [x] Memoize type properties and schedule effect/failure summaries by dependencies.
- [x] Eliminate redundant record field copies and consume unique list elements safely.
- [x] Optimize direct character-list consumption and eligible collection fusion without changing fault order.
- [x] Implement incremental HTTP framing and test in-memory fragments/limits/malformed inputs.
- [x] Cache bundled frontend work and native artifacts with complete invalidation and API checks.
- [x] Measure concurrent task granularity; batch only where justified and preserve bounded Stream consumption.
- [x] Run affected offline tests, native/reference regressions, strict lint, formatting and independent final review; record measurements and update owning docs.

User has authorized the complete implementation. This extends the active task and does not require another planning approval. Two bounded implement workers own analysis and cache respectively; main owns emission/runtime/protocols and benchmark integration. All permanent tests stay in root test/. Never run workspace-wide tests, existing services or global installation; use test-owned temporary inputs/processes only.

## Packaged Runtime Footprint Follow-through (2026-09-09)

- Composite runtime values and generated records/choices now append recursively through `Render::render_to`; named entry outputs reuse that same String instead of building a rendered value and then copying it into a second formatted String. Numeric `to_text` retains its direct `to_string()` path after the bounded hot-path gate exposed a slower generic formatting attempt.
- Native binding temporaries now transfer uniquely owned Text/Bytes storage and List/Map values through consuming operations. Live aliases retain copy-on-write behavior, and resource-containing values remain non-movable under the existing type-property contract.
- Exact-size finite parallel iterators create no more workers than items/batches. Unknown-length Stream dispatch keeps the caller-requested worker limit, bounded queue, cancellation and joined lifecycle.
- Production optimization arguments now use fat LTO and strip symbols; the workspace release runtime uses matching fat LTO/one-unit settings. The same allocation-performance fixture decreased from 4,680,864 to 452,352 bytes (90.3%) and measured 3,468 KiB peak RSS, versus 3,608 KiB without LTO.
- Thin LTO was rejected because ordered Map reached a 1.255 native/baseline ratio. Fat LTO passed all five gates: Int 0.713, Float 1.001, List 0.668, Decimal 0.985 and ordered Map 1.030. It trades substantially longer cold compilation for the user-approved packaged-runtime priority.
- An attempted fixed 64 KiB clamp for one-shot reads was rejected and removed: it changed a 1 MiB regular-file result to 64 KiB and failed the exact performance result. `read(limit)` remains caller-sized; low-memory programs use smaller limits or `chunks`.
- Focused runtime/concurrency/native ownership/core-native checks and both explicit performance scenarios cover the final contracts. No workspace-wide suite, service, external network, global installation or commit was used.

## Performance Evidence (2026-09-08)

- Nominal type properties are memoized after cycle validation. Facts, effects and failures share one dependency graph; summary changes reschedule callers instead of rescanning every function. The same 24-level shared-type debug regression went from 11.56 s to 0.47 s across the combined analysis/frontend changes.
- Native emission reuses checked HIR and last-use analysis. Safe record aliases are coalesced, scalar reads can be snapshotted before consuming large records, and exact field overwrites can move their old value. Unique Lists move elements; shared Lists clone only consumed elements. Resources retain their original lifetime and aliases retain value semantics. Empty Bytes concatenation reuses storage without weakening copy-on-write.
- Text splitting has one iterator implementation. Direct consumers avoid an intermediate List; eligible each/filter stages fuse only when handlers are proven free of observable effects and faults. A pure Int handler alone is insufficient because overflow can change fault order. Generated-code, alias, mutation and native/reference cases cover these boundaries.
- HTTP wire decoding now retains its header boundary and parsed frame between feeds. Transport uses that state; legacy one-shot scan delegates to the same implementation. Three new in-memory cases cover fragmentation, binary bodies, EOF, limits, duplicate headers and private/aliased state; five existing pure protocol tests passed. No live HTTP service was used.
- Bundled AST and the last complete frontend result are cached within a process. Native artifacts persist across processes, keyed by source-generated code, compiler/toolchain/runtime/dependency identities and native options. Atomic publication, fingerprint validation, isolated executable copies and create-new saves preserve correctness. All six cache tests and the explicit actual-CLI API-baseline regression passed. Independent CLI processes still check current sources and API baselines; no persistent HIR cache was introduced.
- Finite parallel inputs use bounded batches through the existing cancellation/join owner; Stream keeps bounded producer-driven pulls. With 32,768 inputs and four workers, median light-handler queue/batch/serial times were 140.945/3.894/0.247 ms; 512-round handlers measured 178.267/20.033/60.185 ms. Batching reduces dispatch cost but does not make tiny tasks preferable to serial execution.
- Main-agent final ownership/JSON/reference verification passed 9/7/10 tests respectively; only three network-dependent differential cases were skipped. Concurrency passed five functional cases, contracts passed 38 affected cases, and strict Clippy/type checks passed for core/runtime/CLI and affected test targets. Actual CLI library formatting and contracts/library checks passed. Independent final review found no blocking issue.
- After the final Bytes optimization, the main agent reran all three incremental HTTP cases and six default native-cache cases successfully. The actual-CLI cache test is ignored by default and had already passed when explicitly selected. Final targeted Rust formatting and new-file whitespace checks passed.

The same source was compiled before and after this change. Linux x86-64, Rust 1.98.0, CPU affinity 4; five alternating processes per side, native internal timing excluding compilation/startup. Text/List/JSON each retain five samples per process after discarding warmup; Map retains one per process. All expected results were asserted. Medians:

| Source workload | Before | After |
| --- | --- | --- |
| 16,384 record Text appends | 14.093 ms | 0.279 ms |
| 16,384 Text List elements | 4.786 ms | 1.342 ms |
| Long JSON string parsing | 28.072 ms | 6.977 ms |
| Record accumulation of 16,384 distinct Map keys | 548.061 ms | 1.999 ms |
| Process peak resident memory | 4,648 KiB | 3,604 KiB |

- Separate HTTP strategy measurement used identical 16,947-byte input in 64-byte fragments, both paths on the new compiler, seven alternating samples after warmup: repeated one-shot scan 4.541 ms, incremental decode 0.366 ms. This is parser work, not server throughput or an old/new compiler comparison.
- Same release CLI hello: cold native miss 0.84 s; subsequent independent-process hits 0.17/0.17/0.16 s, including source/API checks and runtime freshness. In-process frontend cold/hit measured 171.273/3.742 ms.
- The existing five-kernel matching-runtime gate passed again with 45 effective samples per side: native/baseline Int 0.989, Float 0.998, List 0.578, Decimal 0.979, ordered Map 1.006 (limit 1.15). This checks code-generation overhead against the same runtime, not against the old compiler. Build times were measured separately: native 2,137–2,693 ms, baseline 252–680 ms.
- These are bounded local workloads, not whole-language speed guarantees. README, IMPLEMENTATION and owning specs describe the optimization boundaries. No full suite, service integration, global installation or commit; the performance scope is complete while the umbrella language roadmap remains active.

## Active Follow-through Checklist (2026-09-07)

- [x] AST formatter, comments/precedence/idempotence, all-file preparation and CLI fmt/check.
- [x] Text/number library expansion and explicit argument/environment/clock primitives.
- [x] Opt-in reference evaluator, core semantics and native differential cases.
- [x] Last-use and reduction-state ownership optimization with alias/order regressions.
- [x] Bounded reproducible native performance baseline and evidence.
- [x] JSON/HTTP source packages and focused protocol fixtures.
- [x] Controlled concurrent execution with explicit limits/lifecycle and focused verification.
- [ ] Resolve database choice from pending user answer; implement or record explicit deferral.
- [x] Integrate delivered flows, review, update canonical specs and actual validation evidence.

Workers implement formatter and reference evaluator independently; the main agent owns libraries and integration. Checks are targeted, offline and bounded; no full suite, live service, global install, commit or cross-platform distribution. Later checklists are historical.

## Follow-through Evidence (2026-09-07)

- Implemented syntax-only AST formatting and CLI `fmt`/`--check`. Source snapshots are reloaded before staging and before replacement, comparing both the complete path set and file contents. Independent review found the initial missing path-set check; the shared source loader now closes that gap without a second traversal implementation. Per-file replacement remains atomic, not a multi-file OS transaction.
- Added Unicode Text operations, numeric parsing/rendering, process arguments/environment and explicit clocks/sleep through the existing intrinsic catalog and source-defined result choices. JSON and HTTP algorithms are ordinary `.dever` packages; no runtime JSON/HTTP parser or new dependency was added.
- JSON validates grammar, precise numeric spelling, duplicate keys, Unicode, graph references, nesting and expansion limits. HTTP supports a declared HTTP/1.1 single-message subset with binary bodies, Content-Length, strict ASCII headers and typed failures; independent peers check canonical request/response bytes, fragmentation, invalid requests/400, EOF and no-body statuses. Blocking read/write syscalls have explicit timeouts; no strict overall message deadline is claimed.
- `parallel_each` reuses static handlers/sequence classification and adds a bounded producer/worker owner with explicit worker limits, fault cancellation at pull boundaries and joined completion. Stream remains producer-owned; worker inputs/context cannot recursively contain Stream.
- Ordinary last-use moves and transferred reduction state preserve live aliases and output/field order. Resource-containing locals retain lexical lifetime; a differential socket regression verifies no premature peer EOF. Full-domain Int/Decimal guards omit redundant comparisons without altering bounded clauses.
- Passed 25 targeted core/library tests: `standard_library` 5, `native_ownership` 3, `concurrency` 4 and opt-in `differential` 13. `formatter` passed 11 tests including added/renamed source snapshots and all current official packages/examples. JSON passed 7 and HTTP passed 8 in worker validation; the main agent independently reran Unicode/exact-number JSON and the real HTTP server wire/400 case. Production CLI feature resolution excludes `reference`.
- Actual CLI `check`, `fmt`, `run` and `build` succeeded for `examples/library/src`; normal run and a standalone executable under `env -i PATH=/nonexistent` printed `json = {"message":"你好 Dever","total":6}` and `http_roundtrip = true`.
- Independent final review found no remaining blocker in formatter, reference, ownership, numeric domain optimization, concurrency and JSON after the snapshot fix. Main-agent HTTP review checked framing/status/timeout contracts. Database selection remains pending rather than implicitly chosen.
- Passed the explicitly ignored performance target on Linux x86-64, CPU affinity 4, Rust 1.98.0, cached runtime, optimization level 3 and one codegen unit. Each side used five alternating processes and 45 effective samples. Native/baseline median ratios: checked Int 0.952, Float 1.000, List each/filter/sum 0.207, Decimal 1.001, ordered Map 1.145; all met 1.15. Native build times were 1,206–1,793 ms, baseline compile times 255–668 ms, recorded separately from execution. These figures cover the selected kernels only.
- Decimal comparison was corrected to use identical lexical `0.0` representation with baseline parsing outside timing. Map diagnostics identified missed generic hash-table inlining under multiple codegen units; production and baseline now share one optimization-argument constant. This changes compiler optimization scope without changing hashing, panic, arithmetic or value semantics; it trades compilation parallelism for optimization consistency.
- After the final native compiler setting, all 13 differential cases passed again, including exact fault traces, signed zero, Map semantics and resource lifetime. Strict Clippy/type checks passed for core/runtime/CLI and every new test target; targeted rustfmt and explicit whitespace checks passed. The complete follow-through has 51 functional tests plus one five-kernel performance gate, not a full workspace test run.
- No full suite, existing service, external database, global toolchain/command change, Git commit or archive. Distribution/cross-platform remains deferred; self-hosting is still unshipped.

- Status: Approved core foundations and foundation extension implemented and verified on 2026-09-07
- Requirement source: `prd.md`
- Architecture source: `design.md`
- Execution mode: one tightly coupled task; no obsolete child-task dispatch

## Completed Foundation Delivery Rule (Historical)

Implement the foundation extension as one bounded delivery: `reduce`, `reduce_until`, Bytes traversal, `Stream<Item>`, restricted static handler inputs, file/TCP stream wrappers and focused verification. Reuse the existing collection checker, typed HIR, intrinsic catalog, native emitter and resource owner; do not build a parallel execution path. Compiler distribution, cross-platform work, formatter/evaluator work and full HTTP/JSON/CRUD/database packages remain deferred. The completed core and hello milestones are regression baselines; the old API and original phase checklists are historical.

## Foundation Extension Checklist (Authoritative, Complete)

### 1. Syntax And Semantic Contracts

- [x] Parse contextual `handler(inputs) (named_outputs)` only in function input declarations, using a dedicated AST input kind rather than an ordinary value type.
- [x] Parse `Stream<Item>` through the standard generic-type path; reject wrong arity, nullable handler signatures and handler syntax in fields, outputs or local/value positions.
- [x] Add semantic `HandlerSignature` and handler-parameter symbols separately from runtime `Type`; add opaque `Stream(Item)` to the type model with non-comparable, non-Map-key resource semantics.
- [x] Centralize handler compatibility, allowed reference positions and sequence element classification so direct calls, forwarding, `each`, `reduce` and `reduce_until` cannot drift.

### 2. Typed HIR And Static Specialization

- [x] Replace collection-only handler IDs with shared `HandlerTarget`, `CallTarget` and handler/value call arguments.
- [x] Lower direct handler calls and multi-level forwarding without storing handlers in value locals or aggregates.
- [x] Key native specializations by function plus ordered concrete handler bindings, substitute every forwarded target, and emit only direct calls.
- [x] Detect direct and indirect cycles in the substituted specialization graph before private Rust compilation.

### 3. Sequence Operations

- [x] Add `reduce` and `reduce_until` to the single standard-operation registry and HIR, with exact state and `stop: Bool` output contracts.
- [x] Lower List and Bytes traversal as left-to-right direct loops; emit Bytes elements as Int values without an intermediate List.
- [x] Extend `each` to Bytes and zero-output Stream consumption while preserving existing List mapping/action behavior.
- [x] Leave `filter`, `find` and `sum` List-only and reject implicit Stream collection.
- [x] Add idempotent zero-output `close(stream)` and reject it for non-Stream values.

### 4. Stream Runtime And Official Packages

- [x] Add one generic runtime Stream owner with shared cursor/end/close state and serialized pull/close operations; do not add prefetch, threads or async machinery.
- [x] Implement natural EOF, one terminal typed failure item, idempotent close, alias continuation after `reduce_until`, and release of only the Stream-held source alias.
- [x] Add `ReadEvent`, `ReadStreamResult` and `chunks` to bundled `dever.io`; add socket `chunks` and listener `connections` to bundled `dever.net`.
- [x] Extend fixed `dever.system` intrinsic signatures and generated typed adapters only for Stream creation/pulling; keep public result choices in `.dever` source and OS/resource mechanics in runtime.
- [x] Preserve existing one-shot read/write/accept/close contracts without compatibility aliases or behavior changes.

### 5. Focused Verification And Cleanup

- [x] Add parser/semantic negatives for handler placement, type/arity/output mismatch, forbidden storage/return/comparison/clause use and indirect recursion.
- [x] Add native List/Bytes cases for empty input, order, state typing, stop behavior, zero-output each and absence of intermediate Bytes allocation.
- [x] Add runtime/native Stream cases for shared aliases, natural EOF, early stop/resume, repeated close, source close, invalid limit and one terminal failure.
- [x] Compile official wrappers through the normal source loader and test file chunks plus loopback socket chunks/connections with owned resources, bounded data and timeouts.
- [x] Exercise actual CLI `check`, `run`, `build` and the resulting standalone executable with a chunked state-parser fixture whose behavior is defined in `.dever`.
- [x] Run focused formatting/type/lint/test checks only, inspect generated code/artifacts for direct handler calls, validate Trellis context and run whitespace checks.
- [x] Review the final diff for duplicate contracts, runtime handler dispatch, eager Stream collection, HTTP/JSON leakage and unrelated changes; update the canonical specs and implementation evidence.

Planned focused validation targets:

```bash
cargo test --offline -p dever-tests --test syntax_and_format <handler-or-stream-filter>
cargo test --offline -p dever-tests --test core_semantics <reduce-or-handler-filter>
cargo test --offline -p dever-tests --test core_native <sequence-or-specialization-filter>
cargo test --offline -p dever-tests --test runtime_foundations <stream-or-loopback-filter>
cargo test --offline -p dever-tests --test hello_native <regression-filter>
cargo check --offline -p dever-core -p dever-runtime -p dever-cli
python3 ./.trellis/scripts/task.py validate 09-04-dever-language-mvp
git diff --check
```

Filters will be replaced by the concrete test names introduced during implementation. No full suite, workspace-wide test, performance gate, service start, external network call or global command mutation is authorized. File tests use test-owned temporary directories; TCP tests bind only `127.0.0.1:0` and terminate within explicit bounds.

Rollback boundaries are syntax/semantic representation, HIR/specialization, finite sequences, runtime Stream and official wrappers. A boundary is not accepted while it relies on a runtime function value, duplicated checker rule, eager collection fallback or partially exposed public API.

## Foundation Extension Evidence (2026-09-07)

- Handler inputs use dedicated syntax, semantic symbols and HIR arguments. Clause coverage and runtime locals traverse value parameters only. One specialization graph resolves direct and multi-level handler forwarding, records real cross-package handler references and rejects substituted recursion; generated functions contain direct calls and no user-handler function values.
- `reduce`, `reduce_until` and `each` reuse one sequence classifier. List and Bytes lower to direct left-to-right loops, Stream pulls on demand, and `reduce_until` does not pull past the stopping item. Bytes traversal yields Int directly without constructing a List.
- Runtime `Stream<T>` shares cursor/end/close state across aliases and serializes pull/close. File/socket chunks validate creation, release their producer at EOF or after one terminal failure, and support early-stop continuation. Bundled `dever.io` and `dever.net` expose source-defined stream choices through the normal loader.
- Passed `cargo check --offline -p dever-core -p dever-runtime -p dever-cli` and strict Clippy for core/runtime/CLI plus the root test crate. `cargo fmt --all --check` passed.
- Passed 74 focused tests: 22 `syntax_and_format`, 16 `core_semantics`, 16 `core_native`, 8 `runtime_foundations` and 12 `hello_native`. File tests owned their temporary directories; TCP tests used `127.0.0.1:0`, explicit timeouts and bounded transfers.
- Final review made handler signatures reuse ordinary nested Map validation, preserved `Pull::Last` through mapped official streams, and added generated-code plus official creation-failure regressions. An independent read-only review then found no blocking correctness, safety or contract issue.
- Actual CLI `check`, `run` and `build` succeeded for `examples/stream/src`; run and the standalone executable both printed `total = 208`. The executable also ran with an empty environment and no Cargo/Rust path.
- No formatter/evaluator, async/concurrency system, HTTP/JSON/database package, external service, full workspace test, benchmark, distribution change, global command change or commit was included.

## Active Core Checklist

- [x] Record clause-only conditional dispatch as a permanent language rule and remove conditional blocks from the deferral list.
- [x] Retain settled repetition rules and remove the redundant planning blocker.
- [x] Record the user's approval and map the implementation boundary to core language and system interfaces.
- [x] Implement scalar/aggregate types, complete symbols, expressions, outputs and value-semantic assignments.
- [x] Implement clause type inference, normalization, overlap/exhaustiveness analysis and typed native dispatch.
- [x] Implement numeric and collection runtime operations plus shared intrinsic contracts.
- [x] Compile bundled official source and implement Bytes/file/TCP resource foundations.
- [x] Verify focused semantic/native/runtime cases and review the final cross-layer changes.

## Core Foundation Evidence (2026-09-07)

- Implemented six scalars, records/choices/nullable/List/Map/MapEntry, typed expressions, named outputs, stable assignments and value semantics through check/HIR/native execution. Added naming and unused-write diagnostics.
- Function groups infer one signature, use symbolic per-axis domains and rectangle subtraction to prove disjoint coverage, and lower into concrete native dispatch. No conditional blocks, source loops or user recursion were introduced.
- Official source in `library/` shares the same frontend; one intrinsic enum bridges the small system boundary. Decimal128 and ordered maps reuse pinned `dec`/`indexmap` runtime wrappers. Added Bytes, create-new/read-only files, TCP and shared-close opaque handles with explicit source-defined result choices.
- Passed 46 focused tests: 13 `core_semantics`, 9 `core_native`, 5 `runtime_foundations`, 12 existing `hello_native`, and 7 relevant `syntax_and_format` source/diagnostic regressions. Runtime tests used owned temporary files and loopback port 0 only. No full suite or integration environment was run.
- Passed targeted rustfmt and strict Clippy/type checks for compiler/runtime/CLI and affected test targets. Existing tool archives were invoked with command-scoped paths; a local ignored verification wrapper supplies the toolchain library path after Cargo sanitizes its environment. No global installation or environment changes.
- Actual CLI `run examples/core/src app.main` produced all 22 expected named outputs. `check` succeeded with nonexistent RUSTC. `build` produced `target/dever-core-example`, confirmed as a native Linux x86-64 ELF, and it produced the same outputs with `env -i PATH=/nonexistent`.
- Independent read-only review found and closed duplicate choice Map keys, Group-sensitive inference, null-only `first`, and Float signed-zero constant propagation. Added semantic/native regressions; Float literal contextual typing does not retype existing Decimal/Int locals.
- The initial wrapper/fixture errors and one stale numeric rejection fixture were corrected; final affected checks passed. Trellis context validation passed with implementation/check JSONL absent and skipped for inline work. Ordinary `git diff --check` passes but does not inspect the untracked compiler files; targeted formatting and explicit new-file whitespace checks cover those files.
- Formatter, reference evaluator/differential checks, performance gates, full HTTP/JSON/database packages, self-hosting and distribution/cross-compilation were not implemented or claimed by this delivery. No commit/archive; the wider language task remains active.

## Completed Hello Checklist

- [x] Correct the superseded API-first requirements and record the approved Text/action subset.
- [x] Add package/function resolution, semantic diagnostics and checked HIR.
- [x] Add standard output runtime and native compilation.
- [x] Add local check/build/run CLI and `examples/hello/src/app/user.dever`.
- [x] Run focused semantic/native/runtime tests and the actual hello command; announce local process execution first.
- [x] Review the final changes, update implementation evidence and record the session.

## Native Hello Evidence (2026-09-06)

- Implemented the approved Text/action subset, checked HIR, shared stdout runtime, native Rust generation and local `deverc` CLI. Example: `examples/hello/src/app/user.dever`.
- Passed 12 `hello_native` tests and the existing 20 `syntax_and_format` tests. After the final emitter readability change, reran all 12 native tests successfully.
- Passed rustfmt and strict Clippy for core/runtime, both targeted test binaries and the actual CLI binary.
- Actual CLI `check` succeeds even with a nonexistent RUSTC, proving it does not invoke compilation. `run` prints exactly `hello\n`; an unknown entry returns exit 1. `build` produced `target/hello-user`, identified as a native x86-64 ELF, which prints `hello\n` with PATH containing no Rust tools.
- Independent read-only reviewer found no blocking issue; the noted CLI verification gap was closed by the actual commands above. Tests exercise Unicode/escaping, source locations, value semantics and write failures, not a User/hello-specific execution branch.
- Added README run/build instructions and updated owning compiler specs. No global command replacement, external dependency, commit, service, database or load test.
- This bounded milestone is complete. The overall language roadmap remains open; HTTP/JSON/CRUD and persistence are deferred pending the user's next step.

Preserve unrelated changes and do not create a commit without explicit authorization. Verification for the new API must use its own local process and test storage, with functional checks and bounded performance checks described before execution. Do not test against, restart or benchmark existing services. No runtime or load check has been run during this planning turn.

## Historical API Replanning Checklist (Deferred)

- [x] Record the real user API goal and supersede the pure-computation-only boundary.
- [x] Keep `.dever` source and the existing surface syntax; defer the speculative contract-declaration system.
- [ ] Confirm persistence selection.
- [ ] Specify typed HTTP/JSON/storage boundaries and necessary effect/concurrency semantics.
- [ ] Finalize user API behavior and meaningful performance acceptance.
- [ ] Replace the next execution steps with a bounded native application delivery plan.
- [ ] Review the concrete plan with the user before implementation.

Proposed delivery order, to finalize after the open product decisions:

1. Implement name/type/clause/flow checking and HIR for the language features the API actually uses.
2. Establish concrete native business-function execution and source-located failures.
3. Add typed HTTP/JSON boundaries, explicit resource/effect semantics and the selected persistent database adapter.
4. Implement `examples/user-api` entirely in `.dever`, with real CRUD and configuration supplied by the runtime environment.
5. Run announced, isolated functional/persistence/concurrency checks and a bounded native performance comparison.

Complete Decimal/Float/Map libraries, the full reference evaluator, formatter, component distribution and AI indexing remain on the roadmap; they are not all prerequisites for the first user API. Preserve unsupported-source diagnostics rather than silently approximating unimplemented language behavior.

## Initial Frontend Evidence (2026-09-05)

- Implemented: dependency-free Rust workspace, source directory loading, UTF-8 and line/column handling, deterministic diagnostics, lexer, spanned AST and parser.
- Passed: 20 focused `syntax_and_format` tests, including the complete PRD package; Rust formatting check; focused Clippy with `-D warnings`; diff whitespace check.
- Independent review: fixed iterative expression-tree depth bypass and removed inaccurate terminal-width caret rendering. Follow-up review confirmed both fixes.
- Validation tools: rustc/Cargo 1.98.0 are installed but absent from the default PATH. Rustfmt and Clippy were missing, so their exact 1.98.0 official archives were downloaded to ignored `target/verification-tools/`, checked against the installed Rust manifest SHA-256 values and invoked directly with command-scoped library/tool paths. No global toolchain installation or command replacement occurred.
- At that checkpoint, native/differential/performance checks had not run. The Native Hello Evidence above records the subsequently implemented and verified native subset.
- The previous API-first next step was superseded by the approved hello-world milestone; formatter and other language work remain on the roadmap.

## Historical Original Roadmap Notice

The phase checklist below records the original repository bootstrap plan. Its unchecked boxes are not the current implementation status and do not expand the authoritative extension above. The Active Core Checklist and dated evidence describe completed work.

## Phase 1: Initialize The Repository

### 1.1 Preserve and classify current changes

- [x] Record `git status --short`; current changes contain planning and agent metadata, plus two deleted old documents.
- [x] Confirm no old Rust workspace, implementation or test files exist in the working tree or HEAD.
- [x] Preserve existing user changes; no old implementation needs removal.

### 1.2 Establish the new workspace

- [x] Create `crates/dever-core` as the single language library.
- [ ] Add `crates/dever-runtime` in Phase 7 when numeric/fault consumers exist; keep parser, checker, HIR and evaluator code out of it.
- [ ] Add `crates/dever-cli` in Phase 10 for the `dever` binary; do not expose syntax-only success as the public semantic `check` command.
- [x] Create `test/dever-tests` as the only permanent test crate.
- [x] Create root `Cargo.toml`; pin external dependencies when they are first consumed.
- [ ] Add a decimal128 dependency behind `dever-runtime::number` and reuse that wrapper from the evaluator.
- [ ] Add an insertion-ordered map dependency behind `dever-runtime::collections` and reuse that wrapper from the evaluator.
- [x] Do not add parser generators, async runtimes, plugin systems or CLI frameworks.

### 1.3 Establish current rules

- [x] Confirm the rejected prototype and its fixtures/tests are absent; do not add migration scaffolding.
- [x] Preserve existing deletion of superseded plan/spec documents.
- [x] State in `AGENTS.md` that `.dever` source is the trusted source.
- [x] Replace relevant Trellis spec placeholders with the actual frontend ownership and verification rules.

Rollback point: kickoff adds compiler files and updates canonical planning only; existing user deletions and unrelated metadata are preserved.

## Phase 2: Source, Diagnostics And Lexer

### 2.1 Source ownership

- [x] Implement `SourceId`, `SourceFile`, `SourceMap` and half-open `Span`.
- [x] Precompute line starts and render stable one-based line/column positions.
- [x] Load `.dever` paths in lexical order and reject invalid UTF-8.

### 2.2 Diagnostics

- [x] Implement the shared diagnostic model; semantic phases will consume it when implemented.
- [x] Support primary and related spans plus stable diagnostic codes.
- [x] Sort diagnostics deterministically.
- [x] Add renderer tests for the syntax-phase messages, related source files and Unicode locations.

### 2.3 Lexer

- [x] Tokenize identifiers, keywords, delimiters, operators, newlines and comments.
- [x] Preserve numeric spelling and comment trivia.
- [x] Decode Text escapes with precise bad-escape diagnostics.
- [x] Reject non-ASCII identifiers while allowing UTF-8 strings/comments.

Focused checks:

```bash
cargo test --offline -p dever-tests --test syntax_and_format
```

## Phase 3: Parser And Formatter

### 3.1 Syntax tree

- [x] Define focused AST nodes for package, type, function clauses, patterns, statements and expressions.
- [x] Keep every node spanned; do not attach resolved IDs or runtime values.
- [x] Preserve ordered comments with source spans; formatter attachment remains part of Phase 3.3.

### 3.2 Recursive-descent parser

- [x] Parse the package header and exposure list.
- [x] Parse record and choice type declarations.
- [x] Parse function signatures, clause patterns and linear bodies.
- [x] Parse record/List/Map values, calls, field access and operators with fixed precedence.
- [x] Recover only at bounded declaration/statement delimiters.
- [x] Reject excluded constructs rather than accepting compatibility syntax.

### 3.3 Canonical formatter

- [ ] Format every valid syntax node from structure.
- [ ] Preserve comments and normalize whitespace, blank lines and indentation.
- [ ] Add parse-format-parse and twice-format idempotence tests.
- [ ] Refuse to rewrite invalid files.

Focused check:

```bash
cargo test -p dever-tests --test syntax_and_format
```

Milestone boundary: the parser is currently an internal library for focused tests. Do not expose a public checked-source or formatting command until its complete contracts are implemented.

## Phase 4: Package And Name Resolution

- [ ] Register package/type/function headers before resolving bodies.
- [ ] Validate path/package equality and one-file ownership.
- [ ] Load only the supplied source root and fixed standard catalog; do not search external paths or perform network access.
- [ ] Reserve core single-segment package names and `dever.*` against local declarations.
- [ ] Validate `exposes` against locally declared names.
- [ ] Resolve local short names and cross-package fully qualified names.
- [ ] Enforce visibility without import tables.
- [ ] Group same-name/same-arity clauses and enforce contiguity.
- [ ] Reject package, recursive type and direct/indirect function cycles.
- [ ] Include standard core and qualified package names through one fixed catalog.
- [ ] Report unavailable future official/third-party references as unknown packages without an implicit install fallback.

Focused check:

```bash
cargo test -p dever-tests --test package_resolution
```

## Phase 5: Type Checking And Linear Flow

### 5.1 Type interning and declarations

- [ ] Implement scalar, record, choice, nullable, List, Map, MapEntry, Stream and multi-output types, with static handler inputs represented separately from values.
- [ ] Reject record/choice member mixing and recursive user types.
- [ ] Validate choice payload arity and record completeness.
- [ ] Validate Map key eligibility.

### 5.2 Expressions and calls

- [ ] Check literals using expected type without premature numeric conversion.
- [ ] Implement operator tables exactly as specified in the PRD.
- [ ] Permit only lossless Int-to-Decimal implicit promotion.
- [ ] Resolve positional function calls and named multi-output access.
- [ ] Restrict named handler references to sequence operations, direct static invocation and approved handler-input forwarding.

### 5.3 Definite assignment and mutation

- [ ] Track uninitialized/initialized locals in one forward pass.
- [ ] Reject reads and field writes before complete initialization.
- [ ] Require every output on every clause path.
- [ ] Keep a local's inferred type stable across reassignment.
- [ ] Report unused writes without adding defensive runtime behavior.

Focused check:

```bash
cargo test -p dever-tests --test type_checking
```

## Phase 6: Clause Analysis And Typed HIR

### 6.1 Normalize patterns

- [ ] Normalize Bool, null, choice, scalar constant and `other` patterns.
- [ ] Normalize Int/Decimal comparisons into open/closed intervals.
- [ ] Support at most one lower and one upper bound joined by `and`.
- [ ] Reject Float ranges and dynamic/function-based guards.

### 6.2 Prove clause validity

- [ ] Compute overlap, reachability and exhaustiveness over symbolic domains.
- [ ] Support products of multiple input domains without enumerating numeric values.
- [ ] Emit diagnostics that label both conflicting clauses or the uncovered domain.
- [ ] Prove that permuting source clauses leaves the normalized decision model unchanged.

### 6.3 Lower typed HIR

- [ ] Lower only fully checked packages.
- [ ] Replace names with resolved IDs and attach static types/spans.
- [ ] Build decision nodes from normalized clause domains.
- [ ] Keep HIR internal to `dever-core`; do not serialize it as a second source format.

Focused check:

```bash
cargo test -p dever-tests --test clause_analysis
```

Rollback point: do not add an ordered first-match fallback if coverage analysis is incomplete. Unsupported patterns must fail compilation.

## Phase 7: Values, Numbers And Reference Evaluator

### 7.1 Runtime values

- [ ] Implement the language-owned `Value` variants.
- [ ] Use shared immutable aggregate storage with copy-on-write updates.
- [ ] Wrap insertion-ordered Map behavior and use order-preserving removal.
- [ ] Implement deterministic value rendering and named output order.

### 7.2 Numeric layer

- [ ] Implement checked i64 arithmetic, including negative division/remainder edges.
- [ ] Wrap Decimal128 Context with round-half-even and per-operation status clearing/checks; accept inexact/rounded and fault on overflow, underflow, division by zero or invalid operation.
- [ ] Reject Decimal special values at the `DecimalValue` boundary.
- [ ] Implement exact Int-to-Decimal and explicit Int-to-Float conversion.
- [ ] Reject inexact Decimal literals while allowing specified inexact/rounded arithmetic results.
- [ ] Implement f64 arithmetic and IEEE classification functions without fast math.
- [ ] Add Decimal conformance and numeric fault fixtures.

### 7.3 Evaluator

- [ ] Execute HIR with explicit frames and no mutable global state.
- [ ] Select clauses through compiled decision nodes.
- [ ] Preserve source-level value semantics during local and field mutation.
- [ ] Return named outputs or an uncatchable `RuntimeFault` with source span.
- [ ] Add a Dever call trace only as diagnostic context.
- [ ] Keep evaluator entry points internal to compiler tests; production `build` and `run` must not depend on them.

Focused checks:

```bash
cargo test -p dever-tests --test value_semantics
cargo test -p dever-tests --test numbers
cargo test -p dever-tests --test failures runtime
```

## Phase 8: Standard Functions

- [ ] Define one static signature/evaluator catalog for core and qualified standard names.
- [ ] Implement `append`, `first`, `get`, `put`, `remove` and `entries`.
- [ ] Implement `each`, `reduce`, `reduce_until`, `filter`, `find` and `sum` with shared sequence/handler helpers.
- [ ] Enforce one optional context and element-first handler inputs.
- [ ] Preserve order and short-circuit `find`.
- [ ] Keep Float sum left-to-right.
- [ ] Implement minimum `text`, `decimal`, `float` and `math` functions used by the PRD.
- [ ] Do not add unrequested collection aliases, lazy Stream transforms or reduction names besides `reduce` and `reduce_until`.

Focused check:

```bash
cargo test -p dever-tests --test collections
cargo test -p dever-tests --test numbers standard_functions
```

## Phase 9: Native AOT Compilation

### 9.1 Whole-program specialization

- [ ] Start from one checked, exposed, zero-input entry function and collect its reachable HIR.
- [ ] Specialize every concrete record, choice, nullable, List, Map, Stream and static handler-binding combination.
- [ ] Lower clauses to deterministic native decision trees and collection functions to direct loops.
- [ ] Perform last-use/liveness analysis so dead sources move instead of clone while observable value semantics remain unchanged.
- [ ] Reject any native path that would require a generic runtime `Value`, reflective lookup or virtual per-element dispatch.

### 9.2 Private Rust emission

- [ ] Emit concrete Rust types and direct functions from typed HIR through one structured writer with centralized identifier/literal escaping.
- [ ] Emit explicit checked numeric and source-located runtime-fault operations.
- [ ] Make generated programs depend only on `dever-runtime`, never `dever-core` or the evaluator.
- [ ] Keep generated Rust and Cargo metadata in a compiler-owned transient directory with no public emit command.
- [ ] Compile with an optimized release profile starting at opt-level 3, thin LTO and one codegen unit.
- [ ] Translate toolchain failures into stable compiler diagnostics without leaking generated identifiers as Dever names.

### 9.3 Native conformance and performance

- [ ] Differentially compare evaluator and native outputs/fault categories across the language conformance corpus.
- [ ] Inspect native artifacts to prove the evaluator is not linked.
- [ ] Benchmark scalar arithmetic and List `each`/`filter`/`sum` against equivalent safe Rust using the same algorithm, data layout and toolchain.
- [ ] Benchmark Decimal and ordered Map separately against Rust baselines using the same underlying semantic wrappers.
- [ ] Fail the performance gate when representative median native hot-path time exceeds 1.15 times the matching Rust baseline.

Focused checks:

```bash
cargo test -p dever-tests --test native
cargo test -p dever-tests --test differential
cargo test -p dever-tests --test performance -- --ignored
```

Rollback point: the evaluator remains the semantic oracle, but an incomplete native backend cannot silently become the production `run` path. Unsupported lowering is a compile diagnostic.

## Phase 10: CLI And End-To-End Example

### 10.1 CLI

- [ ] Implement `check`, `fmt`, `build` and `run` as thin adapters.
- [ ] Do not add registry, `add`, `update`, signature or component-cache commands in Core 0.1.
- [ ] Validate exact argument counts and stable exit codes.
- [ ] Keep diagnostics on stderr and results on stdout.
- [ ] Make `fmt` all-or-nothing when any source file is invalid.
- [ ] Restrict `build` and `run` to exposed zero-input functions.
- [ ] Make `build` write an optimized native executable to `--output` and make `run` compile/execute the same native path.
- [ ] Keep the Cargo binary and canonical command name as `dever`; do not rename compiler internals or public command semantics to `deverc`.
- [ ] Treat `deverc`, when explicitly installed for coexistence, as a thin link or launcher to the same compiler binary with no divergent behavior.
- [ ] Do not replace the existing framework `dever`, modify system `PATH` or create global executables during repository implementation or automated tests.
- [ ] If global `deverc` setup is later authorized, resolve and record both command targets and verify that the existing global `dever` remains unchanged.

### 10.2 Example and integration

- [ ] Add the canonical account/user package from the PRD.
- [ ] Add a zero-input executable example covering types, clauses, collections and numbers.
- [ ] Verify multiple source files and full qualification.
- [ ] Verify the zero-input example produces identical evaluator and native results.
- [ ] Ensure no example source depends on Rust, JSON graph, React or a hidden host fallback.

Focused check:

```bash
cargo test -p dever-tests --test cli
```

## Phase 11: Cleanup And Final Verification

### 11.1 Structural review

- [ ] Search for duplicate operator, standard-function, diagnostic and type rules.
- [ ] Keep each language rule owned by one table/module.
- [ ] Remove obsolete compatibility code, commented code, placeholders and old architecture references.
- [ ] Confirm the CLI contains no language semantics.
- [ ] Confirm no public Rust API leaks into `.dever` diagnostics or docs.
- [ ] Confirm generated binaries depend on the minimal runtime but not compiler/evaluator modules.
- [ ] Confirm private generated Rust is not documented or tested as a public compatibility format.

### 11.2 Acceptance mapping

- [ ] Map all 20 PRD acceptance criteria to named test cases.
- [ ] Run each relevant focused test target.
- [ ] Run static formatting and diff checks.
- [ ] Record passed, failed, skipped and not-run checks separately.

Permitted final checks, announced before execution:

```bash
cargo fmt --check
cargo check -p dever-core -p dever-runtime -p dever-cli
cargo test -p dever-tests --test syntax_and_format
cargo test -p dever-tests --test package_resolution
cargo test -p dever-tests --test type_checking
cargo test -p dever-tests --test clause_analysis
cargo test -p dever-tests --test value_semantics
cargo test -p dever-tests --test collections
cargo test -p dever-tests --test numbers
cargo test -p dever-tests --test failures
cargo test -p dever-tests --test native
cargo test -p dever-tests --test differential
cargo test -p dever-tests --test cli
python3 ./.trellis/scripts/task.py validate 09-04-dever-language-mvp
git diff --check
```

The performance test is an explicit, separately announced gate and is not part of the default test list. Do not substitute `cargo test --workspace`, `npm run build` or service/integration tests. If a focused check reveals a shared compiler defect, fix the owning implementation once and rerun only affected targets.

## Finalization

- [ ] Run `trellis-check` after implementation.
- [ ] Update canonical Trellis specs for the new crate ownership and language rules.
- [ ] Review the final diff against PRD scope and remove accidental abstractions.
- [ ] Do not commit until the user explicitly authorizes a commit.
- [x] Confirm there are no obsolete planning children to archive.
# Active Compiler Contract Checklist (2026-09-08)

- [x] Contract syntax, formatter and semantic metadata.
- [x] Field privacy, numeric facts, construction/write/output constraints.
- [x] Failure obligations and explicit recovery, including nested values and static handlers.
- [x] Transitive effects, package dependencies and API snapshot/CLI enforcement.
- [x] Evidence-backed redundancy diagnostics and application export analysis.
- [x] Official source/examples migration, targeted tests, native equivalence, docs/specs and final review.

User approved the full contract implementation. Complete the list before claiming delivery. Keep earlier evidence below as historical; no full test suite, external services, distribution work or commit.

## Compiler Contract Evidence (2026-09-08)

- Implemented private record fields, normalized Int/Decimal field/payload/output/handler bounds, explicit error/recover and pure/depends. HIR facts reuse existing clause domains; handler boundaries support safe variance and concrete output inference without forcing wrapper annotations.
- Failure obligations follow copies, nested fields, calls and collections. Official Failed variants are explicitly marked; HTTP transport preserves earlier failure state and returns the inspected result with its stop decision. The Stream example now propagates a CountResult failure instead of silently returning zero. Presentation/test recovery reasons are explicit and consistent across clauses.
- Effects and recovery policies propagate through invoked static handlers. API generation is create-new; check/run/build compare an existing baseline. Adding a private recovering helper changes the effective public snapshot without exposing that helper's name. Depends includes normalized system resource types. W001 is reused; W002–W005 and entry-scoped W007 are advisory, not arbitrary abstraction requirements.
- Final six contract targets passed 47 tests: syntax 4, facts 16, semantics 10, API 9, redundancy 6, execution 2. Existing core semantics passed 16 (15 in the first run, W001 assertion narrowed to its diagnostic then rerun); existing syntax passed 22, formatter passed 11 and the new contracts example roundtrip passed. The independent expected-output core differential case passed. Forty-one migrated source fixtures were checked without executing their network/resource scenarios; the temporary probe was removed.
- Actual CLI api/check/run/build and standalone execution with `env -i PATH=/nonexistent` passed. A temporary source's new exposure was rejected by its unchanged baseline. `examples/contracts/src/dever.api` records the example's contract. Strict installed-driver Clippy/type checks passed for core/CLI and focused contract tests; formatting and explicit whitespace checks cover untracked files as well.
- Independent review found and closed empty-reduce seed consumption, primitive resource dependency omission, and exponential/deep proof graphs. Added positive/negative regressions; immutable shared fact/value graphs preserve aliases, proof work is capped at 16,384 and iterative nominal depth validation caps 128 before recursive analysis. Exhaustion reports C015; no incomplete proof becomes a successful Program.
- Updated LANGUAGE.md, IMPLEMENTATION.md, README and the owning compiler-contract spec. No database/distribution expansion, full suite, integration environment, existing service, global installation or Git commit. The umbrella task stays active for its previously deferred roadmap; this approved contract extension is complete.

## Direct Example Roots And Coverage (2026-09-08)

- Moved all five original source roots from `examples/<name>/src` to `examples/<name>`, including the unchanged contracts API baseline. Removed empty src directories; package-relative nesting, such as hello/app/user.dever, remains. Updated executable test references, current guides and owning specs; earlier dated evidence retains its historical paths.
- Added sequences, text, privacy, parallel and system examples, bringing runnable roots to ten. Added six separately checkable rejected roots for C006/C009/C011/C012/C013/C014. The examples index documents commands, outputs, nondeterministic ordering, runtime input paths and explicit recovery boundaries.
- Reused compiler/check/native APIs in one example runner. Automatic discovery checks every runnable root and any baseline; formatter coverage discovers all examples, including rejection fixtures. No compiler or runtime behavior changed.
- Passed seven example-target tests covering all ten successful roots, nine native executions and six rejection cases; the existing core native test separately verified its 22 expected outputs. The targeted formatter roundtrip passed over every official/example source. Actual CLI contract check and expected C012 rejection passed; example fmt --check passed. Strict Clippy/type checks compiled all affected test targets, including feature-gated differential references.
- No full suite, network/service integration, compiler feature expansion, global installation, commit or archive.

## Isolated PostgreSQL And Dual-source CMS Measurements (2026-09-18)

- In a nonprivileged, owned PostgreSQL 16 instance with a private Unix socket and no TCP listener, the ignored `postgres_orm_uses_the_configured_isolated_database` test passed 1/1. Its shared schema cleanup left no test schemas. Added a focused live named-migration probe for bound before/after SQL, atomic rollback on after failure, successful DDL and once-only re-execution. Test connection came only from a temporary root `config/setting.json`; no database URL environment variable or existing PostgreSQL service was used. After verification, the temporary setting was deleted, the owned instance stopped and its test data removed.
- Focused `cms-profiles` build compiles both current `examples/cms/{dever,md}` source formats using one shared benchmark entry and separately builds base/SQLite/PostgreSQL/both runtime profiles. Artifacts: `target/performance/cms-dual-build-09-18-v2/manifest.json` and `profile-report.json`. Profile bytes: 411,352 / 2,558,200 / 2,708,112 / 4,672,888. CMS Dever/Markdown bytes: 3,151,352 / 3,151,416.
- `target/performance/cms-dual-rss-09-18/report.json` completed all six runs (three per source, READY=3 throughout). Dever/Markdown median readiness 19.87/19.69 ms, idle RSS 5.37/5.37 MiB, PSS 3.28/3.33 MiB, VmHWM 5.37/5.37 MiB. This is warm-cache initialization and bounded idle sampling, not throughput. No hard 64/128 MiB CMS budget or cgroup peak: the local controller is not writable. No broad/full suite, existing service check, or global installation.
- Benchmark runner unit tests pass 40/40; focused test compiled with `--features postgres`; Trellis task context validation passed. Rustfmt and Clippy are unavailable in the installed toolchain. This root roadmap remains in progress; API stays deferred pending its redesign, and hard-budget CMS stress requires a delegated cgroup environment.

# Source Visibility And Module API Implementation (2026-09-18)

- Replaced `package/exposes` syntax with path-derived package identity and explicit `public` for cross-package type/function visibility; fixed private `main.main` as the only run/build entry. Updated bundled sources, CMS Dever/Markdown roots, old runnable examples and Markdown metadata. The hello example now delegates through a public library function to its own `main.dever`.
- Added statically checked module `api/` routes with GET/POST/DELETE naming, recursive paths, private handlers and one `response` output. `dever.api.serve()` opts the application into routing and the existing bounded HTTP engine; typed inputs, JSON serialization and `{code,message,data}` envelopes reuse a single runtime adapter. Listener settings are read from `config/setting.json`, not environment variables; no CMS business API or authentication policy was added.
- Focused checks: core/CLI/runtime/tests cargo check passed; `source_visibility` 10/10, `syntax_and_format` 22/22, `core_semantics` 16/16, `formatter` 11/11 and `markdown_source` 20/20 passed. `api_routes` passed 7/7 before the final test adjustment, then its application-mode native compilation passed 1/1 with config bootstrap and no listener. An isolated temporary-project CLI `check/run/build` and API-baseline rejection test passed. Maintained examples, CMS Dever/Markdown source parity and rejection fixtures passed selected contract tests. Two ORM fixture static-check tests passed without database connections. `deverc check` passed for hello and both CMS formats (CMS retains W001 unused-write warnings).
- Not run: full suite, live HTTP/API service check, real PostgreSQL, CMS runtime/build or benchmarks. Rustfmt and Clippy binaries are unavailable in the installed toolchain. The umbrella task remains in progress for separately deferred roadmap work; historical entries above retain their original date/context.

# AI-Oriented Source Architecture Implementation (2026-09-19, Complete)

- [x] Add one shared source-role classifier and project-layout validation.
- [x] Replace application `public` visibility with automatic App capabilities and logical App aliases.
- [x] Enforce role dependency direction, API non-callability and Model-operation ownership.
- [x] Migrate the Dever and Markdown CMS sources from `service/` to component/domain roles without function-per-file fragmentation.
- [x] Update the language, Markdown and backend compiler/database contracts.
- [x] Add focused layout, visibility, API, Model and CMS parity tests; run only the affected checks.

The implementation does not add environment variables, implicit HTTP startup, authentication policy, a new Port declaration syntax, or backward compatibility for the superseded application layout.

## AI-Oriented Source Architecture Evidence (2026-09-19)

- `SourceLayout` is the single main/component/domain/role/topic classifier. Strict project loading rejects loose application paths, generic buckets, role file/directory conflicts, nested non-API topics and one-file role directories. The check cache includes strict-layout identity; in-memory compiler fragments remain isolated from project-layout enforcement and cannot weaken CLI projects.
- App declarations receive stable `<component>.<domain>.<name>` identities without exposing topic filenames. Role-aware lookup enforces Main/API/App/private-role direction, API non-callability and owning-domain Model operations. App signatures and inferred failures reject private type leakage. Port remains a reserved, rejected declaration role.
- Plain and Markdown CMS now contain only `main`, `user/account/{app,domain,model}` and `news/article/{app,model}`. Both forms have equal normalized declarations, Model/API snapshots and settings. Current examples contain no package/exposes/public syntax, old service/model roots or environment-variable configuration.
- The current performance packager now stages `benchmark/<domain>/app.dever` plus generated `main.dever`; it no longer generates package/exposes headers or passes arbitrary entries to `deverc build`. ORM, profile, Dever CMS and Markdown CMS staged projects all passed actual CLI check. The CLI CMS regression test compiles against the fixed main-only interface and uses a test-owned App probe instead of injecting a loose root source.
- Focused verification passed: core/runtime/CLI Cargo check; `source_architecture` 8/8; `source_visibility` 10/10; `orm_performance_fixture` 3/3; `concurrency` 5/5 with its explicit benchmark ignored; CMS source/config parity 1/1; archived parallel and source-root native regressions passed after the synchronous typed-error fix. Earlier in this same phase `api_routes` passed 7/7. Actual CLI `check` and `fmt --check` passed for both CMS formats, and Python syntax checking passed for the performance builder.
- Not run: full workspace suite, live CMS `run/build`, `cms_project` integration execution, HTTP listener, real PostgreSQL, performance/size/RSS builds or 64/128 MiB stress. Rustfmt and Clippy components remain unavailable. The umbrella language roadmap stays in progress for separately deferred work.

# 执行记录

## 实现

- [x] 用户批准方案后实施，复用 Hyper 1.11.1；启用 http2 feature，锁定 h2 0.4.19 和四个传递依赖，不引入第二套协议栈。
- [x] 官方 Limits.http2/Http2Limits、精确记录检查和仓库调用者一起迁移；原生桥接复用 Nullable/Record 转换，无新增 intrinsic。
- [x] protocol/message 集中拥有流控预算、ALPN 派生、authority/Host 和方向相关的头校验。
- [x] buffered/live 服务端和 single-use/pooled 客户端接通；流式上传、下载、SSE、GOAWAY 和取消沿用现有 API。
- [x] ScopedOwner/ScopedSpawner 接入已有 Task Scope、全局容量和 blocking 清理；每流独立 Group，物理许可持有到协议后代清理完成。
- [x] 按工作流完成服务端/客户端实现分工及独立并发、全范围检查；主代理复查关键所有权和运行证据。
- [x] 更新现有 LANGUAGE、IMPLEMENTATION、README、toolchain-and-library 和性能说明；P1/P2 报告仍保留为 HTTP/1 数据。
- [x] 最终定向验证与检查闭环。
- [x] 完成任务状态、会话记录和自有临时目录清理，不提交。

## 复现及修正

- Hyper Executor 不携带 Dever CURRENT_SCOPE：协议 future 改由连接 owner 创建子 Scope；关闭 admission 与登记使用同一把锁。故障报告/注销先于完成信号；不可取消 supervisor 排空后代，等待者被取消也不能遗失 completion。
- 池握手的连接 owner 必须在持久 driver 首次 poll 时创建，不能归临时请求 Scope。
- RST/正文 idle 超时使背压 write 同时失败：先记录传输取消，避免把该流结束当成全局 handler 故障；真正程序错误仍保留原消息。发布失败时先释放 state 锁再析构响应体，避免取消标记重入锁。
- GOAWAY 后 Hyper dispatch ready 不等于能建新流：仅 TrySendError 结构化返回的未发送请求可在原期限内重排；先释放旧上传/流租约，不重放已发送请求。
- HTTP/2 Response TE 必须显式拒绝，不能交给 Hyper 静默删除；请求 TE 只接受 trailers。
- 测试修正：Dever 子任务用 await；静默 h2 peer 可能先收到 SETTINGS，需读到 EOF；独立 h2 的 FlowControl/RecvStream/SendStream 在等 driver 结束前释放。共享原生 fixture 的 HTTP/1 服务端保留三条连接容量，两条供池内嵌套代理，一条供独立 WSS；HTTP/2 客户端仍只允许一条物理连接。

## 验证约束

使用 PATH 指向已有 stable 工具链、CARGO_TARGET_DIR=target/native-runtime、TMPDIR=/dev/shm/dever-http2-native.fW3PNS，Cargo 均 --offline --locked --release。一轮 fetch 后不再联网。仅自有随机 loopback 端口和 test/ 下测试证书，不运行全量测试、既有服务、性能矩阵或全局安装。

首轮 CLI release 构建和 backend 示例 check 通过。官方库 fmt-check 仅报告原有 net.dever 格式差异；修改的 http.dever 无差异。Rustfmt/Clippy 未安装，不作为通过项。

低磁盘处理仅将可再生 target/debug/{incremental,deps,build} 暂存到 /dev/shm/dever-http2-cache.wOoaI9；现有 debug/deverc、源码和 target/performance 报告保留。最后统一记录验证结果与缓存清理。

## 最终测试证据

88 个不同定向用例全部通过（修复后重跑不重复计数）：http2_client 6、http2_server 6、http_engine 12、http_library 7、live_network 16、pooled_network 14、pooled_library 6、async_runtime 10、structured_concurrency 11。

- 独立 h2 对端证明一条物理连接的并发请求、流式上传/下载、RST 前后隔离、背压写取消、WINDOW_UPDATE 恢复、流上限与活跃正文不会被 idle 回收、GOAWAY 的物理预算与连接替换、TLS ALPN 成功/拒绝、Host/target、413 与原始 handler 故障。
- 原生 Dever 程序证明 single-use HTTP/2、TLS 同池嵌套代理（客户端只有一个物理名额）、上传、SSE 和 handler 内 run/await。共享 HTTP/1 变体另覆盖 WSS 与优雅停机；测试服务端三连接容量的修正经实际运行确认。
- 最新响应发布取消/锁序改动已重新编译，并由 http2_server 6 项和 live_network 16 项覆盖；已有结构化任务、HTTP/1 连接池与源码语义检查通过。
- 源码空白/冲突标记扫描、性能构建器 Python 语法和任务 context JSONL 验证通过。Rustfmt/Clippy 未安装；未运行性能矩阵、全量测试或既有服务验证。

复现上述范围：

```bash
cargo test --offline --locked --release -p dever-tests --test http2_client --test http2_server --test http_engine --test http_library --test live_network --test pooled_network --test pooled_library --test async_runtime --test structured_concurrency
```

最终 CLI release 构建通过，产物为 `target/native-runtime/release/deverc`；backend 示例的实际 CLI check/fmt-check 均无输出并退出 0。迁移后的 network_bench example 及启用 reference 的 core/runtime 离线 release check 通过。未替换全局命令。

独立审查与主代理最终所有权、调用链、规范和改动范围核对闭环，无剩余生产阻断。清理专属 shm 构建目录，并删除暂存的约 378 MiB 可重建 debug 中间缓存；源码、现有编译器和全部性能报告保留。按用户约束手动更新状态/日志，不提交、不调用自动提交归档流程。N5 完成，HTTP/2 性能测量、RFC 8441、数据库与发行等仍在后续路线图。

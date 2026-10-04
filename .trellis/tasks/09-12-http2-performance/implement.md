# 执行计划

## 1. 任务启动

- [x] 用户审核本 PRD、设计和执行计划后，运行 `task.py start`；启动前不改产品或基准源码。
- [x] 使用 `trellis-before-dev` 重新加载 backend 规范，核对工作区状态和已有性能产物，不清理用户文件。

## 2. 基准协议模型

- [x] 在 `network_bench` 中加入 `h1|h2` 与 `ConnectionShape` 参数校验；H1 限制每连接一槽，H2 限制复用且乘积有界。
- [x] 保留一个速率调度和 metrics 路径；把 H1 独占连接与 H2 共享 sender 封装为两个小的协议执行分支，不引入通用 trait 层级。
- [x] H2 计时前建立固定数量物理连接，按连接分配 sender clone，关闭时排空/终止 driver；不重试、不自动扩容。
- [x] 在结果中输出并校验协议、shape 和握手计数，继续保证全部请求计数守恒和完整正文校验。

## 3. 三类服务实现

- [x] 扩展 `ServiceKind` 和 TLS ALPN 派生，新增 runtime/raw Hyper 的 h2c 与 TLS+h2 模式。
- [x] direct runtime 与 raw Hyper 使用相同的 streams、窗口、header/frame/send-buffer 约束；复用响应路由、accept 限额和关闭流程。
- [x] 重构 Dever HTTP fixture，让四个入口共享 serve/route/lifecycle，只在入口选择协议和 TLS。
- [x] 扩展构建配置与 manifest，生成并指纹化 `http2`、`https2`，保留 H1 产物。

## 4. 运行器与摘要

- [x] 增加 `http2`、`http2-recovery` suite、物理连接档位、实现筛选及恢复参数，并按 manifest 校验 capacity、streams 和窗口。
- [x] HTTP/2 吞吐 case 记录 warmup、正式负载、断开后样本、服务/客户端资源和 cgroup 状态；case 标识包含完整维度。
- [x] 恢复 case 在一个服务进程内运行多轮固定 shape，逐轮校验请求和握手并记录 FD/RSS 回落。
- [x] 从逐 case 结果生成分组中位数 summary；失败时仍保存已完成 case 和可诊断日志。

## 5. 定向验证

- [x] 运行 `python3 -m unittest discover -s test/performance -p 'test_runner.py' -v`，覆盖参数、计数、summary、进程回收和错误路径。
- [x] 离线 release 构建 `network_bench`，运行现有独立 Python H1 对端测试，确认一次性 CLI 迁移没有改变 H1 测量语义。
- [x] 运行 H2 loopback smoke，覆盖 1 条物理连接、至少 16 个请求槽、明文/TLS、`/plain`/`/bytes`、三类服务模式和错误正文拒绝。
- [x] 用新的输出目录构建 Dever 性能夹具，核对 manifest 指纹、HTTP/2 limits 和四个 HTTP 入口。
- [x] 运行最小 `http2`/`http2-recovery` runner smoke，检查失败报告、FD 回落和原始计数完整性。

## 6. 有界正式测量

- [x] 先确认可用 CPU affinity、cgroup v2 cpu/memory 控制器、磁盘和已有服务边界，再说明将执行的有界命令。
- [x] 运行 128 MiB `/plain` 矩阵：5k/15k/30k，1x16/4x16，三实现、h2c/TLS、3 repeats。
- [x] 运行 128 MiB `/bytes` 矩阵：100/500/1000，1x16/4x16，三实现、h2c/TLS、3 repeats。
- [x] 运行 128 MiB 10 轮恢复矩阵，并运行 64 MiB Dever 1x16 明文/TLS 活跃场景。
- [x] 核对全部报告 status、errors、dropped、OOM、FD、RSS/PSS、CPU 和摘要分组；不以单次异常样本下结论。

## 7. 收尾审查

- [x] 更新 `test/performance/README.md` 和 `.trellis/spec/backend/quality-guidelines.md` 的 CLI、矩阵、实测结果和边界说明。
- [x] 检查最终改动是否仍有重复协议分支、混合职责或无必要依赖；确认未修改生产运行时、旧报告和用户文件。
- [x] 运行任务 JSONL 校验、Python 语法检查及受影响的最小回归；不运行全量测试。
- [x] 记录实际通过、失败、跳过和不可用项；完成 Trellis 检查与会话记录，不提交 Git。

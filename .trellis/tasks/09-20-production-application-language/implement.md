# Dever 生产级大型应用语言支持执行计划

## Start Gate

- [x] 用户审阅并明确批准本 PRD、设计和执行顺序。
- [x] 父任务保持 planning；不把父任务作为实现目标。
- [x] 依次完善并启动拥有当前交付物的子任务。
- [x] 开始每个子任务前读取 `trellis-before-dev`、子任务文档和 backend specs。
- [x] 保留现有脏工作区和历史任务，不提交、不归档、不清理用户改动。

## Child Order

### 1. secure-data-contracts

- [x] Model private 字段的领域所有权和公共边界。
- [x] Secret 类型、批准 sink、泄漏拒绝。
- [x] secure random、Argon2id、SHA-256/HMAC、常量时间验证。
- [x] 删除应用 `dever.process.environment`，迁移仓库调用方和文档。
- [x] 定向安全、Model、API/log 边界测试。

### 2. time-typed-codec

- [x] UTC DateTime/Date/Time/Duration API 和无溢出运算。
- [x] 共享静态 wire schema 与具体编码/解码生成。
- [x] 严格 JSON、日期格式、大小/深度和私密/Secret 拒绝。
- [x] 迁移当前 API response codec，保持 envelope。
- [x] 定向 codec、API、Markdown、ORM 表示回归。

### 3. port-adapter

- [x] Port bodyless contract、明确 failure contract 和 formatter/Markdown 文档。
- [x] Adapter/test fake 实现检查与静态 dispatch。
- [x] 收紧角色调用矩阵和类型可见性。
- [x] setting.json 多实现绑定、Adapter-local typed setting 和 Secret 字段。
- [x] 单实现、多实现、错误绑定、fake 隔离和 effect/failure 回归。

### 4. durable-jobs

- [x] Job role、声明、目标引用、payload schema 和 effect 分析。
- [x] SQLite/PostgreSQL 私有 schema、原子入队、claim/lease/retry/dead-letter。
- [x] immediate、run-at、UTC recurring schedule 和 schema drift 行为。
- [x] `dever.job.serve()`、runtime mode、signal/drain 和数据库关闭顺序。
- [x] 应用测试隔离、SQLite 端到端和 PostgreSQL 定向编译/可选隔离验收。

### 5. production-api

- [ ] POST record/query scalar decode 与共享 codec 接入。
- [ ] 注入 Context、Cookie/Header/request metadata。
- [ ] 标准 API Error 映射和未映射失败脱敏。
- [ ] 受限上传、临时文件生命周期和 storage Port 路径。
- [ ] 路由、输入、错误、上传取消和 HTTP drain 回归。

### 6. large-cms-acceptance

- [ ] 扩展 plain/Markdown CMS 领域和等价文档。
- [ ] 实现账户、会话、权限、内容、修订、分类、媒体、发布和审计业务。
- [ ] 接入 secure values、Port/Adapter、Job、API 和 runtime all mode。
- [ ] 增加等价应用测试和无外部服务 fake。
- [ ] 执行最终定向 check/test；任何 run/build 或持久服务验证先向用户说明。

## Cross-child Gates

- [ ] 每个阶段的公共合同只有一个 owner；JSON、配置、Job payload 不复制类型转换。
- [ ] 新增 Intrinsic 时 checker、effects、native emitter、reference policy 和官方 package 同步穷尽。
- [ ] `.dever` 与 `.dever.md` 使用同一 AST/check/HIR，不增加 Markdown 专用语义。
- [ ] 所有长期测试位于仓库根 `test/`，一次性 fixture 在交付前删除。
- [ ] 任何配置只来自 `config/setting.json`；搜索确认无应用环境变量兼容分支。
- [ ] 不添加 front、分发、外部队列、动态插件或内置 RBAC。

## Focused Validation Policy

每个子任务在自己的 `implement.md` 中列出具体命令。共同允许：

```bash
cargo check --offline -p dever-core -p dever-runtime -p dever-cli
cargo test --offline -p dever-tests --test <affected-target>
git diff --check
python3 ./.trellis/scripts/task.py validate <child-task>
```

如 rustfmt/clippy 组件存在，再运行受影响 package/target 的检查；组件缺失时如实记录。默认不运行 workspace 全量测试、性能套件、真实 PostgreSQL、front build、全量应用 build 或持久服务。

## Parent Completion Gate

- [ ] 六个子任务分别完成质量检查、规范同步和归档。
- [ ] 父任务重新核对所有 Requirement 与跨子任务数据流。
- [ ] CMS plain/Markdown 的 schema/API/Job/Port/settings/test 快照一致。
- [ ] 搜索确认不存在旧环境变量入口、App 直调 Adapter、公开私密字段或无界后台任务。
- [ ] 汇总实际通过、跳过和受阻的验证，不把定向测试描述成全量证明。
- [ ] 未经用户明确授权不创建 Git commit。

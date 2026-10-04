# 执行记录

用户已授权实施；API 明确延期。沿用上一轮审查已验证的缺口。

## 顺序

- [x] Model 字面量与 Markdown 声明合同、对应定向测试。
- [x] 基准配置迁移到 config/setting.json，Python/Rust 定向检查。
- [x] ORM 恢复错误的类型/效应/生成器贯通及事务捕获回归。
- [x] 命名数据迁移扩展及历史/事务定向回归。
- [x] examples 重排、CMS 双源码业务实现、路径与文档同步。
- [x] 双源码 check/run/build 与打包定向验证、独立审查、规范更新。

## 验证边界

仅当前合同及示例的离线定向测试。SQLite 数据库使用测试临时目录。若需要 loopback 短时 peer 验证，运行前说明；不连接外部数据库、不运行全量集成或压力测试。未安装的 lint 工具如实记录。

## 当前证据

- Model default/Seed 定向 3/3；Markdown 静态/格式化 18/18（包括新 Model 声明支持）。
- 基准 runner 37/37；network/live 无网络配置测试 2/2；Rust peers offline check/build 通过。现有根 setting.json 未修改。构建产物协议标记 setting-json-v1，旧产物必须重建。
- error_effects 原有 8 项通过；新增共享错误包装/native+reference 1 项通过（含显式外层错误身份保持）。
- SQLite 共享 ORM fixture 通过；新增 NotFound/Constraint 经任务和事务回滚捕获通过，最终记录 0 条。
- 所有迁移后示例统一合同检查通过。两套 CMS 声明、Model、API、配置 parity 通过。
- CMS 实际 native 首次运行发现 first() 行解码裸块的 ?/return 错误边界问题，已在共用 decode_included_row 修复；关联查询 native 回归 1/1，通过。
- CMS CLI/打包验收 1/1（392.43 秒）：两格式各 check、2 次 CLI run、build、2 次无 module 独立打包运行，均 users=2/news=1。业务 probe 确认邮箱/显示名规范化，重复注册 Constraint 后数据不增加，禁用作者后发布 AuthorDisabled 且文章不增加。临时项目和 SQLite 已清理。
- 两套 CMS 经 deverc fmt 整理后，fmt --check 和 parity 再次通过。CMS 基准 fixture 与当前业务入口的静态检查 1/1；旧 system 示例移除配置环境变量后的 native 检查 1/1。
- 命名数据迁移 7/7（包括真实 native 双版本 SQLite 升级）；既有 orm_migration 7/7；最后增加 dollar-quote 回归后，静态/SQLite 7/7、共享 typed SQL 3/3 通过。native 升级未因纯扫描器边界修正重复运行。
- 默认 core 及 core + PostgreSQL runtime cargo check 通过；最终扫描器修正后再次通过。
- 独立审查确认 1 项 P2：PostgreSQL SQL 以未闭合 $tag 结束时，then_some 急切切片越界。最小回归先复现 panic，再修正共用扫描器；未闭合 delimiter/body 均给出 C014，合法 Unicode tag 和含 $ 的标识符保留。修复已独立复核，无剩余阻塞。
- rustfmt/clippy 未安装。性能没有重测，旧 CMS 报告对应旧业务入口，不能替代新实现验收。

## 收口

- examples 仅保留 cms、old 两个目录：cms/dever 与 cms/md 同步；历史示例位于 old/dever、old/markdown。正式测试仍在 test/。
- README、LANGUAGE、MARKDOWN-SYNTAX、示例说明和 backend specs 已同步。维护中的路径及配置环境变量搜索无旧入口；历史任务/日志保留原始事实。
- 本次验收完成。业务 HTTP API 等用户重新设计；真实 PostgreSQL、性能/体积/64/128 MiB 压测不属于本轮验收。命名迁移只保证单 Model 事务，不承诺跨 Model 原子升级。
- 本地任务未关联 PR；按项目要求不创建 commit，归档和日志均使用 no-commit。

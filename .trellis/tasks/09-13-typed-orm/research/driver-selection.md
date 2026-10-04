# 数据库驱动与连接池选择

## 结论

- SQLite 使用 [rusqlite](https://github.com/rusqlite/rusqlite) 并启用 `bundled`，目标机器不依赖系统 SQLite。
- SQLite 池与同步任务适配使用 [deadpool-sqlite](https://docs.rs/crate/deadpool-sqlite/latest)，它直接基于 rusqlite，并能把 `bundled` feature 传递给 rusqlite。
- PostgreSQL 使用 [tokio-postgres](https://github.com/sfackler/rust-postgres)，保留原生异步连接与流水线能力；TLS 使用 [tokio-postgres-rustls](https://docs.rs/tokio-postgres-rustls/latest/tokio_postgres_rustls/) 复用项目现有 rustls 根证书和主机名校验。
- PostgreSQL 连接池使用 [deadpool-postgres](https://docs.rs/deadpool-postgres/latest/deadpool_postgres/)，复用其连接回收和静态语句缓存，不自行实现通用连接池。

## 选择理由

- 四个 crate 属于两组相互配套的驱动/池实现，不需要引入完整 ORM 或第二套查询语义。
- rusqlite 的 bundled 模式静态编译 SQLite，符合打包后二进制直接运行的要求。
- tokio-postgres 与当前 Dever Tokio runtime 一致；它的连接 driver 可以纳入现有结构化任务生命周期。
- deadpool-postgres 已包装 `tokio_postgres::Client`/`Transaction` 并提供 statement cache，适合 Dever 已静态确定 SQL 形状的查询。
- deadpool-sqlite 负责连接池和同步连接的安全交互；Dever 仍在外层施加全局 blocking 上限、取消和超时合同。

## 不采用的方案

- 不采用 SQLx、Diesel 或 SeaORM：Dever 自己拥有 Model、QueryPlan、迁移和类型检查，再引入完整 ORM 会形成重复语义与额外二进制负担。
- 不直接手写 PostgreSQL/SQLite 连接池：池的回收、等待、关闭与失效连接处理不是 Dever 的业务差异点。
- 不让 SQLite 和 PostgreSQL 共用同一个执行 trait object 热路径；共享的是静态 QueryPlan 和错误分类，具体执行通过编译时 profile 与连接枚举分派。

## 二进制裁剪

`dever-runtime` 将数据库依赖设为 optional features：

- `sqlite`：deadpool-sqlite + rusqlite bundled
- `postgres`：deadpool-postgres + tokio-postgres + tokio-postgres-rustls
- `database`：共享 config、ORM 值和错误基础

构建阶段从 `config/setting.json` 读取所有连接的数据库类型，只把驱动能力集合加入 runtime profile 和原生产物缓存身份，不嵌入账号、连接串或其它配置值。运行时设置可以在同一能力集合内改变；改用未打包的数据库类型必须重新构建。

## 验证重点

- 分别比较 base、SQLite-only、PostgreSQL-only 和双驱动二进制大小。
- 64/128 MiB 环境测量空池、最大池、稳定负载和连接归还后的 RSS。
- 验证连接获取超时、取消、driver task 失败、SQLite busy 和 shutdown drain。
- 对静态 SQL 验证 statement cache 命中；迁移 DDL 不进入无界 statement cache。

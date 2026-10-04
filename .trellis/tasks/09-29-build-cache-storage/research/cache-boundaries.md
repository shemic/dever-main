# Cache Boundary Evidence

## Existing Owners

- `.trellis/spec/backend/toolchain-and-library.md`：正式 `dever` launcher 统一选择版本并分派项目命令；机器共享缓存只能通过认证 `deverd` 服务访问，服务不可用时必须明确失败，不能退回普通用户可写目录。
- `.trellis/tasks/09-28-shared-toolchain/prd.md`：`dever cache status/clean` 属于跨项目、跨用户的机器级缓存合同，运行条目使用租约，清理与构建必须互斥。
- `crates/dever-cli/src/toolchain/launcher.rs:58`：当前 `cache status/clean` 调用统一返回认证缓存服务尚不可用，未直接构造 `CacheStore`。
- `crates/dever-cli/src/toolchain/cache.rs:157`：`CacheStore::clean` 是未来特权服务内部的文件系统引擎，不是项目进程可调用的公开回退路径。

## Task Boundary

- 新的 `clean <project-root>` 只能清理当前用户、当前 Dever 版本严格识别的过期临时构建目录和旧项目 staging 文件。
- `clean` 不读取、修改或删除机器共享缓存条目，不改变 `dever cache clean` 的命令和失败合同。
- 编译器仓库的 `target/debug` 继续由 Cargo 管理；普通 Dever 项目不会通过应用配置控制 Cargo 开发缓存。

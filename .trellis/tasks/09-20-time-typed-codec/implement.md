# Dever 时间与类型化编解码实施计划

## Start Gate

- [x] `secure-data-contracts` 完成并提供稳定 Secret/private 类型属性。
- [x] 用户已批准父任务规划。
- [x] 读取 compiler/toolchain/database/error specs 和当前 API codec、JSON runtime、DateTime ORM codec。

## Implementation

- [x] 增加 checked DateTime/Date/Time/Duration runtime 操作与官方 time API。
- [x] 增加共享 wire schema 类型分类、策略和 schema fingerprint 基础。
- [x] 增加 bounded strict JSON input parser/decoder 和 concrete value encoder。
- [x] 生成具体 scalar/list/record/nullable codec，缓存重复 schema。
- [x] 将 API response 迁移到共享 encoder，删除 `native/api.rs` 平行转换。
- [x] 保持 ORM storage codec/fingerprint 不变并增加回归。
- [x] 更新 LANGUAGE、Markdown、compiler/toolchain/database specs。

### 实施合同（2026-09-20）

- 固定 chrono 0.4.45，std-only；主代理统一 fetch 后全部定向检查使用 locked/offline。Date 是 UTC 午夜 epoch 毫秒，Time 是午夜起毫秒；RFC3339 offset 转 UTC，拒绝闰秒、超过三位小数和四位日历范围以外值。checked 算术仅检查 i64 溢出，超日历范围在格式化边界失败。
- `core/wire.rs` 是唯一类型允许表，policy 进入 identity；`native/wire.rs` 生成具体类型的双向 helper，API 按 identity 缓存。允许 Secret 的输入 policy 只生成 decoder。private/bounds/Choice/Map/ModelId/Related/resource 稳定拒绝；没有放行尚未实施的字段约束验证。
- runtime wire 使用借用 RawValue 保留原始 JSON 数字，逐层 visitor 检查重复键；16 MiB、64 层容器、65536 个值。concrete decoder 拒绝未知和缺失必填字段，缺失 nullable 得到 None。API envelope 只嵌入 Encoded，移除旧 response Value round-trip。
- 官方标准库原来没有 `.dever.md` 副本。经主代理确认，不新增同包重复源码；以 LANGUAGE 合同和 plain/Markdown 调用方同签名、同输出 parity 覆盖 Markdown 验收。
- 没有改 ORM/model fingerprint owner、配置来源、API 输入协议或 CMS 业务；无应用环境变量配置。

### 验证结果（2026-09-20）

- core/runtime/cli，api + reference，以及 postgres + api + reference 两次定向 cargo check：通过。
- 新 `time_codec` 最终 9/9 通过（40.39 秒）。覆盖实际编译运行的 concrete decoder、Decimal/Id/Uuid/负零/原始 Json 精度、缺失/未知/重复字段、无监听器 API dispatch、native/reference 时间成功和失败、plain/Markdown、预算与 policy。随后仅增强 Model 专项为当前 App → Model 调用，独立复验 1/1 通过：四种时间值仍编码为 ORM Int，存储 logical type 和 i64 表示不变。
- 既有 `api_routes` 7/7、`json_library` 7/7、`markdown_source` 20/20、`model_syntax` 3/3、`standard_library` 5/5 均通过。
- `sqlite_orm::shared_orm_fixture_remains_valid_without_opening_a_database`：失败，未打开数据库。`support/orm_fixture.rs` 的旧 fixture 在 `main.dever` 中直接调用 `user.create`，当前 `check/orm.rs::resolve_model` 只允许 Loose 使用 table 短名，当前 Main/App 角色合同不再接受该用法，报 C004。该 fixture 和角色/ORM owner 本次均未修改；按主代理确认记录现存不匹配，不扩大本任务迁移旧 fixture。其余 SQLite 执行用例未运行。
- `git diff --check` 通过；仓库主体为既有未跟踪文件，另对本次新增源码做显式空白和旧转换路径扫描。没有旧 `json_type`、`json_value` 或 API `parse_json` 输出路径残留。
- 未运行全量、真实 PostgreSQL、服务、性能；rustfmt/clippy 组件缺失，未安装。

## Focused Verification

### Trellis 独立检查（2026-09-20）

- 核对共享 schema policy/identity、nominal ID 位移稳定性、根 response bounds、private/Secret/资源拒绝、具体 decoder 与 API envelope；没有发现并行输出类型允许表或 ORM storage/fingerprint 改动。
- 修复 runtime Encoder 的异常状态：对象缺值时 `end()` 保留未完成 frame；写入部分 JSON 后触及字节上限时锁定失败，`finish()` 不再可能发布残缺 Encoded。新增缺值、闭合括号超限和逗号写入后值超限三个回归，同步 toolchain spec。生成代码原本逐步 `?` 传播，修复针对公共 Encoder 自身的 Encoded 不变量。
- 独立 `cargo check --locked --offline -p dever-core -p dever-runtime -p dever-cli --features dever-runtime/postgres,dever-runtime/api,dever-core/reference` 通过（16.79 秒）。命令使用上述 incremental/debug/toolchain 设置。
- 修复后 `time_codec` 9/9 通过（44.22 秒），包括新增 Encoder 回归、native/reference 时间、生成 concrete codec 与无监听器 API dispatch。
- `git diff --check` 及新增/修改源码显式尾随空白扫描通过。rustfmt/clippy 二进制未安装，不能宣称 lint 全通过；未安装组件。
- 未运行全量、真实 PostgreSQL、SQLite 执行、服务或性能测试。已有 SQLite fixture 角色不匹配仍按上述记录保留；无本任务未修复阻塞。

```bash
cargo check --locked --offline -p dever-core -p dever-runtime -p dever-cli --features dever-runtime/postgres,dever-runtime/api,dever-core/reference
cargo test --locked --offline -p dever-tests --features api,reference --test time_codec -- --test-threads=1
cargo test --locked --offline -p dever-tests --features api,reference --test standard_library --test json_library --test api_routes --test model_syntax --test markdown_source -- --test-threads=1
cargo test --locked --offline -p dever-tests --features sqlite --test sqlite_orm shared_orm_fixture_remains_valid_without_opening_a_database -- --exact --test-threads=1
git diff --check
```

不运行全量测试、真实 PostgreSQL、服务或性能基准。PostgreSQL 仅做受影响 feature 的定向 `cargo check`，除非隔离连接被明确提供。

以上命令实际均额外使用 `CARGO_INCREMENTAL=0`、`--config profile.dev.debug=0 --config profile.test.debug=0` 和已安装 stable toolchain 的命令级 PATH；这些是编译工具设置，不是应用配置。

## Review Risks

- JSON parser 必须保留 duplicate key 可见性，不能先解析成会覆盖键的 Map。
- DateTime 外部格式变化不能修改数据库列类型、migration history 或内部排序。
- schema 缓存 identity 必须包含 policy，避免 setting 允许的 Secret 被普通 output 复用。
- 生成 codec 必须保留 record 声明顺序和输入求值顺序，且不泄漏 runtime Value 到业务层。

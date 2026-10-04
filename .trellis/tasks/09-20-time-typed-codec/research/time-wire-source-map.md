# Research: 时间与共享类型化 wire codec 源码地图

- Query: 查清现有时间表示、API JSON 路径、可复用 parser/schema/type graph、依赖和最小验证入口。
- Scope: mixed
- Date: 2026-09-20

## Findings

### 已有时间表示和必须保留的合同

- `crates/dever-core/src/types.rs:18` 已有 DateTime/Date/Time/Duration 四个独立 Type；`check/symbols.rs:313` 负责源码类型解析。它们不是 Int 别名，源码不能随意把 Int 赋给 DateTime。
- `native.rs:770` 四种类型均生成 Rust i64；`native/orm.rs:1312` 编码为 ORM 整数，`:1375` 通过 `orm::int` 解码；`:1746` PostgreSQL DDL 为 BIGINT。不要为外部字符串格式修改这些分支。
- `model.rs:146` Model revision、`:158` model_snapshot、`:364` field_type 共同拥有 Model 语义身份；四种时间 logical type 字符串应保持原样。SQLite/PostgreSQL fingerprint 属持久化合同，wire schema 不应写入它。
- `.trellis/spec/backend/database-guidelines.md:83` 已明确 Date/time 使用整数毫秒。建议 Date 为 epoch 起 UTC 午夜毫秒，Time 为午夜起毫秒且限制 `[0, 86400000)`；不能悄悄改为 Date epoch 天数。当前没有更具体的 Date 单位/范围实现，需在实现文档明确。
- `reference/value.rs:14` 只有 Int 值载体，没有四种单独时间变体；HIR 保留具体 Type，reference 新 intrinsic 可复用 Value::Int，不必新建动态时间对象。
- `crates/dever-runtime/src/time.rs:4` 仅有 unix_millis、monotonic_nanos、sleep；unix_millis 当前拒绝系统时钟早于 epoch，但 parse/format 应独立支持负 epoch。
- `library/dever/time.dever:1` ClockResult.Read(Int)、SleepResult；普通函数调用系统 intrinsic 后分派并 fail，继续复用这种真实源码 Result choice。
- 桥接修改位置：`intrinsic.rs:33/152` enum/name mapping；`check/intrinsics.rs:91` 签名和 Result shape；`native/intrinsics.rs:79` resource_result；`reference/intrinsics.rs:76` recover；`contracts/effects.rs:529/553` blocking/time effect。只有 now/时钟读取有 time effect；确定性 parse/format/算术应 pure，不能给所有时间 intrinsic 统一外部 time effect。

### API 与 parser 路径

- `check/api.rs:121` 调用私有 `json_type` (`:149`) 递归判断输出可序列化，`native/api.rs:72` 有平行 `json_value` Type match。共享 schema 应同时替换这两份允许类型逻辑，否则 checker 和 emitter 会继续漂移。
- `native/api.rs:51` 是 response 转换接入点，转换失败保持日志脱敏和 500；`:32` handler 输入只绑定 Text/Int/Bool(nullable)，本任务不要顺手扩大 API 输入协议。
- `runtime/api.rs:188` success 和 `:216` envelope 拥有 `{code,message,data}`；404/405 Allow/500 保持 API owner，不塞入通用 codec。
- `runtime/api.rs:140` InputObject 仅拒绝顶层重复键；nested `serde_json::Value` 会静默覆盖重复键。`:191` parse_json 是普通 serde_json parse，没有项目字节/集合上限。不能宣称已有全面严格 parser。
- `library/dever/json/parse.dever:31` 普通 Dever parser；`:212` depth guard、`:288` duplicate rejection；`json/value.dever:27` maximum_depth=256；`json/write.dever:24` 输出预算 16 MiB。它使用 postorder Document/Number(raw Text)，保持原始数字准确性；不能把它当现成可调用 Rust runtime parser。没有发现 parser 输入字节或集合总量上限。
- `runtime/config.rs:142` serde_json::from_str(Document)，`:182` HTTP from_value；未来 setting 需接新 parser，当前无需实现 setting 功能。

### 最小清晰 owner/API

1. 新 `crates/dever-core/src/wire.rs`：只依赖已检查 Type/Definition，导出 crate 可见的 Policy、Schema/Node、从具体 Type 构建 schema 的 Result API。当前 Type 本身 crate-private，因此所谓公共 compiler API 是 compiler 各消费者共用，不必把所有 HIR 类型变成外部 public。
2. 新 `native/wire.rs`：从 schema 生成具体 Rust encode/decode helper；名义类型按 type-id + policy + direction 缓存 specialization，用 schema 引用而非展开共享 DAG。API、setting、Job 只能调用此 owner。最终业务参数必须具体 T{id}/List/scalar，Value 只停留在转换边界。
3. 新 `runtime/wire.rs`：serde DeserializeSeed/Visitor 式递归 bounded parser，每层 map 在插入前检测重复键，进入容器/接收元素时扣预算。输入总字节在 parse 前检查、显式 end() 拒绝尾随内容；未知字段/必填字段由具体 decoder 检查。不要先 Value parse 再检查 duplicates。
4. runtime scalar conversion 复用 `number.rs:44` DecimalValue::parse（严格拒绝 inexact/nonfinite）、`orm.rs:53` Uuid::parse、`lib.rs:40` Id(String) 和新增 time parse/format。Float decode/encode 拒绝非有限。Id/Decimal/Uuid 只收字符串，Int 只收精确 i64 JSON number。
5. 新 wire runtime feature 最自然由 api/database 等消费者启用；当前 serde/serde_json 在 runtime 是 optional，`native/build.rs:309` runtime artifact cache identity 与 feature 选择要同时包含任何新 feature。不要让生成 decode helper 依赖未启用模块，也不要为未使用 codec 的普通程序开启整套 JSON。

### 类型图与敏感值

- `types.rs:84` observable 复用缓存的 Secret/private Model 敏感性；缓存 `cache_properties` 在名义类型校验后生成。不要在其初始化之前调用 observable。
- `types.rs:290` cache_properties 用依赖顺序算属性；`nominal_sensitivity` 独立 worklist 传播 Related 环。`check/cycles.rs` 拒绝正常名义布局环。新 schema 不该再从 AST 建图，也不能把 Related 当普通记录递归展开。
- wire 支持 record 且每个 field 必须非 private；observable 单独不够，因为普通 record private 不属于 private Model 敏感性，仍须逐字段拒绝。
- API/setting input 允许 Secret，ordinary output/Job payload 禁止；不要用 transferable 代替 wire eligibility（Secret 可 transferable，资源也可能 transferable）。Choice、Map、Related、资源均不在当前 PRD 支持集。
- `DefinitionKind::ModelId` 是特殊 nominal（native为 T{id}(i64)），不是 builtin Id(String)；当前 API json_type 对非 record 已拒绝它。不可意外将 ModelId 当空 record 编码 `{}`，若本任务不扩展其合同应稳定拒绝。
- 当前 bounds 是 Field.hir::Domain；任务只要求类型转换，但 decoder 若接收受约束 record，必须防止生成值绕过既有字段范围保证。应明确生成 bounds 校验或 schema 构造拒绝未支持 bounds，不能默认通过。

### 日期依赖与精度风险

- 检查 Cargo.lock 和 `/root/.cargo/registry/{src,cache}`，未发现 chrono/time/jiff 日期库。不能按离线已缓存处理；引入固定版本后需由主代理统一 fetch/lock，再使用 offline+locked 验证。
- 官方 chrono 0.4.45 文档给出 MSRV 1.62，满足工作区 Rust 1.85；可固定 `=0.4.45`, default-features=false, features=["std"]，复用已有 SystemTime，不开启 clock/local timezone。相关官方文档见下面链接。
- Chrono 的 from_timestamp_millis 返回 Option，日期范围小于全 i64；格式化超出日历范围应返回明确失败。RFC3339 秒小数与 leap-second 不能直接 timestamp_millis 截断：对非毫秒精度和 `:60` 要显式拒绝或先固定精确可表示策略，避免静默丢失。
- 四位 `YYYY-MM-DD` wire 合同意味着需要限制年份表示；DateTime canonical 精度（固定 .sss 或零毫秒省略）须统一定义并测试。Time 可选毫秒只能接受可无损表示文本。
- DateTime 加减 Duration、difference、Duration 倍率构造复用 checked_add/sub/mul；i64 checked 成功不代表仍能格式化成约定 RFC3339。

### 定向验证与现有缺口

- `test/dever-tests/tests/standard_library.rs:105` clock_failures_are_results 可扩展时间 wrapper；更完整的新 `time_codec` target 可集中 runtime 时间边界和 compiler native/reference 最小回环，避免散入一堆测试。
- `api_routes.rs:37` 生成路径检查、`:64` native compile（不启动监听）、`:118` typed input、`:148` errors/envelope；运行需 `--features api --test api_routes`。
- 注意 `api_routes.rs:148` malformed_or_extra_inputs 中 if-let 仅检查解析成功分支，未断言坏输入一定失败；新 parser 回归必须直接 assert error，覆盖 nested duplicate、unknown/missing、i64 extremes、depth/bytes/list limits、NaN/Infinity。
- `json_library.rs:47/85/146/237` 覆盖 ordinary Dever JSON 正常/坏输入/depth/构造图限制。新 Rust parser 不应改变此官方源码 JSON API；需要原始 Json 数字精度保真测试，serde_json 默认 Value 可能把巨大原始数字转浮点并改变值。
- `model_syntax`/`markdown_source`/`contract_execution` 可以验证 Model snapshot/plain-Markdown parity，schema/int codegen 字符串断言可确认 PostgreSQL BIGINT 而不连真实数据库。
- 已有 API DateTime 序列化是整数 (`native/api.rs:75`)，本任务将明确改成 UTC 字符串，需要新 fixture 证明，而不只是测试 time runtime。

## External References

- https://docs.rs/crate/chrono/0.4.45 — 固定版特性、MSRV、日期范围。
- https://docs.rs/chrono/0.4.45/chrono/struct.DateTime.html — parse_from_rfc3339、from_timestamp_millis、timestamp_millis、to_rfc3339_opts。

## Related Specs

- `.trellis/spec/backend/{index,directory-structure,error-handling,quality-guidelines}.md`
- `.trellis/spec/backend/compiler-contracts.md`：type depth、敏感属性、API 输入/输出。
- `.trellis/spec/backend/toolchain-and-library.md`：源码 JSON 与 intrinsic/reference 合同。
- `.trellis/spec/backend/database-guidelines.md`：整数毫秒和 fingerprint。
- 当前任务 `prd.md`、`design.md`。

## Caveats / Not Found

- 子代理 `task.py current --source` 返回 none；输出目录由主代理 dispatch 显式指定，没有猜路径或更改 task 状态。
- 此研究未改源码、规范或任务状态；未运行服务、数据库、全量或性能测试。
- Shared schema 与 typed decoder 目前不存在；上面的文件名/API 是最小新增 owner 建议，不是已有事实。
- 普通 JSON 的 exact raw numeric 保真与 serde_json Value 的数字语义有真实差异，需要实现前固定 Json passthrough 策略。

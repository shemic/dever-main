# Research: 安全数据合同源码边界

- Query: Model private、Secret、密码学及 environment 移除的最小实现路径。
- Scope: internal
- Date: 2026-09-20

## Findings

### 字段与领域身份

- `crates/dever-core/src/syntax.rs:124` 的 AST `Field.private` 已存在；`parser.rs:517`、`:556` 解析普通 record 的 private，选择分支不允许 private。无需新增语法关键字。
- `check/symbols.rs:373` 将 AST private 原样落到 `types.rs:188` 的 checked `Field`。HIR 表达式直接持有 checked Type，因此不需要另造字段隐私 HIR。
- `types.rs:209` 的 `Definition` 已有 owner、kind；`DefinitionKind::Model` 可区分普通 record 与 Model。`source.rs:25` 的 SourceRole、`:84` 的 `SourceLayout::domain()` 是领域身份唯一来源，不应拆字符串判断归属。
- `check/model.rs:539` 当前硬拒绝 Model private；移除此拒绝后已有字段落地流程可保留 metadata。`synthesize_fields`（`:219`）仅插入公开 id/created_at，生成 Page/Cursor 及 Related 字段。持久 schema `ModelField` 不必承载语言隐私，因为数据库列格式不因访问策略改变。
- `check/expressions.rs:167` 的 `Body::field_type` 按 type.owner == context.owner 检查私有字段；写路径复用字段解析。`check/values.rs:41` 独立进行 record 构造授权。两处应改用同一 owner 检查：普通 record 保留 package 私有；Model 只允许同领域 App。可直接复用 `check/orm.rs:247` 对 SourceRole::App 和 layout.domain 的判定。
- `check.rs:506` 的 validate_public_boundaries 只拒绝 private nominal type，并跳过 public record 的 private 字段；它不会拒绝一个 public Model 内含 private 字段。`:557` 的 validate_exported_failures 只检查错误类型自身 public 标志，嵌套 Model 同样需要补覆盖。
- `check/api.rs:149` 的 json_type 已拒绝 record private，但自写递归，与 App 合同尚未共享。建议统一类型可观察性属性，再由 API 保留具体可编码标量白名单。

### Secret 与类型能力唯一所有者

- `types.rs:7` 的 Type 增加独立 Secret；`check/symbols.rs:300` 附近是源码内置标量解析入口；`native.rs:766` 附近是 Rust 表示映射；`reference/value.rs:17` 是 reference Value 枚举。不要用 Named record 或 Bytes 别名模拟 Secret。
- `types.rs:232` 的 Properties + `cache_properties` 已按 nominal DAG 缓存 comparable/transferable/movable。建议在此集中增加可渲染/敏感类型属性；避免在 API、App、fault 三处复制 nominal 图遍历。
- `check/symbols.rs:452` 的 map_key 是独立白名单，Secret 默认不匹配；`check/model.rs:1320` 的 normalize_field_type 是持久化白名单，Secret 默认不可持久化。无需把所有存储参数检查迁入通用 Properties；仅共享安全属性。
- 风险：`Type::Related` 目前直接返回 Properties::ALL（types.rs:92），且 referenced_types（check/symbols.rs:471）特意不展开 Related，避免合法关联 Model 环。敏感信息属性若沿用该路径会漏检，若简单递归则可能死循环。使用有界反向传播/固定点处理关联图，或者在对外可观察类型中拒绝 Related；不能影响现有布局/可比较缓存算法。
- `native/records.rs:40`、`:82` 无条件为 record/choice 生成 Render，逐字段调用 render_to，当前 private 字段同样被渲染。`reference/value.rs:134`、`:177` 同样逐字段展示。Secret 的静态禁令必须覆盖容器和选择 payload；不应靠运行期脱敏代替编译拒绝。
- 原生类型统一 derive Clone/Debug（native/records.rs:25）；Secret Rust 表示可有安全 Debug 占位，但不能输出原始字节。不提供普通 Render 实现时，生成器需要避免为包含 Secret 的 nominal 类型生成不可编译 Render，同时错误/入口输出等 render sinks 必须先检查。
- 日志内置签名在 `check/intrinsics.rs:108`，当前接收 Text；应保持此边界，不增加任意值日志转换。JSON 标准库采用显式 JSON 类型，API 泛型序列化是需额外保护的边界。

### environment 完整删除链

1. `library/dever/process.dever:6` EnvironmentResult、`:23` environment 及两个 environment_value 分支。
2. `crates/dever-core/src/intrinsic.rs:26` ProcessEnvironment 枚举、`:140` process_environment 名称解析。
3. `check/intrinsics.rs:85` 签名及官方 EnvironmentResult shape 验证。
4. `contracts/effects.rs:549` process effect 分类，保留 ProcessArguments。
5. `native/intrinsics.rs:72` resource_result → `dever_runtime::process::environment`。
6. `reference/intrinsics.rs:69` recover → 同一个 runtime 函数。
7. `crates/dever-runtime/src/process.rs:16` environment 桥接函数。
8. `test/dever-tests/tests/standard_library.rs:77` process 输入测试及 `:115` 直接 runtime 错误测试；`differential.rs:331` environment 错误一致性样例。
9. `README.md:65` 文档明确宣传该入口，也需要同步删掉。

已有 argv arguments 是独立能力，保留。测试环境配置不要换成另一个应用环境变量入口；删除 environment 专项并增加旧 API/旧 intrinsic 被拒绝的定向断言。

### 依赖与密码学复用

- `Cargo.toml:36` 已精确锁定 getrandom = 0.2.17；runtime `orm.rs:86` 用 getrandom::getrandom。runtime Cargo.toml 当前只在 database feature 下启用，非数据库 crypto 不能依赖 database 才有随机源。
- Cargo.lock 中另有 getrandom 0.4.3（433）、hmac 0.13.0（490）、sha2 0.11.0（1059）、subtle 2.6.1（1143）；这些只是已解析的间接依赖，不代表 runtime 可直接 import。
- Cargo.lock 已有 zeroize（1650）；没有 argon2。采用 Argon2 crate 时需核对其 digest/password-hash/rand_core 版本，不应假定已锁 sha2/hmac 与任意版本 Argon2 相容。
- 官方包装遵循 process/time 的 Result choice + checker shape 校验 + native resource_result + reference recover；crypto 重用该协议。运行时集中到 crypto 模块；Secret 字节访问保持 crate 内部，禁止公共 reveal。

### 最小定向测试

- `contract_semantics::private_fields_protect_access_mutation_and_construction`（218）保留普通 record package-owner 基线。
- `source_architecture::model_operations_belong_to_the_owning_domain_app`（209）扩展 Model private 字段读写、同领域不同 App topic、Domain/API/Test/跨域拒绝、App 嵌套参数/输出/错误泄漏。
- `model_syntax` 覆盖 private 持久声明及 Secret 存储拒绝；`markdown_source` 验证 private 往返和两种源码一致。
- `contract_api` 覆盖快照不会暴露私有表示，以及公开边界敏感类型拒绝。
- crypto 新的根 test/dever-tests/tests 定向文件可以集中运行时已知答案、native/reference 一致性、Secret 禁止 Render/eq/Map-key/JSON/persistence；`test/dever-tests/Cargo.toml` autotests=false，必须注册新目标。
- `standard_library` 和 reference feature 下 `differential` 修改 environment 案例。优先 cargo test -p dever-tests --test <target> <filter>，无需真实数据库、服务、全量测试。

### Related specs / External references

- `.trellis/spec/backend/compiler-contracts.md`：字段隐私、App 边界、角色及有界类型图。
- `.trellis/spec/backend/database-guidelines.md`：Model 所有权、静态 schema、settings-only 配置。
- 当前任务 `design.md`：Model domain-App owner、opaque Secret、无环境变量兼容路径。
- 外部资料未访问；版本事实来自当前 Cargo.lock，密码算法参数/具体 crate API 需实施者按实际选定版本的官方资料核实。

## Caveats / Not Found

- 子会话 `task.py current --source` 返回 none；输出路径由父代理明确提供，未修改任何任务指针。
- 未执行构建、测试或服务；结论为源码研究，不是运行验收。
- 未发现可复用 Secret 或 argon2 实现。上述 Render 风险是当前实现事实；静态渲染入口完整枚举应在实现阶段继续检查 native entry/failure/assert 渲染，不能只修改 native record emitter。

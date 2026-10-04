# Dever 时间与类型化编解码

## Goal

补齐生产业务所需的 UTC 日期时间操作，并建立唯一的静态类型 wire 编解码合同，为 Adapter setting、Job payload 和 API 输入输出提供同一套安全、无损、可审计的转换。

## Dependencies

- `secure-data-contracts`：提供 Secret 和统一不可观察类型属性。

## Requirements

- DateTime 内部和数据库继续使用 Unix epoch 整数毫秒；Date/Time/Duration 保持既有 i64 存储兼容。
- `dever.time` 提供 UTC now、RFC3339 DateTime parse/format、Date/Time parse/format、Duration 构造，以及 DateTime 加减 Duration、DateTime difference；溢出和非法文本返回显式失败。
- 外部 wire 表示固定：DateTime 为规范 UTC RFC3339 字符串，Date 为 `YYYY-MM-DD`，Time 为带可选毫秒的 `HH:MM:SS[.sss]`，Duration 为整数毫秒。
- 编译器从具体 Type/Definition 派生共享静态 wire schema；setting、Job 和 API 只能通过该 owner 请求 encode/decode 代码。
- 支持 Bool/Int/Float/Decimal/Text/Id/Uuid/DateTime/Date/Time/Duration、nullable、List 和无 private 字段的 record；Json 作为已验证原始 JSON 时保留既有语义。Secret 只允许在 API/setting input policy 中解码，禁止普通 output 和 Job payload。
- Float 编码拒绝 NaN/Infinity；Decimal/Id/Uuid 使用无损字符串；整数严格检查 i64。
- 对象解码拒绝重复键、未知字段和缺失必填字段；嵌套深度、集合长度和总字节受运行时已有/新增明确上限约束。
- 编解码器生成具体值，不把动态 `serde_json::Value` 传入业务函数，不增加反射或 runtime 类型注册表。
- 迁移当前 API response 编码使用共享 schema，保持 `{code,message,data}` envelope 和既有路由行为。

## Acceptance Criteria

- [ ] 时间 parse/format round-trip、UTC 规范化、闰年边界、负 epoch、非法文本和 i64 溢出均有定向测试。
- [ ] SQLite/PostgreSQL Model 的整数毫秒表示和现有 schema fingerprint 不因外部格式变化而改变。
- [ ] 支持类型的 encode/decode round-trip 无损；Decimal/Id/Uuid/DateTime 不经 Float。
- [ ] duplicate/unknown/missing/over-depth/over-size/non-finite/private/resource 稳定拒绝；Secret 仅在批准输入策略成功，普通输出和 Job payload 稳定拒绝。
- [ ] API response 改用共享 codec 后，已有 envelope、状态码和 API route tests 保持一致；DateTime 输出变为 RFC3339。
- [ ] `.dever` 与 `.dever.md` 的时间签名、文档合同和生成行为一致。
- [ ] 后续 setting/Job/API 能消费一个公共 compiler schema API，而无需复制类型匹配分支。

## Out Of Scope

- 本地时区数据库、DST 推断和用户界面格式化。
- 任意 schema registry、Avro/Protobuf 或动态 JSON schema 发布。
- Model 结构自动存储为 JSON；持久化仍需显式 Json 字段。

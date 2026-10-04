# Dever 时间与类型化编解码设计

## Time Representation

保留现有 i64 毫秒运行时表示和数据库 codec。新增成熟、固定版本的日期时间库只用于严格解析、日历校验、UTC 格式化和 checked arithmetic，不手写日历算法。DateTime format 统一输出 `Z`，parse 可接受 RFC3339 offset 后转换 UTC。Duration 是有符号毫秒。

官方 `dever.time` 继续用普通 `.dever` Result choice 暴露失败，system intrinsic 返回具体 i64/Text。`unix_millis` 可保留为底层数值能力，但业务首选 `now() -> DateTime`。

## Shared Wire Schema

在 checker/HIR 边界新增一个只读的具体 wire schema 描述，节点仅包含已解析类型、record 字段顺序和策略标记。schema 构建复用 nominal cycle/depth 和 private/Secret 属性，不独立遍历 AST。

Schema policy 区分：

- ordinary/API input：可按协议解码，显式 Secret 字段生成 opaque Secret；
- ordinary output：必须可观察且无 Secret/private；
- setting input：允许 Secret，但不允许资源和业务 failure choice；
- persisted Job payload：要求跨进程稳定、无资源和 Secret。

Native emitter 为每个被使用 schema 生成具体转换函数并缓存 specialization。runtime 只提供重复键可见的 bounded JSON parser、标量 parse/format 和错误位置；业务值保持具体 Rust struct/list/scalar。

## API Migration

先将 `native/api.rs` 的 response `json_value` 迁移到共享 encoder，保持 envelope 构造仍归 API owner。输入扩展留给 production-api 子任务。迁移后删除原有平行 Type match，添加生成代码断言防止回退。

## Compatibility

数据库和 Model snapshot 不变。DateTime API JSON 从内部整数改为 RFC3339 是预发布协议的直接替换；仓库内 API fixture 同步迁移，不保留双格式输入输出。

# Dever 安全数据合同设计

## Model Privacy

复用现有字段 `private` 语法，不增加 Model 专用修饰符。类型检查为字段记录声明 owner 的 domain identity：普通 record private 继续 package-owned，Model private 改为 domain-App-owned。Model 生成 CRUD 的匿名 create/update block允许所属 App 写 private 字段，但投影、记录构造和公开边界仍由统一字段访问检查控制。

App 是跨领域能力边界，因此任何直接或嵌套包含 private Model 字段的 nominal record 都不能出现在 App 参数、输出或可传播错误中。Model ID 和不含 private 字段的显式 View 仍可公开。API/Job/wire 检查复用同一 `observable` 类型属性，不再各自遍历字段猜测。

## Secret Type

Secret 是独立非泛型标量，内部持有受保护字节，不实现普通 Render/Clone-to-Text/JSON。值语义允许受控传参和 record 字段，但以下静态能力为 false：comparable、Map key、persistent、ordinary wire output、fault rendering。

构造来源：

- `dever.crypto.token(bytes)`；
- 后续 typed setting/API Secret 字段解码；
- 必要的测试固定 Secret 构造仅存在于 compiler-owned Test source，不进入生产标准库。

消费 sink 由 Intrinsic 精确枚举，不能通过一个通用 reveal 函数扩散。后续 API/Adapter 新 sink 必须在 checker effect/role policy 中显式加入。

## Crypto Runtime

使用固定版本的成熟 Rust crates 实现 Argon2id、HMAC/SHA-256 与 constant-time 比较；随机源复用 OS getrandom。Argon2id 参数是经过测试的运行时常量，避免应用通过 setting 降低成本。hash 输出为标准 PHC Text，verify 对格式错误返回显式失败而非 panic。

标准 `.dever` package 定义公开 Result choice 和简单函数；runtime 只承载密码学实现。checker 校验官方 choice shape，native emitter 复用现有 resource-result 模式，不创建第二套失败协议。

## Environment Removal

从官方 `dever.process`、Intrinsic、checker effect、native emitter 和 runtime system bridge 同步删除 environment。仓库调用方整体迁移到 setting.json 或测试显式输入，不添加 feature flag 或旧名兼容。

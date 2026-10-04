# 双源码 CMS 与语言合同修正

## 需求

用户要求 examples 最终只有 cms、old 两个目录；cms/md 与 cms/dever 同步实现同一 CMS，旧示例移到 old。API 留待用户重新设计，本任务不实现 HTTP API。

承接源码审查：修复 Markdown Model 识别/声明合同、Model 默认值与 Seed 字面量校验、ORM 可恢复错误传播与捕获；补齐命名迁移的数据变换能力；示例与基准配置统一从 config/setting.json 读取；同步过期的语言返回值说明。

## 验收

- 两套 CMS 执行相同的注册、发布业务，数据库结构和可观察结果等价；重复执行可预测，禁用作者不能发布。
- 所有旧示例移动到 examples/old；正式测试留在 test；当前代码与文档没有悬空路径。
- Markdown Model 支持普通 Model 的声明，仍执行文档合同校验。
- 不合法 Int/Float/Decimal/Text 默认值与 Seed 在编译期拒绝，不推迟到 Rust 编译或生成器 panic。
- ORM 故障保留错误类别，进入既有隐式失败传播与 result 捕获合同；事务边界不抹掉错误。
- 命名迁移支持参数化、按数据库方言选择的数据变换；事务、历史一致性约束不降级。
- 无应用/基准配置环境变量入口；保留构建工具自身环境及通用 process.environment 语言能力。
- 定向离线检查与最小测试通过，明确未运行的验证。不做全量测试或性能重测，不改全局工具、不提交 Git。

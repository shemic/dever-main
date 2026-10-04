# 设计边界

- 复用同一 source/parser/check/native 流程；Markdown 只负责抽取及文档合同，不另建 ORM。
- Model 字面量约束收敛在已有 normalize_value 所有者；覆盖 default/seed 两种调用者。
- ORM 错误复用现有 choice/error 隐式失败机制；数据库错误身份不得通过文本猜测，非数据库故障仍走 Fault。
- 标准库 `dever.database.Error` 对应 runtime ErrorKind 的十个 error 分支，每个保留 message。调用方结果类型可用一个 error-only choice 载荷包装原错误，例如 `error Failed(error: dever.database.Error)`；checker/native/reference 共用捕获映射，显式抛出的外层错误仍保留名义身份，重叠包装及不匹配错误集合均拒绝。
- 数据迁移扩展已有命名 migration 与参数化 SQL 校验/运行流程，不新增迁移框架。
- CMS 两套源文件具有相同包名和声明，Markdown 补充所需中文文档；共同测试防漂移。普通 CRUD 直接调用 Model，业务状态校验留在 service。
- CMS 演示管理员 Seed、编辑注册与文章发布；注册规范化邮箱/显示名并检查必填数据，单次 create 不增加空事务包装。发布事务验证作者 Active，再按 slug upsert。入口首次注册编辑、重复运行复用编辑，稳定输出 users=2/news=1；哈希仅为演示值，不实现登录或密码算法。
- 旧 examples/dever/cms 移至 examples/cms/dever；其余 dever/markdown 示例整体放 examples/old 下。更新维护中的路径，历史日志保留事实。
- 基准 Python 驱动按受控测试目录写 config/setting.json，Rust peers 使用统一读取规则；工具链配置不混入业务配置。
- 不触及 HTTP API 设计、路由、认证协议或新增 API 配置。现有编译器 dever.api 合同快照仍是编译器能力，并非用户延期的业务接口。

实现分工：Model/Markdown 与基准配置分别独立处理；主线程负责错误链、迁移、CMS 与集成。相邻文件修改前明确交接。

## 命名数据迁移

`migrate name` 保留 `drop field`，增加可重复的 `before { sqlite = "..."; postgres = "..."; parameters = [...] }` 与 `after { ... }` 块；这里的分号仅表示文档分隔，实际源码使用换行。每块必须显式给出两个方言与参数列表，参数用现有绑定编码，不做字符串插值。

- SQL 只接受一条 INSERT、UPDATE 或 DELETE；扫描器跳过引号/注释后检查首关键字及语句终止符，拒绝多语句、事务控制、DDL、PRAGMA。占位符继续复用现有方言扫描与连续参数覆盖检查。
- `_dever_`、`sqlite_`、`pg_` 前缀及 `information_schema` 是保留对象，普通、引用和 SQLite 单引号名称都不能出现在迁移 SQL 中；同名业务文本使用绑定参数。PostgreSQL Unicode 转义标识符不接受，防止绕开保留名称检查。
- 无类型上下文的绑定常量支持 Bool、Int、Text、null 和指数形式 Float；无指数小数属于 Decimal，缺少目标精度/scale 时拒绝，不隐式改为 Float。
- 已有表先校验已应用迁移与实际 catalog，再按源码中迁移/块的顺序执行全部未应用 before，执行自动结构变更，再执行全部未应用 after，最后 Seed/history。所有步骤在现有单 Model 迁移事务中完成；失败整体回滚。
- 新表直接建立最终结构和 Seed，登记迁移已应用，不重放旧数据转换。无 SQL 的 drop-only revision 保持原算法；包含 SQL 的 revision 纳入名字、drop、方言文本、参数类型/值、phase 与块顺序，以有边界的规范表示计算，避免拼接歧义。修改或删除已应用迁移继续拒绝。
- 运行语句放 `Model.data_migrations`，历史 Schema 仍保存声明名、revision、drop；复用 `orm::Value` 和数据库绑定，不新增通用值系统。
- 事务边界是同一数据库中的单 Model 初始化；跨表变换只能引用届时已存在的表。编译器现有 Model 初始化顺序不变，不承诺跨 Model 原子升级、任意对象拓扑或跨数据库迁移。

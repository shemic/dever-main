# Dever 应用测试体系

## Goal

让 Dever 应用可以用 Dever 源码编写和运行确定性的领域回归测试，使 AI 修改 App、Domain 或 Model 后能够验证业务合同，而不依赖 Rust 宿主测试、HTTP 接口或正式数据库。

测试是可选能力。没有测试的项目保持现有开发和运行方式；测试能力不得引入第二套解释器、生产可见测试入口或环境变量配置。

## Background

- 当前 CLI 只有 `check`、`api`、`fmt`、`run` 和 `build`，应用自身没有测试命令。
- 编译器仓库已有 Rust 回归测试，但它们验证编译器实现，不能表达具体 Dever 应用的业务合同。
- 生产源码从 `module/` 加载，`main.main` 是 `run/build` 的唯一启动入口；App 是跨领域能力边界，Domain、Model、Port、Adapter 和 API 是领域实现边界。
- parser、checker、HIR、原生后端、源码位置、错误传播、Model 迁移和 Seed 已有单一正式路径，应由测试复用。
- ORM 的真实 PostgreSQL 验收已经由独立编译器测试覆盖，不属于首版应用测试命令。

## Requirements

### Source And Discovery

- 应用测试固定放在项目根 `test/`，与 `module/` 生产源码分离。
- 测试路径必须是 `test/<component>/<domain>/<topic>.dever` 或 `test/<component>/<domain>/<topic>.dever.md`，不支持更深层级。
- 每个测试文件由文件名选择同名普通函数作为唯一入口。例如 `test/user/account/register.dever` 执行其中的 `register() ()`。
- 测试入口必须是零输入、零输出的 ordinary 函数。缺少同名函数、同名入口签名错误或重复测试身份必须在执行前产生源码诊断。
- 测试文件中的其他函数和类型只属于该文件，不自动执行，也不能被其他测试文件或生产源码调用。
- 不增加 `test` 关键字、`test_` 前缀、每文件 `main()`、package/expose 声明或顶层可执行语句。
- `dever test <project-root>` 同时加载生产源码和测试源码，并在运行任何用例前完成全部语法、名称、类型、分句、effect、失败义务和架构检查。
- 发现顺序按测试身份稳定排序，不依赖文件系统遍历顺序。
- 项目没有 `test/` 或目录为空时，合法项目执行 `dever test` 输出 `0 tests` 并成功退出。
- `check/api/run/build` 不加载测试源码。测试声明不进入生产二进制、API 快照或 HTTP 路由。
- `fmt` 作为源码工具应同时格式化 `module/` 和存在的 `test/`，保持两者统一的 `.dever`/`.dever.md` 格式合同；任一源码失败时不得部分改写。

### Architecture And Visibility

- 测试文件是所属 `<component>/<domain>` 的测试友元，可以调用同领域 App 和 Domain。
- 跨领域调用只能经过目标 App。
- 测试可以读取 App 返回的 Model 值和同领域类型，但不能直接调用 Model CRUD、Adapter、Port 或 API。
- 生产 App、Domain 和其他测试文件都不能反向调用测试函数或访问测试私有类型。
- 测试不能声明 Model、HTTP 路由或生产 App 能力。
- 测试文件必须表达一个内聚业务场景；不建立 `common`、`shared`、`utils`、`helper`、全局 fixture 等测试垃圾桶。

### Assertions And Failures

- 首版只提供测试源码专用的 `assert(condition)` 和 `assert_eq(actual, expected)`。
- `assert` 只接受 `Bool`。`assert_eq` 要求两侧是相同且可比较的类型，不做数值提升、字符串求值或运行时反射。
- `assert_eq` 复用现有比较与渲染合同，支持标量、record、choice、可空值、List、Map、MapEntry 和 Model ID；失败报告测试身份、源码位置、实际值和期望值。
- 不增加 `assert_true`、`assert_false`、`assert_not_equal`、`assert_null`、异常断言或自定义 matcher。
- 业务失败继续使用现有 `result(call)` 和 choice 分句验证；不增加 `try/catch/throws`。
- 测试成功表示入口正常完成；断言失败、未捕获业务失败或程序 fault 表示该用例失败。
- 一个用例失败后继续运行其余独立用例，最终汇总通过、失败和未运行数量，并以非零状态结束。

### Isolation And Lifecycle

- 首版没有 `before/after`、全局 setup、共享 fixture、用例排序依赖或测试间共享状态。
- 每个测试文件在独立原生进程和独立临时项目目录中运行。
- 不触达 Model 的测试不初始化数据库。
- 触达 Model 的测试由运行器生成每用例独立的临时 SQLite 配置，走正式配置解析、迁移、Seed、事务、连接池和 shutdown 路径。
- 测试运行器不读取、复制或连接项目正式数据库 URL；不使用环境变量或命令行数据库覆盖。
- 临时数据库、数据目录、配置、可执行文件和运行产物在成功或失败后由运行器清理，不写入项目 `data/`。
- 首版不使用外层事务回滚代替隔离，因为这会改变被测 App 的真实提交和回滚语义。
- 测试按稳定顺序串行运行；并行、随机顺序、重试和自动超时不属于首版。

### CLI And Reporting

- 新增正式命令 `dever test <project-root>`；当前开发二进制对应 `deverc test <project-root>`。
- 默认输出简洁的人类可读状态和最终汇总。测试进程 stdout/stderr 被捕获，不得冒充 runner 状态；失败时按独立区块展示。
- 源码诊断、原生编译失败、进程启动失败、断言失败、业务失败和 runtime fault 保留各自可辨认的信息。
- 测试复用与 `run/build` 相同的原生后端、runtime feature 裁剪、源码位置和构建缓存，不链接 reference evaluator。
- 首版不增加测试名过滤、机器报告格式或新的数据库配置结构。

### Maintained Example

- CMS 的 Dever 与 Markdown 项目各自增加等价测试，验证账户规范化、Model 写入/Seed 和文章发布等现有 App 合同。
- 两套测试使用相同路径、入口名称和断言，只因源码容器格式不同而不同。
- 旧 `examples/old/` 不迁移为应用测试模板。

## Acceptance Criteria

- [x] `.dever` 与 `.dever.md` 测试均能按 `<component>/<domain>/<topic>` 稳定发现，文件名选择同名零输入、零输出函数。
- [x] 非法路径、缺失/错误入口、重复身份、测试跨域越权、直接 Model/API 调用和生产源码反向访问测试均在执行前被拒绝并定位源码。
- [x] `assert` 和 `assert_eq` 有正反类型检查；相等失败包含测试身份、原源码位置、实际值和期望值。
- [x] 测试可以通过 `result(call)` 验证业务失败；未捕获业务失败、断言失败和 runtime fault 均使当前用例失败而不阻止后续用例。
- [x] 无 `test/` 的合法项目执行 `dever test` 报告 `0 tests` 并成功；非法生产源码仍被拒绝。
- [x] 纯非数据库套件使用 base runtime；混合套件的非数据库用例不读取配置或初始化数据库；使用 Model 的两个测试各自执行迁移和 Seed，写入互不影响，顺序互换结果不变。
- [x] 测试不读取正式 `setting.json` 的数据库 URL，不连接 PostgreSQL，不在项目 `data/` 或源码目录留下临时文件。
- [x] 测试进程成功和失败后均清理临时配置、SQLite 文件、可执行文件和 native 临时目录。
- [x] `check/api/run/build` 的源码集合、API 快照、CMS 运行结果和生产产物不因存在 `test/` 而变化。
- [x] `fmt` 对 module/test 的变更采用同一次准备后提交；任一格式诊断或准备失败时两处源码均保持原样。
- [x] CMS Dever/Markdown 测试覆盖相同业务合同并由 `dever test` 通过。
- [x] 同一项目连续运行两次时测试顺序和汇总一致，生成测试产物不链接 reference evaluator。
- [x] 一次 `dever test` 只生成和编译一个测试套件程序；每个用例仍在独立子进程和临时项目中运行。
- [x] 测试套件复用 native 内容寻址缓存；未改变有效生成代码时再次运行不调用 rustc 编译测试程序。
- [x] 混合数据库/非数据库用例时，只有数据库用例进程读取运行器生成的 SQLite 配置并初始化 Model；非数据库用例不读取配置或初始化数据库。

## Out Of Scope

- 真实 PostgreSQL 应用测试配置、schema 管理和外部数据库清理；继续由现有 `postgres_orm` 专项验收负责。
- 自动 `before/after`、共享 fixture 文件、mock/stub、Port/Adapter 替身和依赖注入协议。
- 测试筛选、watch、并行、随机顺序、重试、自动超时、覆盖率、快照、属性测试和模糊测试。
- 浏览器/UI、HTTP 录制回放、外部服务沙箱和机器可读报告。
- API 新设计、本体、智能体、package registry、跨平台发行和编译器自举。

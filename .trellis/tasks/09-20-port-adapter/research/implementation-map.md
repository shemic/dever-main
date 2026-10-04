# Research: Port / Adapter 最小落地路径

- Query: 按现有编译器、静态合同、共享 wire、应用测试机制实现 bodyless Port、闭合 Adapter 选择和 case-local fake。
- Scope: internal
- Date: 2026-09-20
- Task: `/data/project/dever/.trellis/tasks/09-20-port-adapter`，来自父代理显式分派；本代理 `task.py current --source` 返回 none，未修改 task 状态。

## Findings

### 1. 语法与声明注册

现状：`syntax.rs:168` FunctionClause 的 name 是单个 Name，body 为 Vec；`FunctionKind` 只有 Ordinary/Transaction。`parser.rs:685` 强制单名和 `{}`，不存在可复用 bodyless 声明；`function_contracts:718` 已将 pure/recover 作为 contextual Name 处理，可复用 `at_name`，无需新增全局 token 关键字。Parser 已拥有 source layout，但结构只保留 model/application，可增加 role 或必要布尔上下文。

建议最小 AST：保留普通 name，不将所有函数名改 Path；加 `implementation: Option<Path>`、`failure: Option<TypeRef>` 与明确 bodyless 标记（FunctionKind::Port 或独立 declaration kind 均可，不能用空 Vec 判断，因为普通空函数合法）。qualified definition 的末段仍为 name，前缀保存 Port target。仅 Port role 接受 bodyless/fails；仅 Adapter/Test 接受以 port 开始的 qualified definition。普通函数继续当前推断合同，遇到 fails 直接拒绝。Port 仅一个无模式、无 handler 参数的签名起步，拒绝 transaction/recover、重载歧义和多 clause；若要支持 handler 合同，必须额外定义实现方 failure/effect 参数依赖，不能默默沿用未闭合语义。

推荐语法（其中 `setting` 是新 contextual record 声明，不是 `type setting`）：

```dever
// module/notification/mail/port.dever
type DeliveryError {
  error Unavailable(message: Text)
  error Rejected(message: Text)
}
send(message: Text) () fails DeliveryError

// module/notification/mail/adapter/smtp.dever
setting {
  endpoint: Text
  token: Secret
}
port.send(message: Text) () {
  deliver(message, setting.endpoint, setting.token)
}
// deliver 为同文件 helper；真实外部失败需捕获并映射到 port.DeliveryError

// test/notification/mail/delivery.dever
port.send(message: Text) () {
  assert_eq(message, "hello")
}
delivery() () {
  app.deliver("hello")
}
```

注意：目前 role 目录至少两文件，不能以只含 smtp.dever 的 adapter/ 作为合法单实现例子。单实现使用 adapter.dever（identity default），多个实现再用 adapter/smtp.dever 与 adapter/local.dever。

`check.rs:97-177` 按 qualified name + arity 合并 clauses；qualified 实现必须有自己的物理内部函数 ID，另存 target contract ID，不能注册为原 Port 名或让两个 Adapter 合并成 clauses。`check.rs:236` 先完成全部签名，适合在此后建立 contract/implementation table；bodyless 跳过 clauses coverage/body 输出赋值检查。`check/names.rs:60` 对 qualified 各段独立 snake 检查。`check.rs:485` 现有 Port 一律拒绝应替换成正向声明校验。

### 2. 路径、调用与类型权限

路径已有足够信息：`source.rs:25-105` SourceRole 含 Port/Adapter，Role 保存 component/domain/topics，Test 保存 component/domain/topic。`check/layout.rs` 负责 role 文件/目录互斥、flat topics、至少两文件；不需新目录体系。

`check/symbols.rs:206` can_call 当前：App 能调全部同域 private role；Domain/Model/Port/Adapter 可相互调同域非 App；Test 只能同域 Domain / 任意 App / 自身 Test；API 完全不可作为目标。需要集中 role policy，至少区分 Call 与 Type，不能用同一布尔强行覆盖所有访问。

建议调用矩阵：Main→App；API→本域 App；App→任意 App、本域 Domain/Port；Domain→本域 Domain；Adapter→同文件普通 helper；Test→同文件 helper、本域 Domain、任意 App。Port contract 无 body。生产 qualified implementation 永远不能被 source lookup/handler capture 直接引用。Adapter 不能调其他 Adapter 的 helper，即使同域也不行。Model 操作仍由 `check/orm.rs:247-258` 独立限定本域 App，不能只靠 function call 矩阵，因为 ORM bypass 普通函数 lookup。

`can_access_type:256` 目前先放行 public Model type、自己文件，然后同域 Domain/Port/Adapter 类型互通；因此“Domain 不访问 Model”“Adapter 不反向依赖业务层”必须同时收紧类型 owner，而不是只收紧调用。建议 Adapter 可引用自身类型、本域所实现 Port 的合同类型、public 标准类型；Port 可引用本 Port 类型/public 标准类型，避免通过 App/Domain 造成反向依赖；Domain 可引用本域 Domain 与允许的 App DTO（若保留现有 business error 约定），但不可访问 Model/Port/Adapter。App 可引用本域 Domain/Port、公开 App/Model，不能引用 Adapter setting/helper type。Test 为 fake 新增本域 Port 类型访问，但不新增直接 Port 调用权限。类型矩阵是否允许 Domain 引用 App DTO 与“Domain 只调用 Domain”并不冲突，需明确不要顺手删除现有 App 错误类型引用约定。

重要合同矛盾：`check.rs:559` 拒绝 App 传播 private error 类型。Port 内定义的 DeliveryError 是 private，不能直接从 App 泄漏。最小解法是 App 用既有 result capture + error-only choice wrapper 捕获，再映射 App 自己的公开 error；不要将 Port 类型整体 public 或放开跨域 Port。Port 返回私有 DTO 同理在 App 映射 View。

另一个绕过：can_call 在 `target.bundled` 提前放行所有标准公开函数。单纯角色矩阵不能阻止 App/Domain 直接调用 HTTP/socket/process/file，也不能阻止 Adapter 调用 dever.api.serve。若目标包括“不绕过 Port 外部 I/O”，必须添加 capability 校验并覆盖 direct system intrinsics、别名、handler 与间接调用。现有 effects 是 time/network/blocking 等粗粒度，不能只禁止全部 network（API serve 需要）；应按具体允许的边界/effect owner 定义规则，不从模块名 substring 猜。需要父代理固定本期准确范围。

### 3. HIR / failure / effect / specialization

`hir.rs:11` Program 存摘要向量，`Function:71` 存普通函数签名与 clauses，`CallTarget:513` 仅 Function/Handler。最低侵入路径：Port 仍拥有一个 Function ID，新增 Program.port_contracts / adapter_implementations；调用保留 CallTarget::Function(port_function)，native 对该函数发出 compiler-owned dispatcher。这样签名解析、result、run/blocking、handler reference 继续用一个 ID，不必对所有 CallTarget match 添加 Port 分支。但必须让 contract Function 明确无 body，并在每个分析 owner 注入 Port 摘要和实现边，不能仅生成 native wrapper。

推荐数据：PortContract { function, identity, allowed_failures: BTreeSet<Failure>, implementations: Vec<ImplementationId> }；AdapterImplementation { owner, name, port, operations: BTreeMap<contract_function, implementation_function>, setting_type: Option<TypeId> }；TestCase.fake_bindings: BTreeMap<port_id, implementation_id>。identity 根为 component.domain、topic 为 component.domain.topic；实现 name 为 adapter topic 或 default，不使用 package 路径、源码 ID、排序下标作为 JSON 稳定身份。不同 operation 同名/同 arity 用完整 Port identity 区分。

`contracts/dependencies.rs:11` 是共享依赖 owner：contract→所有生产实现；effects/failure worklist reverse callers 必须看到这些边。`specialize.rs:174` Frame::new 当前只遍历 expression static calls，应扩展 synthetic Port successors；`concrete()` 会枚举所有普通函数，也要避免把 fake 当全局生产实现。native `expressions.rs:310` 统一 call_instance/invoke 可复用；dispatcher 每个分支直接调用具体 specialization，并按分支实际 suspension await。所有候选实现必须参与 reachable/codegen，不能按 build-time setting 剪枝，否则无法不重编译切换且 runtime features 错漏。

failure 不是简单 union：`contracts/error_effects.rs:42` 每轮从空摘要推断；Port 函数需每轮 seed **声明允许集合**，而非实现实际 union。否则一个实现暂未抛出的声明错误会消失，`result(call)` 精确错误集合验证随实现变化，违反合同稳定性。允许集从指定 Shape::Choice 中 `variant.error == true` 变体映射 `Failure {ty,variant}`；不得按名字或消息匹配。实现实际 program.failures 是允许集子集，且必须无未约束 handler error 集。实现不写 fails；实现 errors 超集在 contracts 完成后报错。凡 choice 非 error variant 不属于允许集；建议要求至少一个 error variant。是否允许 error-free Port 需明确无失败语法，当前 PRD “每个显式 choice”没有表达空集合的办法，不能假造 error。

`contracts/effects.rs:228` Effects 摘要要 union 所有候选的 known/recoveries/databases/blocking/suspends/handler dependencies；Port 无 body时目前会推成空。`contracts/failures.rs:571` 为值内 failure obligation，不等于 error_effects 的传播错误，不能混淆。`contracts/facts` / clause bounds 如果 Port 带输出 bounds，需要使用已声明输出保证且严格校验实现，不能因空 body绕过保证。最低范围可拒绝 Port bounds，文档明确，而非静默忽略。

### 4. config 与 Secret setting

`runtime/config.rs:72` Document deny_unknown_fields，目前仅 app/http/database/log；app/log 被丢弃，http 延迟解析。加入 adapter 顶层必须修改此唯一 owner。`Settings::load:138` 使用 serde_json::from_str，map 会丢失重复 key；不要先 from_value 再 wire decode，因为重复 key 已不可恢复。建议 adapter 部分保留 raw JSON 子树或用既有 runtime wire Node 从原文本定位，再交共享 concrete decoder；对 Port key、use、setting 重复/未知字段都严格检查。以 bounded wire parse 先验证整个文档也可，但要确认既有配置大小/深度合同变化。

`wire.rs:7` Policy::SettingInput 已允许 Secret；`Schema::build/from_field` 是 eligibility owner，支持 visible record，不允许 bounded/private fields、Choice/Map/ModelId/Related。`native/wire.rs:7` emit 已产生 `<prefix>_decode(text)` 返回具体 T{id}，SettingInput 不产生 encoder；Secret 使用 from_input(text bytes)，无 need 自制 JSON 转换。register setting 为 owner-private 合成 record（如物理 Adapter 下 Settings），字段本身不可 private，schema fingerprint 包含 policy/nominal identity。setting 字段读取应新 HIR SettingField 或 Setting record expression；禁止赋值、局部 shadow 绕过、从 Adapter 输出 Secret。无需暴露 generic runtime Value 给业务。

JSON 建议仍使用设计稿 `{ "adapter": { "notification.mail": { "use":"smtp", "setting":{...} } } }`：它表达一个 Port identity 选一个实现，该实现完整实现此 Port 的所有 operations。一份 Adapter 实现多个 Port 时，setting 生命周期/多份配置 ownership 尚不一致，MVP 应禁止并以明确诊断说明，或先规定每 Port 独立配置，不能暗中跨 Port 复用某一个 setting。

单实现无 setting 时可完全省略 adapter 条目；单实现有 setting 时条目/setting 必需，use 可省略；多实现必需 use；若配置 use 必须匹配。未选实现 setting 不解码，但所有候选源码/Schema 要检查。未知 Port identity 需按编译闭合表拒绝。选中实现无 setting 时若提供非空 setting 应拒绝（建议完全不允许 setting key，规则更清晰）。错误上下文只报 Port/Adapter/字段路径，不回显原值。

`config::bootstrap:272` 当前将 settings/数据库初始化与 OnceLock 绑定；`native.rs:96` 仅有 profile 才调用 database setup。Port-only 应用也要初始化选择，不能搭便车要求 database 或 HTTP。建议从数据库 setup 抽取共享 settings 初始化/读访问，再生成 Adapter 初始化，保持只读固定绑定；按全部 required bindings 校验完再执行用户 main 或任何外部 I/O。generated immutable OnceLock<closed enum with concrete settings> 符合“一次启动固定”且不是可变 service registry。Secret 不可 Debug/render、不要 derivation泄漏。

`native/build.rs:346` runtime feature selection 与 generated code关联；所有候选 emission保证 HTTP/crypto特征不丢。`native/build/cache.rs:163` cache 包含生成源码和 runtime 输入；选择值不应进入生成源码，metadata/schema/候选集合必须进入。CLI run/build 会预读部署 Settings，须允许新 adapter section且 build不把 use 静态固化；测试运行时不读取部署 Settings。

### 5. case-local fake

`hir.rs:28` TestCase 只有 checked function/span/database metadata；`check.rs` finalize_tests 在 contracts 后；适合加入 fake table 以及每 case Port reachable 验证。相同测试文件就是一个 entry/TestCase，qualified fake 私有实现归属该 owner。fake 与生产实现都走同一个签名/allowed failure checker，fake 不加入生产 adapter table；无 fake的可达 Port check 失败，即使生产恰好单实现也不能退回。

`native.rs:142` suite 单次 emit_definitions 全部 test roots，selected index选择 case；`test_runner.rs:70` 每 case 独立进程/临时项目，仅数据库 case写隔离SQLite setting，现成强隔离可复用。

建议使 specialization 带 immutable binding context（Production / Test(case id)），并在传播 App/helper/handler调用时保持此上下文，Port 在 Test context仅追加对应 fake successor。同一个 App 被两个 case调用将生成两个必要 specialization，不会共享错误 fake。alternative 是每进程选定 immutable fake dispatch enum，但 suite静态 effect/reachability必须仍排除生产实现，并处理同 App不同 fake suspension；该路线较易误把生产代码/features带入 suite。切忌用可变全局 fake registry且每 case reset。

静态生产合同仍是生产实现 effect union，fake不得让业务合同缩窄；测试 codegen可使用各 fake实际 suspension决定 await，但函数签名/specialization应一致，必要时保守 async化。finalize_tests 的 database metadata 要基于 Test context可达集合，防止从生产Adapter推导数据库（理论上Adapter禁止Model操作，但其他effects同理）。测试缺fake检查应检查经App、普通helper、静态handler、run/blocking所有到达路径，不能只扫Test直接calls。

### 6. Markdown / formatter

`format.rs:264-309` 从token重新组装函数名、pure/recover、body；需要输出qualified path、fails类型、bodyless无花括号，保留注释消费顺序。setting 新 Declaration 分支须覆盖 formatter、markdown::declaration_span、source section validation、names及所有 Declaration exhaustive matches。

`markdown/contract/model.rs:36/60` 文档函数仅name/inputs/outputs；`metadata.rs:52` strict ordered元数据，建议Port段新增 `- 允许失败：` 只在Port存在，与 AST failure choice精确比较；Adapter段 name 使用完整 `port.send`，无额外猜测。setting段作为独立record-like Section，字段类型/顺序/数量与源码一致。`validate.rs:232` 用 owner + 最后一段函数名 + arity寻找HIR，会错误定位qualified实现，改为AST declaration span或显式 declaration↔HIR ID映射，不能 `.rsplit('.').next()` 猜。public_functions metadata继续角色导出规则，不能把实现列为公开App能力。

### 7. 建议分阶段实现与最小定向测试

1. AST/parser/formatter/Markdown + contract/implementation registration + role矩阵；只检查，不先运行native。正反例包括Port body、普通fails、qualified非法owner、缺/重复operation、参数类型/输入名/输出名/顺序/arity不匹配、跨域target。
2. 固定failure declaration seed、effect union、synthetic dependencies、handler/run/transaction分析；测试一个blocking实现+一个suspending实现的保守拒绝、错误子集/超集、捕获声明全集，生产配置不能改变合同。
3. shared wire setting + strictJSON选择 +直接native dispatch；确定性本地实现两分支，不联网；同一二进制仅换配置得到不同结果、未知use、缺use、未知字段、重复key、错误类型、未选配置不需存在、选中Secret错误不回显。
4. TestCase绑定与specialization context；两个测试同一个App各自fake结果、同套件单编译、无fake失败、生产配置坏/含真实地址仍不读取、生产实现不作为fallback、case执行失败不影响后续。
5. 同步LANGUAGE/Markdown/compiler/toolchain与root test fixtures，独立复核。

现有入口：`test/dever-tests/tests/{source_architecture,source_visibility,error_effects,structured_concurrency,application_config,application_testing,markdown_source,formatter}.rs`。建议新 `port_adapter.rs` 聚合端到端闭合合同；只运行必要exact测试和cargo check，无真实HTTP/数据库/全量。旧source_architecture中“port reserved”和App→Adapter正例要按新合同更新，不能保留兼容开关。

## Related specs

### 补充：建议本期固定的执行决定

Port failure 合同建议本期采用声明全集、实现子集、App显式映射三条规则。普通App不透传Port私有错误；`fails DeliveryError` 只提取error variants，result仍要求全集匹配。先要求每个Port都有至少一个error variant，error-free语法另行设计，避免本期引入第二种fails语义。

“App不能绕过Port”必须本期实施，否则PRD目标未满足。最小可执行限制是：App/Domain禁止未经Port边界的 `file` 和 `network` intrinsic可达路径；Domain的Model操作另由既有owner检查拒绝。复用 `contracts/effects.rs:547` 的穷尽intrinsic_effect分类，不另写模块名白名单。新增一个内部摘要 `unmediated_io`，沿普通函数和具体handler传递，但穿过Port contract时清空这一项；正常effect/blocking/suspension摘要仍完整union，不能一并清空。这能允许App→Port→Adapter→HTTP，并拒绝App→helper/标准包装→HTTP。检查direct intrinsic同样经过此摘要，handler绑定用既有specialization保守展开。Adapter只允许自己的helpers/标准库，仍需单独禁止 `ApiServe`，并将该入口限制为Main显式启动；不要把整个network类禁在Adapter上。日志、时间、随机、密码散列、process.arguments本期保持已有授权，不把“Port约束”扩大为全新纯函数制度。若希望Domain完全无任何副作用，属于更强约束，不能偷偷从本期文件/网络隔离推出。

- `.trellis/workflow.md`：研究仅写task research；main负责后续 implement/check。
- `.trellis/spec/backend/compiler-contracts.md`：source角色、App公共边界、failure/effects/shared dependencies、Model owner、Test权限、wire sole owner。
- `.trellis/spec/backend/toolchain-and-library.md`：Markdown严格合同、native cache/feature选择、配置只用setting.json、Secret不可观察。
- `.trellis/spec/backend/directory-structure.md`：目录/role归属。

## External references

本研究仅依据当前仓库，不引入新依赖，不需要外部版本或API资料。

## Caveats / Not Found

- 当前没有 bodyless contract、qualified实现、setting record、Port dispatch 或 fake机制；均不能靠修改一条调用矩阵完成。
- PRD未给error-free Port表示法、App绕过Port的标准库I/O精确限制、跨Port Adapter setting owner；这些应在实现前由主代理用既有范围做一致决定并同步设计。
- 明确区分“Port已声明failure全集”和“所有实现实际failure并集”；后者不能替代前者。
- 查不到单独 `.trellis/spec/backend/application-testing.md`，应用测试规则当前在compiler-contracts与toolchain文档；不要引用不存在的规范文件。
- 未运行测试，未修改源码/规范/状态。

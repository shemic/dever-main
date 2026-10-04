# Dever 语言实现原理

本文解释 Dever 从源码到可执行程序的完整工作方式。语法、标准包接口和应用示例见 [Dever 语言开发指南](LANGUAGE.md)。

这里描述的是 Dever 必须长期保持的架构和语义边界，不依赖某一种引导工具链，也不公开内部临时产物格式。只要这些边界保持不变，编译器内部可以替换实现而不影响 Dever 源码。

## 1. 设计目标

Dever 的实现围绕几个约束展开：

- 源码中的错误尽量在运行前发现。
- 条件分句由编译器证明互斥、可达且完整，不依赖书写顺序碰运气。
- handler 是静态绑定的行为参数，不是运行时函数对象。
- 普通数据遵循值语义，文件、网络连接和 Stream 等资源遵循共享身份语义。
- JSON 等算法使用 Dever 源码；HTTP 复用 Hyper 协议引擎。运行时边界不包含应用业务规则。
- 生产程序走提前编译路径，不携带通用解释器、语法分析器或类型检查器。
- 默认离线工作，不在检查、运行或构建时隐式下载依赖。

这使 Dever 的公开语言契约和编译器内部实现分离：前者应当稳定，后者可以持续优化。

## 2. 总体流水线

一个 Dever 程序会经过以下阶段：

```text
用户 .dever / .dever.md 源码 + 官方 .dever 标准包
                         |
                         v
              确定性源码加载与位置记录
                         |
                         v
                 词法分析 -> 语法分析
                         |
                  +------+------+
                  |             |
                  v             v
              源码格式化     全局语义检查
                                  |
                                  v
                         类型化中间表示 HIR
                           /             \
                          v               v
                 规范执行器（测试）   可达性分析与静态特化
                                              |
                                              v
                                      原生后端与本机链接
                                              |
                                              v
                                   独立程序 + 最小运行时
```

语法分析成功只说明源码结构合法，不表示程序可以执行。名称解析、类型推导、分句证明、确定赋值和调用检查全部通过后，编译器才会生成可执行的类型化中间表示。

## 3. 源码加载与位置模型

编译器先递归加载指定源码根目录中的 `.dever` 和 `.dever.md` 文件，并把随编译器提供的官方 `.dever` 包加入同一个编译单元。普通 `.md` 不加载。

加载阶段遵守以下规则：

- 文件路径按稳定顺序排序，因此诊断顺序不受文件系统遍历顺序影响。
- 源码必须是有效 UTF-8，非法内容在进入语法分析前报错。
- 不跟随源码树中的符号链接，避免同一文件被重复或越界加载。
- 每段源码都带有来源身份；用户源码不能伪装成官方保留包。
- 每个 token、语法节点和后续 HIR 节点都保留源文件位置。

内部位置使用半开字节区间，展示给开发者时转换成从 1 开始的行号和按 Unicode 字符计算的列号。这样既能高效切片源码，也不会把多字节字符显示成错误列号。

Markdown 是带强制说明契约的源码容器。编译器通过 CommonMark 解析库识别顶层 `dever` 或 `typescript dever` 围栏，并把唯一的 H1 映射到 package、把每个 H2 映射到一个 type 或逻辑函数。结构阶段检查固定说明列表、代码块数量和声明归属；函数签名解析后，再核对公开清单、调用方式、类型结构以及函数输入输出。引用、列表、HTML 和展示代码块中的伪围栏不执行；程序围栏未闭合、没有程序块、声明跨块或说明与代码不一致时明确失败。每个包仍由一个文件完整拥有，不能把 `.dever` 和 `.dever.md` 的同名包合并。

`SourceFile` 的原文用于诊断、语义缓存和 CLI 格式化快照。程序块的 token、AST、HIR 及原生故障保持该文件的字节位置，不能把临时抽取文本的行列暴露给开发者。Markdown 解析依赖仅在编译器中使用，不进入最终程序的最小运行时。

## 4. 词法、语法树与格式化

### 4.1 词法分析

词法分析器把文本转换为关键字、名称、字面量、标点和注释。字符串转义、数字拼写和注释边界在这一阶段确认，但名称含义和类型仍未判断。

每个 token 都保留原始位置。后续阶段不需要重新猜测错误来自哪段文本。

### 4.2 语法分析

语法分析器只负责回答“源码的结构是什么”，生成保留源代码意图的语法树，例如：

- 从文件相对路径推导的 package 身份，以及 type/function 声明各自的 `public` 标记；
- type、function 和多个 function 分句；
- pattern、表达式和 function body 中的线性语句；
- 显式括号、注释和 record 字段顺序。

语法树不会提前把未解析名称变成具体声明，也不会把表达式强行塞进通用运行时值。名称和类型属于语义检查器的职责。

### 4.3 格式化

格式化器直接基于语法树工作，不依赖类型检查。它必须保留：

- 注释内容和相对归属；
- 字面量的实际值；
- 会改变结合关系的括号；
- 源码中 record 表达式的字段顺序。

格式化结果要求幂等：连续格式化两次，第二次不能再产生变化。

Markdown 先检查 H1/H2 说明结构和完整包语法，再按原代码块边界格式化其中的声明和注释；只替换程序块内部，保留说明、围栏及展示代码块。名称和类型的说明一致性由语义检查负责。格式化与编译使用相同的代码块识别、说明结构和声明边界，不能各自实现一套 Markdown 提取规则。

对一个源码根目录执行写入格式化时，编译器先解析全部目标文件，再暂存每个结果，最后逐文件原子替换。任何文件解析失败时都不开始写入。这里保证的是单个文件不会留下半写内容，不承诺多个文件构成操作系统级事务；替换途中发生系统错误时，已经完成的文件不会整体回滚。`fmt --check` 只比较结果，不修改文件。

## 5. 全局语义检查

Dever 不是逐文件解释语言。编译器先收集整个源码根目录和标准包，再统一检查。因此声明可以跨文件引用，但错误也会在全局检查时一起暴露。

语义检查按依赖关系分阶段进行：

1. **注册声明头**：建立 package、type、function 和公开成员索引。function 以名称和输入数量共同标识。
2. **解析类型形状**：确认 record、choice、Nullable、List、Map、Stream 等类型组合是否合法，并检查递归类型约束。
3. **建立 function 签名**：合并同名同输入数量的分句，推导或校验输入输出契约。
4. **规范化 pattern**：把源码 pattern 转换成可证明的符号域。
5. **证明分句集合**：检查分句是否重叠、不可达或不完整。
6. **检查 function body**：完成名称解析、表达式类型检查、确定赋值、稳定局部变量和 handler 调用检查。
7. **检查依赖环**：拒绝 package 循环、递归 type，以及 function 的直接或间接递归；这条规则同样适用于用户源码和官方 `.dever` 标准包。
8. **构造 HIR**：只有没有错误时，才形成可交给执行后端的完整程序。

这种分阶段结构避免把“尚未收集到声明”误报成“声明不存在”，也使同一套符号和类型信息能够被所有文件复用。

## 6. 条件分句如何工作

Dever 没有传统的 `if`。一个 function 的多个分句共同描述完整输入空间：

```dever
type Result {
  Ok(value: Int)
  error Failed(message: Text)
}

show(result: Result.Ok(value)) (text: Text) recover("render result as user-visible text") {
  text = "value=" + int.to_text(value)
}

show(result: Result.Failed(message)) (text: Text) recover("render result as user-visible text") {
  text = "failed: " + message
}
```

编译器不会简单地从上往下寻找第一个匹配项。它先把每个 pattern 转换成符号域：

- Bool 和不带 payload 的 choice 是有限原子集合；
- Nullable 在原类型域之外增加 `null`；
- 带 payload 的 choice 先选择变体，再为 payload 建立绑定；
- Text 和数值常量代表单点集合；
- Int、Decimal 范围代表区间；
- `other` 代表同一 function 分句组中、同一输入位置全部显式 pattern 的补集，与它写在第几个分句无关。

多输入 function 的一个分句是多个输入域的笛卡尔积，可以理解为一个多维矩形。编译器通过矩形相减计算尚未覆盖的区域，不会枚举无限数值域中的每个值。

由此可以在编译期证明三件事：

- **互斥**：任意两个分句的输入域都没有交集。
- **可达**：每个分句规范化后的输入域都非空；空域分句没有任何输入能够到达。
- **完整**：全部分句输入域的并集等于 function 的整个合法输入空间。

实现上可以逐个从完整输入域中减去分句矩形，以计算交集和剩余区域；这个处理次序只用于算法和诊断，不形成“先写者优先”的语言规则。通过证明后，后端可以自由生成决策树或其他高效控制流。源码顺序不参与语义选择，因此调整互斥分句的顺序不会改变结果。

## 7. 类型化中间表示 HIR

HIR 是语法树和执行后端之间清晰、集中的内部语义契约。它不是公开文件格式，不承诺序列化或跨编译器版本兼容，也不需要在程序运行时保留。

与语法树相比，HIR 中已经没有以下不确定性：

- 名称已经解析成确定的 package、type 或 function 标识；
- 每个表达式都有具体类型；
- 每个局部名称都有确定存储位置和生命周期；
- 每组分句都有规范化输入域；
- 直接调用和静态 handler 调用已经区分；
- 每个节点仍能追溯到源码位置。

HIR 同时承担两个职责：一方面为生产后端提供类型完整、无需再次猜测的输入；另一方面供测试使用的规范执行器执行，作为生产后端行为的对照。

### 7.1 为什么不从语法树直接生成程序

若后端直接读取语法树，它就必须重复名称解析、类型判断、pattern 选择和错误处理。多个后端会逐渐产生不同语义。

把这些规则集中在语义检查器并固化为 HIR，可以保证：

- 一个语言规则只有一个拥有者；
- 执行后端只处理已经证明合法的程序；
- 更换后端不需要改变 Dever 源码；
- 规范执行器和生产程序可以对同一个 HIR 做差分验证。

## 8. 从 HIR 到生产程序

生产路径采用提前编译，而不是把源码或通用 AST 带到运行时。

```text
类型化 HIR
    |
    v
选定零输入公开入口
    |
    v
计算入口可达的 function 与类型
    |
    v
展开静态 handler 绑定并生成具体实例
    |
    v
降低为原生后端的具体控制流和数据表示
    |
    v
本机优化、链接最小运行时
    |
    v
独立可执行程序
```

语义检查仍会检查编译单元中的所有声明，避免未使用源码长期积累错误；代码生成则只保留入口可达的实例，避免把无关标准包和编译器能力塞进最终程序。

最终程序包含具体 record、choice、循环、调用和必要的系统原语，不包含：
- Dever 词法分析器或语法分析器；
- 类型检查器和分句证明器；
- 规范执行器；
- 通用动态 `Value` 分派层；
- 运行时反射注册表；
- 每个元素调用一次的虚拟 handler 分派。

构建期间产生的后端材料只存在于独占临时目录中，成功或失败后都会清理。这里的“独立程序”是指运行时不需要 Dever 源码或编译器继续参与，不表示文件能跨操作系统运行，也不承诺完全静态链接；它仍可依赖目标操作系统正常提供的系统接口和动态库。

## 9. 静态 handler 与特化

handler 用来把行为作为参数传入，但它不是闭包、函数指针或普通一等值。最外层调用必须把它绑定到编译期可确定的具名 function，中间 function 可以继续转发自己的 handler 参数：

```dever
total = reduce(add, numbers, 0)
```

编译器为每个实际绑定组合生成一个具体实例。可以把实例键理解为：

```text
原 function + 按声明顺序排列的具体 handler 绑定
```

例如，同一个 `reduce` 分别绑定 `add` 和 `max` 时，会得到两个可独立优化的实例。实例内部直接调用目标 function，不需要运行时查询 handler 是谁。

静态特化还会：

- 沿调用链继续解析 handler 转发；
- 只生成入口可达的绑定组合；
- 复用已经生成的相同实例；
- 在仍处于展开状态时发现同一实例再次依赖自身，并按当前禁止递归的规则报告编译错误。

这种模型牺牲了任意运行时闭包的动态性，换来简单的类型系统、可预测的生命周期和直接调用性能。

## 10. 值、资源与生命周期

### 10.1 普通值

Int、Text、Bytes、List、Map、record 和 choice 等普通数据在语言层面遵循值语义。把一个值赋给另一个名称后，后续观察不能因为某处内部修改而意外改变另一个名称。

实现不必为每次赋值立刻深拷贝。后端可以使用共享不可变存储、写时复制和最后一次使用分析：

- 只读值可以共享底层数据；
- 真正需要修改且仍有别名时才分离；
- 一个值最后一次使用时可以直接移动；
- 优化不能改变相等性、遍历顺序或错误行为。

Map 的插入顺序属于语言可观察语义。相等性、显示、删除和重新插入都必须遵守同一顺序契约，不能由后端容器的偶然行为决定。

具体来说，覆盖已有 key 不改变位置；删除后再次写入同一个 key，则把它作为新条目追加到末尾。两个 key/value 相同但插入顺序不同的 Map 不相等。

后端在已检查 HIR 的副本上合并安全的记录更新。只有后续旧记录读取不与已更新字段重叠时，才把旧局部直接转交给新局部；调用后仅剩标量字段读取时，先保留这些标量。字段赋值允许移动右侧最后一次读取的同一字段。旧别名仍需保留时继续复制，资源所在作用域不变。

### 10.2 资源值

File、Socket、Listener 和 Stream 等资源不是普通可复制数据。它们的多个别名共享同一资源身份。按资源类型不同，共享状态包括：

- 当前游标、消费位置或耗尽状态；
- 打开和关闭状态；
- 底层系统句柄；
- 尚未消费的输入。

任一别名关闭资源后，其他别名也必须观察到关闭状态。资源由作用域清理和显式 `close` 共同管理，不依赖通用垃圾回收器决定何时释放系统资源。

资源可以作为 record 字段或 choice payload。复制外层聚合仍遵循值语义，但其中的资源字段只复制对同一资源身份的引用，不复制文件、连接或消费状态。普通数据与资源身份因此保持清晰边界。

### 10.3 数值一致性

数值规则由语言定义，而不是交给后端默认行为决定。编译期常量计算、规范执行器和生产程序必须经过同一组数值语义：

- Int 运算检查溢出，除法和余数遵守 Dever 规定的边界行为；
- Decimal 使用固定精度、舍入和非法操作规则；
- Float 保留 IEEE 754 的 NaN、无穷和有符号零行为；
- `sum` 和普通表达式按源码规定的从左到右顺序计算，不能用会改变结果的重排或快速数学优化。

后端只有在证明不会改变这些结果时才能省略重复检查。静态可知的故障在检查阶段报告，依赖运行值的故障由生产程序携带源码位置报告。

## 11. 序列、Stream 与并发

List、Bytes 和 Stream 复用一部分序列操作，但各自允许的行为有明确边界：

- List 直接遍历已经存在的元素，支持 `each`、`filter`、`find`、`sum`、`reduce` 和 `reduce_until`。
- Bytes 直接产生 Int 字节值，不先转换成 List，支持 `each`、`reduce` 和 `reduce_until`。
- Stream 通过拉取接口按需产生下一项，不预先物化全部数据，支持零输出 `each`、`reduce` 和 `reduce_until`。

`each` 在 List 或 Bytes 上绑定单输出 handler 时承担映射职责并产生 List；Dever 没有另一套独立的 `map` 操作。`filter`、`find` 和 `sum` 当前只接受 List。

编译器内部使用统一的序列分类来复用控制流生成，而不是为每个序列操作复制三套实现。

`reduce_until` 在 handler 表示停止后立即结束，不会为了探测结束条件而额外拉取下一项。因此后续操作仍能消费那一项，这对文件和网络流尤其重要。

`parallel_each` 的并发边界是显式且有界的：

- 调用方拥有唯一生产者，Stream 不会被多个 worker 同时拉取；
- 非挂起 handler 的 worker 数量为 1–256，挂起 handler 的任务组容量为 1–65536，另受全局任务上限约束；
- 待处理队列容量与 worker 数量绑定，不会无限积压；
- worker 在调用结束前全部回收，不产生脱离作用域的后台任务；
- 记录第一个程序 fault，并在后续拉取或分派边界停止继续分发；
- 返回前等待全部已经启动的工作结束，不强制取消正在阻塞的系统调用；
- 为避免共享消费状态，context 和单项输入都不能递归包含 Stream。

普通序列操作保持确定性顺序。并发 handler 的实际完成顺序不作为语言可移植语义的一部分。

独占普通列表通过消费迭代转移元素；共享列表只在拉取时复制当前元素，提前停止不复制剩余部分。直接消费 `text.split` 的遍历使用同一运行时分割规则，避免完整中间列表。相邻单次使用的集合阶段可以融合，但前置 handler 必须同时非挂起、无副作用、无程序故障；仅有 `pure` 不够，整数溢出、错误顺序和 Float 归约顺序仍需保留。挂起 handler 不进入同步迭代器融合。

List/Bytes 的并行动作按有限小批次分发，每批最多 256 项；Stream 仍逐项拉取。两者共用有界调度和故障取消状态，批内也检查取消，返回前回收全部线程。

### 11.1 异步任务、多线程与 Channel

挂起能力沿用同一条编译流水线：parser 只建立统一函数语法，checker 通过共享依赖图推断调用和 handler 的挂起 effect，并检查可传递性与 Task/Group 仿射消费。HIR 保存内部 `suspends` 和具体并发操作，原生后端再生成 Rust future。Task 输出类型和静态 handler 目标都在 HIR 中确定，不引入通用 Future、动态 Value 或运行时 handler 查找。

完全非挂起的入口直接调用生成函数，不创建 Tokio。推断为挂起的入口调用 `dever_runtime::task::run_entry`，统一创建一个默认有界 runtime；业务源码不能通过 `dever.task.start` 手工创建或嵌套 runtime。宿主嵌入继续使用 `run_entry_with`。运行时集中拥有调度线程、CPU/阻塞 worker 和子任务上限；两个线程数均为 1–64，默认 CPU/阻塞 worker 最多 8，默认活动子任务 4096，任务上限为 65536。每次 `run` 不创建 runtime 或系统线程。

执行类别保持分离：普通调用顺序完成，并由后端按目标 effect 生成直接调用或内部 `.await`；`run` 建立受父作用域约束的 Task；`wait` 消费 Task/Group；`parallel` 把 pure 非挂起计算提交到有界 CPU 通道；`blocking` 只承接编译器已识别的阻塞系统调用。effect 检查会穿过普通调用、静态 handler 替换和集合 handler，防止文件、同步睡眠、标准输出或嵌套同步并行留在 Tokio 调度线程。List/Bytes 的命名非挂起 `parallel_each` handler 在挂起路径中进入 blocking 通道；挂起 handler 复用 AsyncStream 的 Group 路径。完全非挂起的 `parallel_each` 保持作用域线程实现。参数先按源码顺序求值，再移动到工作闭包，避免改变故障和副作用顺序。

根入口和每个由 `run`/Group 启动的任务都有内部 Scope。业务 future 与不可被业务 Task 直接中止的 supervisor 分离：`stop` 或析构只中止业务 future，supervisor 继续回收其子任务和已经启动的 blocking 工作，完成信号交回父 Scope。入口返回 `Ok` 或 `Err` 前都会取消并等待剩余工作，因此不在 `Drop` 内执行 `block_on`，也不会让阻塞副作用在 `stop` 返回后继续。

Group 使用正容量限制和 `JoinSet` 拥有动态零输出 supervisor。容量满时先回收一个已完成任务形成背压；首个故障会取消并清理其余成员。显式 `stop` 执行取消并等待。Group 在异常路径析构时分离 supervisor 句柄，但父 Scope 仍持有完成登记并负责等待；编译器同时拒绝正常分句出口遗留未消费 Task 或 Group。

Channel 由共享有界队列、可用槽位/数据量信号量和短临界区组成，不为等待方创建线程。关闭会同时唤醒发送方和接收方，缓存值仍按 FIFO 排空，之后 `receive` 返回 `null`。类型系统不表示嵌套可空值，原生后端复用 List/Map 可空结果的压平规则处理 `Channel<T?>`。容量、元素可传递性和挂起上下文由编译器负责；运行时只拥有队列、唤醒和关闭状态。

同步 `dever.time.sleep` 保持阻塞系统原语。官方 `dever.task.sleep` 映射到 Tokio timer。文件和同步 Stream 通过明确的 blocking 边界隔离；TCP 使用相同 runtime 的 I/O driver。

`timeout` 和 `race` 复用 Task 的 supervisor 所有权，等待时只借用其句柄，取消路径仍能请求停止并排空。timeout 到期后才调用匹配输出签名的零输入 async fallback；race 收取胜出任务后取消并等待其余任务，程序故障不能被竞争结果掩盖。编译器将它们纳入仿射消费、静态 handler、effect、失败义务和命名输出检查。

List/Bytes 的 async 顺序 handler 在原有序列生成路径逐项等待。`stream(Channel/List/Bytes)` 和 `ticks` 复用 AsyncStream 的 producer，不额外预取或创建任务；Channel 流保留实际 null 元素，关闭只释放自己的别名。周期流使用延迟首次触发和跳过错过周期的 Tokio interval。

### 11.2 AsyncStream 与 TCP

`AsyncStream<T>` 保存一个 Send 的 stream producer，使用 futures-util 的 unfold/map 和 Tokio 异步锁；producer 只在创建时装箱，拉取不逐项分配 boxed future。别名共享游标和关闭信号，close 唤醒挂起的 pull；组合器在结束、故障和取消时释放生产者。流不预取，读取缓冲区在同方向读锁可用且 socket 可读后才分配。

异步流的 `parallel_each` 复用结构化 Group，先取得组内和全局容量，再拉取下一项；等待生产者期间也收集 handler 故障，随后取消并等待其他成员。List/Bytes 的 CPU 并行仍使用共享 blocking 通道，网络 handler 则生成具体 async future，不创建独立线程池。

TCP Socket/Listener 由 net 模块统一拥有资源关闭和唤醒。Socket 的读写锁分开，允许双向同时工作；一次写入保存偏移，写出部分内容后的取消、超时或故障关闭连接，后续写入不能接在残缺内容后继续。DNS 进入已有有界 blocking 通道；数值地址直接使用 Tokio。HTTP 采用 Hyper HTTP/1 和 HTTP/2，连接作用域统一拥有协议任务、流式响应/SSE 和普通 WebSocket。

## 12. 标准包与系统原语的边界

Dever 的标准能力分成两层：

```text
应用 .dever 源码
       |
       v
官方 .dever 标准包：公开类型、校验规则、算法、状态组织
       |
       v
dever.system.*：有类型的最小系统原语契约
       |
       v
最小运行时：文件、网络、时钟、进程等原子系统操作
```

官方 `.dever` 包和用户源码经过相同的解析、类型检查、分句证明与代码生成。它们不是绕过语言规则的隐藏实现。

只有同时满足下列条件的能力才应进入系统原语：

- Dever 源码无法自行完成，例如打开文件或创建网络连接；
- 操作能定义成小而稳定的有类型契约；
- 不把高层业务算法、协议状态机或公开数据结构藏进运行时。

系统原语由一份穷尽目录统一管理。检查器、规范执行器和生产后端必须同时实现同一条目，避免某个后端悄悄支持额外行为。

新增标准能力时，应先问“能否用现有 Dever 语法和原语实现”。答案为可以时，它就属于 `.dever` 标准包，而不是新的隐藏内建函数。

## 13. JSON 算法与 HTTP 引擎边界

JSON 和 HTTP 是验证语言基础能力是否完整的两类代表性功能。它们需要字节处理、受控重复、choice、record、错误传播、Stream、资源和有界状态，但不应要求专用语法。

### 13.1 JSON

JSON 的公开模型和算法由 Dever 源码定义。解析过程使用序列归约完成扫描和状态推进，写出过程遍历同一公开文档结构。

当前文档模型使用扁平、无环的节点表：

- 节点 ID 连续；
- 子节点总是先于父节点出现；
- 引用只能指向更小的 ID；
- number 可以保留原始文本拼写；
- 解析深度、输出大小和非法引用都有显式失败结果。

这种表示不依赖运行时反射或任意递归对象，也便于 Dever 自身验证结构是否合法。

### 13.2 HTTP

N2 将 HTTP 协议边界交给成熟 Hyper 1.x 引擎；公开 Request、Response、Header、Limits 和结果类型仍在 `library/dever/http.dever`。旧 Dever wire/field/transport 解析模块已删除，无备用实现。JSON 仍使用 Dever 源码算法。

`dever.system.http_serve` 的 handler 沿用静态绑定和 specialization。HIR Intrinsic 与 Collection 共享 callback 依赖提取；调用环、包依赖、effects/recovery 和 native 可达性都包含该 handler。校验器检查 HTTP 名义记录的字段类型、名称和可见性，native 按字段名生成双向转换；不存在运行期通用 Value 或动态路由解释器。

运行时使用 Hyper HTTP/1、HTTP/2 connection API 与 hyper-util 的 TokioIo/Timer。服务端从已有 Listener 接受独占 Tokio TCP 流，复用 AsyncStream 和 Task/Group 先预留容量再接受连接。连接和 handler 子任务归入现有 Scope，停止服务会等待清理。对端错误按协议终止对应流或连接，程序故障沿原位置向父任务传播。

单次 send 在同一请求 future 内同时推进 Hyper driver 与响应读取，完成或取消即释放连接；可复用客户端由 13.4 节的池拥有，不自动重试。服务端持久连接、流水线和 chunked 输入由 Hyper 处理；静态业务 handler 仍在编译产物中直接运行。

Bytes 使用 bytes::Bytes 共享缓冲区，切片不复制，独占 Text/Bytes 仍可转移存储；别名写入保持值语义。缓冲正文受 body_bytes 限制，单帧共享、多帧增量合并；重复头使用 List，值使用 Bytes。头部、正文、handler 和停滞写入各有期限。N3 提供流式响应、SSE 和 WebSocket，N4 提供 TLS 和客户端池，N5 接通 HTTP/2；精确接口见 LANGUAGE.md。

N5 的 Limits.http2 显式选择 HTTP/2，protocol.rs 集中配置 Hyper 的固定流控窗口、流数、16 KiB 帧/发送缓冲和头预算。HTTP 专用 ALPN 配置从共享 TLS 配置派生，HTTPS 必须协商 h2，明文使用 prior knowledge。消息编解码仍共用 message.rs；HTTP/2 authority 规范化到 Host，source target 只保留路径和查询，不暴露协议伪头。

http/executor.rs 将 Hyper H2Stream、ConnTask 和发送任务接到 task/scoped.rs。每个 Hyper 任务都有自己的 Dever 子 Scope；连接 owner 的不可取消 supervisor 排空所有后代后才释放物理资源。关闭 admission 和新任务登记使用同一把短锁，故障报告与注销都包含在任务完成通知之前。等待关闭的 future 被取消时，cleanup supervisor 仍由父 Scope 的 completion 登记持有。HTTP/2 handler 错误需要显式报告，因为 Hyper 会在流内部消费该错误。

### 13.3 流式响应、SSE 与 WebSocket

N3 通过 `serve_live` 接入零输出静态 handler 和 HttpReply，共用请求解码、响应校验和 HTTP 连接配置。handler 的挂起 effect 由调用图推断。HTTP/1 每连接以容量 1 的 Group 拥有 handler；HTTP/2 每请求流独立持有 Group，并归该流的 Scope。正文通道只有 1 个槽位。响应体释放信号、Session guard 和 Scope 一起处理 HEAD、提前结束、故障、断开及取消；pending head 被重置时也必须结束该 handler。HTTP/1 升级后 Hyper driver 完成，连接 owner 继续等待 WebSocket handler。

SSE 编码按 EventSource 行规则保留空 id 和尾部换行，先计算展开长度再分配。心跳由响应体被拉取时的 timer 产生，不创建定时任务。WebSocket 复用 tokio-tungstenite 0.30.0 的 handshake 功能。Hyper 升级后取回原 TCP/TLS Transport 和预读帧，释放 HTTP 写期限；WebSocket 用独立方向锁、4 KiB 读缓冲和有界消息/写缓冲。Endpoint 与 TCP 共享关闭和唤醒语义；帧写入的取消 guard 只在取得方向锁后生效。

HttpReply/WebSocket 纳入资源传递、效果、API/Markdown 类型名称和 Render；静态桥接复用按字段名转换，扩展到 SSE 可空字段和 WebSocket Message 选择项。没有动态 Value、额外解释器或兼容接口。

### 13.4 可复用客户端、TLS 与服务生命周期

HttpClient 固定一个 origin；`http/client.rs` 拥有请求编码和租约，`client_driver.rs` 拥有连接限额、空闲回收和驱动，`client_body.rs` 拥有流式上传/下载。三者复用 Hyper 低层 HTTP/1、HTTP/2 connection API、消息校验和 AsyncStream，不另写协议解析器或全局主机注册表。

请求许可保留到正文完成或丢弃；物理连接许可保留到传输对象及其协议任务释放，空闲、队列内和运行中的连接合计不能超过 connections。每池一个受 Scope 管理的 Task 使用有界队列与 FuturesUnordered 推进连接 driver，空闲 timer 主动回收到期连接。driver owner guard 在创建任务之前构造，父作用域即使在任务首次 poll 前结束，也会关闭池。HTTP/1 Entry 析构中止相应 driver；HTTP/2 单流的租约释放不影响邻流。显式 close_client 等待池任务排空，不能遗留 detached driver。

上传用 StreamBody 按需拉取，下载逐帧读取并以共享 Bytes 切出限额内的块。HTTP/1 完整 EOF 才将连接归还空闲池，提前关闭、故障或取消均丢弃连接。HTTP/2 上传和下载共享流租约，双方结束才释放流许可；连接可供邻流继续使用，只有无活跃流时才开始 idle 回收。HTTP/2 握手 job 在池 driver 首次 poll 时创建连接 Scope，避免把长寿命连接登记到临时请求 Scope。GOAWAY 停止新派发，排空中的旧连接仍占物理许可；仅当 Hyper 通过 TrySendError 交回确定未发送的请求时，在原期限内重新排队，已发送请求不重放。缓冲 request 保留总正文和总期限；流式 open/open_stream 只限制头阶段、单块和每次读取，不收集整条 SSE。不跟随重定向。

TLS 使用 tokio-rustls 0.26.5、rustls 0.23.44 的 ring 后端和 webpki-roots 1.0.8。ClientTls/ServerTls 是共享配置，支持公共 CA、显式自定义 CA、服务端 PEM 身份，保留链、有效期和主机名校验，客户端与服务端会话缓存的容量参数均设为 32。具体 Transport 枚举只有 TCP 与装箱 TLS 两种，HTTP/WS 共用；普通 TCP 不承担 TLS 大状态或额外装箱。DNS 复用已有有界 blocking 解析路径。

四种 http_serve 原语共享可选末尾 Context 的类型检查和静态生成路径。Context 是可传递的具体值，Channel/HttpClient 字段共享资源；不引入一般闭包。Listener.close 对公共 connections 流仍产生终止失败，对 HTTP 内部接收流表示正常结束；连接 driver 收到关闭信号后调用 Hyper graceful_shutdown，等待已开始的请求和升级会话。应用用 timeout(server, grace_ms, fallback) 限制最终排空时间。

模块 API 的站点归属由检查器把 API 物理父目录与 `setting.json` 的 `sites.path` 做分段前缀匹配，文件名不参与匹配，站点也不改写 URL。配置引用的 verify 必须解析为一个只读 App 根；它和其数据库读依赖进入生产可达图及原生初始化，不能在运行时按字符串反射调用。

原生路由先由 runtime 校验站点绑定的 Bearer 或 Secret Cookie，再把结构化 Claims 交给该静态 verify。verify 输出的 App-owned record 经生成桥接转换为 runtime Identity，随后使用 Tokio task-local 包住真实 handler。源码只能通过 `dever.auth`、`dever.site` 读取复制值，不能构造或保存上下文本体。JWT 固定 HS256，并校验 algorithm、issuer/provider、audience/site、时间边界及签名；public 路由忽略浏览器自动附带的 Cookie，但显式错误 Authorization 仍拒绝。Cookie 写入和 Cookie 认证的非安全方法要求 Origin 与 Host 精确匹配站点配置的外部 HTTP/HTTPS origin，不根据内部监听协议推断，也不信任转发 Header。响应 Header/Cookie 先暂存，只在 handler、wire 编码与事务提交全部成功后发布。含密码 hash/verify 的 HTTP 调用链不生成覆盖整个 handler 的事务；检查器要求全部写入位于显式短 transaction 中，并禁止 transaction 到达密码计算，避免 SQLite writer 锁和 WAL 读快照升级错误。

权限目录由编译后的 API 自动生成并同步到核心 RBAC 表，受保护入口按站点、租户、用户和角色校验精确权限，不由 verify 返回权限字符串，也不要求逐个调用 `auth.require`。verify 只提供经验证的用户/租户/会话身份，public 声明显式允许匿名；业务数据所有权是独立边界。官方 `dever.api.Error` 由后端按确切类型与变体映射为固定响应，未知业务失败只记录调用元数据并返回脱敏 500。日志在 runtime 追加站点和已验证身份字段，不记录 token、Cookie 或 JWT payload。

外部 Adapter 不增加第二套调用模型。checker 从同领域唯一 Port 合成完整实现，复用既有类型 wire schema、failure/effect 图、Adapter 选择和 setting 解码；HIR 保存封闭的 exec/pip/npm/go 元数据。native 为每个具体操作生成静态输入/输出/业务错误 codec，业务签名中不会出现 JSON、进程句柄或动态值；reference 遇到可达外部实现时明确拒绝。

runtime `component` 是唯一 Worker owner。它从当前可执行程序目录解析编译器规范化的相对 entry，以清空环境、固定 `--dever-component` 参数和专用管道直接启动子进程，不使用 shell 或 `PATH`。协议帧为四字节大端长度加 UTF-8 JSON，继续使用 16 MiB、64 层、65536 值以及重复字段拒绝规则。`hello/ready` 绑定版本、Port、Adapter、schema hash、操作、capability 和 setting；随后使用严格递增 request id 的 `call/result/error/cancel`，并以 `health` admission 和 `shutdown` 收口。业务 `error` 必须属于 Port 已声明 variant；协议、超时、崩溃和非法 payload 都是运行 fault，不伪造业务错误，也不自动重放请求。

每个 Worker 的有界命令队列、进程、stderr drain 和 supervisor 都注册到现有结构化 Scope。超时或调用任务取消会发送 `cancel`，随后必须消费同一 id 的一个 `result` 或 `error` 才能继续；错误 id、未知 kind、截断/超限 frame 会终止该 Worker 并关闭 pending 调用。崩溃后的下一次新调用可以在固定启动频率内重建，但失败中的调用不重放。生成入口在应用成功、业务失败和运行 fault 后都走同一 cleanup，先完成 Component shutdown，再关闭数据库，并保留主错误及 cleanup cause。allow 集合参与编译期 effect 和握手精确匹配；Linux 启动还统一进入 `dever-sandbox` 的 namespace/seccomp/只读运行树与显式文件授权边界。GPU、其他 OS 和不允许 namespace 的宿主明确失败，不绕过隔离。

External Lib 的项目边界由 `dever-cli::libs` 统一拥有：`config/setting.json` 根 `lib` 数组保存显式请求，canonical v5 `dever.lock` 保存精确版本、传递依赖、runtime/schema/artifact 摘要、源码构建收据和每个 checked Worker 的绑定。每个 Worker 独立校验环境闭包与版本冲突，打包精确比对整份 provider 元数据，不允许锁中多写未嵌入资源。`dever lib add/update` 才能解析，`run/build` 只离线消费锁。全部 exec 候选与锁资源形成嵌入清单，bundle identity 包含路径、SHA-256、长度、可执行位；runtime 原子提取到私有 `data/cache/lib/<digest>`，每次 Worker 启动重新核对清单、字节与 Unix 所有权/权限，不扫描别的 bundle。

Python、JavaScript、Go SDK 从 `ExternalWorkerContract::sdk_manifest()` 的 checked wire DAG 生成 typed 边界，保留 i64/JSON 预算和不合作取消的一秒有界退出。真实 PyPI wheel/npm registry/Go proxy resolver 共用确定性 lock 和摘要缓存；生产入口只接受安装版本中经过签名验证的 runtime 描述与 pack，不回退到宿主工具。Python/JavaScript 离线打包器生成每个 Adapter 私有的运行树和同名 handler 绑定；Python 固定隔离参数并先加载可信 SDK，Node 支持 ESM/CommonJS。启动清单绑定精确 entry、解释器、资源 argv 和工作目录，仍由唯一 component supervisor 管生命周期。原始依赖归档只在配置根或 exec 仍需它时保留，避免与展开树重复嵌入。资源采用独立二进制编译输入，编译前按实际字节验证摘要，启动校验按 64 KiB 分块执行。

Go 受管编译/链接已由 `dever-cli::workers::go_build` 拥有：只启动锁定 pack 内的 compile/link/analyzer，清空环境，按目标 tags、标准库 importcfg、锁定 module ZIP 与 `go:embed` 准备输入；生成的 main 静态绑定同名操作并复用 Go SDK typed manifest。应用只保留编译后的 Worker 与合同，不嵌入构建工具或源归档。Go sumdb 验证签名、包含证明、checkpoint 一致性与 h1；Python extras/wheel/sdist 和 npm 高级安装图也由对应 resolver 校验。PEP517 与 npm 安装钩子共用隔离构建 Stage，只在显式 Lib 准备时执行，收据绑定输入和产物，run/build 离线重放。官方 simplejson C 扩展、bufferutil addon 和 UUID 已有真实构建/Worker 验收；这不等于正式公开或六平台发行。

Dever Package 安装由 `dever-cli::packages` 拥有：HTTPS origin、semver、单版本传递闭包、归档 SHA/manifest 校验与所属组件边界，复用现有 ArtifactStore、Lib resolver 和原子锁。`SourceMap::load_with_packages` 在普通项目入口加载 Package 源码，配置变更失败恢复原始字节，remove 离线裁剪且不更新剩余版本。源码与 Worker 字节只来自精确锁定归档；exec 和受管生态共用锁归属判断，不按文件是否存在来决定是否回退本地。

`dever-backend-bridge` 已实现安全 Rust API 到进程内 LLVM 18/LLD 的连接。workspace 保留 `unsafe_code=forbid`，只有该 crate 的私有 `ffi.rs` 使用授权例外；编译器返回缓冲仍由原 C++ allocator 释放，LLD 串行调用。输出在当前用户的私有目录中独占预留，失败只清理自有输出。可恢复链接失败允许后续调用，不可恢复的 LLD 状态终止私有编译 worker。

`dever-core::llvm::emit_kernel` 直接消费 checked HIR，已有同步强类型函数与推导挂起函数转换，不生成 Rust 或动态 Value。支持 Int/Float/Bool/Decimal、Text/Id、Bytes 标准操作、List/Map、Nullable、普通 Record/Choice、多输出、分句投影、记录值拷贝/字段修改、静态 handler、左到右求值及短路。List/Bytes 的 each/filter/find/reduce/reduce_until/sum 使用静态 handler 与 typed 遍历，支持其现有可挂起 handler 合同；Int 保留 checked overflow/div/rem/neg，Float 保留 IEEE 行为。局部、临时及嵌套受管值通过具体类型的 retain/drop 和所有权 guard 清理；复用原有 last-use/字段替换分析，成功根输出转交调用者。入口初始化输出与 fault，错误保留原始来源与调用链；调用者通过 `dever_outputs_release`/`dever_fault_release` 释放。Model/Port/Adapter/API/Job、鉴权/上传等应用 intrinsic 与其余未支持操作仍带源码位置拒绝，不以默认值或 Rust 回退代替。简单内核六目标 object/link、受管/资源/异步内核六目标 object 和 Linux x86_64 原生执行已验收；Windows 复杂链接缺目标 runtime 的 `__chkstk`/`_fltused`，不能算跨平台执行通过。

File 的 create_new/open/read/write/close 与共享别名、同步 Stream 的共享游标/文件 chunks/close/each/reduce/reduce_until 复用原 runtime。原始 I/O 失败进入标准 Choice，文件已关闭的 Stream 产生一次 Failed 后结束，提前遍历不吞掉未访问行。源码没有新增 pull/first(Stream) 接口；pull 仅用于内部遍历 ABI。

`llvm/asynchronous.rs` 生成 LLVM18 switched-resume 协程及静态 OwnedType/AsyncFunction/SyncFunction 描述符；`llvm/concurrency.rs` 接通 Task/Group/Channel 与同步 blocking/parallel。构造帧时在 initial suspend 前克隆 borrowed 参数；真实 Pending 保留帧，初始/挂起销毁清理 live guards，正常完成先清理再 final suspend，最终销毁仅释放帧。ForeignFuture 使用现有单 runtime Scope/supervisor，不逐函数 block_on。跨线程边界按 checked 字段、完整可达异步帧及 inferred failure 字段验证 Send；内部 Unit/Outputs ABI row 以字段决定可传递性，不更改语言类型属性。同步 invoke 借用 runtime 拥有的输入，runtime 在完成或取消后释放；typed fault 移交保留 nominal 载荷、代码和来源，只有 Business=4 可被 result 捕获。

`llvm/network.rs` 接通 AsyncStream 的 List/Bytes/Channel 来源、ticks 和全部 TCP 操作；Socket/Listener/异步流保持原 runtime 的共享身份、方向锁、期限和取消合同。异步流消费在返回、提前停止、故障或取消时关闭共享 producer；同步 Stream 的提前停止仍保留未访问行。timeout 的独立 presence 区分真正超时与成功输出 null，fallback 在任务及后代排空后调用；race 保持同签名的 typed 多输出，并排空失败/取消的竞争者。`llvm/parallel.rs` 使用静态 typed input pack 与同一运行时的 scoped concurrent、bounded blocking 或 async stream dispatch，不用普通 Group 循环近似替代 pull 前容量预留和空闲 producer 期间的故障竞速。

`llvm/http.rs`、`http_values.rs` 和 `http_handlers.rs` 把 HTTP/HTTP2 连接池、TLS、流式 Reply、SSE 与 WebSocket 接入同一 runtime 协议实现。跨 ABI 使用固定命名字段、不透明资源 owner 与静态 typed handler/context；请求或上下文在最初挂起前取得所有权，不经过 JSON/Value 转换。WebSocket 事件保留 kind 与 typed Text/Bytes。HTTP handler 的 typed fault 在请求边界最终渲染：记录错误并返回 500/中止回复，服务器继续处理后续请求，不改成全服务失败。协议异步操作在 poll/I/O 前验证输出槽，非法 metadata 不推进操作，修正后可重试。

`llvm/system.rs` 接入时间/日历、参数、stdout/日志、UUID 与 Secret/crypto；Uuid 使用真实 UUID owner 而非 Text 近似，密码运算继续走已有 blocking 边界。仅可达日志或 HTTP server 的根入口在成功/失败返回前刷新日志。runtime logger 的单一固定容量队列保留 FIFO、低级别丢弃和高级别背压；固定确认状态在此前记录释放和输出刷新后唤醒调用者，不依赖临时 ACK 通道或首次阻塞的惰性分配。

`llvm::emit_application` 已接通独立 CMD 与 HTTP/REST 应用入口。CMD 保持完整名称和单个 JSON 对象输入；混合 HTTP/CMD 项目执行 CMD 时不绑定监听器。`llvm/api.rs`、`api_context.rs`、`rest.rs` 与 `application_commands.rs` 生成静态路由、具体输入/输出、Claims/Identity 及权限描述符，复用已有 JWT/业务 verify、站点、RBAC、database-only 租户、Cookie/Header、Upload 与日志实现，不另建协议栈或向业务传入动态 Value。非 public 路由缺少认证或精确权限时，源码与私有 ABI 两处均拒绝；权限 key 仍不含 HTTP method。

完整应用和可达日志的 CMD 共用 invocation-owned `application::Session`，只读取一次 `config/setting.json`。数据库、迁移临时池、租户 manager 和 lifecycle 均由该 owner 持有；子任务先排空，再在同一 runtime 中关闭连接池、刷新日志。POST/PUT/DELETE 输入解码后开始事务，输出编码后提交，成功才发布响应 metadata。租户管理入口沿用 migrate/owner/component 合同，缺少 ready marker、租户身份或启用组件不能回退平台库。Upload 经 Port/Adapter 消费到存储，部分解码失败清理 affine owner。私有 CLI 和 Linux managed daemon 编译已接 LLVM；正式目标 packs 未交付。

`llvm/external.rs` 接通 checked external Adapter；选中的 Setting 成功解码后才启动 Worker，具名输入/输出和声明业务错误复用 typed wire codec。缺失可空输出字段、额外字段、错类型和未声明错误均为运行故障。`llvm::emit_application_with_resources` 消费原打包器的 `EmbeddedResource`，与 Rust backend 共用摘要/去重，运行时复用安全提取和受管 launch；不查找宿主解释器。`runtime-external` 与编译器 SDK 分离。`component::Session` 由每次 task 根持有，传播到子任务、blocking 和 API；supervisor 挂在根 Scope，短命调用者退出不关闭共享 Worker。根退出先停止接收和关闭 Worker，再排空 Scope，typed 原始错误保留并附加清理原因。显式 shutdown 被取消时仍保留 Task owner，根可继续排空；普通 Test 仍只有 fake。

`llvm/jobs.rs` 接通 Job-only、API/worker/all、持久 payload、事务内入队、每次 User verify/精确权限/组件重验与租户迁移。`job::resources::Resources` 持有本次入口的 StorageBinding、Spec 和 Clock，复用已有 store/worker/cron/lease fencing；同步 handler 共用既有 bounded blocking，异步 handler 共用 typed coroutine。服务策略只在实际启动服务时校验，入队 CMD 和租户管理无需 HTTP/worker 配置。编译期的租户组件归属不等于部署启用了 tenant；两后端共用 `job::require_components`，仅在启用租户时要求身份并查询组件状态。Job 保持原有 HTTP 请求/身份 getter 禁用规则，不伪造请求上下文。

`llvm::emit_test_suite` 复用每个 checked `test_program` 的 fake、effect/failure 和可达闭包，在同一 IR 中按声明符号隔离各 case。稳定 index 选择私有文件名入口；fault 按同一 index 查询大小、渲染和释放，业务载荷保持具体类型。普通测试用独立进程和临时 SQLite；不读取部署配置、启动生产 Adapter/Lib 或服务，无数据库用例也不初始化无关 Model。断言保留源码位置，虚拟时钟和 drain 复用生产队列；根 Scope 排空后关闭 Session，再释放注册表、时钟和入口锁。测试 CLI 切换仍属于后续统一后端阶段。

`llvm/database.rs`、`database_rows.rs`、`orm*.rs`、`database_schema.rs` 和 `transaction.rs` 接通 Model；API 应用通过 Session 同时支持平台库与租户物理库。SQL/DDL 与关联查询复用 `native/orm.rs` 的纯构造函数，不生成 Rust、不复制数据库驱动。持久化固定标量在 ABI 边界转换为既有 `orm::Value`，业务记录和错误仍保持具体类型。Row/Rows/RowStream/Related 是不透明所有者；ToOne 保留可空关联，ToMany 按 998 个父键分块并使用既有 per-parent 限额 SQL。启动按实际数据库分组先迁移全部 Model，再建立外键；数据迁移与 Seed 复用原历史校验、事务和顺序规则。

`database::Session` 持有本次入口的 Settings 和连接池，复用原配置校验和驱动初始化，不写入全局 OnceLock。`task::run_entry_with_typed_cleanup` 先排空 root Scope，再在同一个 Tokio runtime 内停止所有连接池、等待驱动关闭，最后销毁 runtime。需要租户的应用使用上述 application Session 与 tenant owner，不能回退到平台库。写入 CMD 在输入解码后开事务，普通调用和顺序 handler 继承隐藏事务，Task/parallel 回调使用 detached 上下文；嵌套 transaction 复用外层、不制造 savepoint。输出编码成功后才提交，提交后才发布 stdout；失败/取消清理 typed 输出并回滚或丢弃状态未知的连接。rollback 保留原始 fault，可附加 cleanup cause；DbError 十个变体使用静态 typed 回调，不按消息猜类别。PostgreSQL 的 SQL 约束失败会使当前事务失败，不能承诺捕获后继续使用该事务；NotFound 等非 SQL 失败仍可正常捕获。

`llvm/failures.rs` 接通业务 `fail`、默认传播和 `result(named_call)`，直接复用 `specialize::failures` 与 `capture::targets`。fault 原前五字段不变，末尾追加 nominal type（编译期 ID+1，0 表示无载荷）、variant 与按可达错误类型最大布局对齐的内联 Choice；Business code 为 4。载荷用具体类型 store/load 和静态 drop，不是动态 Value 或序列化消息。捕获支持零/单/多输出、直接 error 与一层 error-only Choice 包装；显式 raised outer variant 保留身份。匹配后移交载荷并清空 fault，未匹配的数值/RuntimeAbi 故障继续追加调用帧传播。BytesToText/BytesFromInts/BytesSlice 复用既有 C ABI：status 1 的原始消息转换为标准 Failed(Text)，status 2 保持 RuntimeAbi fault；成功句柄与错误 Buffer 各自准确转交/释放。真实执行覆盖捕获后重失败、非零 variant 根释放、受管 COW 与精确源调用链。

`dever_rt_v1_*` ABI 的唯一 canonical 签名在 `crates/dever-backend-bridge/include/runtime.h`。`runtime-abi` feature 构建独立 staticlib，不包含 LLVM/C++ SDK；所有导出仍局限于私有 `ffi.rs`。固定宽度标量传参、u32 状态及显式输出指针避免聚合按值传参；Int/Float/Decimal 复用现有数值/Render，Decimal 使用显式 little-endian 双 u64。Text/Bytes/List/Map 使用不透明 Arc 所有者；集合以 process-static `DeverRtType` 的 clone/drop/equal/hash 回调操作具体类型，复用现有 List/Map COW，不引入动态 Value。普通元素保留 PartialEq（含 NaN），只有合法 Map key 包装提供 Eq/Hash。`_take` 接口即使失败也消费其非空 receiver；消费式 cursor 的提前释放同时清理未访问行。可空元素取值按完整元素类型分配输出槽，外层存在标记不覆盖元素自身的 null；Channel receive 共用相同的 optional-row lowering。输出/错误 Buffer 由 Rust Box 分配并通过同一 ABI 释放，不能与编译器 Reply 的分配器混用。C header→实际库、checked Dever→LLVM object→实际库及重复执行分配平衡夹具已经执行；这不代表完整应用系统 ABI、性能/RSS 容量、六目标 runtime packs 或干净机器发行已完成。

私有 `dever run/build/test` 默认由 binary 私有 `compile` 模块执行 HIR→LLVM→object→进程内 LLD；共享 CLI 库与 launcher/daemon 不承担 LLVM 动态依赖。runtime pack 固定在编译器同级，校验版本、ABI、目标、profile、路径、长度和 SHA-256；缓存命中仍校验 pack，链接前复制到独占目录并重新校验，只接受显式 CRT object 与普通静态 archive。缓存和 create-new 输出沿用 `native::compile_artifact`，旧 Rust backend 仅保留作者回归入口。

`llvm::emit_executable`/`emit_test_executable` 在原 callable 根外生成 C `main`，复用 typed fault 渲染、输出释放与日志刷新。Unix 进程启动显式忽略 SIGPIPE，让关闭 stdout 返回带来源的 I/O 错误；callable 根不改变嵌入宿主信号策略。Test 入口只接受单个有效十进制 case index，fault size/render/release 使用同一 index。

Linux `deverd` 已接可信编译 IPC。managed core 提交逻辑源码、无秘密编译绑定及资源；daemon 验证签名版本与完整 pack 引用后，启动受限核心 worker 重新检查并编译。只由 daemon 发布缓存，不能通过普通 artifact 上传污染它；App/Test/Lib 准备与运行留在 caller。服务缺失明确失败。

原生发行包作者入口 `sdk/native-release.rs` 从独立作者根的 `config/setting.json` 读取显式输入及摘要，复用既有 runtime manifest、验证器和 Ed25519 发行协议。制作器复制并复核四 profile、CRT 和核心/LLVM 文件，在私有 staging 中签名，再以不覆盖方式原子发布；私钥不进入产物。`runtime-pack` Cargo profile 为 base/sqlite/postgres/both 分别生成保留链接符号的优化 archive，共享一个构建缓存。相同输入和密钥的重复打包一致，不等同于跨机器源码构建一致。作者流程见 [native release SDK](sdk/native-release.md)。

作者配置的可选 `runtimes` 已接 Python/Node/Go 完整运行/构建输入，制作器与 Worker 共用 manifest 和文件闭包校验。排序且固定元数据的 gzip/tar、registry 描述统一进入顶层发行签名；Go SDK、Adapter 和依赖的编译共用 `-trimpath`，避免随机临时目录改变 Worker 与共享缓存身份。LLVM 的二进制资源附件复用既有摘要去重，在 verify 前初始化常量，IR 仍限8 MiB；附件最多4096项、单项128 MiB/总256 MiB，共享编译请求384 MiB、产物320 MiB，不透明 artifact 仍64 MiB。Linux x86_64 三生态签名安装、受管 `check/run/build`、三条共享缓存及移除源码/机器安装后无系统语言环境的独立执行已通过，包含Python/Node源码构建原生依赖与离线收据重放。Linux签名首装、旧签名模板升级、篡改拒绝、双项目及真实双UID共享核心亦已通过自有完整验收。Linux root 沙箱、真实第三方高级依赖、双 CMS/PG 和 LLVM 容量报告已补齐；公开首次安装包、六平台产品代码/资产、受限 AppArmor 非 root 入口与完整质量门仍未完成。各阶段证据及限制见当前生产化任务。

database 租户模式把 verify 返回的内部 tenant ModelId 放入同一 task-local 调用域。`global type` 继续使用平台连接，其他 Model、自动 REST 与 Job 通过 `StorageBinding` 选择已显式迁移且 fingerprint 匹配的租户物理库；缺少上下文或 ready marker 直接失败。runtime tenant manager 对 `(connection, tenant_id)` 池设置总量与 idle 回收，生成的 `tenant migrate` 入口按稳定顺序迁移全部 tenant Model/Job 后才写本地和 control marker；PostgreSQL 先通过控制库幂等建租户库。User Job 持久化 Provider/站点/session/租户声明和已通过的能力名，Worker 重新调用静态 verify 并使用最新权限复核，不能把历史任务降级为 System。table/field 模式尚未实现，不能在固定表 SQL 上只补一个配置分支。

## 14. 错误与诊断

Dever 把失败分成三个层次：

1. **加载错误**：路径、符号链接、文本编码等问题，发生在可靠源码位置建立之前。
2. **编译诊断**：词法、语法、名称、类型、分句和生命周期问题，带稳定错误码、主位置和必要的关联位置。
3. **运行失败**：除零、越界、关闭资源、协议失败等，尽量映射回 Dever 源码位置。

多个诊断按确定性规则排序。内部实例名称、后端标识和宿主调用栈不应泄漏到面向 Dever 开发者的错误信息中。

可预期的业务和 I/O 失败使用显式 choice 结果，让 Dever 源码能够处理；违反已经建立的运行前提时才产生运行 fault。这两类失败不应混为一个无类型异常通道。

CLI 只负责参数解析、退出码和标准输出/错误输出分流。语言规则必须由加载器、检查器、HIR 或执行后端拥有，不能散落在命令分支里。

### 14.1 编译期契约分析

完成名称、类型、分句和循环检查后，HIR 进入独立的契约阶段。字段范围复用分句的 Int/Decimal 区间归一化；private 由字段访问和构造的原有检查入口拥有，不增加平行的对象系统。

事实分析按依赖顺序计算函数输出保证，跟踪输入来源、已知区间、分支值和字段事实。复制创建独立事实快照，赋值使目标旧事实失效。函数输入到输出的直接传递可以在调用处替换；未知运算扩大为未知，不能建立范围。构造、字段写入、返回和 handler 边界共用范围包含判断，诊断保留依据位置。

失败分析为显式 error 分支建立义务，跟随值及嵌套载荷流动。消费输入的函数本身也必须检查所有分句，空函数无法洗掉义务。失败输出保证与“可能含错误”分开：未知列表可能为空、可空值可能为 null，不能借它们证明已经返回失败；短路和空遍历不会错误消费未执行路径上的输入。recover 是可审查的显式消费边界，编译器只验证声明，不能验证理由的业务真实性。

效果分析从穷尽的系统原语分类出发，经调用和静态 handler 绑定传播到稳定摘要；pure 在此处验证。package 依赖从已经解析的类型、调用和静态 handler 引用自动构图，不在源码重复声明。结构建议复用 HIR 和效果摘要，只处理可证明的结构，保留源码执行行为。

类型图验证后一次计算可比较、可移动、可跨线程传递属性，避免反复展开共享类型。事实、效果和失败分析共用依赖图；单调摘要变化只通知受影响的调用方，依赖环仍通过工作队列收敛。

API 快照由检查完成的 Program 生成，使用公开名称而非内部编号，排序保证稳定。CLI 只负责读写文件和报告差异；已有基线不会更新，验证先于原生编译。快照和源码的修改授权属于项目治理，不能通过语言内一个新关键字自动建立。

## 15. 离线构建与产物生命周期

Dever 的默认工作流是本地离线：

- `check` 只加载和检查源码，不启动原生后端；
- `fmt` 只使用语法前端，不执行程序；
- `run` 和 `build` 使用同一条生产编译路径；
- 编译过程不隐式访问网络或下载 package；
- `run` 在临时位置生成程序、执行并清理；
- `build` 只写入明确指定且尚不存在的输出路径，避免误覆盖文件。

`run` 和 `build` 需要当前机器已经具备与编译器匹配的本地原生后端。正式发行工具应随发行包提供或在安装时明确检查它，不能在一次构建过程中临时联网补齐。当前尚未交付面向最终用户的完整发行工具链。

内置标准包 AST 在进程内缓存；每线程只保留上一份按有序路径和完整文本精确匹配的检查结果，含错误与警告。新的 CLI 进程仍完整检查源码和 API 基线，没有持久化 HIR 格式。

私有 LLVM CLI 使用编译器同级 `cache/native`；旧 Rust 作者回归入口使用仓库 `target/native-artifacts`。两者复用同一产物 owner，SHA-256 身份区分生成内容、编译器、runtime/profile、平台与选项。缓存只持久化可执行文件、身份与完整性摘要，临时生成内容与运行时快照按所有权清理。完整产物原子发布；命中后核验并复制到本次临时目录，损坏时报错，显式输出仍不覆盖已有文件。缓存不替代每次 CLI 的源码/API 与 pack 检查。

稳定 `dever` 与版本核心分离。机器根固定拥有 `bin/state/versions/cache`，发行 manifest 以 Ed25519 验证原始字节，并逐项复核平台、长度和 SHA-256；安装先进入 staging，自检成功后才以目录 rename 发布。活动版本使用追加式校验日志：每条完整记录独立带摘要，读取只选择最后一条完整记录，因此写入中断保留上一版本，且不依赖 Windows 不支持的覆盖式 rename。项目锁定只来自 `config/setting.json` 的精确 `dever.version`。

`CacheStore` 是 `deverd` 进程内部的不可变条目、摘要复核、操作租约和清理引擎。Linux 普通用户通过经内核 UID 校验的本地 IPC 共享 Lib 资产，不能读取管理员 token 或执行清理；状态只查询元数据并明确返回 `verified:false`，管理员完整校验返回 `verified:true`。上传下载只接受 SHA-256 和精确长度，单资产 64 MiB、总量 2 GiB/4096 条，分块传输到私有 staging 后原子发布；每个外部 UID 最多占两个连接且总量保留管理员处理容量。服务不可用不会回退到 caller-writable store。编译请求与不透明资产传输分离：完整输入及签名 manifest 参与身份，源码仅留在私有 staging，缓存只保留程序和摘要。请求全程持有版本共享锁和 cache operation；取消、断连或180秒期限到达均回收 worker 和 staging。独立编译缓存也受2GiB/4096条预算限制。macOS/Windows peer credential、系统服务注册、提权和完整原生发行仍未完成，不能把开发态 socket 描述为完整跨平台安装服务。

离线不等于跨平台。省略构建目标时使用本机平台；Linux x86_64 编译器可通过 `build --target linux-aarch64` 选择已准备的 ARM64 运行库和第三方资源，生成 ARM 应用。编译器核心与 Go 编译工具仍在宿主执行，应用及 Worker 使用目标架构；缓存和签名校验都包含明确目标。其他平台仍需独立适配和验证。模拟执行不等于 ARM 真机性能或所有硬件/内核兼容性验收。

源码排序、Map 顺序、诊断排序和归约顺序是确定的。不同平台生成的可执行文件不承诺字节完全相同，但对相同合法程序必须保持相同语言结果；显式并发完成顺序等已声明例外除外。

## 16. 规范执行器与验证方法

规范执行器只在测试中使用。它直接执行已经检查完成的同步 HIR，目标是清楚地表达语言语义，而不是承担生产性能。异步 HIR 当前由规范执行器明确拒绝，异步语义以原生后端和 runtime 的定向测试为准，不使用不完整解释作为静默回退。

生产后端和规范执行器共享 HIR，但不共享最终执行机制。测试可以把同一个程序交给两条路径，比较：

- 正常输出值和公开 stdout；
- choice、record、List、Map 和 Bytes 语义；
- pattern 选择与 `other` 补集；
- 数值溢出、除零和边界行为；
- 资源别名、关闭和 Stream 消费位置；
- 可预期错误与运行 fault 的分类。

这类差分测试能发现后端优化改变语言语义的问题。协议测试则使用独立构造的字节报文验证 JSON/HTTP，不以被测实现自身生成的数据作为唯一正确答案。

测试只依赖公开 Dever 行为和检查完成的 HIR 内部契约，不应绑定编译器内部帮助函数。这样重构前端或后端时，测试仍在验证语言，而不是验证某个暂时实现。

## 17. 一段源码如何被实现

以前面的 `show` 为例，编译器依次完成：

1. 词法分析得到 `type`、choice 变体、function 名称、pattern 和表达式 token。
2. 语法分析得到一个 `Result` choice 和两个同名同输入数量的 `show` 分句。
3. 声明注册把两个分句归入同一个 function 组。
4. 类型检查确认 `Ok` 绑定 Int，`Failed` 绑定 Text，两个分句都输出 Text。
5. 分句分析把输入域拆成 `{Ok}` 与 `{Failed}`，证明两者互斥且完整。
6. HIR 记录确定的变体、payload 局部位置、文本操作和源码位置。
7. 可达性分析只在入口能调用 `show` 时保留它。
8. 后端生成对 choice 标签的具体分支和直接的 payload 读取，不进行字符串名称查找或通用 pattern 解释。

这条路径体现了 Dever 的核心原则：源码保持声明式和类型化，复杂证明发生在编译期，生产程序只保留已经确定的具体操作。

## 18. 扩展语言时的规则

新增语法、类型或标准能力时，应遵守以下边界：

1. 新语法先进入语法树，再由语义检查器建立唯一语义，不能只在某个后端特殊识别。
2. 新 pattern 必须加入符号域和完整性证明，不能退回源码顺序优先匹配。
3. 新 handler 使用位置必须保持静态可解析；若要引入运行时闭包，应作为明确的语言模型变更单独设计。
4. 新标准算法优先写成 `.dever`；只有无法由语言表达的原子系统操作才扩展 `dever.system.*`。
5. 新系统原语必须同时更新类型契约、规范执行路径、生产路径和一致性测试。
6. 后端优化必须保持值语义、Map 顺序、数值规则、资源身份和 fault 位置。
7. CLI 不拥有语言语义，只组合已有编译阶段。
8. 生产路径不能引入通用解释器或动态 `Value` 作为功能回退。

这些规则让每项能力只有一个清晰拥有者，也避免标准包、检查器和执行后端出现三份逐渐分叉的逻辑。

## 19. 当前边界

本文描述的是当前语言和已经接通的编译架构，不代表所有生态能力都已完成。SQLite/PostgreSQL 类型安全 ORM 已使用编译器拥有的 `ModelSchema`/`QueryPlan`、具体生成代码和薄方言 executor 接入；Model、事务、迁移、Seed、关联和 typed native SQL 的合同见 [语言开发指南](LANGUAGE.md#13-modelorm-与数据库) 与 [Database Guidelines](.trellis/spec/backend/database-guidelines.md)。当前仍未交付的主要部分包括：

- WebSocket-over-HTTP/2、子协议/扩展、CONNECT、推送及更完整的 HTTP 生态；
- package 注册表和正式安装发行流程；
- 跨平台发行验证；
- 用 Dever 自身实现编译器。

这些能力应建立在现有语义和系统原语边界之上，不能通过在 CLI 或运行时中增加隐藏业务逻辑来绕开语言设计。

## 20. 核心结论

Dever 的实现可以概括为：

```text
源码负责表达意图
编译器负责全局证明
HIR 固化已经证明的语义
静态特化消除运行时行为分派
标准 .dever 包负责高层能力
最小运行时只负责原子系统操作
生产后端生成独立的本机程序
```

因此，开发者看到的是一门具有明确值语义、穷尽分句和类型化错误的语言；编译器内部则保留更换实现、优化数据表示和扩展原生后端的空间。

# Design

## Resolver Boundary

CLI 使用一个 closed ecosystem dispatch：parse requested spec、resolve fixture/registry metadata、fetch verified artifact、produce normalized locked graph。生态实现只处理自己的 metadata；缓存发布、摘要、路径安全、lock 排序和诊断由共享 owner 负责。

## Lock

`dever.lock` 是 compiler-owned canonical document，包含 format version、Dever toolchain identity、Adapter identity、Port schema hash、ecosystem、requested spec、runtime pack、target artifacts、dependency graph 和 SHA-256。未知字段、重复键或手工修改在 doctor/project command 中拒绝并提示重新解析。

每个 Adapter 拥有隔离 dependency graph，因此两个 Adapter 可以锁定同一生态包的不同版本。共享 cache 只按已验证内容去重，不合并运行环境。

## Runtime Packs And SDK

- pip：固定 CPython pack，优先验证 wheel；source distribution 只能进入隔离构建 sandbox。
- npm：固定 Node pack，lifecycle scripts 默认禁用，需要的 build hook 进入隔离 sandbox。
- go：固定 Go pack，按 target 编译 worker；需要 cgo 时必须存在受管 target toolchain，否则明确拒绝。

三个 SDK 共用生成的 operation/type/error schema fixture；语言层只负责数据映射和协议循环。日志写 stderr，不能读取应用 setting 文件或环境变量配置。

## Network Boundary

网络只由显式 add/update resolver 使用。registry URL 和信任根属于签名工具链发行合同，不由项目或环境变量覆盖。测试使用本地 fixture registry 和 test-owned cache，不连接真实 registry。

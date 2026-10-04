# Dever 安全数据合同

## Goal

让密码、令牌和部署密钥在 Dever 中具有编译器可识别的非观察边界，使大型应用能够存储私密 Model 字段、处理 Secret 和调用成熟密码算法，同时彻底移除环境变量配置入口。

## Dependencies

- 无前置子任务；这是 Port setting、认证 API 和 CMS 会话的基础。

## Requirements

- Model `private` 字段由所属 component/domain 的 App 拥有；生成的 Model CRUD 可以持久化，所属 App 可以访问，其他角色和领域不能访问。
- 含 private Model 字段的完整 Model 不能出现在 App 输入/输出、API/Job wire 输出、日志或跨领域类型观察面；App 使用显式 View record。
- 新增 opaque `Secret` 标量：不可 Render、比较、拼接、普通 JSON 编码、Map key、Model 存储或错误 payload。
- Secret 只允许进入批准 sink：Argon2id hash/verify、HMAC、常量时间验证、后续受控 Cookie/bearer setting sink；不提供通用 Secret-to-Text。
- 提供操作系统 CSPRNG token、Argon2id 密码 hash/verify、SHA-256、HMAC-SHA256 和常量时间字节/摘要验证；使用成熟依赖，不自实现密码算法。
- 密码哈希存储为 private Text，明文密码使用 Secret；错误不得包含明文、密钥或摘要原文。
- 删除 `dever.process.environment` 及对应 intrinsic/runtime bridge、文档和测试；不保留别名、fallback 或 deprecated 路径。
- `dever.process.arguments` 保留，但不得成为 setting override；配置来源规则由后续 Port/Job/API setting 合同继续约束。
- `.dever` 与 `.dever.md` 使用相同语法/语义；所有正式测试放根 `test/`。

## Acceptance Criteria

- [ ] 所属 App 可创建、更新、读取 private Model 字段，并能用其验证密码；其他角色/领域的字段访问在 check 阶段拒绝。
- [ ] App 签名、API response、日志和普通 JSON 对 private/Secret 的直接或嵌套泄漏均被静态拒绝。
- [ ] Secret 不能被渲染、比较、拼接、存入 Model/Map key 或放进普通错误 payload。
- [ ] secure token 两次调用不复用值且使用 OS CSPRNG；无随机源时返回显式安全失败。
- [ ] Argon2id hash/verify 验证成功、错误密码、损坏 hash 和资源上限；SHA/HMAC 与已知向量一致。
- [ ] 搜索和负向编译测试确认应用无法调用环境变量读取，仓库没有兼容入口。
- [ ] 既有普通 record private 字段、Model schema、API envelope、日志和 Markdown 回归保持通过。

## Out Of Scope

- 通用密钥管理服务、密钥轮换协议和硬件安全模块。
- JWT/OAuth、会话业务模型、RBAC 和登录 API；由后续 API/CMS 子任务实现。
- 数据库列加密；private 表示可见性，不声称静态或磁盘加密。

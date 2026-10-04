# Dever Port 与 Adapter

## Goal

把外部 HTTP、邮件、对象存储等依赖变成编译期验证、运行时可配置替换的 Port/Adapter 边界，同时收紧应用角色依赖，禁止 App 绕过 Port 和 Domain/Adapter 反向依赖业务层。

## Dependencies

- `secure-data-contracts`：Secret setting 和不可观察边界。
- `time-typed-codec`：类型化 setting 解码和共享 wire schema。

## Requirements

- `port.dever`/`port/*.dever` 只允许类型和 bodyless function contract；每个 contract 显式 `fails` 同领域 App 公开 error choice 或公开标准 error choice，至少一个显式 error variant。Port 自有 DTO 仍私有。
- Port contract 输入、命名输出、failure contract 和逻辑身份稳定，不从任一 Adapter 实现反推。
- Adapter 通过 qualified Port target 显式实现；签名必须相同，实际 failure set 必须是声明集合的子集。
- Adapter 可包含同文件私有类型/helper，但不能调用 App、Domain、Model、其他 Adapter 或 Port；只能调用标准库和自身 helper。
- App 只能调用同领域 Port；Domain 只能调用同领域 Domain；Model 无业务函数；API/Job 只调用同领域 App。
- Port 调用的 suspend/effect 是所有可选实现的保守并集，仍由现有静态 effect 系统决定 worker/await 边界。
- 单实现自动绑定；多个实现必须由 `config/setting.json` 的稳定 Port identity 选择，未配置、未知实现或未知 setting 字段启动失败。
- Adapter 可声明一个 typed setting record；只有被选择实现的 setting 必须存在并解码，Secret 字段不进入日志/错误。
- 编译产物包含闭合实现集合和直接 match，不使用动态库、反射、trait-object business dispatch 或运行时源码扫描。
- Test 可在同一个测试文件完整 fake 多个 Port；fake 只绑定当前 TestCase，测试不读取项目 Adapter setting 或回退到生产实现。
- Port/Adapter 的 `.dever.md` 文档合同与普通源码一致。

## Acceptance Criteria

- [x] 合法 bodyless Port 和单 Adapter 可被 App 调用并生成直接静态实现调用。
- [x] Port body、Adapter 缺失/重复实现、签名/命名输出/failure 不匹配在 check 阶段拒绝。
- [x] App 直调 Adapter、Domain 访问 Model/Port/Adapter、Adapter 反调 App/Domain/Model 和跨领域 Port 调用全部拒绝。
- [x] 多 Adapter 通过 `setting.json` 在不重新编译的情况下选择；未知选择、缺配置、未知字段和错误类型启动失败。
- [x] 未选择 Adapter 的 setting 不要求存在，其代码仍通过静态检查并包含在闭合产物中。
- [x] Secret setting 可以用于批准 sink，但不能出现在日志、错误、API/Job output 或普通字符串中。
- [x] 两个应用测试可以对同一 Port 使用不同 fake，单套件编译后各自运行且互不泄漏。
- [x] Port effect/failure 通过 App、transaction、handler specialization 正确传播，不出现动态 fallback。

## Out Of Scope

- 运行时安装插件、动态库、服务发现和远端代码加载。
- 跨领域直接共享 Port；领域间继续通过 App 合同协作。
- 通用依赖注入容器、Service locator 或应用可变全局 registry。
- Adapter 热切换；setting 在进程启动时验证并固定。

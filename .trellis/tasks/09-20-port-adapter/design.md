# Dever Port 与 Adapter 设计

## Source Contract

Port role按路径给出边界身份。根 `port.dever` 的 identity 为 `<component>.<domain>`；topic Port 增加 topic。bodyless contract 示例：

```dever
send(message: Text) () fails app.DeliveryError
```

`fails` 必须引用同领域 App 的公开 error choice（例：App 定义 `DeliveryError`），或公开标准 error choice；至少有一个显式 error variant。Port 自有类型仍私有。这样保留 App 全部函数公开的规则，App 能传播自己的错误，无需增加隐式私有 helper。Port/Adapter 只能引用 Port 签名使用的 App 类型，不能调用 App。普通有 body function 仍沿调用图推断 failure，不新增通用 throws 声明。

Adapter/Test 通过 qualified definition 实现，例如 `port.send(...) () { ... }` 或 `port.mail.send(...) () { ... }`。普通单名 function 是该文件 helper。一个生产 Adapter 文件完整实现一个 Port identity；禁止跨 Port 聚合。一个 TestCase 可以完整 fake 多个 Port。

## Registration And Dispatch

Checker 在所有签名完成后建立 Port contract/implementation table。实现身份来自 adapter role topic；根 `adapter.dever` 使用 `default`。逻辑 Port identity 不包含 Adapter 文件路径。每个可达 Port 必须至少有一个生产实现；不可达声明按既有 unused contract 报 warning。

单实现 native call 直接静态调用。多实现生成固定选择和 concrete match，分支调用具体 specialization。effect analysis 在 HIR 中看到生产实现保守并集；failure 摘要固定为声明全集，实现实际失败必须是子集。运行时选择不改变类型正确性。

## Settings

Adapter 文件可声明一个 contextual `setting` record，并以只读 `setting.<field>` 访问。编译器把它纳入共享 setting wire schema。建议 JSON：

```json
{
  "adapter": {
    "notification.mail": {
      "use": "smtp",
      "setting": {
        "endpoint": "https://mail.example.test",
        "token": "secret-value"
      }
    }
  }
}
```

`use` 在多实现时必需，单实现可省略但若提供必须匹配。端点/证书/凭据属于 Adapter setting；重试业务规则和权限不放配置。错误只报告 Port/Adapter/字段位置，不回显 Secret 值。

## Test Fakes

Test qualified implementation使用相同 signature/failure checker，但 owner 是 TestCase。Suite emission 为每个 case 生成独立 closed binding；case selection 同时选择 test entry 和 fake table。没有 fake 的可达 Port 在测试检查阶段失败，不退回生产 Adapter，确保测试不会意外访问外部系统。

## Role Matrix Migration

`check/symbols.rs` 继续作为唯一 call/type access owner。新增明确矩阵而不是负向条件组合。Model operation owner 检查保持独立但与 role matrix 一致。现有 CMS 尚未使用 Adapter，迁移只需修复拒绝 fixtures 和文档，不保留 App-to-Adapter compatibility。

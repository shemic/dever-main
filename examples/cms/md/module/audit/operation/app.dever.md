# 操作审计应用

审计应用写入最小化操作摘要。

- 包：`audit.operation.app`
- 公开类型：无
- 公开方法：
  - `record`
- 使用：
  - `audit.operation.record(kind, resource, resource_id)`

## 记录操作

保存操作类型、资源和资源标识。

- 函数：`record`
- 输入：
  - `kind: audit.operation.model.OperationKind`：操作类型
  - `resource: Text`：资源类型
  - `resource_id: Text`：资源 ID
- 输出：无

```dever
record(kind: model.OperationKind, resource: Text, resource_id: Text) () {
  stored = model.create(
    {
      kind = kind
      resource = resource
      resource_id = resource_id
    }
  )
  discarded = stored
}
```

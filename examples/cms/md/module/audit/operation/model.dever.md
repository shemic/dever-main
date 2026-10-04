# 操作审计模型

审计记录保存敏感操作摘要，不保存密码、Token 或请求全文。

- 包：`audit.operation.model`
- 公开类型：
  - `OperationKind`
  - `Operation`
- 公开方法：无
- 使用：无

## 操作类型

审计操作分类。

- 类型：`OperationKind`
- 分支：
  - `Login`：登录
  - `Logout`：登出
  - `Publish`：发布
  - `Upload`：上传
  - `Access`：授权

```dever
type OperationKind {
  Login = "登录"
  Logout = "登出"
  Publish = "发布"
  Upload = "上传"
  Access = "授权"
}
```

## 操作记录

审计记录只保存资源摘要。

- 类型：`Operation`
- 字段：
  - `actor_id: user.account.model.principal.id?`：操作者
  - `kind: OperationKind`：操作类型
  - `resource: Text(1, 120)`：资源类型
  - `resource_id: Text(1, 120)`：资源 ID

```dever
type Operation {
  actor_id: user.account.model.principal.id?
  kind: OperationKind index
  resource: Text(1, 120)
  resource_id: Text(1, 120)
}
```

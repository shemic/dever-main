# 系统健康

提供无需业务状态的本地运行检查。

- 包：`system.health.app`
- 公开类型：
  - `HealthView`
- 公开方法：
  - `ping`
- 使用：
  - `system.health.ping()`

## HealthView

返回健康检查的就绪状态。

- 类型：`HealthView`
- 字段：
  - `status: Text`：状态

```dever
type HealthView {
  status: Text
}
```

## 健康检查

返回固定的就绪状态。

- 函数：`ping`
- 输入：无
- 输出：
  - `result: HealthView`：健康状态

```dever
ping() (result: HealthView) {
  result = HealthView {
    status = "ready"
  }
}
```

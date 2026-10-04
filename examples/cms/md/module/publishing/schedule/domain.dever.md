# 发布计划规则

计划任务只允许待发布记录进入执行流程。

- 包：`publishing.schedule.domain`
- 公开类型：无
- 公开方法：无
- 使用：无

## require_due

计划任务只允许待发布记录进入执行流程。

- 函数：`require_due`
- 输入：
  - `valid: Bool`：该声明的业务值
- 输出：无

```dever
require_due(valid: true) () pure {}

require_due(valid: false) () pure {
  fail(dever.api.Error.Invalid)
}
```
